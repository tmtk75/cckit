# Repository Guidelines

## Project Structure & Module Organization
`src/main.rs` is the CLI entrypoint and delegates most command logic to `src/cli.rs`. Session monitoring lives under `src/monitor/`, split by concern: `hook.rs` for Claude Code hook ingestion, `storage.rs` for file-backed state, `tui.rs` for the terminal UI, and `menubar.rs` / `window.rs` / `notification.rs` for macOS UI. Extra binaries live in `src/bin/` (`cckit_app.rs`, `cckit_window.rs`). Reference material is in `docs/`, visual assets in `assets/`, and the macOS app bundle script in `scripts/macos/build_app.sh`.

## Build, Test, and Development Commands
Use Cargo directly or the matching `mise` tasks:

- `cargo build --release --bins` or `mise run build`: build all binaries.
- `cargo test` or `mise run test`: run the full test suite.
- `cargo clippy -- -D warnings`: enforce lint-clean code.
- `cargo fmt --check`: verify formatting before review.
- `cargo install --path .` or `mise run install`: install the local build.
- `mise run build-app`: build the macOS app bundle.

## Coding Style & Naming Conventions
This project uses Rust 2024 edition. Follow `rustfmt` defaults: 4-space indentation, trailing commas where formatter expects them, and small focused functions over broad helpers. Use `snake_case` for functions, modules, and files; `CamelCase` for types and enums; `SCREAMING_SNAKE_CASE` for constants. Keep platform-specific code behind `cfg(target_os = "macos")` or in the existing macOS-specific modules.

## Testing Guidelines
Tests are inline `#[cfg(test)]` modules near the implementation, for example in `src/cli.rs` and `src/monitor/*.rs`. Add unit tests alongside any parsing, hook handling, or state-transition changes. Prefer descriptive names like `test_has_cckit_hook_found` that state the scenario. Run `cargo test` locally before opening a PR.

## Commit & Pull Request Guidelines
Recent history mixes short prefixes such as `docs:` and `add:` with plain imperative subjects. Prefer concise, imperative commit messages that describe the observable change, for example `docs: add permissions command to README` or `add session sync cleanup`. For pull requests, include the purpose, user-facing impact, and validation steps. Attach screenshots or GIFs when changing TUI or macOS UI behavior.

## Configuration & Safety Tips
`cckit` reads `~/.claude.json` and `~/.claude/settings.json`; avoid hardcoding local paths in committed changes. Hook-related features depend on `cckit session install`, and session state is stored in the platform data directory (`~/Library/Application Support/cckit/` on macOS).
