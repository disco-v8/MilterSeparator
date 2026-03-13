// =========================
// client.rs
// MilterSeparator クライアント接続処理モジュール
//
// 【このファイルで使う主なクレート】
// - tokio: 非同期TCP通信・I/O・ブロードキャスト・タイムアウト等の非同期処理全般（net::TcpStream, io::AsyncReadExt, sync::broadcast）
// - std: 標準ライブラリ（アドレス、コレクション、時間、文字列操作など）
// - super::milter_command: Milterプロトコルのコマンド種別定義・判定用（MilterCommand enum, as_str等）
// - super::milter: Milterコマンドごとのペイロード分解・応答処理（decode_xxx群）
// - crate::printdaytimeln!: JSTタイムスタンプ付きログ出力マクロ
//
// 【役割】
// - クライアント1接続ごとのMilterプロトコル非同期処理
// - ヘッダ受信 → コマンド判定 → ペイロード受信 → コマンド別処理 → 応答送信
// - BODYEOB時にメールパース・出力処理の呼び出し
// - タイムアウト・エラーハンドリング・シャットダウン通知処理
// =========================

use tokio::{
    io::AsyncReadExt, // 非同期I/Oトレイト（read等）
    net::TcpStream,   // 非同期TCPストリーム
    sync::broadcast,  // 非同期ブロードキャストチャンネル
};

use super::milter_command::MilterCommand;
use crate::{
    init::{LOG_DEBUG, LOG_INFO, LOG_TRACE},
    milter::{
        decode_body, decode_connect, decode_data_macros, decode_header, decode_helo, decode_optneg,
        send_milter_response, send_replace_body,
    },
}; // Milterコマンド種別定義・判定 // 各Milterコマンドの分解・応答処理

use crate::init::Config;
use crate::parse::parse_mail;
use std::sync::{Arc, RwLock};

/// クライアント1接続ごとの非同期処理（Milterプロトコル）
/// 1. ヘッダ受信 → 2. コマンド判定 → 3. ペイロード受信 → 4. コマンド別処理 → 5. 応答送信
///    クライアント1接続ごとのMilterプロトコル非同期処理
pub async fn handle_client(
    mut stream: TcpStream,                    // クライアントTCPストリーム
    mut shutdown_rx: broadcast::Receiver<()>, // サーバーからのシャットダウン通知受信
    config: Arc<RwLock<Config>>,              // サーバー設定
) {
    // クライアントのIP:Portアドレス取得（接続元識別用）
    let peer_addr = match stream.peer_addr() {
        Ok(addr) => addr.to_string(),    // 正常時はアドレス文字列
        Err(_) => "unknown".to_string(), // 取得失敗時はunknown
    };

    // 設定取得（タイムアウト秒など）
    let config_val = config.read().unwrap().clone(); // 設定をロックしてクローン
    let timeout_duration = std::time::Duration::from_secs(config_val.client_timeout); // タイムアウト値をDuration化

    // BODYコマンド受信後はEOHをBODYEOB扱いにするフラグ
    let mut is_body_eob = false; // BODY受信後にEOHをBODYEOBとして扱う
    // DATAコマンドでヘッダブロック開始/終了を判定
    let mut is_header_block = false; // ヘッダブロック中かどうか
    // マクロ情報（SMTPセッション情報）
    let mut macro_fields: std::collections::HashMap<String, String> =
        std::collections::HashMap::new(); // マクロ格納用
    // ヘッダ情報（複数値対応）
    let mut header_fields: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new(); // ヘッダ格納用
    // ボディ情報
    let mut body_field = String::new(); // ボディ格納用
    // メインループ: 切断・エラー・タイムアウト・シャットダウン通知以外は繰り返しコマンド受信・応答
    loop {
        // メインループ: 切断・エラー・タイムアウト・シャットダウン通知以外は繰り返しコマンド受信・応答
        // --- フェーズ1: 5バイトヘッダ受信（4バイト:サイズ + 1バイト:コマンド） ---
        let mut header = [0u8; 5]; // 5バイトのMilterヘッダバッファ
        let mut read_bytes = 0; // 受信済みバイト数カウンタ
        // 5バイト受信するまでループ
        while read_bytes < 5 {
            // 5バイト受信するまでループ
            // タイムアウト付きでヘッダ受信（shutdown通知も同時監視）
            // タイムアウト・シャットダウン通知を同時監視しつつ受信
            match tokio::select! {
                res = tokio::time::timeout(timeout_duration, stream.read(&mut header[read_bytes..])) => res, // ヘッダ受信
                _ = shutdown_rx.recv() => { // サーバー再起動/終了通知（ブロードキャスト）
                    return; // サーバー都合で切断
                }
            } {
                Ok(Ok(0)) => {
                    // クライアント切断（0バイト受信）
                    crate::printdaytimeln!(LOG_TRACE, "[client] 切断(phase1): {}", peer_addr);
                    return; // ループ脱出
                }
                Ok(Ok(n)) => {
                    // 受信バイト数を加算
                    read_bytes += n; // 進捗更新
                }
                Ok(Err(e)) => {
                    // 受信エラー（I/O例外）
                    crate::printdaytimeln!(
                        LOG_INFO,
                        "[client] 受信エラー(phase1): {}: {}",
                        peer_addr,
                        e
                    );
                    return; // ループ脱出
                }
                Err(_) => {
                    // タイムアウト切断
                    crate::printdaytimeln!(
                        LOG_INFO,
                        "[client] タイムアウト(phase1): {} ({}秒間無通信)",
                        peer_addr,
                        config_val.client_timeout
                    );
                    return; // ループ脱出
                }
            }
        }

        // --- フェーズ2: コマンド判定（Milterコマンド種別） ---
        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]); // 4バイト:コマンド+ペイロードサイズ
        let command = header[4]; // 1バイト:コマンド種別
        let milter_cmd = MilterCommand::from_u8(command); // コマンド種別をenum化
        match milter_cmd {
            // コマンド種別ごとに分岐
            Some(cmd) => {
                // EOHコマンド時はEOH/BODYEOB名で出力、それ以外は通常名
                if let MilterCommand::Eoh = cmd {
                    let eoh_str = MilterCommand::Eoh.as_str_eoh(is_body_eob);
                    crate::printdaytimeln!(
                        LOG_DEBUG,
                        "[client] コマンド受信(phase2): {} (0x{:02X}) size={} from {} [is_body_eob={}]",
                        eoh_str,
                        command,
                        size,
                        peer_addr,
                        is_body_eob
                    );
                } else {
                    crate::printdaytimeln!(
                        LOG_DEBUG,
                        "[client] コマンド受信(phase2): {} (0x{:02X}) size={} from {}",
                        cmd.as_str(),
                        command,
                        size,
                        peer_addr
                    );
                }
            }
            None => {
                // 未定義コマンドは切断
                crate::printdaytimeln!(
                    LOG_INFO,
                    "[client] 不正コマンド(phase2): 0x{:02X} (addr: {})",
                    command,
                    peer_addr
                );
                return;
            }
        }

        // --- フェーズ3: ペイロード受信（4KB単位で分割） ---
        let mut remaining = size.saturating_sub(1) as usize; // 残り受信バイト数（コマンド1バイト分除外）
        let mut payload = Vec::with_capacity(remaining); // ペイロード格納バッファ
        // ペイロード全体を受信するまでループ
        while remaining > 0 {
            // ペイロード全体を受信するまでループ
            // 受信するバイト数を決定（最大4KBずつ）
            let chunk_size = std::cmp::min(4096, remaining); // 受信単位（最大4KB）
            let mut chunk = vec![0u8; chunk_size]; // チャンクバッファを確保
            // タイムアウト付きでペイロード受信
            // タイムアウト・シャットダウン通知を同時監視しつつ受信
            match tokio::select! {
                res = tokio::time::timeout(timeout_duration, stream.read(&mut chunk)) => res, // ペイロード受信
                _ = shutdown_rx.recv() => { // サーバー再起動/終了通知（ブロードキャスト）
                    return; // サーバー都合で切断
                }
            } {
                Ok(Ok(0)) => {
                    // クライアント切断（0バイト受信）
                    crate::printdaytimeln!(LOG_INFO, "[client] 切断(phase3): {}", peer_addr);
                    return; // ループ脱出
                }
                Ok(Ok(n)) => {
                    // 受信データをペイロードへ格納
                    payload.extend_from_slice(&chunk[..n]); // バッファに追加
                    // 残りバイト数を減算
                    remaining -= n; // 進捗更新
                }
                Ok(Err(e)) => {
                    // 受信エラー（I/O例外）
                    crate::printdaytimeln!(
                        LOG_INFO,
                        "[client] 受信エラー(phase3): {}: {}",
                        peer_addr,
                        e
                    );
                    return; // ループ脱出
                }
                Err(_) => {
                    // タイムアウト切断
                    crate::printdaytimeln!(
                        LOG_INFO,
                        "[client] タイムアウト(phase3): {} ({}秒間無通信)",
                        peer_addr,
                        config_val.client_timeout
                    );
                    return; // ループ脱出
                }
            }
        }

        // ペイロード受信完了ログ（実際の受信サイズを出力）
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[client] ペイロード受信完了: {} bytes from {}",
            payload.len(),
            peer_addr
        ); // 受信サイズ出力

        // --- コマンド別処理: OPTNEG, EOH/BODYEOB, その他 ---
        if let Some(cmd) = milter_cmd {
            // コマンド種別ごとに処理分岐
            // PostfixのMilterプロトコルで送られてくる順番に分岐を並び替え
            // 主要なMilterコマンドごとに分岐し、各処理を実行
            if let MilterCommand::OptNeg = cmd {
                // OPTNEGコマンド解析処理（ネゴシエーション情報の分解・応答）
                decode_optneg(&mut stream, &payload).await; // ネゴシエーション応答
            } else if let MilterCommand::Connect = cmd {
                // CONNECTコマンド時は接続情報の分解＆応答（milter.rsに分離）
                decode_connect(&mut stream, &payload, &peer_addr).await; // 接続情報応答
            } else if let MilterCommand::HeLO = cmd {
                // HELOコマンド時はHELO情報の分解＆応答（milter.rsに分離）
                decode_helo(&mut stream, &payload, &peer_addr).await; // HELO応答
            } else if let MilterCommand::Data = cmd {
                // DATAコマンド時(のマクロ処理)（milter.rsに分離）
                decode_data_macros(&payload, &mut is_header_block, &mut macro_fields);
                // マクロ情報処理
                // DATAコマンドではCONTINUE応答を送信しなくてもよい
            } else if let MilterCommand::Header = cmd {
                // SMFIC_HEADER(0x4C)コマンド時、ペイロードをヘッダ配列に格納＆出力（milter.rsに分離）
                decode_header(&payload, &mut header_fields); // ヘッダ格納
            // HEADERコマンドではCONTINUE応答を送信しなくてもよい（Postfix互換）
            } else if let MilterCommand::Body = cmd {
                // BODYコマンドが来たら以降0x45はBODYEOB扱いにする
                is_body_eob = true; // BODY受信後はEOHをBODYEOB扱い
                is_header_block = false; // BODYコマンドでヘッダブロック終了
                // BODYペイロードをデコード・保存（ヘッダ配列・ボディも渡す）
                decode_body(&payload, &mut body_field); // ボディ格納
            // BODYコマンドではCONTINUE応答を送信しなくてもよい
            } else if let MilterCommand::Eoh = cmd {
                if is_body_eob {
                    // パース処理でメール全体をパース・デバッグ出力・構造化
                    // parse_mail は async 関数のため .await で待機する
                    if let Some(parse_result) = parse_mail(
                        &header_fields,
                        &body_field,
                        &macro_fields,
                        &config_val.storage_path,
                        config_val.remote_ip_target,
                        &config_val,
                    )
                    .await
                    {
                        // ====================================================
                        // 機能1〜3: ボディ変更がある場合は SMFIR_REPLBODY 送信
                        //
                        // parse_mail が modified_body を返した場合、MTA に対して
                        // SMFIR_REPLBODY (0x62) でメール本文を差し替えるよう指示する。
                        // SMFIR_REPLBODY は SMFIR_ACCEPT より前に送信しなければならない。
                        // ====================================================
                        if let Some(ref new_body) = parse_result.modified_body {
                            crate::printdaytimeln!(
                                LOG_INFO,
                                "[client] sending SMFIR_REPLBODY ({} bytes) to {}",
                                new_body.len(),
                                peer_addr
                            );
                            if let Err(e) = send_replace_body(&mut stream, new_body).await {
                                crate::printdaytimeln!(
                                    LOG_INFO,
                                    "[client] SMFIR_REPLBODY send error: {}: {}",
                                    peer_addr,
                                    e
                                );
                                // 送信失敗してもメール処理は続行（ボディ変更なしで受理）
                            }
                        }

                        crate::printdaytimeln!(
                            LOG_INFO,
                            "[client] parsed mail, returning ACCEPT to {}",
                            peer_addr
                        );
                        // 最終応答（ボディ変更有無にかかわらず CONTINUE/ACCEPT を送信）
                        let response = Some(("CONTINUE".to_string(), "continue".to_string()));
                        send_milter_response(&mut stream, &peer_addr, response).await;
                    }
                } else {
                    // actionは "CONTINUE"（0x06）で応答
                    crate::printdaytimeln!(
                        LOG_DEBUG,
                        "[client] EOHコマンド受信: CONTINUE応答 (0x06) to {}",
                        peer_addr
                    );
                    let response = Some(("CONTINUE".to_string(), "continue".to_string()));
                    // クライアント(Sendmail/Postfix)への応答処理
                    send_milter_response(&mut stream, &peer_addr, response).await;
                }
                // BODYEOB(=is_body_eob==true)のときのみ、直前のヘッダ情報とボディ情報を出力
                if is_body_eob {
                    // BODYEOB時はヘッダ・ボディ・マクロ情報をクリア
                    macro_fields.clear(); // マクロ初期化
                    header_fields.clear(); // ヘッダ初期化
                    body_field.clear(); // ボディ初期化
                    is_body_eob = false; // BODYEOB→EOH遷移
                }
            } else {
                // その他のコマンドや拡張コマンド時
                // ペイロードデータを16進表記で出力（デバッグ用）
                if !payload.is_empty() {
                    let hexstr = payload
                        .iter()
                        .map(|b| format!("{b:02X}"))
                        .collect::<Vec<_>>()
                        .join(" "); // 16進ダンプ生成
                    crate::printdaytimeln!(LOG_DEBUG, "[client] ペイロード: {}", hexstr);
                    // 16進ダンプ出力
                }
                // その他の正式なコマンドにはCONTINUE応答を送信しない
            }
        }
    } // メインループ終端
}
