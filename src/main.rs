// =========================
// main.rs
// MilterSeparator メインプログラム（Milterプロトコル受信サーバ）
//
// 【このファイルで使う主なクレート】
// - tokio: 非同期TCPサーバ・シグナル・ブロードキャスト（net::TcpListener, sync::broadcast, signal::unix）
// - std: スレッド安全な参照カウント・ロック（Arc, RwLock）
// - client: クライアント受信処理
// - init: 設定ファイル管理
// - logging: JSTタイムスタンプ付きログ出力
// - milter_command: Milterコマンド定義
//
// 【役割】
// - サーバー起動・設定管理・クライアント接続受付・シグナル処理
// =========================

mod client; // クライアント受信処理
mod db;
mod download;
mod init; // 設定ファイル管理
mod logging; // JSTタイムスタンプ付きログ出力
mod milter; // Milterコマンドごとのデコード・応答処理
mod milter_command; // Milterコマンド定義
mod parse; // メールパース・出力処理
mod zipper; // 添付保存 / ZIP 処理 // download URL generator (axum)

use init::{LOG_INFO, LOG_TRACE, load_config};
use std::env;
use std::sync::{Arc, RwLock}; // スレッド安全な参照カウント・ロック
#[cfg(unix)]
use tokio::signal::unix::{SignalKind, signal}; // Unix系: シグナル受信
use tokio::{net::TcpListener, sync::broadcast}; // 非同期TCPサーバ・ブロードキャスト

/// 非同期メイン関数（Tokioランタイム）
/// - サーバー起動・設定管理・クライアント接続受付・シグナル処理
#[tokio::main]
async fn main() {
    // コマンドライン引数から設定ファイルパス取得（デフォルト: "MilterSeparator.conf"）
    let mut args = env::args().skip(1);
    let mut config_path = {
        #[cfg(unix)]
        {
            "/etc/MilterSeparator.conf".to_string()
        }
        #[cfg(windows)]
        {
            "MilterSeparator.conf".to_string()
        }
        #[cfg(not(any(unix, windows)))]
        {
            "MilterSeparator.conf".to_string()
        }
    };
    while let Some(arg) = args.next() {
        if arg == "-f"
            && let Some(path) = args.next()
        {
            config_path = path;
        }
    }
    let config_path = Arc::new(config_path);
    // 設定をスレッド安全に共有（Arc+RwLock）
    let config = Arc::new(RwLock::new(load_config(&*config_path)));
    // グローバルConfigをセット
    logging::set_global_config(Arc::clone(&config));
    // サーバー再起動・終了通知用ブロードキャストチャネル
    let (shutdown_tx, _) = broadcast::channel::<()>(100);

    #[cfg(unix)]
    {
        // SIGHUP/SIGTERM用にクローン
        let config = Arc::clone(&config); // 設定参照用
        let config_path = Arc::clone(&config_path); // 設定ファイルパス参照用
        let shutdown_tx_hup = shutdown_tx.clone(); // SIGHUP用
        let shutdown_tx_term = shutdown_tx.clone(); // SIGTERM用

        // SIGHUP受信: 設定ファイル再読込
        tokio::spawn(async move {
            let mut hup = signal(SignalKind::hangup()).expect("SIGHUP登録失敗");
            while hup.recv().await.is_some() {
                printdaytimeln!(LOG_INFO, "[main] SIGHUP受信: 設定ファイル再読込");
                let new_config = load_config(&*config_path); // 新設定読込
                *config.write().unwrap() = new_config; // 設定更新
                let _ = shutdown_tx_hup.send(()); // 全クライアントへ再起動通知
            }
        });
        // SIGTERM受信: サーバー安全終了
        tokio::spawn(async move {
            let mut term = signal(SignalKind::terminate()).expect("SIGTERM登録失敗");
            if term.recv().await.is_some() {
                printdaytimeln!(LOG_INFO, "[main] SIGTERM受信: サーバー安全終了");
                let _ = shutdown_tx_term.send(()); // 全クライアントへ終了通知
                std::process::exit(0); // プロセス終了
            }
        });
    }

    #[cfg(windows)]
    {
        // Windows用のシグナル処理（Ctrl+Cのみ対応）
        let shutdown_tx_ctrl_c = shutdown_tx.clone(); // Ctrl+C用

        // Ctrl+C受信: サーバー安全終了
        tokio::spawn(async move {
            if let Ok(()) = tokio::signal::ctrl_c().await {
                printdaytimeln!(LOG_INFO, "[main] Ctrl+C受信: サーバー安全終了");
                let _ = shutdown_tx_ctrl_c.send(()); // 全クライアントへ終了通知
                std::process::exit(0); // プロセス終了
            }
        });
    }

    loop {
        // サーバー再起動ループ
        let current_config = config.read().unwrap().clone(); // 現在の設定取得

        printdaytimeln!(LOG_INFO, "[main] 設定読込: {}", current_config.address); // バインドアドレス表示
        let log_level_str = match current_config.log_level {
            0 => "info",
            2 => "trace",
            8 => "debug",
            n => &format!("unknown({})", n),
        };
        printdaytimeln!(LOG_INFO, "[main] Log_level: {}", log_level_str); // ログレベル表示

        // initialize database according to configuration
        if let Err(e) = db::init_db(&current_config).await {
            crate::printdaytimeln!(LOG_INFO, "[main] db init failed: {}", e);
        }

        let bind_result = TcpListener::bind(&current_config.address).await; // TCPバインド
        let listener = match bind_result {
            Ok(listener) => {
                printdaytimeln!(LOG_INFO, "[main] 待受開始: {}", current_config.address); // バインド成功
                listener // リスナー返却
            }
            Err(e) => {
                eprintln!(
                    "[main] ポートバインド失敗: {}\n他プロセスが {} 使用中?",
                    e, current_config.address
                ); // バインド失敗
                std::process::exit(1); // 異常終了
            }
        };
        let mut shutdown_rx = shutdown_tx.subscribe(); // 再起動・終了通知受信
        loop {
            // クライアント受信ループ
            tokio::select! {
                Ok((stream, addr)) = listener.accept() => {
                    printdaytimeln!(LOG_TRACE, "[client] 接続: {}", addr); // クライアント新規接続
                    let shutdown_rx = shutdown_tx.subscribe(); // クライアント用レシーバ
                    let config = Arc::clone(&config);
                    tokio::spawn(client::handle_client(stream, shutdown_rx, config)); // クライアント処理開始
                }
                _ = shutdown_rx.recv() => {
                    printdaytimeln!(LOG_INFO, "[main] 再起動のためリスナー再バインド\n"); // 再起動通知
                    // 設定再読込
                    let new_config = load_config(&*config_path);
                    *config.write().unwrap() = new_config;
                    break; // サーバーループ再開
                }
            }
        }
    }
}
