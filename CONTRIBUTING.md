# Contributing to MilterSeparator

Thank you for your interest in contributing to MilterSeparator! This document provides guidelines for contributing to the project.

## Development Setup

### Prerequisites

- Rust 1.70 or later
- Git
- A text editor or IDE with Rust support

### Building

```bash
# Debug build
cargo build

# Release build
cargo build --release

# Run tests
cargo test

# Check code formatting
cargo fmt --check

# Run clippy lints
cargo clippy
```

### Running the Server

```bash
# Copy sample configuration
cp MilterSeparator.conf.sample MilterSeparator.conf

# Run in development mode
cargo run
```

## Code Style

- Follow standard Rust formatting (use `cargo fmt`)
- Use meaningful variable and function names
- Add comprehensive comments, especially for complex logic
- Maintain the existing comment style in each module
- Use the existing logging macros (`printdaytimeln!`) for output

### Comment Style

Each module should have a header comment block like:

```rust
// =========================
// module_name.rs
// MilterSeparator Module Description
//
// 【このファイルで使う主なクレート】
// - crate_name: Description of usage
//
// 【役割】
// - Primary responsibility
// - Secondary responsibility
// =========================
```

## Making Changes

### Bug Fixes

1. **Identify the issue** clearly
2. **Write a test** that reproduces the bug (if applicable)
3. **Fix the bug** with minimal changes
4. **Verify the fix** works and doesn't break existing functionality
5. **Update documentation** if necessary

### New Features

1. **Discuss the feature** by opening an issue first
2. **Design the feature** with minimal impact on existing code
3. **Implement the feature** following existing patterns
4. **Add tests** for the new functionality
5. **Update documentation** and README if needed

### Documentation

- Update README.md for user-facing changes
- Add inline comments for complex logic
- Update configuration documentation for new options

## Testing

### Manual Testing

1. **Start the server**:
   ```bash
   cargo run
   ```

2. **Test with Postfix** (if available):
   - Configure Postfix to use the milter
   - Send test emails
   - Verify output

3. **Test configuration reload**:
   ```bash
   kill -HUP $(pidof milter_separator)
   ```

### Automated Testing

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests with output
cargo test -- --nocapture
```

## Submitting Changes

1. **Commit your changes** with clear messages:
   ```bash
   git add .
   git commit -m "Add feature: brief description"
   ```

2. **Push to your fork**:
   ```bash
   git push origin feature/your-feature-name
   ```

3. **Create a Pull Request** on GitHub with:
   - Clear title and description
   - Reference any related issues
   - List of changes made
   - Testing performed

## Code Review Process

1. **Automated checks** must pass (formatting, clippy, etc.)
2. **Manual review** by maintainers
3. **Testing** on different environments
4. **Approval** and merge

## Reporting Issues

When reporting bugs or requesting features:

1. **Search existing issues** first
2. **Use issue templates** if available
3. **Provide clear reproduction steps** for bugs
4. **Include relevant log output** and configuration
5. **Specify your environment** (OS, Rust version, etc.)

## Communication

- **GitHub Issues**: For bug reports and feature requests
- **Pull Request Comments**: For code-specific discussions
- **Commit Messages**: Should be clear and descriptive

## License

By contributing to MilterSeparator, you agree that your contributions will be licensed under the MIT License.

## Questions?

If you have questions about contributing, feel free to open an issue with the "question" label.
