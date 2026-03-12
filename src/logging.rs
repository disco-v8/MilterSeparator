// =========================
// logging.rs
// MilterSeparator ログ出力マクロ定義
//
// 【このファイルで使う主なクレート】
// - std::sync::{Arc, RwLock}: スレッド間でのグローバル設定共有（読み書きロック付き）
// - std::sync::OnceLock: スレッドセーフな一度だけ初期化される静的変数（Rust標準ライブラリ）
// - chrono: 日時操作・整形（Local::now, format）
// - chrono-tz: タイムゾーン変換（Asia::Tokyo/JST指定）
// - std::fs::OpenOptions: ログファイルの作成・追記モード操作
// - std::io::Write: ファイルへの書き込み操作
//
// 【役割】
// - グローバル設定管理: ログファイルパスなどの設定をスレッド間で共有
// - printdaytimeln!: JSTタイムスタンプ付きで標準出力またはファイルにログを出すマクロ
// =========================

use crate::init::Config;
use std::sync::{Arc, OnceLock, RwLock};

// グローバル設定を格納する静的変数（一度だけ初期化、スレッドセーフ）
static GLOBAL_CONFIG: OnceLock<Arc<RwLock<Config>>> = OnceLock::new();

/// グローバルConfigをセット（アプリケーション起動時に一度だけ呼び出し）
///
/// # 引数
/// - cfg: Arc<RwLock<Config>> - スレッド間で共有するConfig設定
///
/// # 説明
/// - OnceCell::set()で一度だけ設定可能（二回目以降は無視される）
/// - マルチスレッド環境でも安全に設定を共有
pub fn set_global_config(cfg: Arc<RwLock<Config>>) {
    let _ = GLOBAL_CONFIG.set(cfg); // エラーは無視（既に設定済みの場合）
}

/// グローバルConfigを取得（各種処理からログ設定を参照するために使用）
///
/// # 戻り値
/// - Some(Arc<RwLock<Config>>): 設定が存在する場合
/// - None: まだ設定されていない場合
///
/// # 説明
/// - 設定済みのグローバルConfigを安全に取得
/// - cloned()でArcの参照カウンタを増やして返す
pub fn get_global_config() -> Option<Arc<RwLock<Config>>> {
    GLOBAL_CONFIG.get().cloned() // Arcのクローン（参照カウンタのみコピー）
}

/// JSTタイムスタンプ付きでログを出力するマクロ（ファイルまたは標準出力）
///
/// # 使い方
/// ```rust
/// printdaytimeln!(LOG_INFO, "メッセージ: {}", val);
/// printdaytimeln!(LOG_INFO, "単純なメッセージ");
/// ```
/// /// # 引数
/// - $level: ログレベル（u8） Log_levelで定義された定数を使用
///   * LOG_INFO: 通常ログ
///   * LOG_TRACE: 詳細ログ
///   * LOG_DEBUG: デバッグログ
/// - $($arg:tt)*: 可変引数（format!マクロと同様の形式でメッセージを指定）
///
/// # 機能
/// - 現在時刻をJST（日本標準時）で取得し、[YYYY/MM/DD HH:MM:SS]形式で先頭に付与
/// - ログファイルが設定されている場合はファイルに追記
/// - ログファイルが未設定の場合は標準出力に表示
/// - format!マクロと同様の可変引数対応
/// - マルチスレッド環境でも安全に動作
///
/// # 内部処理フロー
/// 1. JST現在時刻を取得・整形
/// 2. グローバル設定からログファイルパスを取得
/// 3. ログファイルが設定されていればファイルに追記
/// 4. 設定されていなければ標準出力に表示
#[macro_export]
macro_rules! printdaytimeln {
    ($level:expr, $($arg:tt)*) => {{
        // Step 1: JST現在時刻を取得してフォーマット
        let now = chrono::Local::now().with_timezone(&chrono_tz::Asia::Tokyo);
        let log_time = now.format("[%Y/%m/%d %H:%M:%S]");

        // Step 2: タイムスタンプ + メッセージを結合
        let msg = format!("{} {}", log_time, format!($($arg)*));

        // Step 3: グローバル設定からログファイルパスとlog_levelを安全に取得
        let (log_file, log_level) = {
            if let Some(cfg_arc) = $crate::logging::get_global_config() {
                let cfg = cfg_arc.read().unwrap();
                (cfg.log_file.clone(), cfg.log_level)
            } else {
                (None, 0)
            }
        };

        // Step 4: levelがlog_level以下なら出力
        if $level <= log_level {
            if let Some(ref path) = log_file {
                // ログファイルが設定されている場合：ファイルに追記
                use std::io::Write;
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path) {
                    let _ = writeln!(file, "{}", msg);
                }
            } else {
                // ログファイルが未設定の場合：標準出力に表示
                println!("{}", msg);
            }
        }
    }};
}
