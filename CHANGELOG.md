# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-02-10

### Added
- Initial release of MilterSeparator
- Full Milter protocol implementation compatible with Postfix/Sendmail
- Asynchronous TCP server using Tokio runtime
- MIME email parsing with mail-parser crate
- Comprehensive email analysis and output:
  - From/To/Subject extraction
  - Content-Type and encoding detection
  - Multi-part email support
  - Attachment detection with filename extraction
  - Text/non-text part classification
- JST timestamp logging with chrono-tz
- Configuration file support (`MilterSeparator.conf`)
- Signal handling:
  - SIGHUP for configuration reload
  - SIGTERM for graceful shutdown
- Debug features:
  - NUL byte visualization
  - Hex dump output for unknown commands
  - Detailed protocol logging
- Error handling and timeout management
- IPv4/IPv6 dual-stack support

### Technical Features
- Modular architecture with clear separation of concerns:
  - `main.rs`: Server startup and management
  - `client.rs`: Per-client Milter protocol handling
  - `milter.rs`: Milter command processing
  - `milter_command.rs`: Protocol definitions
  - `parse.rs`: Email parsing and analysis
  - `init.rs`: Configuration management
  - `logging.rs`: Timestamp logging utilities
- Comprehensive documentation and comments
- Rust 2021 edition compatibility
- MIT license

### Dependencies
- tokio 1.38 (async runtime)
- mail-parser 0.11 (MIME parsing)
- chrono 0.4 (date/time handling)
- chrono-tz 0.8 (timezone support)
- lazy_static 1.5.0 (global variables)
