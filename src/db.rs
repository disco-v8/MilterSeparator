// =========================
// db.rs
// MilterSeparator データベースアクセスモジュール
//
// 【このファイルで使う主なクレート】
// - rusqlite: SQLite3 への同期アクセス（バンドルビルドで外部 .so 不要）
// - mysql: MySQL/MariaDB への同期アクセス（接続プール付き）
// - tokio_postgres (as tokio_pg): PostgreSQL への非同期アクセス
//   - tokio ランタイム上で動作させるため、同期クレート postgres は使用しない
//   - NoTls: TLS なし接続（同一サーバー内通信を想定）
// - chrono: タイムゾーン付き現在日時の生成（inserted_at カラム用）
// - serde_json: auth_info 等の JSON 値を文字列にシリアライズして DB へ保存
// - std::process::Command: chown コマンドを呼び出してファイルオーナーを設定
// - std::os::unix::fs::PermissionsExt: Unix パーミッションを数値で設定
//
// 【役割】
// - DB 初期化（テーブルが存在しない場合のみ CREATE TABLE を実行）
// - ダウンロードレコードの INSERT（uuid 重複時は expires_at を更新）
// - DB 種別（sqlite / mysql / postgres）を設定値で切り替え
//
// 【設計上の注意】
// - Postgres には `tokio::spawn` でバックグラウンドタスクとして挿入する。
//   これは「Tokio ランタイム内で同期ブロッキング接続を開こうとするとパニックする」
//   問題を避けるための設計である。
// - 整数フィールド（expire_hours / max_downloads）は Rust 側で i64 を使い、
//   DB スキーマも BIGINT に統一することで型不一致エラーを防ぐ。
// =========================

use crate::init::Config;
use chrono::Local;
use mysql::params; // MySQL 名前付きパラメータマクロ
use mysql::prelude::Queryable; // MySQL クエリ実行トレイト（query_drop, exec_drop 等）
use serde_json::Value;
use std::os::unix::fs::PermissionsExt; // Unix パーミッション数値設定用（set_mode）
use std::process::Command; // 外部コマンド（chown）呼び出し用
use tokio_postgres as tokio_pg; // PostgreSQL 非同期クライアント（エイリアス）
use tokio_postgres::NoTls; // TLS なし接続設定（ローカル接続向け）

// =========================
// 構造体定義
// =========================

/// ダウンロードレコードを一括管理するための値渡し構造体
///
/// 呼び出し側（parse.rs）で構築した情報を DB 挿入関数へ渡すために使用する。
/// `Clone` を derive することで、Postgres の非同期タスクへ所有権ごと移動できる。
#[derive(Clone)]
pub struct DownloadRecord {
    /// 添付ファイルセットの一意識別子（UUIDv7 / ハイフン付き36文字文字列）
    pub uuid: String,
    /// ダウンロード有効期限（"YYYY-MM-DD HH:MM:SS +HH:MM" 形式の文字列）
    pub expires_at: String,
    /// ZIP パスワード（None の場合はパスワードなし）
    pub zip_password: Option<String>,
    /// ダウンロード用 URL（auth_mode に応じてトークン等が付加される）
    pub url: String,
    /// 認証方式（"minimal" / "token" / "basic"）
    pub auth_mode: String,
    /// 認証補足情報（Basic 認証の user/password 等を JSON で保持、None の場合は不要）
    pub auth_info: Option<Value>,
    /// ダウンロード有効期間（時間単位 / i64 で保持し DB は BIGINT に対応）
    pub expire_hours: i64,
    /// 最大ダウンロード回数（i64 で保持し DB は BIGINT に対応）
    pub max_downloads: i64,
}

// =========================
// SQL 定数定義
// =========================

/// `download_tbl` の CREATE TABLE 文（SQLite / MySQL / Postgres 共通）
///
/// uuid を VARCHAR(36) にしているのは、MySQL が TEXT 型主キーに
/// キー長指定を要求するためである（TEXT では "ERROR 1170: key without length" が発生する）。
/// 整数フィールドは BIGINT を使用し Rust の i64 型と一致させる。
const CREATE_TABLE_SQL: &str = r#"
CREATE TABLE IF NOT EXISTS download_tbl (
    uuid          VARCHAR(36)  PRIMARY KEY,
    download_count BIGINT      NOT NULL DEFAULT 0,
    expires_at    TEXT         NOT NULL,
    zip_password  TEXT,
    url           TEXT         NOT NULL,
    auth_mode     TEXT         NOT NULL,
    auth_info     TEXT,
    expire_hours  BIGINT       NOT NULL,
    max_downloads BIGINT       NOT NULL,
    created_at    TEXT         NOT NULL
);
"#;

/// `download_tokens` の CREATE TABLE 文（SQLite 用）
///
/// SQLite では AUTOINCREMENT / TEXT / INTEGER を使用する。
/// 日時は TEXT として保存し、PHP 側で文字列比較できるようにする。
const CREATE_DOWNLOAD_TOKENS_SQLITE: &str = r#"
CREATE TABLE IF NOT EXISTS download_tokens (
    id                INTEGER     PRIMARY KEY AUTOINCREMENT,
    uuid              VARCHAR(36) NOT NULL UNIQUE,
    filename          TEXT        NOT NULL,
    created_at        TEXT        NOT NULL,
    expire_at         TEXT        NOT NULL,
    max_downloads     INTEGER     NOT NULL DEFAULT 1,
    current_downloads INTEGER     NOT NULL DEFAULT 0
);
"#;

/// `download_tokens` の CREATE TABLE 文（MySQL 用）
///
/// MySQL では AUTO_INCREMENT / DATETIME を使用する。
/// max_downloads / current_downloads は BIGINT で Rust の i64 と一致させる。
const CREATE_DOWNLOAD_TOKENS_MYSQL: &str = r#"
CREATE TABLE IF NOT EXISTS download_tokens (
    id                INT AUTO_INCREMENT PRIMARY KEY,
    uuid              VARCHAR(36) NOT NULL UNIQUE,
    filename          TEXT        NOT NULL,
    created_at        DATETIME    NOT NULL,
    expire_at         DATETIME    NOT NULL,
    max_downloads     BIGINT      NOT NULL DEFAULT 1,
    current_downloads BIGINT      NOT NULL DEFAULT 0
);
"#;

/// `download_tokens` の CREATE TABLE 文（PostgreSQL 用）
///
/// Postgres では SERIAL（自動採番）/ TIMESTAMP を使用する。
/// max_downloads / current_downloads は BIGINT で Rust の i64 と一致させる。
const CREATE_DOWNLOAD_TOKENS_POSTGRES: &str = r#"
CREATE TABLE IF NOT EXISTS download_tokens (
    id                SERIAL      PRIMARY KEY,
    uuid              VARCHAR(36) NOT NULL UNIQUE,
    filename          TEXT        NOT NULL,
    created_at        TIMESTAMP   NOT NULL,
    expire_at         TIMESTAMP   NOT NULL,
    max_downloads     BIGINT      NOT NULL DEFAULT 1,
    current_downloads BIGINT      NOT NULL DEFAULT 0
);
"#;

// =========================
// 関数定義
// =========================

/// DB を初期化する非同期関数
///
/// # 概要
/// 設定ファイルで指定された DB 種別（sqlite / mysql / postgres）に応じて接続し、
/// テーブルが存在しない場合のみ `CREATE TABLE IF NOT EXISTS` を実行する。
/// SQLite の場合は DB ファイルと親ディレクトリのパーミッション設定も行う。
///
/// # 引数
/// - `config`: MilterSeparator 設定情報（DB 接続情報を含む）
///
/// # 戻り値
/// - `Ok(())`: 初期化成功
/// - `Err(...)`: 接続失敗 / テーブル作成失敗（呼び出し元でログ出力・fallback を行うこと）
///
/// # 設計上の注意
/// Postgres は `tokio-postgres` の非同期 API を使用する。
/// 同期クレート `postgres` を Tokio ランタイム内で使うと
/// "Cannot start a runtime from within a runtime" panic が発生するため使用しない。
pub async fn init_db(config: &Config) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match config.database_type.as_str() {
        // ===== SQLite =====
        "sqlite" => {
            if let Some(path) = &config.database_path {
                // SQLite の場合、親ディレクトリが存在しないと open() 自体が失敗するため
                // 事前に create_dir_all で作成する（既存の場合は何もしない）
                if let Some(parent) = std::path::Path::new(path).parent() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        crate::printdaytimeln!(
                            crate::init::LOG_INFO,
                            "[db] warn: could not create parent dir {}: {}",
                            parent.display(),
                            e
                        );
                    } else {
                        // PHP/Apache が SQLite のジャーナルファイルを作成できるよう
                        // ディレクトリに setgid (2770) を設定する
                        if let Ok(meta) = std::fs::metadata(parent) {
                            let mut perms = meta.permissions();
                            perms.set_mode(0o2770); // rwxrws--- (setgid)
                            if let Err(e) = std::fs::set_permissions(parent, perms) {
                                crate::printdaytimeln!(
                                    crate::init::LOG_INFO,
                                    "[db] warn: could not chmod parent dir {}: {}",
                                    parent.display(),
                                    e
                                );
                            }
                        }
                        // ディレクトリの所有者を milter:apache 等に変更（best-effort）
                        let chown_target =
                            format!("{}:{}", &config.milter_user, &config.milter_group);
                        let _ = Command::new("chown")
                            .arg("-R")
                            .arg(&chown_target)
                            .arg(parent)
                            .status();
                    }
                }

                // SQLite DB ファイルをオープン（存在しない場合は新規作成）
                let conn = rusqlite::Connection::open(path)?;
                // ダウンロードレコード管理テーブルを作成（既存の場合はスキップ）
                conn.execute_batch(CREATE_TABLE_SQL)?;
                // ダウンロードトークン管理テーブルを作成（SQLite 用 SQL を使用）
                conn.execute_batch(CREATE_DOWNLOAD_TOKENS_SQLITE)?;

                // DB ファイルにグループ書き込み権限を付与（PHP からも書き込めるようにする）
                if let Ok(meta) = std::fs::metadata(path) {
                    let mut perms = meta.permissions();
                    perms.set_mode(0o660); // rw-rw----
                    if let Err(e) = std::fs::set_permissions(path, perms) {
                        crate::printdaytimeln!(
                            crate::init::LOG_INFO,
                            "[db] warn: could not chmod db file {}: {}",
                            path,
                            e
                        );
                    }
                }
                // DB ファイルの所有者を milter:apache に変更（best-effort）
                let chown_target = format!("{}:{}", &config.milter_user, &config.milter_group);
                let _ = Command::new("chown").arg(&chown_target).arg(path).status();

                crate::printdaytimeln!(
                    crate::init::LOG_INFO,
                    "[db] initialized sqlite at {}",
                    path
                );
            } else {
                // 設定ファイルに Database_Path が存在しない場合は初期化をスキップ
                crate::printdaytimeln!(
                    crate::init::LOG_INFO,
                    "[db] sqlite selected but no Database_Path provided"
                );
            }
        }

        // ===== MySQL / MariaDB =====
        "mysql" => {
            // 設定値を読み込み、デフォルト値でフォールバック
            let host = config.database_host.as_deref().unwrap_or("localhost");
            let port = config.database_port.unwrap_or(3306);
            let user = config.database_user.as_deref().unwrap_or("");
            let pass = config.database_password.as_deref().unwrap_or("");
            let db = config.database_name.as_deref().unwrap_or("");

            // mysql クレートは URL 形式で接続設定を受け取る
            let url = format!("mysql://{}:{}@{}:{}/{}", user, pass, host, port, db);
            let opts = mysql::Opts::from_url(&url)?;
            // 接続プールを生成して 1 本接続を借用
            let pool = mysql::Pool::new(opts)?;
            let mut conn = pool.get_conn()?;

            // ダウンロードレコード管理テーブルを作成（既存の場合はスキップ）
            conn.query_drop(CREATE_TABLE_SQL)?;
            // ダウンロードトークン管理テーブルを作成（MySQL 用 DATETIME 型を使用）
            conn.query_drop(CREATE_DOWNLOAD_TOKENS_MYSQL)?;

            crate::printdaytimeln!(
                crate::init::LOG_INFO,
                "[db] initialized mysql on {}:{}",
                host,
                port
            );
        }

        // ===== PostgreSQL =====
        "postgres" => {
            // 設定値を読み込み、デフォルト値でフォールバック
            let host = config.database_host.as_deref().unwrap_or("localhost");
            let port = config.database_port.unwrap_or(5432);
            let user = config.database_user.as_deref().unwrap_or("");
            let pass = config.database_password.as_deref().unwrap_or("");
            let db = config.database_name.as_deref().unwrap_or("");

            // tokio-postgres はキースペース区切りの接続文字列を使用する
            let connstr = format!(
                "host={} port={} user={} password={} dbname={}",
                host, port, user, pass, db
            );

            // 非同期接続を確立する（await しているため Tokio ランタイム上で動作）
            match tokio_pg::connect(&connstr, NoTls).await {
                Ok((client, connection)) => {
                    // `connection` は I/O ポーリングを担うオブジェクトで、
                    // 別タスクで駆動させないと `client` の操作がブロックする。
                    tokio::spawn(async move {
                        if let Err(e) = connection.await {
                            crate::printdaytimeln!(
                                crate::init::LOG_INFO,
                                "[db] tokio_postgres connection error: {}",
                                e
                            );
                        }
                    });

                    // テーブルを一括作成（IF NOT EXISTS なので冪等）
                    client.batch_execute(CREATE_TABLE_SQL).await?;
                    client
                        .batch_execute(CREATE_DOWNLOAD_TOKENS_POSTGRES)
                        .await?;

                    crate::printdaytimeln!(
                        crate::init::LOG_INFO,
                        "[db] initialized postgres on {}:{}",
                        host,
                        port
                    );
                }
                Err(e) => {
                    // 接続失敗は復旧不能なためエラーを上位に伝播させる
                    return Err(Box::new(e));
                }
            }
        }

        // ===== 未知の DB 種別 =====
        other => {
            // 設定ミスに備えて警告を出して処理をスキップする（panic はしない）
            crate::printdaytimeln!(
                crate::init::LOG_INFO,
                "[db] unknown database type '{}', skipping init",
                other
            );
        }
    }
    Ok(())
}

/// ダウンロードレコードを DB へ挿入する関数
///
/// # 概要
/// 設定で指定された DB 種別に応じて `download_tbl` に 1 件レコードを挿入する。
/// uuid が既存の場合は `expires_at` のみ更新（UPSERT）することで冪等性を保つ。
///
/// Postgres の場合のみ、Tokio ランタイム内で同期ブロッキング接続を開けない制約があるため
/// `tokio::spawn` でバックグラウンドタスクとして非同期実行する。
/// そのためエラーはタスク内でログに記録し、呼び出し元には伝播しない。
///
/// # 引数
/// - `config`: MilterSeparator 設定情報（DB 接続情報を含む）
/// - `record`: 挿入するダウンロードレコード情報
///
/// # 戻り値
/// - `Ok(())`: 挿入成功（Postgres の場合はタスク投入が成功した時点で Ok を返す）
/// - `Err(...)`: SQLite / MySQL の挿入失敗（Postgres タスク内のエラーは Err にならない）
pub fn insert_download_record(
    config: &Config,
    record: &DownloadRecord,
) -> Result<(), Box<dyn std::error::Error>> {
    // auth_info（JSON Value）を文字列に変換して DB の TEXT カラムに保存できる形にする
    let auth_info_str = record.auth_info.as_ref().map(|v| v.to_string());
    // レコード作成日時を JST で取得（DB の TEXT カラムに "YYYY-MM-DD HH:MM:SS +09:00" 形式で保存）
    let created_at = Local::now().format("%Y-%m-%d %H:%M:%S %:z").to_string();

    match config.database_type.as_str() {
        // ===== SQLite =====
        "sqlite" => {
            if let Some(path) = &config.database_path {
                // SQLite DB をオープンして INSERT OR REPLACE を実行
                // uuid が既存の場合はレコード全体を置き換える
                let conn = rusqlite::Connection::open(path)?;
                conn.execute(
                    "INSERT OR REPLACE INTO download_tbl \
                     (uuid, download_count, expires_at, zip_password, url, auth_mode, auth_info, \
                      expire_hours, max_downloads, created_at) \
                     VALUES (?1, 0, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                    rusqlite::params![
                        record.uuid,
                        record.expires_at,
                        record.zip_password.as_deref(), // Option<String> → Option<&str>
                        record.url,
                        record.auth_mode,
                        auth_info_str, // Option<String> → NULL or TEXT
                        record.expire_hours,
                        record.max_downloads,
                        created_at
                    ],
                )?;
            }
        }

        // ===== MySQL / MariaDB =====
        "mysql" => {
            // 設定値を読み込み
            let host = config.database_host.as_deref().unwrap_or("localhost");
            let port = config.database_port.unwrap_or(3306);
            let user = config.database_user.as_deref().unwrap_or("");
            let pass = config.database_password.as_deref().unwrap_or("");
            let db = config.database_name.as_deref().unwrap_or("");

            // mysql クレートは毎回 Pool を生成するが、
            // 通常の運用では DB 接続頻度が低いため現状はプールを使い捨てにしている
            let url = format!("mysql://{}:{}@{}:{}/{}", user, pass, host, port, db);
            let opts = mysql::Opts::from_url(&url)?;
            let pool = mysql::Pool::new(opts)?;
            let mut conn = pool.get_conn()?;

            // REPLACE INTO は uuid が PRIMARY KEY に一致する既存行を削除してから挿入する
            conn.exec_drop(
                "REPLACE INTO download_tbl \
                 (uuid, download_count, expires_at, zip_password, url, auth_mode, auth_info, \
                  expire_hours, max_downloads, created_at) \
                 VALUES (:uuid, 0, :expires_at, :zip_password, :url, :auth_mode, :auth_info, \
                  :expire_hours, :max_downloads, :created_at)",
                mysql::params! {
                    "uuid"          => &record.uuid,
                    "expires_at"    => &record.expires_at,
                    "zip_password"  => record.zip_password.as_deref(), // Option<&str> で NULL 送信
                    "url"           => &record.url,
                    "auth_mode"     => &record.auth_mode,
                    "auth_info"     => auth_info_str.as_deref().unwrap_or(""), // None → 空文字
                    "expire_hours"  => record.expire_hours,
                    "max_downloads" => record.max_downloads,
                    "created_at"    => &created_at,
                },
            )?;
        }

        // ===== PostgreSQL =====
        "postgres" => {
            // 設定値を読み込み
            let host = config.database_host.as_deref().unwrap_or("localhost");
            let port = config.database_port.unwrap_or(5432);
            let user = config.database_user.as_deref().unwrap_or("");
            let pass = config.database_password.as_deref().unwrap_or("");
            let db = config.database_name.as_deref().unwrap_or("");

            // Tokio ランタイム内で同期 DB 接続を開こうとすると
            // "Cannot start a runtime from within a runtime" panic が発生する。
            // そのため owned の値に変換してから tokio::spawn に渡す。
            let host = host.to_string();
            let user = user.to_string();
            let pass = pass.to_string();
            let db = db.to_string();
            let record_cloned = record.clone(); // Clone トレイトが必要な理由
            let auth_info_cloned = auth_info_str.clone(); // Option<String> は Copy 不可
            let created_at_cloned = created_at.clone();

            // バックグラウンドタスクとして Postgres 挿入を実行
            // エラーはタスク内でログに記録する（呼び出し元には伝播しない）
            tokio::spawn(async move {
                let connstr = format!(
                    "host={} port={} user={} password={} dbname={}",
                    host, port, user, pass, db
                );

                crate::printdaytimeln!(
                    crate::init::LOG_DEBUG,
                    "[db] tokio_postgres attempting connect to {}:{}",
                    host,
                    port
                );

                match tokio_pg::connect(&connstr, NoTls).await {
                    Ok((client, connection)) => {
                        // connection を別タスクで駆動しないと client 操作がブロックする
                        tokio::spawn(async move {
                            if let Err(e) = connection.await {
                                crate::printdaytimeln!(
                                    crate::init::LOG_INFO,
                                    "[db] tokio_postgres connection error: {}",
                                    e
                                );
                            }
                        });

                        // Option<String> → Option<&str> に変換する
                        // tokio-postgres の ToSql トレイトは Option<String> を直接受け付けないため
                        let zip_pw_param: Option<&str> = record_cloned.zip_password.as_deref();
                        let auth_info_param: Option<&str> = auth_info_cloned.as_deref();

                        // デバッグ時のみパラメータ一覧を出力（通常運用では非表示）
                        crate::printdaytimeln!(
                            crate::init::LOG_DEBUG,
                            "[db] tokio_postgres exec params: uuid={}, expires_at={}, \
                             zip_pw_present={}, url={}, auth_mode={}, auth_info_present={}, \
                             expire_hours={}, max_downloads={}, created_at={}",
                            record_cloned.uuid,
                            record_cloned.expires_at,
                            zip_pw_param.is_some(),
                            record_cloned.url,
                            record_cloned.auth_mode,
                            auth_info_param.is_some(),
                            record_cloned.expire_hours,
                            record_cloned.max_downloads,
                            created_at_cloned
                        );

                        // INSERT … ON CONFLICT (uuid) DO UPDATE で UPSERT を実現する
                        // uuid が既存の場合は expires_at だけ上書きする（冪等設計）
                        // i64 を Postgres BIGINT へ渡す際は型が一致しているため ToSql が通る
                        match client
                            .execute(
                                "INSERT INTO download_tbl \
                             (uuid, download_count, expires_at, zip_password, url, auth_mode, \
                              auth_info, expire_hours, max_downloads, created_at) \
                             VALUES ($1, 0, $2, $3, $4, $5, $6, $7, $8, $9) \
                             ON CONFLICT (uuid) DO UPDATE SET expires_at = EXCLUDED.expires_at",
                                &[
                                    &record_cloned.uuid,
                                    &record_cloned.expires_at,
                                    &zip_pw_param, // Option<&str> → NULL or TEXT
                                    &record_cloned.url,
                                    &record_cloned.auth_mode,
                                    &auth_info_param, // Option<&str> → NULL or TEXT
                                    &record_cloned.expire_hours, // i64 → BIGINT
                                    &record_cloned.max_downloads, // i64 → BIGINT
                                    &created_at_cloned,
                                ],
                            )
                            .await
                        {
                            Ok(rows) => {
                                // 1 行挿入（または更新）成功
                                crate::printdaytimeln!(
                                    crate::init::LOG_INFO,
                                    "[db] tokio_postgres insert success: {} rows, uuid={}",
                                    rows,
                                    record_cloned.uuid
                                );
                            }
                            Err(e) => {
                                // INSERT 失敗（型不一致・制約違反等）
                                crate::printdaytimeln!(
                                    crate::init::LOG_INFO,
                                    "[db] tokio_postgres execute error: {:?}",
                                    e
                                );
                            }
                        }
                    }
                    Err(e) => {
                        // 接続失敗（認証エラー・DB 未起動等）
                        crate::printdaytimeln!(
                            crate::init::LOG_INFO,
                            "[db] tokio_postgres connect error: {}",
                            e
                        );
                    }
                }
            });
        }

        // ===== 未知の DB 種別 =====
        _ => {
            // 初期化時に既に警告を出しているため、ここでは黙ってスキップ
        }
    }
    Ok(())
}
