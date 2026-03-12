# MilterSeparator

MilterSeparator は、メールに添付されているファイルを自動的に分離して ZIP 圧縮し、
指定回数・指定期間のみダウンロード可能なリンクに変換する **PPAP 対策用 Milter サーバー**です。

Postfix / Sendmail の Milter (Mail Filter) インターフェース上で動作し、
添付ファイルを受信時に取り出してサーバー上に保存します。
メール本文は添付ファイルを取り除いたうえで、ダウンロード URL・パスワード・
有効期限の案内テキストに書き換えて配信します。

---

## 主な機能

- **Milter プロトコル実装** — Postfix / Sendmail に対応
- **添付ファイルの自動 ZIP 圧縮** — ランダムパスワード付き（強度 3 段階）
- **ダウンロード管理** — 最大回数・有効期限を設定可能
- **認証方式の選択** — minimal / token / basic の 3 モード
- **マルチ DB 対応** — SQLite3 / MySQL / PostgreSQL を設定で切替
- **SIGHUP によるノーダウンリロード** — サービス停止なしに設定を再読込
- **systemd サービス対応**
- **logrotate 設定同梱**

---

## 動作環境

| 項目 | 内容 |
|------|------|
| OS | Linux（AlmaLinux 9 / Rocky Linux 9 / Ubuntu 22.04 以降推奨） |
| Rust | edition 2024 / Rustup 最新 stable |
| MTA | Postfix 3.x / Sendmail 8.x （Milter 対応版） |
| DB | SQLite3（デフォルト）/ MySQL 8.x / MariaDB 10.x / PostgreSQL 14 以降 |

---

## ビルド

```bash
# リポジトリのクローン後
cd MilterSeparator
cargo build --release
```

バイナリは `target/release/milter_separator` に生成されます。

---

## インストール

```bash
# バイナリを配置
sudo install -m 755 target/release/milter_separator /usr/local/sbin/

# メイン設定ファイルを配置
sudo install -m 640 etc/MilterSeparator.conf /etc/MilterSeparator.conf

# インクルードディレクトリと Parameter.conf を配置
sudo mkdir -p /etc/MilterSeparator.d/templates
sudo install -m 640 etc/MilterSeparator.d/Parameter.conf \
    /etc/MilterSeparator.d/Parameter.conf
sudo install -m 644 etc/MilterSeparator.d/templates/counter_template.cgi \
    /etc/MilterSeparator.d/templates/counter_template.cgi
sudo install -m 644 etc/MilterSeparator.d/templates/download_template.html \
    /etc/MilterSeparator.d/templates/download_template.html

# systemd ユニットを配置して有効化
sudo install -m 644 systemd/milter_separator.service \
    /etc/systemd/system/milter_separator.service
sudo systemctl daemon-reload
sudo systemctl enable --now milter_separator

# logrotate を配置
sudo install -m 644 logrotate.d/milter_separator \
    /etc/logrotate.d/milter_separator
```

---

## 設定

設定は 2 段階のファイルで管理されます。

### 1. MilterSeparator.conf（メイン設定）

| キー | 説明 | デフォルト |
|------|------|-----------|
| `Listen` | 待受ポートまたはアドレス:ポート | `8895` |
| `Client_timeout` | クライアントタイムアウト（秒） | `30` |
| `Log_file` | ログ出力先ファイルパス | 標準出力 |
| `Log_level` | ログ詳細度（`info` / `trace` / `debug`） | `info` |
| `RemoteIP_Target` | 対象メールの送信元（`0`=外部 / `1`=内部 / `2`=全て） | `0` |
| `milter_user` | ストレージ・DB ファイルの所有ユーザー | `milter` |
| `milter_group` | ストレージ・DB ファイルの所有グループ | `apache` |
| `include` | 追加設定ディレクトリのパス | — |

### 2. MilterSeparator.d/ (サブ設定)
### 2.1. Parameter.conf（各種機能設定）

| キー | 説明 | デフォルト |
|------|------|-----------|
| `password_strength` | ZIP パスワード強度（`low` / `medium` / `high`） | `medium` |
| `max_downloads` | 最大ダウンロード回数 | `5` |
| `expire_hours` | ダウンロード有効期限（時間） | `168`（7日） |
| `download_auth_mode` | 認証方式（`minimal` / `token` / `basic`） | `minimal` |
| `basic_auth_user` | Basic 認証ユーザー名 | — |
| `basic_auth_password` | Basic 認証パスワード | — |
| `token_auth_key` | HMAC トークン生成の秘密鍵 | — |
| `token_auth_type` | HMAC アルゴリズム（`hmac-sha256` 等） | `hmac-sha256` |
| `delete_mode` | 削除方式（`delete` / `script`） | `delete` |
| `delete_script_path` | `script` モード時の実行スクリプトパス | — |
| `storage_path` | 添付ファイル保存先ルートディレクトリ | `/var/lib/milterseparator` |
| `base_url` | ダウンロード URL のベース | — |
| `counter_cgi` | カウンタスクリプトのファイル名 | `count.cgi` |

### 2.2. templates/（テンプレート）

添付ファイルを分離する際、UUID ディレクトリ内に以下のファイルがコピーされます。
プレースホルダー（`{{db_type}}` 等）は生成時に実際の設定値で置換されます。

| ファイル | 説明 |
|----------|------|
| `counter_template.cgi` | ダウンロード回数カウンタ（PHP スクリプト）。ファイルが取得されるたびに `download_tbl` の `download_count` をインクリメントする。SQLite / MySQL / PostgreSQL に対応し、PDO 経由で DB へ接続する。ファイル名は `counter_cgi` 設定値に合わせる（デフォルト: `count.cgi`）。 |
| `download_template.html` | ダウンロードページの HTML テンプレート。レスポンシブデザインのカード型 UI で、ファイル名・パスワード・有効期限・ダウンロードボタンを表示する。プレースホルダーが UUID・URL・パスワード等の実際の値に置換されて各 UUID ディレクトリに配置される。 |

---

## データベース設定

`Parameter.conf` に以下のキーを記述することで DB 種別を切り替えられます。

### SQLite3（デフォルト）

```
Database_Type   sqlite
Database_Path   /var/lib/milterseparator/db.sqlite3
```

SQLite の場合、起動時に `Database_Path` のファイルが自動生成されます。
PHP / Apache からも書き込めるよう、`milter_group` に設定したグループに
書き込み権限 (660) が自動付与されます。

### MySQL / MariaDB

```
Database_Type     mysql
Database_Host     127.0.0.1
Database_Port     3306
Database_User     milter
Database_Password パスワード
Database_Name     milter_separator
```

### PostgreSQL

```
Database_Type     postgres
Database_Host     127.0.0.1
Database_Port     5432
Database_User     milter
Database_Password パスワード
Database_Name     milter_separator
```

> **注意**: テーブルは初回起動時に `CREATE TABLE IF NOT EXISTS` で自動作成されます。
> 整数カラム（`expire_hours`, `max_downloads` 等）は `BIGINT` で定義されており、
> Rust の `i64` 型と一致します。

---

## パスワード強度

| 設定値 | 文字種 | 長さ |
|--------|--------|------|
| `low` | 英数字 | 8 文字以上 |
| `medium` | 大文字・小文字・数字・記号 | 12 文字以上 |
| `high` | 大文字・小文字・数字・記号 | 16 文字以上 |

---

## 認証モード

| モード | 説明 |
|--------|------|
| `minimal` | UUID の存在確認のみ。追加認証なし。 |
| `token` | HMAC トークンを URL に付与。`token_auth_key` が必須。 |
| `basic` | Basic 認証。`basic_auth_user` / `basic_auth_password` が必須。 |

---

## シグナル

| シグナル | 動作 |
|----------|------|
| `SIGHUP` | 設定ファイルを再読込し、全クライアント接続をリセット |
| `SIGTERM` | サービスを安全に終了 |

```bash
# 設定をリロード（サービス停止なし）
sudo systemctl reload milter_separator
# または
sudo kill -HUP $(pidof milter_separator)
```

---

## Postfix との連携

`/etc/postfix/main.cf` に以下を追加します。

```
smtpd_milters   = inet:127.0.0.1:8895
milter_default_action = accept
```

設定後、Postfix を再読込します。

```bash
sudo systemctl reload postfix
```

---

## ログ確認

```bash
# journald
sudo journalctl -u milter_separator -f

# ファイルログ（Log_file を設定している場合）
tail -f /var/log/milterseparator.log
```

---

## ディレクトリ構成

```
MilterSeparator/
├── src/
│   ├── main.rs          # サーバー起動・シグナル処理
│   ├── client.rs        # Milter クライアント接続処理
│   ├── milter.rs        # Milter コマンドデコード・応答
│   ├── milter_command.rs# Milter プロトコル定数定義
│   ├── parse.rs         # メール / 添付ファイル解析
│   ├── zipper.rs        # 添付保存・ZIP 圧縮処理
│   ├── download.rs      # ダウンロード URL 生成
│   ├── db.rs            # DB 初期化・レコード挿入
│   ├── init.rs          # 設定ファイル読み込み
│   └── logging.rs       # JST タイムスタンプ付きログ
├── etc/
│   ├── MilterSeparator.conf          # メイン設定
│   └── MilterSeparator.d/
│       ├── Parameter.conf            # 機能設定
│       ├── Parameter.conf.sample     # 設定サンプル
│       └── templates/
│           ├── counter_template.cgi  # カウンタ CGI テンプレート
│           └── download_template.html# ダウンロードページテンプレート
├── systemd/
│   └── milter_separator.service      # systemd ユニットファイル
├── logrotate.d/
│   └── milter_separator              # logrotate 設定
└── Cargo.toml
```

---

## ライセンス

MIT License — 詳細は [LICENSE](LICENSE) を参照してください。

---

## 開発・貢献

バグ報告・機能提案は Issue、コードの貢献は Pull Request でお知らせください。
貢献の方針は [CONTRIBUTING.md](CONTRIBUTING.md) を参照してください。
