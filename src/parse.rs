// =========================
// parse.rs
// MilterSeparator メールパース処理モジュール
//
// 【このファイルで使う主なクレート】
// - mail_parser: MIMEメール構造解析・ヘッダ抽出・エンコーディング処理（Message, MessageParser, MimeHeaders等）
// - std: 標準ライブラリ（コレクション、I/O、文字列操作など）
// - crate::printdaytimeln!: JSTタイムスタンプ付きログ出力マクロ
//
// 【役割】
// - BODYEOB時のヘッダ＋ボディ合体処理
// - mail-parserによるMIME構造パース（マルチパート対応）
// - From/To/Subject/Content-Type/エンコーディング等のメタ情報抽出・出力
// - テキストパート（text/plain, text/html）の本文抽出・出力
// - 非テキストパート（添付ファイル等）の属性情報抽出・出力
// - NULバイト混入の可視化・除去処理
// - パース済みデータを構造化して返却する処理
// =========================

use base64::Engine as _;
use base64::engine::general_purpose;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use encoding_rs::Encoding;
use hmac::{Hmac, Mac};
use mail_parser::{MessageParser, MimeHeaders, PartType}; // メールパース・MIMEヘッダアクセス用
use serde_json::json;
use sha2::{Sha256, Sha512};
use std::collections::HashMap;
use std::fs;
use std::io::Cursor;
use std::io::Read as IoRead;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;

type HmacSha256 = Hmac<Sha256>;
type HmacSha512 = Hmac<Sha512>;
use crate::db;
use crate::init::{LOG_DEBUG, LOG_INFO, LOG_TRACE};
use crate::zipper; // 添付保存ユーティリティ
use chrono::Local;
use uuid::Uuid;

/// 不可視文字と制御文字を包括的に除去する関数
///
/// # 引数
/// - `s`: 処理対象の文字列
///
/// # 戻り値
/// - String: 不可視文字と制御文字を除去した文字列
///
/// # 説明
/// - C0/C1制御文字（改行・タブ・スペース以外）
/// - ゼロ幅文字（\u200B-\u200F）、BOM（\uFEFF）
/// - BiDi制御文字（\u202A-\u202E）
/// - 結合記号（\u0300-\u036F）
/// - 不可視スペース類（\u2000-\u200A, \u00A0, \u202F）
/// - 異字体セレクタ（\uFE00-\uFE0F）
/// - その他の不可視文字（\u00AD, \u034F, \u180E等）を除去
pub fn remove_invisible_and_bidi_chars(s: &str) -> String {
    s.chars().filter(|&c| !is_invisible_or_bidi(c)).collect()
}

/// 文字が不可視文字またはBiDi制御文字かを判定する関数
///
/// # 引数
/// - `c`: 判定対象の文字
///
/// # 戻り値
/// - bool: 除去対象ならtrue
pub fn is_invisible_or_bidi(c: char) -> bool {
    let code = c as u32;

    // 制御文字（改行・タブ・スペースは除く）
    if c.is_control() && !matches!(c, '\n' | '\r' | '\t' | ' ') {
        return true;
    }

    // 包括的な不可視文字・制御文字の除去
    code == 0xFEFF || // BOM
    (0x0000..=0x001F).contains(&code) || // C0 controls
    code == 0x007F || // DEL
    (0x200B..=0x200F).contains(&code) || // ZWSP, ZWNJ, ZWJ, LRM, RLM
    (0x202A..=0x202E).contains(&code) || // Bidi controls
    (0x2060..=0x206F).contains(&code) || // Word Joiner etc.
    (0x0300..=0x036F).contains(&code) || // Combining diacritics
    (0x2000..=0x200A).contains(&code) || // Invisible spaces
    code == 0x202F || // Narrow NBSP
    code == 0x00A0 || // NBSP
    (0xFE00..=0xFE0F).contains(&code) || // Variation Selectors
    code == 0x180E || // Mongolian Vowel Separator
    code == 0x00AD || // Soft hyphen
    code == 0x034F // Combining grapheme joiner
}

/// BODYEOB時にヘッダ＋ボディを合体してメール全体をパース・出力する関数
///
/// # 引数
/// - `header_fields`: Milterで受信したヘッダ情報（HashMap<String, Vec<String>>）
/// - `body_field`: Milterで受信したボディ情報（文字列）
/// - `macro_fields`: Milterで受信したマクロ情報（HashMap<String, String>）
///
/// # 戻り値
/// - Some(()): メール処理成功（添付保存・閲覧URL生成まで完了）
/// - None: 対象外IPや処理スキップ時
///
/// # 説明
/// 1. ヘッダ＋ボディを合体してメール全体の生データを構築
/// 2. mail-parserでMIME構造をパース
/// 3. From/To/Subject/Content-Type/エンコーディング等の情報を出力（デバッグ用）
/// 4. パートごとのテキスト/非テキスト判定・出力（デバッグ用）
/// 5. 添付ファイル名抽出・属性出力（デバッグ用）
/// 6. NULバイト混入の可視化・除去
pub fn parse_mail(
    header_fields: &HashMap<String, Vec<String>>,
    body_field: &str,
    macro_fields: &HashMap<String, String>,
    storage_root: &str,
    remote_ip_target: u8, // 0=外部のみ(ループバック拒否), 1=内部のみ(ループバックのみ許可), 2=全て許可
    config: &crate::init::Config,
) -> Option<()> {
    // ヘッダ情報とボディ情報を合体し、RFC準拠のメール全体文字列を作成
    let mut mail_string = String::new(); // メール全体の文字列構築用バッファ

    // Milterで受信した各ヘッダを「ヘッダ名: 値」形式でメール文字列に追加
    for (k, vlist) in header_fields {
        // 同一ヘッダ名で複数値がある場合（Received等）も全て処理
        for v in vlist {
            mail_string.push_str(&format!("{k}: {v}\r\n")); // RFC準拠のCRLF改行
        }
    }

    mail_string.push_str("\r\n"); // ヘッダ部とボディ部の区切り空行（RFC必須）

    // ボディ部の改行コードをCRLFに統一（OS依存の改行コード差異を吸収）
    let body_crlf = body_field.replace("\r\n", "\n").replace('\n', "\r\n");
    mail_string.push_str(&body_crlf); // 正規化されたボディを追加

    // NULバイト（\0）を可視化文字に置換してデバッグ出力用に整形
    let mail_string_visible = mail_string.replace("\0", "<NUL>");
    // 生メール全体の可視化出力は詳細デバッグ時にのみ表示する
    crate::printdaytimeln!(LOG_DEBUG, "[parser] --- BODYEOB時のメール全体 ---");
    crate::printdaytimeln!(LOG_DEBUG, "{}", mail_string_visible); // 生メールデータは DEBUG レベルで出力
    crate::printdaytimeln!(
        LOG_DEBUG,
        "[parser] --- BODYEOB時のメール全体、ここまで ---"
    );

    // mail-parserでメール全体をパース（バイト配列として処理）
    let parser = MessageParser::default(); // パーサーインスタンス生成（デフォルト設定）
    if let Some(msg) = parser.parse(mail_string.as_bytes()) {
        // === パース成功時の処理開始 ===

        // === マクロ情報から接続情報を抽出 ===
        let (remote_host, remote_ip) = if let Some(macro_space) = macro_fields.get("MACRO_Space") {
            // "unknown [81.30.107.177]" のような形式から情報を抽出
            let mut host = "unknown".to_string();
            let mut ip = "unknown".to_string();

            // IPアドレス部分を抽出 "[xxx.xxx.xxx.xxx]" 形式
            if let Some(start) = macro_space.find('[')
                && let Some(end) = macro_space.find(']')
            {
                ip = macro_space[start + 1..end].to_string();
            }

            // ホスト名部分を抽出（IP部分より前）
            if let Some(bracket_pos) = macro_space.find('[') {
                host = macro_space[..bracket_pos].trim().to_string();
            }

            (host, ip)
        } else {
            ("unknown".to_string(), "unknown".to_string())
        };

        // RemoteIP_Target に基づく早期切断の判定
        // 0: 外部からのメールのみ対象(デフォルト) -> ループバックは即切断
        // 1: 内部からのメールのみ対象 -> ループバック以外は即切断
        // 2: 全てのメールを対象 -> 切断しない
        let is_loopback =
            remote_ip == "127.0.0.1" || remote_ip == "::1" || remote_ip.starts_with("::ffff:127.");
        match remote_ip_target {
            0 => {
                if is_loopback {
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] remote_host: {} (Loopback)",
                        remote_host
                    );
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] remote_ip: {} (Loopback)",
                        remote_ip
                    );
                    return None;
                }
            }
            1 => {
                if !is_loopback {
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] remote_host: {} (Not Loopback)",
                        remote_host
                    );
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] remote_ip: {} (Not Loopback)",
                        remote_ip
                    );
                    return None;
                }
            }
            2 => {
                // 何もしない（全て許可）
            }
            _ => {
                // 未知の値は安全のため0相当として扱う
                if is_loopback {
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] remote_host: {} (Loopback)",
                        remote_host
                    );
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] remote_ip: {} (Loopback)",
                        remote_ip
                    );
                    return None;
                }
            }
        }

        // 基本情報の出力1
        crate::printdaytimeln!(LOG_INFO, "[parser] remote_host: {}", remote_host);
        crate::printdaytimeln!(LOG_INFO, "[parser] remote_ip: {}", remote_ip);

        // === 差出人（From）情報の抽出・整形 ===
        let from = msg
            .from()
            .map(|addrs| {
                addrs
                    .iter()
                    .map(|addr| {
                        let name = addr.name().unwrap_or(""); // 差出人名（表示名）
                        let address = addr.address().unwrap_or(""); // メールアドレス
                        if !name.is_empty() {
                            format!("{name} <{address}>") // 名前付きフォーマット
                        } else {
                            address.to_string() // アドレスのみ
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ") // 複数アドレスをカンマ区切りで連結
            })
            .unwrap_or_else(|| "(なし)".to_string()); // From無し時のデフォルト値

        // From文字列から不可視文字とBiDi制御文字を除去
        let from = remove_invisible_and_bidi_chars(&from);

        // === 宛先（To）情報の抽出・整形 ===
        let to = msg
            .to()
            .map(|addrs| {
                addrs
                    .iter()
                    .map(|addr| {
                        let name = addr.name().unwrap_or(""); // 宛先名（表示名）
                        let address = addr.address().unwrap_or(""); // 宛先メールアドレス
                        if !name.is_empty() {
                            format!("{name} <{address}>") // 名前付きフォーマット
                        } else {
                            address.to_string() // アドレスのみ
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ") // 複数アドレスをカンマ区切りで連結
            })
            .unwrap_or_else(|| "(なし)".to_string()); // To無し時のデフォルト値

        // === 件名（Subject）情報の抽出 ===
        let subject = msg.subject().unwrap_or("(なし)"); // 件名無し時のデフォルト値

        // Subject文字列から不可視文字とBiDi制御文字を除去
        let subject = remove_invisible_and_bidi_chars(subject);

        // 基本情報の出力2
        crate::printdaytimeln!(LOG_INFO, "[parser] from: {}", from); // From出力
        crate::printdaytimeln!(LOG_INFO, "[parser] to: {}", to); // To出力
        crate::printdaytimeln!(LOG_INFO, "[parser] subject: {}", subject); // 件名出力

        // === Content-Type（MIMEタイプ）情報の抽出・出力 ===
        if let Some(ct) = msg
            .headers()
            .iter()
            .find(|h| h.name().eq_ignore_ascii_case("Content-Type")) // 大文字小文字無視でヘッダ検索
            .map(|h| h.value())
        {
            crate::printdaytimeln!(LOG_TRACE, "[parser] content-type: {:?}", ct);
            // MIMEタイプ出力
        }

        // === Content-Transfer-Encoding（エンコーディング方式）情報の抽出・出力 ===
        if let Some(enc) = msg
            .headers()
            .iter()
            .find(|h| h.name().eq_ignore_ascii_case("Content-Transfer-Encoding")) // 大文字小文字無視でヘッダ検索
            .map(|h| h.value())
        {
            crate::printdaytimeln!(LOG_TRACE, "[parser] encoding: {:?}", enc); // エンコーディング出力
        }

        // === メール構造（マルチパート/シングルパート）の判定・出力 ===
        if msg.parts.len() > 1 {
            crate::printdaytimeln!(LOG_TRACE, "[parser] このメールはマルチパートです");
        // 複数パート
        } else {
            crate::printdaytimeln!(LOG_TRACE, "[parser] このメールはシングルパートです");
            // 単一パート
        }

        // === パート分類処理（テキスト/非テキスト判定） ===
        let mut text_count = 0; // テキストパート数のカウンタ
        let mut non_text_count = 0; // 非テキストパート数のカウンタ
        let mut text_indices = Vec::new(); // テキストパートのインデックス格納配列

        // 各パートを走査し、テキスト/非テキストを分類
        for (i, part) in msg.parts.iter().enumerate() {
            if part.is_text() {
                // multipart/*は親パートなので除外（実際のテキストではない）
                let is_multipart = part
                    .content_type()
                    .is_some_and(|ct| ct.c_type.eq_ignore_ascii_case("multipart"));
                if !is_multipart {
                    text_count += 1; // テキストパート数カウント
                    text_indices.push(i); // テキストパートのインデックス記録
                }
            } else {
                non_text_count += 1; // 非テキストパート数カウント
            }
        }

        // パート分類結果の出力
        crate::printdaytimeln!(LOG_TRACE, "[parser] テキストパート数: {}", text_count); // テキストパート数出力
        crate::printdaytimeln!(LOG_TRACE, "[parser] 非テキストパート数: {}", non_text_count); // 非テキストパート数出力

        // パートごとの Content-Type を trace レベルで出力（テキスト/非テキスト問わず）
        for (i, part) in msg.parts.iter().enumerate() {
            let ct_hdr = part
                .headers
                .iter()
                .find(|h| h.name().eq_ignore_ascii_case("content-type"))
                .map(|h| format!("{:?}", h.value()))
                .unwrap_or_else(|| "(不明)".to_string());
            crate::printdaytimeln!(
                LOG_TRACE,
                "[parser] パート({}): Content-Type: {}",
                i + 1,
                ct_hdr
            );
        }

        // === テキストパート情報の出力処理 ===
        // テキストパートのサイズ・文字コード情報をログ出力（添付保存はしない）
        for (idx, _) in text_indices.iter().enumerate() {
            let part = &msg.parts[text_indices[idx]]; // 対象テキストパートの取得

            // パートのサブタイプ（text/plain, text/htmlなど）を取得
            let subtype = part
                .content_type()
                .and_then(|ct| ct.c_subtype.as_deref().map(|s| s.to_ascii_lowercase()));

            if let Some(subtype) = subtype {
                // plainまたはhtmlのテキストパートのみ処理
                if subtype == "plain" || subtype == "html" {
                    // 本文そのものはログに出さず、パートの要約情報のみ出力する
                    let ct = part.content_type();
                    let charset = ct
                        .and_then(|c| {
                            c.attributes()
                                .unwrap_or(&[])
                                .iter()
                                .find(|a| a.name.eq_ignore_ascii_case("charset"))
                                .map(|a| a.value.to_string())
                        })
                        .unwrap_or_else(|| "(不明)".to_string());

                    let size = part.body.len();
                    crate::printdaytimeln!(
                        LOG_TRACE,
                        "[parser] テキストパート({}): subtype={:?}, charset={}, encoding={:?}, size={} bytes",
                        idx + 1,
                        ct.and_then(|c| c.c_subtype.clone()),
                        charset,
                        part.encoding,
                        size
                    );
                }
            }
        }

        // === 非テキストパート（添付ファイル等）の情報抽出・出力処理 ===
        let mut non_text_idx = 0; // 非テキストパートの出力用インデックス
        // 文字列バッファ版の保存リスト（ストリーム版）
        let mut attachments_to_save_stream: Vec<(String, Box<dyn IoRead + Send>)> = Vec::new();

        // 非テキストパート情報を出力
        for part in msg.parts.iter() {
            if !part.is_text() {
                // Content-Type取得（MIMEタイプ情報）
                let ct = part
                    .headers
                    .iter()
                    .find(|h| h.name().eq_ignore_ascii_case("content-type"))
                    .map(|h| format!("{:?}", h.value()))
                    .unwrap_or("(不明)".to_string());

                // エンコーディング取得（Base64, quoted-printable等）
                let encoding_str = format!("{:?}", part.encoding);

                // Content-Disposition ヘッダ（存在すればそのまま出力）
                let disposition_hdr = part
                    .headers
                    .iter()
                    .find(|h| h.name().eq_ignore_ascii_case("content-disposition"))
                    .map(|h| format!("{:?}", h.value()))
                    .unwrap_or_else(|| "(なし)".to_string());

                // ファイル名取得（Content-Disposition優先、なければContent-Typeのname属性）
                // RFC2231 (filename*) と RFC2047 対応のデコードを行う
                fn percent_decode_to_bytes(s: &str) -> Vec<u8> {
                    let mut out = Vec::new();
                    let mut it = s.as_bytes().iter();
                    while let Some(&b) = it.next() {
                        if b == b'%' {
                            let hi = it.next().copied().unwrap_or(b'0');
                            let lo = it.next().copied().unwrap_or(b'0');
                            let hex = [hi, lo];
                            if let Ok(hexstr) = std::str::from_utf8(&hex)
                                && let Ok(v) = u8::from_str_radix(hexstr, 16)
                            {
                                out.push(v);
                            }
                        } else {
                            out.push(b);
                        }
                    }
                    out
                }

                fn decode_rfc2231(v: &str) -> Option<String> {
                    // pattern: charset'lang'%XX%YY...
                    if let Some(pos1) = v.find('\'')
                        && let Some(pos2) = v[pos1 + 1..].find('\'')
                    {
                        let charset = &v[..pos1];
                        let rest = &v[pos1 + 1 + pos2 + 1..];
                        // rest is percent-encoded
                        let bytes = percent_decode_to_bytes(rest);
                        if let Some(enc) = Encoding::for_label(charset.as_bytes()) {
                            let (cow, _, _) = enc.decode(&bytes);
                            return Some(cow.into_owned());
                        } else if let Ok(s) = String::from_utf8(bytes) {
                            return Some(s);
                        }
                    }
                    None
                }

                fn decode_rfc2047_word(s: &str) -> String {
                    // handle =?charset?Q?encoded?= and =?charset?B?encoded?=
                    let mut out = String::new();
                    let mut idx = 0usize;
                    let bytes = s.as_bytes();
                    while idx < bytes.len() {
                        if idx + 2 < bytes.len()
                            && &s[idx..idx + 2] == "=?"
                            && let Some(end) = s[idx + 2..].find("?=")
                        {
                            let token = &s[idx + 2..idx + 2 + end];
                            let parts: Vec<&str> = token.split('?').collect();
                            if parts.len() == 3 {
                                let cs = parts[0];
                                let enc = parts[1];
                                let text = parts[2];
                                let decoded_bytes = if enc.eq_ignore_ascii_case("b") {
                                    general_purpose::STANDARD.decode(text).unwrap_or_default()
                                } else {
                                    // Q encoding: underscores -> spaces, =XX hex
                                    let mut v = Vec::new();
                                    let mut it = text.as_bytes().iter();
                                    while let Some(&c) = it.next() {
                                        if c == b'_' {
                                            v.push(b' ');
                                        } else if c == b'=' {
                                            let hi = it.next().copied().unwrap_or(b'0');
                                            let lo = it.next().copied().unwrap_or(b'0');
                                            let hex = [hi, lo];
                                            if let Ok(hexstr) = std::str::from_utf8(&hex)
                                                && let Ok(val) = u8::from_str_radix(hexstr, 16)
                                            {
                                                v.push(val);
                                            }
                                        } else {
                                            v.push(c);
                                        }
                                    }
                                    v
                                };
                                if let Some(enc) = Encoding::for_label(cs.as_bytes()) {
                                    let (cow, _, _) = enc.decode(&decoded_bytes);
                                    out.push_str(&cow);
                                } else if let Ok(s) = String::from_utf8(decoded_bytes) {
                                    out.push_str(&s);
                                }
                                idx = idx + 2 + end + 2; // skip over ?=
                                continue;
                            }
                        }
                        // default: append single byte as UTF-8 char if possible
                        if let Ok(s) = std::str::from_utf8(&bytes[idx..idx + 1]) {
                            out.push_str(s);
                        }
                        idx += 1;
                    }
                    out
                }

                let fname = {
                    let mut fname_opt: Option<String> = None;
                    // content-disposition attrs
                    if let Some(cd) = part.content_disposition()
                        && let Some(attrs) = cd.attributes()
                    {
                        // filename* first
                        for attr in attrs.iter() {
                            if attr.name.eq_ignore_ascii_case("filename*") {
                                if let Some(d) = decode_rfc2231(&attr.value) {
                                    fname_opt = Some(d);
                                    break;
                                } else {
                                    let b = percent_decode_to_bytes(&attr.value);
                                    if let Ok(s) = String::from_utf8(b) {
                                        fname_opt = Some(s);
                                        break;
                                    }
                                }
                            }
                        }
                        // fallback to filename
                        if fname_opt.is_none() {
                            for attr in attrs.iter() {
                                if attr.name.eq_ignore_ascii_case("filename") {
                                    fname_opt = Some(decode_rfc2047_word(&attr.value));
                                    break;
                                }
                            }
                        }
                    }
                    // content-type attrs if still none
                    if fname_opt.is_none()
                        && let Some(ct) = part.content_type()
                        && let Some(attrs) = ct.attributes()
                    {
                        for attr in attrs.iter() {
                            if attr.name.eq_ignore_ascii_case("name*") {
                                if let Some(d) = decode_rfc2231(&attr.value) {
                                    fname_opt = Some(d);
                                    break;
                                } else {
                                    let b = percent_decode_to_bytes(&attr.value);
                                    if let Ok(s) = String::from_utf8(b) {
                                        fname_opt = Some(s);
                                        break;
                                    }
                                }
                            } else if attr.name.eq_ignore_ascii_case("name") {
                                fname_opt = Some(decode_rfc2047_word(&attr.value));
                                break;
                            }
                        }
                    }
                    let mut fname_res = fname_opt.unwrap_or_else(|| "(ファイル名なし)".to_string());
                    // If the decoded name still contains ISO-2022-JP escape sequences (ESC $ B ...),
                    // re-decode the raw bytes as ISO-2022-JP to produce proper UTF-8.
                    if fname_res.as_bytes().contains(&0x1B)
                        && let Some(enc) = Encoding::for_label(b"ISO-2022-JP")
                    {
                        let (cow, _, _) = enc.decode(fname_res.as_bytes());
                        fname_res = cow.into_owned();
                    }
                    fname_res
                };

                let size = part.body.len(); // パートサイズ（バイト数）

                // 非テキストパート詳細情報の出力
                crate::printdaytimeln!(
                    LOG_TRACE,
                    "[parser] 非テキストパート({}): content_type={}, content_disposition={}, encoding={}, filename={}, size={} bytes",
                    non_text_idx + 1,
                    ct,
                    disposition_hdr,
                    encoding_str,
                    fname,
                    size
                ); // 非テキストパート情報出力
                // 添付を保存対象として収集（ストリームとして Cursor でラップして渡す）
                if let PartType::Binary(b) | PartType::InlineBinary(b) = &part.body {
                    // b は &[u8] 相当なので一旦 Vec に複製して Cursor でラップ
                    let boxed: Box<dyn IoRead + Send> = Box::new(Cursor::new(b.to_vec()));
                    attachments_to_save_stream.push((fname.clone(), boxed));
                }

                non_text_idx += 1; // インデックスを次へ
            }
        }

        // 添付があれば storage_path/<QueueID>/ に保存する（まずは保存のみ）
        if !attachments_to_save_stream.is_empty() {
            // 添付ファイルの件数とファイル名一覧をログ出力（要約は INFO、詳細は TRACE）
            let attach_names: Vec<String> = attachments_to_save_stream
                .iter()
                .map(|(n, _)| n.clone())
                .collect();
            crate::printdaytimeln!(
                LOG_INFO,
                "[parser] attachments detected: {} files",
                attach_names.len()
            );
            crate::printdaytimeln!(LOG_TRACE, "[parser] attachment names: {:?}", attach_names);
            // 保存用ディレクトリ名は常に UUIDv7 を生成して利用する（並びが時系列順になる）
            // 元の Milter QueueID がある場合は mailinfo に `original_queue_id` として残す
            let original_queue_id = macro_fields
                .get("i")
                .cloned()
                .or_else(|| macro_fields.get("I").cloned());

            let queue_id = Uuid::now_v7().to_string();

            let storage_root_path = std::path::Path::new(storage_root);
            match zipper::save_attachments_stream(
                &queue_id,
                storage_root_path,
                attachments_to_save_stream,
            ) {
                Ok(paths) => {
                    // 保存パス一覧を収集してログ出力（ZIP化は行わない）
                    // attachments_meta は保存済みファイルのサイズと、元のファイル名（デコード済）を使って作成する。
                    let mut attachments_meta = Vec::new();
                    for (i, p) in paths.iter().enumerate() {
                        crate::printdaytimeln!(
                            LOG_INFO,
                            "[parser] saved attachment: {}",
                            p.display()
                        );
                        if let Ok(m) = std::fs::metadata(p) {
                            // 元のファイル名リスト (attach_names) は attachments_to_save_stream から構築済み
                            let fname = attach_names.get(i).cloned().unwrap_or_else(|| {
                                p.file_name()
                                    .map(|s| s.to_string_lossy().into_owned())
                                    .unwrap_or_else(|| "".to_string())
                            });
                            attachments_meta.push(json!({"filename": fname, "size": m.len()}));
                        }
                    }

                    // ダウンロード情報を構築（既存ロジックを再利用）
                    let mut download_url =
                        format!("{}/{}", config.base_url.trim_end_matches('/'), queue_id);
                    let mut download_auth_info: Option<serde_json::Value> = None;
                    let auth_mode_lc = config.download_auth_mode.to_ascii_lowercase();
                    let canonical_auth_mode = match auth_mode_lc.as_str() {
                        "token" | "basic" | "minimal" => auth_mode_lc.clone(),
                        other => {
                            crate::printdaytimeln!(
                                LOG_INFO,
                                "[parser] unknown download_auth_mode '{}', fallback to 'minimal'",
                                other
                            );
                            "minimal".to_string()
                        }
                    };
                    if canonical_auth_mode == "token" {
                        if let Some(key) = &config.token_auth_key {
                            let algo = config.token_auth_type.as_deref().unwrap_or("hmac-sha256");
                            let token = match algo.to_ascii_lowercase().as_str() {
                                "hmac-sha512" => {
                                    let mut mac =
                                        HmacSha512::new_from_slice(key.as_bytes()).unwrap();
                                    mac.update(queue_id.as_bytes());
                                    let bytes = mac.finalize().into_bytes();
                                    URL_SAFE_NO_PAD.encode(bytes)
                                }
                                _ => {
                                    let mut mac =
                                        HmacSha256::new_from_slice(key.as_bytes()).unwrap();
                                    mac.update(queue_id.as_bytes());
                                    let bytes = mac.finalize().into_bytes();
                                    URL_SAFE_NO_PAD.encode(bytes)
                                }
                            };
                            download_url = format!("{}?token={}", download_url, token);
                        }
                    } else if canonical_auth_mode == "basic" {
                        let u = config.basic_auth_user.clone().unwrap_or_default();
                        let p = config.basic_auth_password.clone().unwrap_or_default();
                        download_auth_info = Some(json!({"username": u, "password": p}));
                    }

                    let download_meta = json!({
                        "url": download_url,
                        "auth_mode": canonical_auth_mode,
                        "auth_info": download_auth_info,
                        "max_downloads": config.max_downloads,
                        "expire_hours": config.expire_hours
                    });

                    // 保存ディレクトリ
                    let dir = storage_root_path.join(&queue_id);

                    // UUID ディレクトリを作成して即座に count.cgi と .htaccess を生成する
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        crate::printdaytimeln!(LOG_DEBUG, "[parser] create uuid dir error: {}", e);
                    }

                    // expires_at を計算（expire_hours を元に）
                    let expires_at = if config.expire_hours == 0 {
                        // 0 は無期限扱いとして遠い未来を設定
                        (Local::now() + chrono::Duration::weeks(52 * 100))
                            .format("%Y-%m-%d %H:%M:%S %:z")
                            .to_string()
                    } else {
                        (Local::now() + chrono::Duration::hours(config.expire_hours as i64))
                            .format("%Y-%m-%d %H:%M:%S %:z")
                            .to_string()
                    };

                    // counter CGI を生成（テンプレートファイル /etc/.../counter_template.cgi を優先）
                    let db_type = config.database_type.to_string();
                    let db_path = config.database_path.clone().unwrap_or_default();
                    let db_host = config.database_host.clone().unwrap_or_default();
                    let db_port = config
                        .database_port
                        .map(|p| p.to_string())
                        .unwrap_or_default();
                    let db_user = config.database_user.clone().unwrap_or_default();
                    let db_password = config.database_password.clone().unwrap_or_default();
                    let db_name = config.database_name.clone().unwrap_or_default();

                    // テンプレート候補（download_template.html と同じディレクトリを優先）
                    let tpl_paths = [
                        "/etc/MilterSeparator.d/templates/counter_template.cgi".to_string(),
                        "etc/MilterSeparator.d/templates/counter_template.cgi".to_string(),
                    ];

                    // 組み込みフォールバックテンプレート
                    let builtin_tpl = r#"<?php
$uuid = $_GET["uuid"] ?? "";
if ($uuid === "") exit;

$dbtype = "{{db_type}}";
if ($dbtype === "sqlite") {
    $pdo = new PDO("sqlite:{{db_path}}");
} elseif ($dbtype === "mysql") {
    $pdo = new PDO("mysql:host={{db_host}};dbname={{db_name}};charset=utf8mb4", "{{db_user}}", "{{db_password}}");
} elseif ($dbtype === "postgres") {
    $pdo = new PDO("pgsql:host={{db_host}};port={{db_port}};dbname={{db_name}}", "{{db_user}}", "{{db_password}}");
}

$stmt = $pdo->prepare("UPDATE download_tbl SET download_count = download_count + 1 WHERE uuid = ?");
$stmt->execute([$uuid]);
?>"#;

                    // テンプレート読込（優先パスがあればそれを採用）
                    let mut tpl = None;
                    for p in &tpl_paths {
                        if let Ok(s) = std::fs::read_to_string(p) {
                            tpl = Some(s);
                            break;
                        }
                    }
                    let mut count_php = tpl.unwrap_or_else(|| builtin_tpl.to_string());

                    // プレースホルダを置換
                    count_php = count_php.replace("{{db_type}}", &db_type);
                    count_php = count_php.replace("{{db_path}}", &db_path);
                    count_php = count_php.replace("{{db_host}}", &db_host);
                    count_php = count_php.replace("{{db_port}}", &db_port);
                    count_php = count_php.replace("{{db_user}}", &db_user);
                    count_php = count_php.replace("{{db_password}}", &db_password);
                    count_php = count_php.replace("{{db_name}}", &db_name);

                    let count_path = dir.join(&config.counter_cgi);
                    if let Err(e) = std::fs::write(&count_path, count_php) {
                        crate::printdaytimeln!(LOG_DEBUG, "[parser] write count.cgi error: {}", e);
                    } else {
                        // chmod 755
                        let _ = std::fs::set_permissions(
                            &count_path,
                            fs::Permissions::from_mode(0o755),
                        );
                    }

                    // .htaccess を生成（mailinfo.txt への外部アクセスを拒否）
                    let mut ht = String::from(
                        "<Files \"mailinfo.txt\">\n    Require all denied\n</Files>\n",
                    );
                    if canonical_auth_mode == "basic" {
                        ht.push_str("\nAuthType Basic\nAuthName \"MilterSeparator\"\nAuthUserFile .htpasswd\nRequire valid-user\n");
                    }
                    let ht_path = dir.join(".htaccess");
                    if let Err(e) = std::fs::write(&ht_path, ht) {
                        crate::printdaytimeln!(LOG_DEBUG, "[parser] write .htaccess error: {}", e);
                    }

                    // saved_paths を保持しておく（ZIP作成後に削除するため）
                    let saved_paths: Vec<std::path::PathBuf> = paths.into_iter().collect();

                    // ZIP名・パス作成
                    let zip_name = format!("{}.zip", queue_id);
                    // ZIPは親ディレクトリ(storage_root)に生成してから移動する
                    let parent_zip_path = storage_root_path.join(&zip_name);

                    // パスワード生成（Config の強度を zipper::PasswordStrength にマップ）
                    let zipper_pw_strength = match config.password_strength {
                        crate::init::PasswordStrength::Low => zipper::PasswordStrength::Low,
                        crate::init::PasswordStrength::High => zipper::PasswordStrength::High,
                        _ => zipper::PasswordStrength::Medium,
                    };
                    let pw = zipper::generate_password(zipper_pw_strength);

                    // DB にレコードを挿入する（UUID ディレクトリ生成時に行う）
                    let record = db::DownloadRecord {
                        uuid: queue_id.clone(),
                        expires_at: expires_at.clone(),
                        zip_password: Some(pw.clone()),
                        url: download_url.clone(),
                        auth_mode: canonical_auth_mode.clone(),
                        auth_info: download_auth_info.clone(),
                        expire_hours: config.expire_hours as i64,
                        max_downloads: config.max_downloads as i64,
                    };

                    if let Err(e) = db::insert_download_record(config, &record) {
                        crate::printdaytimeln!(
                            LOG_DEBUG,
                            "[parser] insert_download_record error: {}",
                            e
                        );
                    }

                    // ZIP作成を試みる（親ディレクトリに作成してから UUID ディレクトリへ移動）
                    let mut zip_ok = false;
                    // final path inside uuid dir
                    let final_zip_path = dir.join(&zip_name);
                    // prepare mapping (entry name -> saved path) to ensure UTF-8 entry names from attach_names
                    let mut files_for_zip: Vec<(String, std::path::PathBuf)> = Vec::new();
                    for (i, p) in saved_paths.iter().enumerate() {
                        let entry_name = attach_names.get(i).cloned().unwrap_or_else(|| {
                            p.file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "file".to_string())
                        });
                        files_for_zip.push((entry_name, p.clone()));
                    }

                    match zipper::create_zip_from_files(files_for_zip, &parent_zip_path, Some(&pw))
                    {
                        Ok(()) => {
                            // ensure target dir exists and move zip into it
                            if let Err(e) = std::fs::create_dir_all(&dir) {
                                crate::printdaytimeln!(
                                    LOG_DEBUG,
                                    "[parser] create dir error: {}",
                                    e
                                );
                            }
                            let move_res = std::fs::rename(&parent_zip_path, &final_zip_path)
                                .or_else(|_| {
                                    // fallback: copy then remove
                                    std::fs::copy(&parent_zip_path, &final_zip_path)
                                        .and_then(|_| std::fs::remove_file(&parent_zip_path))
                                });
                            match move_res {
                                Ok(_) => {
                                    zip_ok = true;
                                    crate::printdaytimeln!(
                                        LOG_INFO,
                                        "[parser] created zip: {}",
                                        final_zip_path.display()
                                    );
                                }
                                Err(e) => {
                                    crate::printdaytimeln!(
                                        LOG_DEBUG,
                                        "[parser] move zip error: {}",
                                        e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            crate::printdaytimeln!(
                                LOG_DEBUG,
                                "[parser] create_zip_from_files error: {}",
                                e
                            );
                        }
                    }

                    // 添付メタ情報（ファイル名・サイズ）を再構築
                    let mut attachments_meta = Vec::new();
                    for p in &saved_paths {
                        if let Ok(m) = std::fs::metadata(p) {
                            let fname = p
                                .file_name()
                                .map(|s| s.to_string_lossy().into_owned())
                                .unwrap_or_else(|| "".to_string());
                            attachments_meta.push(json!({"filename": fname, "size": m.len()}));
                        }
                    }

                    // mailinfo JSON 構築
                    // 生成日時と ZIP サイズを付与する
                    let generated_at = Local::now().format("%Y-%m-%d %H:%M:%S %:z").to_string();
                    let zip_info = if zip_ok {
                        let zip_size = std::fs::metadata(&final_zip_path)
                            .map(|m| m.len())
                            .unwrap_or(0);
                        json!({
                            "file": zip_name,
                            "password": pw,
                            "size": zip_size,
                            "download_count": 0,
                            "expires_at": null
                        })
                    } else {
                        serde_json::Value::Null
                    };

                    let mailinfo = json!({
                        "from": from,
                        "to": to,
                        "subject": subject,
                        "attachments": attachments_meta,
                        "zip": zip_info,
                        "download": download_meta,
                        "generated_at": generated_at,
                        "original_queue_id": original_queue_id
                    });

                    // mailinfo.txt を書き出す
                    let mailinfo_path = dir.join("mailinfo.txt");
                    match std::fs::File::create(&mailinfo_path) {
                        Ok(mut f) => {
                            if let Ok(s) = serde_json::to_string_pretty(&mailinfo)
                                && let Err(e) = f.write_all(s.as_bytes())
                            {
                                crate::printdaytimeln!(
                                    LOG_DEBUG,
                                    "[parser] write mailinfo error: {}",
                                    e
                                );
                            }
                            // mailinfo を書き終えたら、ダウンロード用静的ファイルを生成する
                            if let Err(e) =
                                crate::download::write_download_static_files(&dir, config)
                            {
                                crate::printdaytimeln!(
                                    LOG_DEBUG,
                                    "[parser] write download static files error: {}",
                                    e
                                );
                            } else {
                                crate::printdaytimeln!(
                                    LOG_INFO,
                                    "[parser] wrote download static files"
                                );
                            }
                        }
                        Err(e) => {
                            crate::printdaytimeln!(
                                LOG_DEBUG,
                                "[parser] create mailinfo file error: {}",
                                e
                            );
                        }
                    }

                    // ZIP作成に成功していれば、元の添付ファイルを削除する
                    if zip_ok {
                        for p in &saved_paths {
                            if p.exists() {
                                match std::fs::remove_file(p) {
                                    Ok(()) => {
                                        crate::printdaytimeln!(
                                            LOG_INFO,
                                            "[parser] removed original attachment: {}",
                                            p.display()
                                        );
                                    }
                                    Err(e) => {
                                        crate::printdaytimeln!(
                                            LOG_DEBUG,
                                            "[parser] failed to remove attachment {}: {}",
                                            p.display(),
                                            e
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    crate::printdaytimeln!(
                        LOG_DEBUG,
                        "[parser] save_attachments_stream error: {}",
                        e
                    );
                }
            }
        }

        // パース処理完了
        return Some(());
    }

    // パース失敗時はNoneを返す
    None // パース失敗：Noneを返却
}
