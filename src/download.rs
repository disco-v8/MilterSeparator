// =========================
// download.rs
// ダウンロード URL 生成 API（Axum 用ハンドラ）
//
// 概要:
// - 管理画面や外部システムからダウンロード用 URL を生成するための小さな HTTP API を提供します。
// - 認証方式は `minimal` / `basic` / `token` の3種類をサポートします。
//
// 実装上の注意:
// - このモジュールは「URL を生成して返す」役割のみを持ちます。実際の配信や認証は Web サーバー
//   (nginx, apache) や別の Axum ハンドラに任せる設計を想定しています。
// - `basic` モードはユーザー名/パスワードを返しますが、検証はサーバ側で行ってください。
// - `token` モードは HMAC(token_auth_key, uuid) を生成して URL に付加します。トークン有効期限
//   や検証は受け側で実装してください（ここでは生成のみ）。

#![allow(dead_code, unused_variables, unused_imports)]

use axum::{Router, extract::Json, response::IntoResponse, routing::post};
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use bcrypt::{DEFAULT_COST, hash};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Sha256, Sha512};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

// HMAC 型の短縮名
type HmacSha256 = Hmac<Sha256>;
type HmacSha512 = Hmac<Sha512>;

/// Basic 認証用設定（オプション）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BasicConfig {
    /// BASIC 認証のユーザー名
    pub basic_auth_user: Option<String>,
    /// BASIC 認証のパスワード
    pub basic_auth_password: Option<String>,
}

/// Token 認証用設定（オプション）
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenConfig {
    /// HMAC 生成に使う秘密鍵
    pub token_auth_key: Option<String>,
    /// HMAC アルゴリズム指定（例: "hmac-sha256", "hmac-sha512"）
    pub token_auth_type: Option<String>,
}

/// ダウンロード URL 生成リクエストの JSON 形式
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateRequest {
    /// 対象の UUIDv7（ディレクトリ名）
    pub uuid: String,
    /// ベース URL（例: https://example.com/download）
    pub base_url: String,
    /// ダウンロード認証モード: "minimal" | "basic" | "token"
    pub download_auth_mode: String,
    /// 認証固有の追加設定
    pub config: Option<GenerateConfig>,
}

/// GenerateRequest の中でネストされる認証設定
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateConfig {
    pub basic: Option<BasicConfig>,
    pub token: Option<TokenConfig>,
}

/// Basic 認証情報をレスポンスで返す場合の構造体
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthInfo {
    pub username: String,
    pub password: String,
}

/// ダウンロード URL 生成 API のレスポンス JSON
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateResponse {
    /// 生成したダウンロード URL
    pub download_url: String,
    /// BASIC 認証情報（必要時のみ）
    pub auth_info: Option<AuthInfo>,
}

/// `/generate_download` ハンドラ本体
///
/// 入力: JSON (GenerateRequest)
/// 出力: JSON (GenerateResponse)
pub async fn generate_download(Json(req): Json<GenerateRequest>) -> impl IntoResponse {
    // download_auth_mode を小文字化して比較
    let mode = req.download_auth_mode.to_ascii_lowercase();
    match mode.as_str() {
        // 認証不要: UUID 存在チェックのみを行う想定（ここでは生成のみ）
        "minimal" => {
            let url = format!("{}/{}", req.base_url.trim_end_matches('/'), req.uuid);
            let resp = GenerateResponse {
                download_url: url,
                auth_info: None,
            };
            (axum::http::StatusCode::OK, Json(resp))
        }
        // BASIC: サーバ側が BASIC 認証を行う想定なので認証情報を返す
        "basic" => {
            let url = format!("{}/{}", req.base_url.trim_end_matches('/'), req.uuid);
            let (u, p) = if let Some(cfg) = &req.config {
                if let Some(b) = &cfg.basic {
                    (
                        b.basic_auth_user.clone().unwrap_or_default(),
                        b.basic_auth_password.clone().unwrap_or_default(),
                    )
                } else {
                    ("".to_string(), "".to_string())
                }
            } else {
                ("".to_string(), "".to_string())
            };
            let auth = AuthInfo {
                username: u,
                password: p,
            };
            let resp = GenerateResponse {
                download_url: url,
                auth_info: Some(auth),
            };
            (axum::http::StatusCode::OK, Json(resp))
        }
        // token: HMAC を生成して URL に付与する
        "token" => {
            let token = if let Some(cfg) = &req.config {
                if let Some(t) = &cfg.token {
                    if let Some(key) = &t.token_auth_key {
                        let algo = t.token_auth_type.as_deref().unwrap_or("hmac-sha256");
                        // uuid を HMAC したバイナリを base64url (no pad) にする
                        match algo.to_ascii_lowercase().as_str() {
                            "hmac-sha256" => {
                                let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
                                mac.update(req.uuid.as_bytes());
                                let result = mac.finalize();
                                let bytes = result.into_bytes();
                                URL_SAFE_NO_PAD.encode(bytes)
                            }
                            "hmac-sha512" => {
                                let mut mac = HmacSha512::new_from_slice(key.as_bytes()).unwrap();
                                mac.update(req.uuid.as_bytes());
                                let result = mac.finalize();
                                let bytes = result.into_bytes();
                                URL_SAFE_NO_PAD.encode(bytes)
                            }
                            _ => {
                                // 未知のアルゴリズムは sha256 をフォールバック
                                let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
                                mac.update(req.uuid.as_bytes());
                                let result = mac.finalize();
                                let bytes = result.into_bytes();
                                URL_SAFE_NO_PAD.encode(bytes)
                            }
                        }
                    } else {
                        "".to_string()
                    }
                } else {
                    "".to_string()
                }
            } else {
                "".to_string()
            };

            let mut url = format!("{}/{}", req.base_url.trim_end_matches('/'), req.uuid);
            if !token.is_empty() {
                // トークンをクエリ文字列に付与
                url = format!("{}?token={}", url, token);
            }
            let resp = GenerateResponse {
                download_url: url,
                auth_info: None,
            };
            (axum::http::StatusCode::OK, Json(resp))
        }
        // 未知のモードは minimal 相当で返す
        _ => {
            let url = format!("{}/{}", req.base_url.trim_end_matches('/'), req.uuid);
            let resp = GenerateResponse {
                download_url: url,
                auth_info: None,
            };
            (axum::http::StatusCode::OK, Json(resp))
        }
    }
}

/// このモジュール向けの Router を構築して返すヘルパー
/// 通常はアプリケーション側で `router()` をマウントして使用します。
pub fn router() -> Router {
    Router::new().route("/generate_download", post(generate_download))
}

/// 作成済みの UUID ディレクトリ（/var/lib/milterseparator/<uuid>/）に
/// ダウンロード用静的ファイルを生成する。
///
/// - `dir` は UUID ディレクトリのパス
/// - テンプレートファイルはまず `/etc/MilterSeparator.d/templates/download_template.html` を探す。
///   見つからなければカレント相対の `etc/MilterSeparator.d/templates/download_template.html`
///   を試す。どちらも無ければ簡易テンプレートで生成する。
pub fn write_download_static_files(dir: &Path, config: &crate::init::Config) -> Result<(), String> {
    // mailinfo.txt を読み込む
    let mailinfo_path = dir.join("mailinfo.txt");
    let s =
        fs::read_to_string(&mailinfo_path).map_err(|e| format!("read mailinfo error: {}", e))?;
    let mailinfo: JsonValue =
        serde_json::from_str(&s).map_err(|e| format!("parse mailinfo json error: {}", e))?;

    // 値を取り出す（安全に）
    let subject = mailinfo
        .get("subject")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let from = mailinfo.get("from").and_then(|v| v.as_str()).unwrap_or("");
    let to = mailinfo.get("to").and_then(|v| v.as_str()).unwrap_or("");

    // zip 情報
    let zip_file = mailinfo
        .get("zip")
        .and_then(|z| z.get("file"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let zip_password = mailinfo
        .get("zip")
        .and_then(|z| z.get("password"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    // download 情報
    let download = mailinfo.get("download").and_then(|d| d.as_object());
    let download_url = download
        .and_then(|d| d.get("url"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let auth_mode = download
        .and_then(|d| d.get("auth_mode"))
        .and_then(|v| v.as_str())
        .unwrap_or("minimal");

    let auth_info = download.and_then(|d| d.get("auth_info"));

    // attachments を HTML リスト化（サイズは人間に読みやすい単位で表示）
    let mut attachments_html = String::new();
    if let Some(arr) = mailinfo.get("attachments").and_then(|v| v.as_array()) {
        attachments_html.push_str("<ul>\n");
        for a in arr {
            let fname = a.get("filename").and_then(|v| v.as_str()).unwrap_or("");
            let size_val = a.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            let size = human_readable_size(size_val);
            attachments_html.push_str(&format!("  <li>{} ({})</li>\n", html_escape(fname), html_escape(&size)));
        }
        attachments_html.push_str("</ul>\n");
    }

    // テンプレートを探す
    let candidates = [
        PathBuf::from("/etc/MilterSeparator.d/templates/download_template.html"),
        PathBuf::from("etc/MilterSeparator.d/templates/download_template.html"),
    ];
    let mut template = None;
    for p in &candidates {
        if let Ok(t) = fs::read_to_string(p) {
            template = Some(t);
            break;
        }
    }
    let template = template.unwrap_or_else(|| default_template().to_string());

    // トークン（URL に含まれる場合）を抽出
    let token_in_url = extract_query_param(download_url, "token");

    // ZIP への直接リンクを作成（download_url のクエリを除去してディレクトリパスにする）
    let zip_url = if !download_url.is_empty() {
        let base = if let Some(pos) = download_url.find('?') {
            &download_url[..pos]
        } else {
            download_url
        };
        // avoid double slash
        if base.ends_with('/') {
            format!("{}{}", base, zip_file)
        } else {
            format!("{}/{}", base, zip_file)
        }
    } else {
        zip_file.to_string()
    };

    // マスクしたパスワード（表示用）と実パスワード（data-attribute）を分ける
    let masked_pw = if zip_password.is_empty() {
        "".to_string()
    } else {
        let len = zip_password.chars().count();
        let n = std::cmp::max(6, len);
        "•".repeat(n)
    };

    // 常に mailinfo.txt 等が外部から参照されないよう .htaccess を出力する
    // Basic モード時は追加で Basic 設定を追記する（下で置換・追記される）
    let deny_mailinfo = "<Files \"mailinfo.txt\">\n  Require all denied\n</Files>\n<Files \".htpasswd\">\n  Require all denied\n</Files>\n";

    // 基本置換
    let mut html = template
        .replace("{{subject}}", &html_escape(subject))
        .replace("{{from}}", &html_escape(from))
        .replace("{{to}}", &html_escape(to))
        .replace("{{zip_file}}", &html_escape(zip_file))
        .replace("{{zip_url}}", &html_escape(&zip_url))
        .replace("{{zip_password_masked}}", &html_escape(&masked_pw))
        .replace("{{zip_password}}", &html_escape(zip_password))
        .replace("{{download_url}}", &html_escape(download_url))
        .replace("{{attachments}}", &attachments_html);

    // 追加情報: generated_at, zip_size
    let generated_at = mailinfo
        .get("generated_at")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let zip_size_val = mailinfo
        .get("zip")
        .and_then(|z| z.get("size"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let zip_size = human_readable_size(zip_size_val);
    html = html.replace("{{generated_at}}", &html_escape(generated_at));
    html = html.replace("{{zip_size}}", &html_escape(&zip_size));

    // BASIC 認証情報がある場合は埋め込み
    if auth_mode.eq_ignore_ascii_case("basic") {
        if let Some(ai) = auth_info {
            let u = ai.get("username").and_then(|v| v.as_str()).unwrap_or("");
            let p = ai.get("password").and_then(|v| v.as_str()).unwrap_or("");
            html = html.replace("{{basic_auth_user}}", &html_escape(u));
            html = html.replace("{{basic_auth_password}}", &html_escape(p));

            // .htaccess と .htpasswd を生成（簡易）
            // Basic 設定に mailinfo 保護ルールを追記する
            let htaccess = format!(
                "AuthType Basic\nAuthName \"MilterSeparator\"\nAuthUserFile {}/.htpasswd\nRequire valid-user\n\n{}",
                dir.display(), deny_mailinfo
            );
            let htaccess_path = dir.join(".htaccess");
            if let Err(e) = fs::write(&htaccess_path, htaccess) {
                return Err(format!("write .htaccess error: {}", e));
            }
            // htpasswd の bcrypt 形式で保存する
            let htpasswd_path = dir.join(".htpasswd");
            match hash(p, DEFAULT_COST) {
                Ok(hashed) => {
                    // bcrypt ハッシュはそのまま htpasswd ファイルに保存可能（username:hash）
                    if let Err(e) = fs::write(&htpasswd_path, format!("{}:{}\n", u, hashed)) {
                        return Err(format!("write .htpasswd error: {}", e));
                    }
                }
                Err(e) => {
                    // ハッシュ化に失敗した場合はエラーを返す
                    return Err(format!("bcrypt hash error: {}", e));
                }
            }
        }
    } else {
        html = html.replace("{{basic_auth_user}}", "");
        html = html.replace("{{basic_auth_password}}", "");
        // Basic 以外のモードでも mailinfo.txt を保護する .htaccess を出力
        let htaccess_path = dir.join(".htaccess");
        if let Err(e) = fs::write(&htaccess_path, deny_mailinfo) {
            return Err(format!("write .htaccess error: {}", e));
        }
    }

    // token モードでは token.js を生成し、HTML にスクリプトタグがあればそれをそのまま使う
    if auth_mode.eq_ignore_ascii_case("token") {
        let token_js = generate_token_js(token_in_url.as_deref().unwrap_or(""));
        let token_js_path = dir.join("token.js");
        if let Err(e) = fs::write(&token_js_path, token_js) {
            return Err(format!("write token.js error: {}", e));
        }
    }

    // download.html を書き出す
    // UUID を取得してテンプレート内で使う
    let uuid = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // ensure template can receive {{uuid}}
    let out_path = dir.join("download.html");
    let mut f =
        fs::File::create(&out_path).map_err(|e| format!("create download.html error: {}", e))?;
    // replace uuid placeholder if present
    let html = html.replace("{{uuid}}", uuid);
    // ensure downloadZip calls configured counter CGI and then redirects to the ZIP URL
    // This overrides any template-provided function to guarantee counting behavior
    let counter_name = &config.counter_cgi;
    let override_script = format!(
        "<script>function downloadZip(){{fetch('{counter}?uuid={uuid}').finally(()=>{{window.location.href='{zip_url}';}});}}</script>",
        counter = counter_name,
        uuid = uuid,
        zip_url = html_escape(&zip_url)
    );
    let html = format!("{}{}", html, override_script);
    f.write_all(html.as_bytes())
        .map_err(|e| format!("write download.html error: {}", e))?;

    Ok(())
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn extract_query_param(url: &str, key: &str) -> Option<String> {
    if let Some(pos) = url.find('?') {
        let qs = &url[pos + 1..];
        for pair in qs.split('&') {
            let mut iter = pair.splitn(2, '=');
            if let Some(k) = iter.next()
                && k == key
            {
                if let Some(v) = iter.next() {
                    return Some(v.to_string());
                }
                return Some(String::new());
            }
        }
    }
    None
}

fn generate_token_js(expected_token: &str) -> String {
    // シンプルなクライアント側トークン検証スクリプト
    format!(
        r#"(function(){{
    function getQueryParam(name){{
        const params = new URLSearchParams(window.location.search);
        return params.get(name);
    }}
    const token = getQueryParam('token');
    const expected = '{}';
    if(!expected){{
        // no expected token configured; allow
        return;
    }}
    if(token !== expected){{
        document.body.innerHTML = '<h1>Invalid token</h1><p>ダウンロード用のトークンが一致しません。</p>';
    }}
}})();"#,
        expected_token
    )
}

fn default_template() -> &'static str {
    r#"<!doctype html>
<html><head><meta charset="utf-8"><title>Download</title></head>
<body>
<h1>{{subject}}</h1>
<p><strong>From:</strong> {{from}}<br><strong>To:</strong> {{to}}</p>
<h2>Attachments</h2>
{{attachments}}
<p>ZIP: <button onclick="downloadZip()">Download ZIP</button></p>
<p>Password: {{zip_password}}</p>
<script>
function downloadZip() {
    const uuid = "{{uuid}}";
    fetch("count.cgi?uuid=" + encodeURIComponent(uuid))
        .finally(() => {
            window.location.href = uuid + '.zip';
        });
}
</script>
</body></html>"#
}

fn human_readable_size(n: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = n as f64;
    let mut idx = 0usize;
    while size >= 1024.0 && idx < UNITS.len() - 1 {
        size /= 1024.0;
        idx += 1;
    }
    if idx == 0 {
        format!("{} {}", n, UNITS[idx])
    } else {
        format!("{:.1} {}", size, UNITS[idx])
    }
}
