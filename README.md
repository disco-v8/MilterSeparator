# MilterSeparator

MilterSeparator is a **Milter server for PPAP countermeasures** that automatically separates attachments from incoming emails, compresses them into a password-protected ZIP file, and replaces them with a time-limited, download-count-limited link.

It operates on the Milter (Mail Filter) interface of Postfix / Sendmail.  
Attachments are extracted on receipt and saved to the server.  
The email body is rewritten with a download URL, password, and expiry notice before delivery.

---

## Features

- **Milter protocol implementation** — Compatible with Postfix / Sendmail
- **Automatic ZIP compression of attachments** — Random password, 3 strength levels
- **Download management** — Configurable max download count and expiry period
- **Authentication modes** — minimal / token / basic
- **Multi-database support** — SQLite3 / MySQL / PostgreSQL, switchable via config
- **Automatic mail body rewriting** — Remove attachment parts, insert download info, or add a new text part (body replacement via SMFIR_REPLBODY)
- **Zero-downtime reload via SIGHUP** — Reload configuration without stopping the service
- **systemd service support**
- **logrotate configuration included**

---

## Requirements

| Item | Details |
|------|---------|
| OS | Linux (AlmaLinux 9 / Rocky Linux 9 / Ubuntu 22.04 or later recommended) |
| Rust | edition 2024 / latest stable via Rustup |
| MTA | Postfix 3.x / Sendmail 8.x (Milter-enabled build) |
| DB | SQLite3 (default) / MySQL 8.x / MariaDB 10.x / PostgreSQL 14+ |

---

## Build

```bash
# After cloning the repository
cd MilterSeparator
cargo build --release
```

The binary is generated at `target/release/milter_separator`.

---

## Installation

```bash
# Install binary
sudo install -m 755 target/release/milter_separator /usr/local/sbin/

# Install main configuration file
sudo install -m 640 etc/MilterSeparator.conf /etc/MilterSeparator.conf

# Install include directory and Parameter.conf
sudo mkdir -p /etc/MilterSeparator.d/templates
sudo install -m 640 etc/MilterSeparator.d/Parameter.conf \
    /etc/MilterSeparator.d/Parameter.conf
sudo install -m 644 etc/MilterSeparator.d/templates/counter_template.cgi \
    /etc/MilterSeparator.d/templates/counter_template.cgi
sudo install -m 644 etc/MilterSeparator.d/templates/download_template.html \
    /etc/MilterSeparator.d/templates/download_template.html

# Install mail body rewriting templates
sudo install -m 644 etc/MilterSeparator.d/templates/DownloadInfoHeadTemplate.txt \
    /etc/MilterSeparator.d/templates/DownloadInfoHeadTemplate.txt
sudo install -m 644 etc/MilterSeparator.d/templates/DownloadInfoTailTemplate.txt \
    /etc/MilterSeparator.d/templates/DownloadInfoTailTemplate.txt
sudo install -m 644 etc/MilterSeparator.d/templates/DownloadInfoTextTemplate.txt \
    /etc/MilterSeparator.d/templates/DownloadInfoTextTemplate.txt

# Install and enable systemd unit
sudo install -m 644 systemd/milter_separator.service \
    /etc/systemd/system/milter_separator.service
sudo systemctl daemon-reload
sudo systemctl enable --now milter_separator

# Install logrotate configuration
sudo install -m 644 logrotate.d/milter_separator \
    /etc/logrotate.d/milter_separator
```

---

## Configuration

Configuration is managed in two levels.

### 1. MilterSeparator.conf (Main configuration)

| Key | Description | Default |
|-----|-------------|---------|
| `Listen` | Listen port or address:port | `8895` |
| `Client_timeout` | Client timeout (seconds) | `30` |
| `Log_file` | Log output file path | stdout |
| `Log_level` | Log verbosity (`info` / `trace` / `debug`) | `info` |
| `RemoteIP_Target` | Target mail origin (`0`=external / `1`=internal / `2`=all) | `0` |
| `milter_user` | Owner user for storage and DB files | `milter` |
| `milter_group` | Owner group for storage and DB files | `apache` |
| `include` | Path to additional configuration directory | — |

### 2. MilterSeparator.d/ (Sub-configuration)

#### 2.1. Parameter.conf (Feature settings)

| Key | Description | Default |
|-----|-------------|---------|
| `password_strength` | ZIP password strength (`low` / `medium` / `high`) | `medium` |
| `max_downloads` | Maximum download count | `5` |
| `expire_hours` | Download expiry period (hours) | `168` (7 days) |
| `download_auth_mode` | Authentication mode (`minimal` / `token` / `basic`) | `minimal` |
| `basic_auth_user` | Basic authentication username | — |
| `basic_auth_password` | Basic authentication password | — |
| `token_auth_key` | Secret key for HMAC token generation | — |
| `token_auth_type` | HMAC algorithm (e.g. `hmac-sha256`) | `hmac-sha256` |
| `delete_mode` | Deletion method (`delete` / `script`) | `delete` |
| `delete_script_path` | Script path used when `delete_mode = script` | — |
| `storage_path` | Root directory for storing attachment files | `/var/lib/milterseparator` |
| `base_url` | Base URL for download links | — |
| `counter_cgi` | Filename of the counter script | `count.cgi` |
| `Remove_Attachments_From_Body` | Remove attachment parts from the mail body (`yes` / `no`) | `no` |
| `Insert_Download_Info_Position_head` | Insert download info at the **beginning** of the first text/plain part (`yes` / `no`) | `no` |
| `Insert_Download_Info_Position_tail` | Insert download info at the **end** of the first text/plain part (`yes` / `no`) | `no` |
| `Add_Download_Info_As_New_Text_Part` | Append download info as a **new text/plain part** at the end of multipart (`yes` / `no`) | `no` |

#### 2.2. templates/ (Templates)

The following files are copied into each UUID directory when an attachment is separated.  
Placeholders (e.g. `{{db_type}}`) are replaced with actual configuration values at generation time.

| File | Description |
|------|-------------|
| `counter_template.cgi` | Download counter (PHP script). Increments `download_count` in `download_tbl` on each file access. Supports SQLite / MySQL / PostgreSQL via PDO. The filename must match the `counter_cgi` setting (default: `count.cgi`). |
| `download_template.html` | HTML template for the download page. Responsive card-style UI displaying filename, password, expiry date, and a download button. Placeholders are replaced with the actual UUID, URL, password, etc. and placed in each UUID directory. |

#### 2.3. templates/ additions (Mail body rewriting templates)

Text templates referenced when `Remove_Attachments_From_Body`, `Insert_Download_Info_Position_*`,  
or `Add_Download_Info_As_New_Text_Part` are enabled. (Placed in the same `templates/` directory as 2.2.)

Available placeholders:

| Placeholder | Expanded value |
|-------------|---------------|
| `{{filename}}` | First attachment filename |
| `{{download_url}}` | Download URL |
| `{{zip_password}}` | ZIP password |
| `{{expire_hours}}` | Expiry period (hours) |
| `{{uuid}}` | UUID |
| `{{expires_at}}` | Expiry date/time string |

| File | Description |
|------|-------------|
| `DownloadInfoHeadTemplate.txt` | Text inserted at the **beginning** of the first text/plain part when `Insert_Download_Info_Position_head yes`. |
| `DownloadInfoTailTemplate.txt` | Text inserted at the **end** of the first text/plain part when `Insert_Download_Info_Position_tail yes`. |
| `DownloadInfoTextTemplate.txt` | Text appended as a **new text/plain part** at the end of the multipart when `Add_Download_Info_As_New_Text_Part yes`. Existing parts are not modified. |

---

## Database Configuration

The database type can be switched by adding the following keys to `Parameter.conf`.

### SQLite3 (Default)

```
Database_Type   sqlite
Database_Path   /var/lib/milterseparator/db.sqlite3
```

The database file is created automatically on first start.  
Write permission (660) is automatically granted to the group specified in `milter_group`  
so that PHP / Apache can also write to it.

### MySQL / MariaDB

```
Database_Type     mysql
Database_Host     127.0.0.1
Database_Port     3306
Database_User     milter
Database_Password yourpassword
Database_Name     milter_separator
```

### PostgreSQL

```
Database_Type     postgres
Database_Host     127.0.0.1
Database_Port     5432
Database_User     milter
Database_Password yourpassword
Database_Name     milter_separator
```

> **Note**: Tables are created automatically on first start with `CREATE TABLE IF NOT EXISTS`.  
> Integer columns (`expire_hours`, `max_downloads`, etc.) are defined as `BIGINT`  
> to match Rust's `i64` type.

---

## Password Strength

| Value | Character types | Length |
|-------|----------------|--------|
| `low` | Alphanumeric | 8+ characters |
| `medium` | Upper, lower, digits, symbols | 12+ characters |
| `high` | Upper, lower, digits, symbols | 16+ characters |

---

## Authentication Modes

| Mode | Description |
|------|-------------|
| `minimal` | UUID existence check only. No additional authentication. |
| `token` | HMAC token appended to URL. `token_auth_key` is required. |
| `basic` | Basic authentication. `basic_auth_user` / `basic_auth_password` are required. |

---

## Signals

| Signal | Behavior |
|--------|----------|
| `SIGHUP` | Reload configuration file and reset all client connections |
| `SIGTERM` | Graceful shutdown |

```bash
# Reload configuration (no service interruption)
sudo systemctl reload milter_separator
# or
sudo kill -HUP $(pidof milter_separator)
```

---

## Postfix Integration

Add the following to `/etc/postfix/main.cf`:

```
smtpd_milters   = inet:127.0.0.1:8895
milter_default_action = accept
```

Then reload Postfix:

```bash
sudo systemctl reload postfix
```

---

## Checking Logs

```bash
# journald
sudo journalctl -u milter_separator -f

# File log (when Log_file is configured)
tail -f /var/log/milterseparator.log
```

---

## Directory Structure

```
MilterSeparator/
├── src/
│   ├── main.rs          # Server startup and signal handling
│   ├── client.rs        # Milter client connection handling
│   ├── milter.rs        # Milter command decode and response
│   ├── milter_command.rs# Milter protocol constant definitions
│   ├── parse.rs         # Email and attachment parsing
│   ├── zipper.rs        # Attachment storage and ZIP compression
│   ├── download.rs      # Download URL generation
│   ├── db.rs            # DB initialization and record insertion
│   ├── init.rs          # Configuration file loading
│   └── logging.rs       # JST timestamp logging
├── etc/
│   ├── MilterSeparator.conf          # Main configuration
│   └── MilterSeparator.d/
│       ├── Parameter.conf            # Feature configuration
│       ├── Parameter.conf.sample     # Configuration sample
│       ├── templates/
│       │   ├── counter_template.cgi  # Counter CGI template
│       │   └── download_template.html# Download page template
│       └── templates/
│           ├── counter_template.cgi  # Counter CGI template
│           ├── download_template.html# Download page template
│           ├── DownloadInfoHeadTemplate.txt  # Body head-insert template
│           ├── DownloadInfoTailTemplate.txt  # Body tail-insert template
│           └── DownloadInfoTextTemplate.txt  # New text-part template
├── systemd/
│   └── milter_separator.service      # systemd unit file
├── logrotate.d/
│   └── milter_separator              # logrotate configuration
└── Cargo.toml
```

---

## License

MIT License — see [LICENSE](LICENSE) for details.

---

## Contributing

Bug reports and feature requests are welcome via Issues.  
Code contributions are welcome via Pull Requests.  
Please see [CONTRIBUTING.md](CONTRIBUTING.md) for contribution guidelines.
