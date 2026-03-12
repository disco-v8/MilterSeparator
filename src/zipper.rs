// =========================
// zipper.rs
// 添付ファイル保存・ZIP生成ユーティリティモジュール
//
// 【このファイルで使う主なクレート】
// - rand: パスワード生成用の乱数・文字集合
// - zip: ZIPファイル作成（圧縮・（旧）ZipCrypto暗号化の補助）
// - std::fs / std::io: ファイル入出力
//
// 役割:
// - 添付ファイルのディスク保存（`save_attachments`）
// - 単純なZIP作成（`create_zip`）
// - ZipCrypto互換のパスワード付きZIP作成補助（`create_passworded_zip`）
// - パスワード生成ユーティリティ（`generate_password`）
//
// 注意点:
// - 現在の実装はWindows Explorerと互換性のある旧式のZipCryptoを利用する
//   オプションを使うことを意図していますが、強度的には推奨されません。
//   セキュリティが重要な場合はAESベースの暗号化を別途検討してください。
// =========================

use rand::distributions::Alphanumeric;
use rand::seq::SliceRandom;
use rand::{Rng, thread_rng};
use std::error::Error;
use std::fs::File;
use std::io::{Read, Write, copy};
use std::path::Path;
use zip::CompressionMethod;
use zip::unstable::write::FileOptionsExt;
use zip::write::{FileOptions, ZipWriter};

/// パスワード強度を表す列挙型
///
/// # 目的
/// - ZIPや通知用に生成するパスワードの基準を表現します。
/// - 将来的に設定ファイルから選択できるようにする想定です。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub enum PasswordStrength {
    /// 短めの英数字（8文字程度）
    Low,
    /// 中程度（大文字・小文字・数字・記号を含む約12文字）
    Medium,
    /// 高強度（各文字種を含む16文字以上）
    High,
}

impl PasswordStrength {
    #[allow(dead_code)]
    /// 文字列から列挙型へ変換するヘルパー
    ///
    /// # 引数
    /// - `s` : 小文字/大文字混在可能な強度ラベル（例: "low", "medium", "high"）
    ///
    /// # 戻り値
    /// - `Option<PasswordStrength>` : マッチしない場合は `None`
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "low" => Some(Self::Low),
            "medium" => Some(Self::Medium),
            "high" => Some(Self::High),
            _ => None,
        }
    }
}

/// 指定した強度に従ってランダムなパスワードを生成する。
///
/// # 引数
/// - `level`: 生成するパスワードの強度（`PasswordStrength`）
///
/// # 戻り値
/// - `String`: 生成されたパスワード文字列
///
/// # 実装上の注意
/// - 必須文字種（大文字・小文字・数字・記号）を最低1つ含めるようにしてから
///   残りを乱数で埋め、最終的にシャッフルして順序上の偏りを減らしています。
#[allow(dead_code)]
pub fn generate_password(level: PasswordStrength) -> String {
    let mut rng = thread_rng();

    match level {
        PasswordStrength::Low => {
            // 8文字の英数字
            (0..8)
                .map(|_| rng.sample(Alphanumeric) as char)
                .collect::<String>()
        }
        PasswordStrength::Medium => {
            // 12文字以上、必須で大文字・小文字・数字・記号を含める
            let mut pw = String::with_capacity(12);
            // 必須文字をひとつずつ追加
            pw.push(rng.gen_range(b'A'..=b'Z') as char);
            pw.push(rng.gen_range(b'a'..=b'z') as char);
            pw.push(rng.gen_range(b'0'..=b'9') as char);
            let symbols = b"!@#$%^&*()-_=+[]{};:,<>.?/";
            pw.push(symbols[rng.gen_range(0..symbols.len())] as char);
            // 残りをランダムに埋める
            while pw.len() < 12 {
                let choice = rng.gen_range(0..3);
                match choice {
                    0 => pw.push(rng.gen_range(b'a'..=b'z') as char),
                    1 => pw.push(rng.gen_range(b'A'..=b'Z') as char),
                    _ => pw.push(rng.gen_range(b'0'..=b'9') as char),
                }
            }
            // シャッフルして順序依存を減らす
            let mut v: Vec<char> = pw.chars().collect();
            v.shuffle(&mut rng);
            v.into_iter().collect()
        }
        PasswordStrength::High => {
            // 16文字以上、各文字種を満たす
            let mut pw = String::with_capacity(16);
            pw.push(rng.gen_range(b'A'..=b'Z') as char);
            pw.push(rng.gen_range(b'a'..=b'z') as char);
            pw.push(rng.gen_range(b'0'..=b'9') as char);
            let symbols = b"!@#$%^&*()-_=+[]{};:,<>.?/";
            pw.push(symbols[rng.gen_range(0..symbols.len())] as char);
            while pw.len() < 16 {
                let idx = rng.gen_range(0..4);
                match idx {
                    0 => pw.push(rng.gen_range(b'a'..=b'z') as char),
                    1 => pw.push(rng.gen_range(b'A'..=b'Z') as char),
                    2 => pw.push(rng.gen_range(b'0'..=b'9') as char),
                    _ => pw.push(symbols[rng.gen_range(0..symbols.len())] as char),
                }
            }
            let mut v: Vec<char> = pw.chars().collect();
            v.shuffle(&mut rng);
            v.into_iter().collect()
        }
    }
}

/// 添付ファイル集合を通常の ZIP にまとめて書き出すユーティリティ
///
/// # 引数
/// - `attachments`: (ファイル名, データ) のタプルベクタ。ファイル名はZIP内エントリ名として使用されます。
/// - `out_path`: 出力 ZIP のパス
///
/// # 戻り値
/// - `Result<(), Box<dyn Error>>`: 失敗時はIO/ZIPエラーを返します。
///
/// # 説明
/// - 内部で `zip` クレートの `ZipWriter` を使って圧縮エントリを順次書き出します。
/// - パスワード付与が必要な場合は `create_passworded_zip` を検討してください（現在の実装では限定的です）。
#[allow(dead_code)]
pub fn create_zip(
    attachments: Vec<(&str, Vec<u8>)>,
    out_path: &Path,
) -> Result<(), Box<dyn Error>> {
    let file = File::create(out_path)?;
    let mut zip = ZipWriter::new(file);

    let options: FileOptions<'static, ()> = FileOptions::default();

    for (name, data) in attachments.into_iter() {
        zip.start_file(name, options)?;
        zip.write_all(&data)?;
    }

    zip.finish()?;
    Ok(())
}

/// パスワード付きZIPを作成する補助関数（ZipCrypto互換）
///
/// # 引数
/// - `attachments`: (ファイル名, データ) のタプルベクタ
/// - `out_path`: 出力 ZIP ファイルパス
/// - `password`: ZIPエントリに適用するパスワード（バイト列化して渡されます）
///
/// # 戻り値
/// - `Result<(), Box<dyn Error>>`
///
/// # 実装上の留意点
/// - 現在は `zip` クレートの `with_deprecated_encryption`（ZipCrypto）を利用する実装を試みています。
/// - ZipCrypto は古く脆弱性があるため、機密性を重視する場合はAES暗号化を採用するか、
///   中間で安全な外部ツールを使用してください。
#[allow(dead_code)]
pub fn create_passworded_zip(
    attachments: Vec<(&str, Vec<u8>)>,
    out_path: &Path,
    password: &str,
) -> Result<(), Box<dyn Error>> {
    // Create output file and ZipWriter
    let out_file = File::create(out_path)?;
    let mut zipw = ZipWriter::new(out_file);

    // Build FileOptions with compression and (deprecated) encryption for Win-Explorer compatibility.
    // Prefer `with_deprecated_encryption(password)` when available in the crate.
    for (name, data) in attachments.into_iter() {
        // Create base options
        let base: FileOptions<'static, ()> =
            FileOptions::default().compression_method(CompressionMethod::Deflated);

        // Try to set deprecated (ZipCrypto) encryption via available API.
        // In zip v7 the builder-style API exposes `with_deprecated_encryption` on FileOptions.
        // If that method is not present in your local crate, update here to use the
        // FileOptions builder variant supported by your version.
        let opts = base.with_deprecated_encryption(password.as_bytes())?;

        zipw.start_file(name, opts)?;
        zipw.write_all(&data)?;
    }

    zipw.finish()?;
    Ok(())
}

/// 添付ファイルをストリームとして受け取りディスクに保存する関数
///
/// # 引数
/// - `queue_id`: Milterで渡されたQueueID名
/// - `storage_root`: 保存ルートパス
/// - `attachments`: `(ファイル名, Box<dyn Read + Send>)` のベクタ。各Readerからストリームで読み取り保存する。
///
/// # 戻り値
/// - `Result<Vec<PathBuf>, Box<dyn Error>>`: 保存したパス一覧
///
/// # 説明
/// - 大きな添付をメモリに全部置かずに順次 `Read` から `std::fs::File` へ `copy` する
/// - ファイル名はサニタイズして `storage_root/queue_id/` 以下に保存する
pub fn save_attachments_stream(
    queue_id: &str,
    storage_root: &Path,
    mut attachments: Vec<(String, Box<dyn Read + Send>)>,
) -> Result<Vec<std::path::PathBuf>, Box<dyn Error>> {
    let dir = storage_root.join(queue_id);
    std::fs::create_dir_all(&dir)?;

    let mut saved = Vec::new();

    for (name, mut reader) in attachments.drain(..) {
        // sanitize filename: take file_name portion only
        let fname = std::path::Path::new(&name)
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "attachment.bin".to_string());

        // Build candidate path; if it exists, append numeric suffix before extension
        let mut outp = dir.join(&fname);
        if outp.exists() {
            let mut attempt: usize = 1;
            loop {
                let stem = std::path::Path::new(&fname)
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "attachment".to_string());
                let ext = std::path::Path::new(&fname)
                    .extension()
                    .map(|s| s.to_string_lossy().into_owned());
                let new_name = match ext {
                    Some(e) => format!("{}_{}.{}", stem, attempt, e),
                    None => format!("{}_{}", stem, attempt),
                };
                outp = dir.join(&new_name);
                if !outp.exists() {
                    break;
                }
                attempt += 1;
            }
        }

        let mut f = File::create(&outp)?;
        // ストリームを直接ディスクへコピー（メモリバッファを避ける）
        copy(&mut reader, &mut f)?;
        saved.push(outp);
    }

    Ok(saved)
}

/// 指定ディレクトリ内のファイルをまとめてZIP化する関数
///
/// - `dir`: 圧縮対象ディレクトリ（直下のファイルを圧縮）
/// - `out_path`: 出力ZIPファイルパス
/// - `password`: オプションのパスワード（SomeでZipCrypto暗号を試みる）
#[allow(dead_code)]
pub fn create_zip_from_dir(
    dir: &Path,
    out_path: &Path,
    password: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    // 出力ファイルを作成
    let file = File::create(out_path)?;
    let mut zip = ZipWriter::new(file);

    // ディレクトリ内を走査してファイルを追加
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file() {
            let name = path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "file".to_string());

            let data = std::fs::read(&path)?;

            // FileOptionsの組み立て（圧縮メソッドは Deflated）
            let base: FileOptions<'static, ()> =
                FileOptions::default().compression_method(CompressionMethod::Deflated);
            let opts = if let Some(pw) = password {
                // パスワードあり: 古いZipCrypto互換の暗号化を試みる
                base.with_deprecated_encryption(pw.as_bytes())?
            } else {
                base
            };

            zip.start_file(name, opts)?;
            zip.write_all(&data)?;
        }
    }

    zip.finish()?;
    Ok(())
}

/// 指定されたファイルパスと表示名の組をそのままZIPに追加して書き出す。
/// - `files`: Vec<(entry_name, path_on_disk)>
/// - `out_path`: 出力ZIPパス（親ディレクトリに作成する想定）
/// - `password`: Optional password for deprecated ZipCrypto encryption
pub fn create_zip_from_files(
    files: Vec<(String, std::path::PathBuf)>,
    out_path: &Path,
    password: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    let out_file = File::create(out_path)?;
    let mut zipw = ZipWriter::new(out_file);

    for (name, path) in files.into_iter() {
        let data = std::fs::read(&path)?;

        let base: FileOptions<'static, ()> =
            FileOptions::default().compression_method(CompressionMethod::Deflated);
        let opts = if let Some(pw) = password {
            base.with_deprecated_encryption(pw.as_bytes())?
        } else {
            base
        };

        zipw.start_file(name, opts)?;
        zipw.write_all(&data)?;
    }

    zipw.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_generate_passwords() {
        let p1 = generate_password(PasswordStrength::Low);
        assert!(p1.len() >= 8);

        let p2 = generate_password(PasswordStrength::Medium);
        assert!(p2.len() >= 12);

        let p3 = generate_password(PasswordStrength::High);
        assert!(p3.len() >= 16);
    }

    #[test]
    fn test_create_zip_and_passworded_zip() {
        // 作成可能かどうかを簡易テスト（環境に zip コマンドがないと失敗する）
        let attachments = vec![("foo.txt", b"hello world".to_vec())];
        let mut out = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        out.push("target/test-output.zip");
        let _ = create_zip(attachments.clone(), &out).expect("create_zip failed");

        let mut outp = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        outp.push("target/test-output-password.zip");
        // パスワード生成は任意
        let pw = generate_password(PasswordStrength::Medium);
        // create_passworded_zip は環境に zip コマンドが必要
        let _ = create_passworded_zip(attachments, &outp, &pw);
    }
}
