// =========================
// init.rs - MilterSeparator 設定管理モジュール
// =========================
//
// 【使用クレート】
// - std::fs: ファイルシステム操作（設定ファイルの読み書き）
// - std::path: ファイルパス処理（設定ファイルのパス操作）
// - std::io::BufRead: バッファ付きファイル読み込み（大容量設定ファイル対応）
//
// 【主要機能】
// 1. メイン設定ファイル(MilterSeparator.conf)の解析
// 2. includeディレクトリ内の追加設定ファイル(.conf)の再帰読み込み
// 3. サーバー設定（Listen、タイムアウト、ログレベル等）の構造化

/// ZIPパスワード強度設定
#[derive(Debug, Clone, Default)]
pub enum PasswordStrength {
    Low,
    #[default]
    Medium,
    High,
}

/// 削除モード
#[derive(Debug, Clone, Default)]
pub enum DeleteMode {
    #[default]
    Delete,
    Script,
}

// メイン設定構造体
// 設定ファイルから読み込んだ全ての設定項目を保持
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct Config {
    pub address: String,          // Milterサーバー待受アドレス（例: "[::]:8895"）
    pub client_timeout: u64,      // SMTPクライアント無応答タイムアウト時間（秒）
    pub log_file: Option<String>, // ログ出力先ファイルパス（None時は標準出力）
    pub log_level: u8,            // ログ詳細度（0=info, 2=trace, 8=debug）
    pub remote_ip_target: u8,     // RemoteIP_Target: 0=外部のみ,1=内部のみ,2=全て

    // MilterSeparator 用設定
    pub password_strength: PasswordStrength,
    pub max_downloads: u32,
    pub expire_hours: u64,
    pub download_auth_mode: String,
    pub basic_auth_user: Option<String>,
    pub basic_auth_password: Option<String>,
    pub token_auth_key: Option<String>,
    pub token_auth_type: Option<String>,
    pub delete_mode: DeleteMode,
    pub delete_script_path: Option<String>,
    pub storage_path: String,
    pub base_url: String,
    // Database settings
    pub database_type: String,         // sqlite / mysql / postgres
    pub database_path: Option<String>, // for sqlite
    pub database_host: Option<String>,
    pub database_port: Option<u16>,
    pub database_user: Option<String>,
    pub database_password: Option<String>,
    pub database_name: Option<String>,
    // CGI filename for counter (default: count.cgi)
    pub counter_cgi: String,
    // Service user/group for ownership when creating storage/db files
    pub milter_user: String,
    pub milter_group: String,

    // =========================================================
    // メールボディ変更機能設定
    // =========================================================
    /// 添付ファイルをメール本文から削除するかどうか（yes=削除, no=削除しない）。
    /// ZIP 保存後も元のメール本文では添付パートが残るため、削除したい場合は yes に設定する。
    pub remove_attachments_from_body: bool,

    /// 最初の text/plain パート先頭にダウンロード情報を挿入するかどうか。
    /// DownloadInfoHeadTemplate.txt のレンダリング結果を先頭に追加する。
    pub insert_download_info_head: bool,

    /// 最初の text/plain パート末尾にダウンロード情報を挿入するかどうか。
    /// DownloadInfoTailTemplate.txt のレンダリング結果を末尾に追加する。
    pub insert_download_info_tail: bool,

    /// 新規の text/plain パートとしてダウンロード情報を追加するかどうか。
    /// 既存パートを変更せず、DownloadInfoTextTemplate.txt を使った新パートを末尾に追加する。
    pub add_download_info_as_new_text_part: bool,
}

// ログレベル定数定義
// アプリケーション全体で使用するログ出力レベルの統一定義
pub const LOG_INFO: u8 = 0; // 通常運用情報（エラー、警告、重要な動作）
pub const LOG_TRACE: u8 = 2; // 処理トレース情報（関数の入出力、状態変化）
pub const LOG_DEBUG: u8 = 8; // デバッグ詳細情報（変数値、内部処理詳細）

// 設定ファイル読み込み・解析のメイン関数
// 指定パスの設定ファイルを読み込み、構造化されたConfig オブジェクトを生成
//
// # 引数
// * `path` - 設定ファイルのパス（.conf ファイル）
//
// # 戻り値
// * `Config` - 解析済み設定情報オブジェクト
//
// # 処理フロー
// 1. 設定ファイルをテキストとして読み込み
// 2. include ディレクトリ内の追加設定ファイルを再帰的に処理
// 3. 各設定項目を対応する構造体フィールドにマッピング
//
// # デフォルト値
// * Listen: "[::]:8895"（IPv6全アドレス、ポート8895）
// * Client_timeout: 30秒
// * Log_level: 0 (info レベル)
pub fn load_config<P: AsRef<std::path::Path>>(path: P) -> Config {
    // 内部用設定値一時保持構造体
    // ファイル解析中に設定値を蓄積し、最終的にConfig構造体に変換するためのワーク領域
    struct ConfigValues {
        address: Option<String>,  // サーバー待受アドレス
        client_timeout: u64,      // 接続タイムアウト時間
        log_file: Option<String>, // ログファイル出力先
        log_level: u8,            // ログ詳細度設定
        remote_ip_target: u8,     // RemoteIP_Target: 0=外部のみ,1=内部のみ,2=全て

        // MilterSeparator specific values (raw/string forms)
        password_strength: String,
        max_downloads: u32,
        expire_hours: u64,
        download_auth_mode: String,
        basic_auth_user: Option<String>,
        basic_auth_password: Option<String>,
        token_auth_key: Option<String>,
        token_auth_type: String,
        delete_mode: String,
        delete_script_path: Option<String>,
        storage_path: String,
        base_url: String,
        // Database raw settings
        database_type: String,
        database_path: Option<String>,
        database_host: Option<String>,
        database_port: Option<String>,
        database_user: Option<String>,
        database_password: Option<String>,
        database_name: Option<String>,
        counter_cgi: String,
        milter_user: String,
        milter_group: String,
        // メールボディ変更機能設定フラグ
        remove_attachments_from_body: bool,
        insert_download_info_head: bool,
        insert_download_info_tail: bool,
        add_download_info_as_new_text_part: bool,
    }

    // 設定テキスト行単位解析関数
    // 設定ファイルの内容を1行ずつ処理し、ConfigValues構造体に値を格納
    fn parse_config_text(text: &str, values: &mut ConfigValues) {
        // キー・値分割ヘルパー関数
        // "Key Value" または "Key\tValue" 形式の行を解析
        // 戻り値: (キー文字列, 値文字列) のタプル、または None
        fn split_key_value(line: &str) -> Option<(&str, &str)> {
            // 最初の空白文字またはタブ文字を探す
            let separator_pos = line.find([' ', '\t'])?;

            // 空白文字の連続部分をスキップして値の開始位置を特定
            let value_start = line[separator_pos..]
                .find(|c: char| !c.is_whitespace())
                .map(|pos| separator_pos + pos)?;

            let key = line[..separator_pos].trim();
            let value = line[value_start..].trim();

            if key.is_empty() || value.is_empty() {
                None
            } else {
                Some((key, value))
            }
        }

        // 設定項目キーのリスト（複数行検出用）
        fn is_config_key(line: &str) -> bool {
            line.starts_with("Listen")
                || line.starts_with("Client_timeout")
                || line.starts_with("Log_file")
                || line.starts_with("Log_level")
                || line.starts_with("RemoteIP_Target")
                || line.starts_with("include")
                || line.starts_with("password_strength")
                || line.starts_with("download_auth_mode")
                || line.starts_with("basic_auth_user")
                || line.starts_with("basic_auth_password")
                || line.starts_with("token_auth_key")
                || line.starts_with("token_auth_type")
                || line.starts_with("max_downloads")
                || line.starts_with("expire_hours")
                || line.starts_with("delete_mode")
                || line.starts_with("delete_script_path")
                || line.starts_with("storage_path")
                || line.starts_with("base_url")
                || line.starts_with("Database_Type")
                || line.starts_with("Database_Path")
                || line.starts_with("Database_Host")
                || line.starts_with("Database_Port")
                || line.starts_with("Database_User")
                || line.starts_with("Database_Password")
                || line.starts_with("Database_Name")
                || line.starts_with("counter_cgi")
                || line.starts_with("milter_user")
                || line.starts_with("milter_group")
                || line.starts_with("Remove_Attachments_From_Body")
                || line.starts_with("Insert_Download_Info_Position_head")
                || line.starts_with("Insert_Download_Info_Position_tail")
                || line.starts_with("Add_Download_Info_As_New_Text_Part")
        }

        // 複数行にわたる設定値を収集する統一関数
        // 次の行が新しい設定項目でなければ継続行として処理
        fn collect_multiline_value(
            lines: &mut std::iter::Peekable<std::str::Lines>,
            initial_value: &str,
            join_with_comma: bool,
        ) -> String {
            let mut current_value = initial_value.to_string();

            while let Some(peek) = lines.peek() {
                let peek_trim = peek.trim();

                // 空行または新しい設定項目の開始を検出した場合は終了
                if peek_trim.is_empty() || is_config_key(peek_trim) {
                    break;
                }

                // コメント行は読み飛ばして次の行へ
                if peek_trim.starts_with('#') {
                    lines.next();
                    continue;
                }

                // 継続行として連結
                if join_with_comma {
                    // カンマ区切りでの連結（include ディレクトリ等）
                    if !current_value.trim().ends_with(',') && !peek_trim.starts_with(',') {
                        current_value.push(',');
                    }
                } else {
                    // スペース区切りでの連結（filter等）
                    current_value.push(' ');
                }
                current_value.push_str(peek_trim);
                lines.next();
            }

            current_value
        }

        let mut lines = text.lines().peekable();

        // 設定ファイルの各行を順次処理
        while let Some(line) = lines.next() {
            let line = line.trim(); // 行頭・行末の空白文字を除去

            // 空行とコメント行（#で始まる行）をスキップ
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Listen設定 - Milterサーバーの待受ソケット設定
            else if line.starts_with("Listen") {
                if let Some((_, addr)) = split_key_value(line) {
                    // 複数行のListen設定値を収集
                    let full_value = collect_multiline_value(&mut lines, addr, false);
                    let addr = full_value.trim();

                    if addr.contains(':') {
                        // "host:port" または "[ipv6]:port" 形式
                        values.address = Some(addr.to_string());
                    } else {
                        // ポート番号のみの場合はIPv6全アドレスでバインド
                        if addr.parse::<u16>().is_ok() {
                            values.address = Some(format!("[::]:{}", addr));
                        } else {
                            crate::printdaytimeln!(
                                LOG_INFO,
                                "[init] Invalid address/port: {}",
                                addr
                            );
                        }
                    }
                }
            }
            // Client_timeout設定 - SMTP接続の無応答タイムアウト時間
            else if line.starts_with("Client_timeout") {
                if let Some((_, val_str)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, val_str, false);
                    if let Ok(val) = full_value.trim().parse::<u64>() {
                        values.client_timeout = val;
                    }
                }
            }
            // Log_file設定 - ログの出力先ファイルパス指定
            else if line.starts_with("Log_file") {
                if let Some((_, path)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, path, false);
                    let path = full_value.trim();
                    if !path.is_empty() {
                        values.log_file = Some(path.to_string());
                    }
                }
            }
            // Log_level設定 - ログ出力の詳細度制御
            else if line.starts_with("Log_level") {
                if let Some((_, level_str)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, level_str, false);
                    let level = full_value.trim().to_ascii_lowercase();
                    values.log_level = match level.as_str() {
                        "info" => 0,  // 基本的な動作情報のみ
                        "trace" => 2, // 処理の流れを追跡
                        "debug" => 8, // 詳細なデバッグ情報
                        _ => 0,       // 不明な値はinfoレベルに設定
                    };
                }
            }
            // RemoteIP_Target設定 - 接続元IPに基づく対象範囲制御
            else if line.starts_with("RemoteIP_Target") {
                if let Some((_, val_str)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, val_str, false);
                    // 整数としてパースし、0/1/2の範囲に制限
                    if let Ok(v) = full_value.trim().parse::<i64>() {
                        let v = if v < 0 {
                            0
                        } else if v > 2 {
                            2
                        } else {
                            v as u8
                        };
                        values.remote_ip_target = v;
                    }
                }
            }
            // password_strength設定 - low|medium|high
            else if line.starts_with("password_strength") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    if v == "low" || v == "medium" || v == "high" {
                        values.password_strength = v;
                    }
                }
            }
            // max_downloads設定 - 整数
            else if line.starts_with("max_downloads") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    if let Ok(n) = full_value.trim().parse::<u32>() {
                        values.max_downloads = n;
                    }
                }
            }
            // expire_hours設定 - 整数
            else if line.starts_with("expire_hours") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    if let Ok(n) = full_value.trim().parse::<u64>() {
                        values.expire_hours = n;
                    }
                }
            }
            // delete_mode設定 - delete|script
            else if line.starts_with("delete_mode") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    if v == "delete" || v == "script" {
                        values.delete_mode = v;
                    }
                }
            }
            // delete_script_path設定
            else if line.starts_with("delete_script_path") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let p = full_value.trim();
                    if !p.is_empty() {
                        values.delete_script_path = Some(p.to_string());
                    }
                }
            }
            // storage_path設定
            else if line.starts_with("storage_path") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let p = full_value.trim();
                    if !p.is_empty() {
                        values.storage_path = p.to_string();
                    }
                }
            }
            // milter_user設定
            else if line.starts_with("milter_user") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.milter_user = v.to_string();
                    }
                }
            }
            // milter_group設定
            else if line.starts_with("milter_group") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.milter_group = v.to_string();
                    }
                }
            }
            // base_url設定
            else if line.starts_with("base_url") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let u = full_value.trim();
                    if !u.is_empty() {
                        values.base_url = u.to_string();
                    }
                }
            }
            // download_auth_mode設定
            else if line.starts_with("download_auth_mode") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.download_auth_mode = v.to_string();
                    }
                }
            }
            // Database settings
            else if line.starts_with("Database_Type") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    if !v.is_empty() {
                        values.database_type = v;
                    }
                }
            } else if line.starts_with("Database_Path") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.database_path = Some(v.to_string());
                    }
                }
            } else if line.starts_with("Database_Host") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.database_host = Some(v.to_string());
                    }
                }
            } else if line.starts_with("Database_Port") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if let Ok(n) = v.parse::<u16>() {
                        values.database_port = Some(n.to_string());
                    }
                }
            } else if line.starts_with("Database_User") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.database_user = Some(v.to_string());
                    }
                }
            } else if line.starts_with("Database_Password") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.database_password = Some(v.to_string());
                    }
                }
            } else if line.starts_with("Database_Name") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.database_name = Some(v.to_string());
                    }
                }
            } else if line.starts_with("counter_cgi") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.counter_cgi = v.to_string();
                    }
                }
            }
            // basic_auth_user
            else if line.starts_with("basic_auth_user") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.basic_auth_user = Some(v.to_string());
                    }
                }
            }
            // basic_auth_password
            else if line.starts_with("basic_auth_password") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.basic_auth_password = Some(v.to_string());
                    }
                }
            }
            // token_auth_key
            else if line.starts_with("token_auth_key") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.token_auth_key = Some(v.to_string());
                    }
                }
            }
            // token_auth_type
            else if line.starts_with("token_auth_type") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim();
                    if !v.is_empty() {
                        values.token_auth_type = v.to_string();
                    }
                }
            }
            // Remove_Attachments_From_Body 設定 - 添付ファイルをメール本文から削除するか
            else if line.starts_with("Remove_Attachments_From_Body") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    values.remove_attachments_from_body = v == "yes" || v == "true" || v == "1";
                }
            }
            // Insert_Download_Info_Position_head 設定 - 先頭にダウンロード情報を挿入するか
            else if line.starts_with("Insert_Download_Info_Position_head") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    values.insert_download_info_head = v == "yes" || v == "true" || v == "1";
                }
            }
            // Insert_Download_Info_Position_tail 設定 - 末尾にダウンロード情報を挿入するか
            else if line.starts_with("Insert_Download_Info_Position_tail") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    values.insert_download_info_tail = v == "yes" || v == "true" || v == "1";
                }
            }
            // Add_Download_Info_As_New_Text_Part 設定 - 新規 text/plain パートとしてダウンロード情報を追加するか
            else if line.starts_with("Add_Download_Info_As_New_Text_Part") {
                if let Some((_, value)) = split_key_value(line) {
                    let full_value = collect_multiline_value(&mut lines, value, false);
                    let v = full_value.trim().to_ascii_lowercase();
                    values.add_download_info_as_new_text_part =
                        v == "yes" || v == "true" || v == "1";
                }
            }
            // include ディレクトリの再帰読み込み処理
            else if line.starts_with("include") {
                crate::printdaytimeln!(LOG_DEBUG, "[init] processing include line: {}", line);
                if let Some((_, dir_path)) = split_key_value(line) {
                    // 複数行のinclude設定値を収集
                    let full_value = collect_multiline_value(&mut lines, dir_path, true);
                    crate::printdaytimeln!(
                        LOG_DEBUG,
                        "[init] include directories: '{}'",
                        full_value
                    );

                    // カンマ区切りで複数のディレクトリを処理
                    for dir in full_value.split(',') {
                        let dir = dir.trim();
                        crate::printdaytimeln!(
                            LOG_DEBUG,
                            "[init] trying to read directory: '{}'",
                            dir
                        );
                        if !dir.is_empty() {
                            if let Ok(entries) = std::fs::read_dir(dir) {
                                for entry in entries.flatten() {
                                    let path = entry.path();
                                    if path.is_file()
                                        && let Some(ext) = path.extension()
                                        && ext == "conf"
                                    {
                                        crate::printdaytimeln!(
                                            LOG_INFO,
                                            "[init] loading sub-conf file: {}",
                                            path.display()
                                        );
                                        if let Ok(sub_text) = std::fs::read_to_string(&path) {
                                            crate::printdaytimeln!(
                                                LOG_DEBUG,
                                                "[init] file content length: {} bytes",
                                                sub_text.len()
                                            );
                                            // 再帰的にサブ設定を解析（新しい設定キーをマージ）
                                            parse_config_text(&sub_text, values);
                                        } else {
                                            crate::printdaytimeln!(
                                                LOG_INFO,
                                                "[init] failed to read file: {}",
                                                path.display()
                                            );
                                        }
                                    }
                                }
                            } else {
                                crate::printdaytimeln!(
                                    LOG_INFO,
                                    "[init] failed to read directory: '{}'",
                                    dir
                                );
                            }
                        }
                    }
                }
                continue;
            }
            // 未知の設定項目の処理 - 将来の拡張性のため警告を出力して無視
            else if (line.contains(' ') || line.contains('\t'))
                && let Some((key, _)) = split_key_value(line)
            {
                crate::printdaytimeln!(LOG_INFO, "[init] Unknown Config Key: {}", key);
            }
        }
    }

    // 設定ファイル本体の読み込み実行
    let text = std::fs::read_to_string(path).expect("設定ファイル読み込み失敗");

    // 初期値を設定した作業用構造体を作成
    let mut values = ConfigValues {
        address: None,
        client_timeout: 30u64, // デフォルト30秒タイムアウト
        log_file: None,        // デフォルトは標準出力
        log_level: 0,          // デフォルトはinfoレベル
        remote_ip_target: 0,   // デフォルトは0（外部のみ）
        password_strength: "medium".to_string(),
        max_downloads: 3,
        expire_hours: 24 * 7,
        download_auth_mode: "minimal".to_string(),
        basic_auth_user: None,
        basic_auth_password: None,
        token_auth_key: None,
        token_auth_type: "hmac-sha256".to_string(),
        delete_mode: "delete".to_string(),
        delete_script_path: None,
        storage_path: "/var/lib/milter_separator/files".to_string(),
        base_url: "http://localhost".to_string(),
        database_type: "sqlite".to_string(),
        database_path: Some("/var/lib/milterseparator/db.sqlite3".to_string()),
        database_host: None,
        database_port: None,
        database_user: None,
        database_password: None,
        database_name: None,
        counter_cgi: "count.cgi".to_string(),
        milter_user: "milter".to_string(),
        milter_group: "apache".to_string(),
        // メールボディ変更機能設定（デフォルト: 全て無効）
        remove_attachments_from_body: false,
        insert_download_info_head: false,
        insert_download_info_tail: false,
        add_download_info_as_new_text_part: false,
    };

    // 設定ファイル内容の解析実行
    parse_config_text(&text, &mut values);

    // 設定の確認ログ
    crate::printdaytimeln!(LOG_INFO, "[init] configuration loaded");

    // 最終的なConfig構造体を生成して返却
    // password_strength を列挙型に変換
    let password_strength = match values.password_strength.as_str() {
        "low" => PasswordStrength::Low,
        "high" => PasswordStrength::High,
        _ => PasswordStrength::Medium,
    };
    let delete_mode = match values.delete_mode.as_str() {
        "script" => DeleteMode::Script,
        _ => DeleteMode::Delete,
    };

    Config {
        address: values.address.unwrap_or_else(|| "[::]:8895".to_string()),
        client_timeout: values.client_timeout,
        log_file: values.log_file,
        log_level: values.log_level,
        remote_ip_target: values.remote_ip_target,

        password_strength,
        max_downloads: values.max_downloads,
        expire_hours: values.expire_hours,
        download_auth_mode: values.download_auth_mode,
        basic_auth_user: values.basic_auth_user,
        basic_auth_password: values.basic_auth_password,
        token_auth_key: values.token_auth_key,
        token_auth_type: Some(values.token_auth_type),
        delete_mode,
        delete_script_path: values.delete_script_path,
        storage_path: values.storage_path,
        base_url: values.base_url,
        database_type: values.database_type,
        database_path: values.database_path,
        database_host: values.database_host,
        database_port: values.database_port.and_then(|s| s.parse::<u16>().ok()),
        database_user: values.database_user,
        database_password: values.database_password,
        database_name: values.database_name,
        counter_cgi: values.counter_cgi,
        milter_user: values.milter_user,
        milter_group: values.milter_group,
        remove_attachments_from_body: values.remove_attachments_from_body,
        insert_download_info_head: values.insert_download_info_head,
        insert_download_info_tail: values.insert_download_info_tail,
        add_download_info_as_new_text_part: values.add_download_info_as_new_text_part,
    }
}
