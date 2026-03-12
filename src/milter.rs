// =========================
// milter.rs
// MilterSeparator Milterコマンド処理モジュール
//
// 【このファイルで使う主なクレート】
// - tokio: 非同期TCP通信・I/O・応答送信などの非同期処理全般（net::TcpStream, io::AsyncWriteExt）
// - std: 標準ライブラリ（バイト操作、コレクション、エラー処理、フォーマット等）
// - crate::printdaytimeln!: JSTタイムスタンプ付きでログ出力する独自マクロ
// - crate::milter_command: Milterマクロ種別enum（MilterMacro）
//
// 【役割】
// - Milterコマンドごとのデコード・応答処理（OPTNEG, CONNECT, HELO, DATA, HEADER, BODY, EOH/BODYEOB）
// - ネゴシエーション情報の分解・応答送信
// - マクロペイロードの分解・出力
// - ヘッダ・ボディ情報の格納・加工
// =========================

use tokio::{
    io::AsyncWriteExt, // 非同期I/Oトレイト（write_all等）
    net::TcpStream,    // 非同期TCPストリーム
};

use crate::init::{LOG_DEBUG, LOG_TRACE};

/// OPTNEGコマンドのデコード・応答送信処理
///
/// # 引数
/// - `stream`: クライアントTCPストリーム（応答送信用）
/// - `payload`: 受信ペイロード（ネゴシエーション情報のバイト列）
///
/// # 説明
/// Milterプロトコルのネゴシエーション（OPTNEG）コマンドを処理し、
/// クライアントとの機能・プロトコル設定を調整してOPTNEG応答を送信する。
/// OPTNEGはMilter接続開始時の最初のコマンドで、双方の対応機能を交換する。
///
/// # 処理フロー
/// 1. ペイロード長の検証（最低12バイト必要）
/// 2. プロトコルバージョン・アクション・フラグの抽出と詳細ログ出力
/// 3. アクションフラグの分解・個別機能の確認と出力
/// 4. プロトコルフラグの分解・省略機能の確認と出力
/// 5. OPTNEG応答バッファの構築（13バイト構成）
/// 6. クライアントへの非同期応答送信とエラーハンドリング
///
/// # 技術詳細
/// - ペイロード構成: プロトコルバージョン(4)+アクション(4)+フラグ(4)=12バイト
/// - 応答構成: サイズ(4)+コマンド(1)+バージョン(4)+アクション(4)=13バイト
/// - NO_BODY/NO_HDRSフラグ制御: ヘッダ・ボディをMilterで受信するため除去
///
/// # 重要な制約
/// - 非同期I/O処理のためawait必須（tokio::io::AsyncWriteExt使用）
/// - ペイロード長不足時はエラーログ出力のみで処理継続
pub async fn decode_optneg(stream: &mut TcpStream, payload: &[u8]) {
    // Step 1: ペイロード長の検証（プロトコルバージョン+アクション+フラグで最低12バイト必要）
    if payload.len() >= 12 {
        // Step 2: プロトコルバージョン・アクション・フラグの抽出と詳細ログ出力
        let protocol_ver = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]); // プロトコルバージョン（4バイト）
        let actions = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]); // アクションフラグ（4バイト）
        let protocol_flags = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]); // プロトコルフラグ（4バイト）

        // 受信したOPTNEG情報を詳細に出力
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[parser] SMFIC_OPTNEG: protocol_ver={} actions=0x{:08X} protocol_flags=0x{:08X}",
            protocol_ver,
            actions,
            protocol_flags
        );

        // Step 3: アクションフラグの分解・個別機能の確認と出力
        let action_flags = [
            (0x00000001, "ADD_HEADERS"),       // ヘッダ追加
            (0x00000002, "CHANGE_BODY"),       // 本文変更
            (0x00000004, "ADD_RECIPIENTS"),    // 宛先追加
            (0x00000008, "DELETE_RECIPIENTS"), // 宛先削除
            (0x00000010, "QUARANTINE"),        // 隔離
            (0x00000020, "REPLACE_HEADERS"),   // ヘッダ置換
            (0x00000040, "CHANGE_REPLY"),      // 応答変更
        ];

        // 各アクションフラグが立っていれば出力
        for (flag, name) in &action_flags {
            if actions & flag != 0 {
                crate::printdaytimeln!(LOG_DEBUG, "[parser] Milterアクション: {}", name);
                // アクションフラグごとに出力
            }
        }

        // Step 4: プロトコルフラグの分解・省略機能の確認と出力
        let proto_flags = [
            (0x00000001, "NO_CONNECT"), // CONNECT省略
            (0x00000002, "NO_HELO"),    // HELO省略
            (0x00000004, "NO_ENVFROM"), // ENVFROM省略
            (0x00000008, "NO_ENVRCPT"), // ENVRCPT省略
            (0x00000010, "NO_BODY"),    // BODY省略
            (0x00000020, "NO_HDRS"),    // HDRS省略
            (0x00000040, "NO_UNKNOWN"), // UNKNOWN省略
            (0x00000080, "NO_DATA"),    // DATA省略
        ];

        // 各プロトコルフラグが立っていれば出力
        for (flag, name) in &proto_flags {
            if protocol_flags & flag != 0 {
                crate::printdaytimeln!(LOG_DEBUG, "[parser] Milterプロトコル: {}", name);
                // サポートフラグごとに出力
            }
        }

        // Step 5: OPTNEG応答バッファの構築（13バイト構成: サイズ4+コマンド1+ペイロード12）
        let mut resp = Vec::with_capacity(13);
        resp.extend_from_slice(&13u32.to_be_bytes()); // 応答サイズ（4バイト）
        resp.push(0x4f); // コマンド: SMFIR_OPTNEG（応答コマンド1バイト）
        resp.extend_from_slice(&protocol_ver.to_be_bytes()); // プロトコルバージョン（4バイト）

        // クライアントから受信したアクションフラグをそのまま応答にセット
        let resp_actions = actions;
        resp.extend_from_slice(&resp_actions.to_be_bytes()); // アクションフラグ（4バイト）

        // NO_BODY(0x10)とNO_HDRS(0x20)を立てないサポートフラグを生成（ヘッダ・ボディもMilterで渡される）
        let resp_protocol_flags = protocol_flags & !(0x10 | 0x20);
        resp.extend_from_slice(&resp_protocol_flags.to_be_bytes()); // サポートフラグ（4バイト）

        // Step 6: クライアントへの非同期応答送信とエラーハンドリング
        match stream.write_all(&resp).await {
            Ok(_) => {
                crate::printdaytimeln!(LOG_DEBUG, "[parser] SMFIR_OPTNEG応答送信完了: {:?}", resp)
            } // 送信成功時
            Err(e) => {
                crate::printdaytimeln!(LOG_DEBUG, "[parser] SMFIR_OPTNEG応答送信エラー: {}", e)
            } // 送信失敗時
        }
    } else {
        // ペイロード長不足時のエラー出力
        crate::printdaytimeln!(crate::init::LOG_INFO,
            "[parser] SMFIC_OPTNEGペイロード長不足: {} bytes",
            payload.len()
        );
    }
}

/// CONNECTコマンドのデコード・応答送信処理
///
/// # 引数
/// - `stream`: クライアントTCPストリーム（応答送信用）
/// - `payload`: 受信ペイロード（接続情報のバイト列）
/// - `peer_addr`: クライアントアドレス（ログ出力用）
///
/// # 説明
/// SMTPプロトコルのCONNECTコマンドで送信された接続情報を処理し、
/// CONTINUE応答(0x06)をクライアントに送信してメール処理の継続を指示する。
/// CONNECTはSMTP接続の最初に送信者のホスト情報を通知するコマンド。
///
/// # 処理フロー
/// 1. ペイロードをUTF-8文字列に変換して接続情報を抽出
/// 2. 接続情報をログ出力（デバッグ・監査用）
/// 3. CONTINUE応答バッファの構築（4バイトサイズ + 1バイトコマンド）
/// 4. クライアントへの非同期応答送信とエラーハンドリング
///
/// # 技術詳細
/// - CONTINUE応答（0x06）: 次のMilterコマンド待機を指示
/// - String::from_utf8_lossy(): 無効UTF-8バイトは置換文字で安全に変換
/// - 応答サイズは固定1バイト（コマンドのみのため）
///
/// # 重要な制約
/// - 非同期I/O処理のためawait必須（tokio::io::AsyncWriteExt使用）
/// - エラー時は詳細ログ出力するが処理は継続（メール処理中断回避）
pub async fn decode_connect(stream: &mut tokio::net::TcpStream, payload: &[u8], peer_addr: &str) {
    // Step 1: ペイロードをUTF-8文字列に変換して接続情報を抽出
    let connect_str = String::from_utf8_lossy(payload); // ペイロードをUTF-8文字列化（無効バイト→置換文字）

    // Step 2: 接続情報をログ出力（デバッグ・監査用）
    crate::printdaytimeln!(LOG_DEBUG, "[parser] 接続情報: {}", connect_str); // 接続情報をJSTタイムスタンプ付きで出力

    // Step 3: CONTINUE応答バッファの構築（Milterプロトコル形式）
    let resp_size: u32 = 1; // 応答サイズ（コマンドのみ1バイト）
    let resp_cmd: u8 = 0x06; // CONTINUEコマンド（処理継続指示）
    let mut resp = Vec::with_capacity(5); // 応答バッファ（5バイト: サイズ4+コマンド1）
    resp.extend_from_slice(&resp_size.to_be_bytes()); // ビッグエンディアンでサイズ（4バイト）
    resp.push(resp_cmd); // 応答コマンド（1バイト）

    // Step 4: クライアントへの非同期応答送信とエラーハンドリング
    if let Err(e) = stream.write_all(&resp).await {
        crate::printdaytimeln!(LOG_DEBUG, "[parser] 応答送信エラー: {}: {}", peer_addr, e);
    // 送信失敗時はエラーログ
    } else {
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[parser] 応答送信(connect): CONTINUE (0x06) to {}",
            peer_addr
        );
        // 送信成功時は詳細ログ（応答コードとクライアントアドレス）
    }
}

/// HELOコマンドのデコード・応答送信処理
///
/// # 引数
/// - `stream`: クライアントTCPストリーム（応答送信用）
/// - `payload`: 受信ペイロード（HELOドメイン名のバイト列）
/// - `peer_addr`: クライアントアドレス（ログ出力用）
///
/// # 説明
/// SMTPプロトコルのHELOコマンドで送信されたドメイン名情報を処理し、
/// CONTINUE応答(0x06)をクライアントに送信してメール処理の継続を指示する。
/// HELOはSMTP接続の初期段階で送信者のドメイン名を通知するコマンド。
///
/// # 処理フロー
/// 1. ペイロードをUTF-8文字列に変換してHELOドメイン名を抽出
/// 2. HELOドメイン名をログ出力（デバッグ・監査用）
/// 3. CONTINUE応答バッファの構築（4バイトサイズ + 1バイトコマンド）
/// 4. クライアントへの非同期応答送信とエラーハンドリング
///
/// # 技術詳細
/// - CONTINUE応答（0x06）: 次のMilterコマンド待機を指示
/// - String::from_utf8_lossy(): 無効UTF-8バイトは置換文字で安全に変換
/// - 応答サイズは固定1バイト（コマンドのみのため）
///
/// # 重要な制約
/// - 非同期I/O処理のためawait必須（tokio::io::AsyncWriteExt使用）
/// - エラー時は詳細ログ出力するが処理は継続（メール処理中断回避）
pub async fn decode_helo(stream: &mut tokio::net::TcpStream, payload: &[u8], peer_addr: &str) {
    // Step 1: ペイロードをUTF-8文字列に変換してHELOドメイン名を抽出
    let helo_str = String::from_utf8_lossy(payload); // ペイロードをUTF-8文字列化（無効バイト→置換文字）

    // Step 2: HELOドメイン名をログ出力（デバッグ・監査用）
    crate::printdaytimeln!(LOG_DEBUG, "[parser] HELO: {}", helo_str); // HELOドメイン名をJSTタイムスタンプ付きで出力

    // Step 3: CONTINUE応答バッファの構築（Milterプロトコル形式）
    let resp_size: u32 = 1; // 応答サイズ（コマンドのみ1バイト）
    let resp_cmd: u8 = 0x06; // CONTINUEコマンド（処理継続指示）
    let mut resp = Vec::with_capacity(5); // 応答バッファ（5バイト: サイズ4+コマンド1）
    resp.extend_from_slice(&resp_size.to_be_bytes()); // ビッグエンディアンでサイズ（4バイト）
    resp.push(resp_cmd); // 応答コマンド（1バイト）

    // Step 4: クライアントへの非同期応答送信とエラーハンドリング
    if let Err(e) = stream.write_all(&resp).await {
        crate::printdaytimeln!(LOG_DEBUG, "[parser] 応答送信エラー: {}: {}", peer_addr, e);
    // 送信失敗時はエラーログ
    } else {
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[parser] 応答送信(helo): CONTINUE (0x06) to {}",
            peer_addr
        );
        // 送信成功時は詳細ログ（応答コードとクライアントアドレス）
    }
}

/// DATAコマンドのマクロペイロードを分解・出力する
///
/// # 引数
/// - `payload`: DATAコマンドのペイロード（0x00区切りのマクロ名・値バイト列）
/// - `is_header_block`: ヘッダブロック判定フラグ（SOHマクロ検出時にtrueに設定）
/// - `macro_fields`: マクロ情報を格納するHashMap（SMTPセッション情報保存用）
///
/// # 説明
/// Milterプロトコルで受信したDATAコマンドのマクロペイロードを解析し、
/// 各フェーズ（DATA/CONNECT/HELO/SOH等）のマクロ名・値を出力し、macro_fieldsに格納する。
/// 先頭バイトでマクロ種別を判定し、拡張マクロ（{name}形式）にも対応。
///
/// # 処理フロー
/// 1. ペイロードを0x00区切りで分割してマクロ要素を抽出
/// 2. 先頭バイトからマクロフェーズ（DATA/SOH等）を判定
/// 3. SOHマクロ検出時はヘッダブロックフラグを設定
/// 4. 先頭マクロ名・値の抽出と出力・格納
/// 5. 残りマクロの順次処理（名前・値のペア単位）
///
/// # 技術詳細
/// - MilterMacroEnum: 標準マクロ（i, j, {auth_author}等）とベンダー拡張マクロに対応
/// - 拡張マクロ形式: {name}で囲まれた独自マクロ名の解析
/// - SOHマクロ: Start of Headers（ヘッダ開始）を示すフェーズマーカー
///
/// # 重要な制約
/// - ペイロード空の場合は早期リターン（マクロなし）
/// - 不正な拡張マクロは"Unknown"として処理継続
/// - インデックス範囲外アクセス防止（bounds check実施）
///
/// 【この関数で使う主なクレート】
/// - crate::milter_command::MilterMacro: マクロ種別enum（Postfix/Sendmail互換）
/// - std: バイトスライス分割・文字列変換
pub fn decode_data_macros(
    payload: &[u8],
    is_header_block: &mut bool,
    macro_fields: &mut std::collections::HashMap<String, String>,
) {
    use crate::milter_command::MilterMacro;

    // Step 1: ペイロードを0x00区切りで分割してマクロ要素を抽出
    let parts: Vec<&[u8]> = payload
        .split(|b| *b == 0x00) // NULバイト区切りで分割
        .filter(|s| !s.is_empty()) // 空要素を除外
        .collect(); // Vec<&[u8]>として収集

    if parts.is_empty() {
        // マクロ無しの場合は早期リターン
        return;
    }

    // Step 2: 先頭バイトからマクロフェーズ（DATA/CONNECT/HELO/SOH等）を判定
    let phase_macro_val = parts[0].first().copied().unwrap_or(0); // 先頭バイトを取得
    let phase_macro = MilterMacro::from_u8(phase_macro_val); // マクロenumに変換
    let phase_macro_str = phase_macro.as_str().to_string(); // 文字列表現を取得

    // Step 3: SOHマクロ検出時はヘッダブロックフラグを設定
    if phase_macro == MilterMacro::Soh {
        *is_header_block = true; // ヘッダブロック開始を通知
    }

    // Step 4: 先頭マクロ名・値の抽出と出力
    if parts[0].len() > 1 && parts.len() > 1 {
        let macro_name_bytes = &parts[0][1..]; // 先頭マクロ名（2バイト目以降）
        let macro_val_bytes = parts[1]; // 先頭マクロ値

        // 拡張マクロ（{name}形式）と標準マクロの判定・解析
        let macro_name = if let Some(&b'{') = macro_name_bytes.first() {
            // {name}形式の拡張マクロの場合
            if let Some(close_idx) = macro_name_bytes[1..].iter().position(|&b| b == b'}') {
                let name = String::from_utf8_lossy(&macro_name_bytes[1..1 + close_idx]);
                format!("{}({})", MilterMacro::Vender.as_str(), name) // ベンダー拡張として処理
            } else {
                format!("{}(Unknown)", MilterMacro::Vender.as_str()) // 不正な拡張マクロ
            }
        } else {
            // 標準マクロの場合（1バイトマクロ名）
            macro_name_bytes
                .first()
                .map(|&b| MilterMacro::from_u8(b).as_str().to_string())
                .unwrap_or(MilterMacro::Unknown(0).as_str().to_string())
        };

        let macro_val = String::from_utf8_lossy(macro_val_bytes).to_string(); // マクロ値をUTF-8変換
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[parser] マクロ[{}][{}]={}",
            phase_macro_str,
            macro_name,
            macro_val
        );
        // 先頭マクロ情報をHashMapに格納（パース処理で参照）
        macro_fields.insert(macro_name.clone(), macro_val.clone());
    }

    // Step 5: 残りマクロの順次処理（名前・値のペア単位で2つずつ処理）
    let mut idx = 2; // インデックス初期化（先頭マクロをスキップ）
    while idx + 1 < parts.len() {
        let macro_name_bytes = parts[idx]; // マクロ名バイト列
        let macro_val_bytes = parts[idx + 1]; // マクロ値バイト列

        // 拡張マクロ（{name}形式）と標準マクロの判定・解析
        let macro_name = if let Some(&b'{') = macro_name_bytes.first() {
            // {name}形式の拡張マクロの場合
            if let Some(close_idx) = macro_name_bytes[1..].iter().position(|&b| b == b'}') {
                let name = String::from_utf8_lossy(&macro_name_bytes[1..1 + close_idx]);
                format!("{}({})", MilterMacro::Vender.as_str(), name) // ベンダー拡張として処理
            } else {
                format!("{}(Unknown)", MilterMacro::Vender.as_str()) // 不正な拡張マクロ
            }
        } else {
            // 標準マクロの場合（1バイトマクロ名）
            macro_name_bytes
                .first()
                .map(|&b| MilterMacro::from_u8(b).as_str().to_string())
                .unwrap_or(MilterMacro::Unknown(0).as_str().to_string())
        };

        let macro_val = String::from_utf8_lossy(macro_val_bytes).to_string(); // マクロ値をUTF-8変換
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[parser] マクロ[{}][{}]={}",
            phase_macro_str,
            macro_name,
            macro_val
        );
        // 残りマクロ情報をHashMapに格納（パース処理で参照）
        macro_fields.insert(macro_name.clone(), macro_val.clone());
        idx += 2; // 次のマクロペア（名前・値）へ移動
    }
}

/// HEADERコマンドのデコード・格納処理
///
/// # 引数
/// - `payload`: HEADERコマンドのペイロード（ヘッダ名+NUL+ヘッダ値のバイト列）
/// - `header_fields`: ヘッダ情報を格納するHashMap（同一ヘッダ名で複数値対応）
///
/// # 説明
/// Milterプロトコルで受信したヘッダペイロードをNUL区切りで分割し、
/// ヘッダ名とヘッダ値を抽出してHashMapに格納する。
/// 同一ヘッダ名（Receivedなど）の複数値にも対応。
///
/// # 処理フロー
/// 1. ペイロードをUTF-8文字列に変換
/// 2. NULバイトを可視化してデバッグ出力
/// 3. NUL区切りでヘッダ名とヘッダ値に分割
/// 4. 前後の空白・末尾NULを除去して正規化
/// 5. HashMapに格納（同一キーは配列で複数値保持）
pub fn decode_header(
    payload: &[u8],
    header_fields: &mut std::collections::HashMap<String, Vec<String>>,
) {
    // Step 1: ペイロードをUTF-8文字列に変換
    let header_str = String::from_utf8_lossy(payload); // ペイロードをUTF-8文字列化

    // Step 2: NULバイトを可視化してデバッグ出力
    let header_str_visible = header_str.replace('\0', "<NUL>"); // NULバイトを可視化（デバッグ用）
    crate::printdaytimeln!(LOG_TRACE, "[parser] ヘッダ内容: {}", header_str_visible); // ヘッダ内容をログ出力

    // Step 3: NUL区切りでヘッダ名とヘッダ値に分割（最大2つに分割）
    let mut parts = header_str.splitn(2, '\0'); // NUL区切りでヘッダ名と値に分割

    // Step 4: ヘッダ名を抽出・正規化
    let key = parts
        .next()
        .unwrap_or("")
        .trim() // 前後の空白を除去
        .trim_end_matches('\0') // 末尾のNULバイトを除去
        .to_string(); // String型に変換

    // Step 5: ヘッダ値を抽出・正規化
    let val = parts
        .next()
        .unwrap_or("")
        .trim() // 前後の空白を除去
        .trim_end_matches('\0') // 末尾のNULバイトを除去
        .to_string(); // String型に変換

    // Step 6: HashMapに格納（同一ヘッダ名は配列で複数値保持）
    header_fields.entry(key).or_default().push(val); // ヘッダ名ごとに値を配列で追加
}

/// BODYコマンドのデコード・格納処理
///
/// # 引数
/// - `payload`: BODYコマンドのペイロード（メール本文の一部分のバイト列）
/// - `body_field`: メール本文を蓄積するためのString（可変参照）
///
/// # 説明
/// Milterプロトコルで受信したBODYコマンドのペイロード（メール本文の断片）を
/// UTF-8文字列に変換し、既存のbody_fieldに追記して蓄積する。
/// 複数回のBODYコマンドで送信される本文を順次結合し、完全な本文を構築する。
///
/// # 処理フロー
/// 1. ペイロードをUTF-8文字列に変換（エラー時は置換文字使用）
/// 2. 変換した文字列を既存のbody_fieldに追記
///
/// # 注意点
/// - 文字コード変換やデコード処理は行わない（生データのまま蓄積）
/// - BODYコマンドは複数回送信される可能性があるため追記処理を採用
/// - BODYEOB時点で完全なメール本文がbody_fieldに格納される
pub fn decode_body(payload: &[u8], body_field: &mut String) {
    // Step 1: ペイロードをUTF-8文字列に変換（無効バイトは置換文字に変換）
    let s = String::from_utf8_lossy(payload); // ペイロードをUTF-8文字列化

    // Step 2: 変換した文字列を既存body_fieldに追記（複数BODYコマンドの結合）
    body_field.push_str(&s); // 既存body_fieldに追記
}

/// Milter応答送信処理
///
/// # 引数
/// - `stream`: クライアントTCPストリーム
/// - `is_body_eob`: trueならBODYEOBとしてACCEPT応答（0x61）、falseならEOHとしてCONTINUE応答（0x06）
/// - `peer_addr`: クライアントアドレス
///
/// # 説明
/// EOH/BODYEOBコマンドを判定し、適切な応答（ACCEPT/CONTINUE）をクライアントに送信する。
pub async fn send_milter_response(
    stream: &mut TcpStream,
    peer_addr: &str,
    response: Option<(String, String)>,
) {
    // actionに応じてレスポンスコマンドを決定
    let (resp_cmd, resp_size) = match &response {
        Some((action, _)) if action == "NONE" => (0x61u8, 1u32), // NONE応答（0x61）
        Some((action, _)) if action == "ACCEPT" => (0x61u8, 1u32), // ACCEPT応答（0x61）
        Some((action, logname)) if action == "WARN" => {
            // WARN応答（0x61）
            // WARN応答の場合はADDHEADERコマンド(0x68)を送信
            let reply_packet = build_response_packet(
                0x68u8, // ADDHEADERコマンド メッセージ部分の先頭には半角スペースをつけないと、つながってしまう
                &format!("X-MilterSeparator\0 Warning: '{logname}' by MilterSeparator"),
            );
            if let Err(e) = stream.write_all(&reply_packet).await {
                crate::printdaytimeln!(
                    LOG_DEBUG,
                    "[response] ADDHEADER送信エラー: {}: {}",
                    peer_addr,
                    e
                );
            }
            (0x61u8, 1u32) // ACCEPT応答 (0x61)
        }
        Some((action, logname)) if action == "REJECT" => {
            // REJECT応答（0x72）
            // REJECT応答の場合はREPLYCODEコマンド(0x79)を送信
            let reply_packet = build_response_packet(
                0x79u8, // REPLYCODEコマンド
                &format!("550 5.7.1 Rejected: '{logname}' by MilterSeparator"),
            );
            if let Err(e) = stream.write_all(&reply_packet).await {
                crate::printdaytimeln!(
                    LOG_DEBUG,
                    "[response] REPLYCODE送信エラー: {}: {}",
                    peer_addr,
                    e
                );
            }
            (0x72u8, 1u32) // REJECT応答 (0x72)
        }
        Some((action, _)) if action == "DROP" => {
            // DISCARD応答（0x64）
            (0x64u8, 1u32) // DISCARD応答 (0x64)
        }
        _ => (0x66u8, 1u32), // デフォルトはCONTINUE(0x66)応答
    };

    let mut resp = Vec::with_capacity(5); // 応答バッファ（5バイト: サイズ4+コマンド1）
    resp.extend_from_slice(&resp_size.to_be_bytes()); // サイズ（4バイト）
    resp.push(resp_cmd); // コマンド（1バイト）
    // クライアントに応答を送信（非同期）
    if let Err(e) = stream.write_all(&resp).await {
        crate::printdaytimeln!(LOG_DEBUG, "[response] 応答送信エラー: {}: {}", peer_addr, e);
    // 送信失敗時はエラーログ
    } else {
        let (action, logname) = response.as_ref().unwrap();
        crate::printdaytimeln!(
            LOG_DEBUG,
            "[response] 応答送信: (0x{:02X}) to {} | action={} logname={}",
            resp_cmd,
            peer_addr,
            action,
            logname
        );
    }
}

fn build_response_packet(response_code: u8, response_message: &str) -> Vec<u8> {
    // 応答内容（null終端）
    let payload = format!("{response_message}\0");
    let bytes = payload.as_bytes();

    let mut packet = Vec::with_capacity(4 + 1 + bytes.len());
    packet.extend(&(bytes.len() as u32 + 1).to_be_bytes()); // サイズ
    packet.push(response_code); // コマンド（1バイト）
    packet.extend(bytes); // メッセージ内容
    packet
}
