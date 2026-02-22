# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

cckit (Claude Code Kit) is a Rust CLI toolkit for managing Claude Code environments. It provides session monitoring (TUI/menubar), project inspection, and cleanup tools. macOS-specific features include a menubar app and desktop notifications via native Objective-C bindings.

## Build & Development Commands

```bash
# Build
cargo build --release --bins    # or: mise run build

# Test
cargo test                      # or: mise run test

# Lint & Format (CI runs these)
cargo clippy -- -D warnings
cargo fmt --check

# Install locally
cargo install --path .          # or: mise run install

# macOS app bundle
mise run build-app              # runs scripts/macos/build_app.sh
```

## Architecture

**Two binaries** defined in `Cargo.toml`:
- `cckit` (default) — main CLI (`src/main.rs` → `src/cli.rs`)
- `cckit-app` — macOS menubar app (`src/bin/cckit_app.rs`)

**CLI layer** (`src/cli.rs`, ~2000 lines): All subcommand definitions (clap derive), project scanning logic (`ls`, `prune`, `config`, `doctor`, `status`), and YAML frontmatter parsing for skills/agents/commands.

**Monitor module** (`src/monitor/`): Session tracking and UI components.

| File | Role |
|------|------|
| `session.rs` | `Session`, `SessionStatus`, `SessionStore` data models |
| `storage.rs` | File-based storage with `fs2` file locking, atomic writes (tmp + rename) |
| `hook.rs` | Claude Code hook event handler (reads stdin JSON from hook events) |
| `setup.rs` | Install/uninstall hooks in `~/.claude/settings.json` |
| `tui.rs` | ratatui-based interactive TUI |
| `menubar.rs` | macOS NSStatusBar/NSMenu via objc2 |
| `notification.rs` | macOS custom notification window via objc2 |
| `focus.rs` | Terminal focus via AppleScript (iTerm2, Terminal.app, Ghostty) |

**Data flow**: Claude Code hooks → `cckit session hook` (stdin JSON) → `storage.rs` (sessions.json with file lock) → TUI/menubar reads and displays.

## Key Conventions

- **Rust edition 2024**, targets macOS primarily (conditional deps for macOS-only features)
- **Version**: embedded via `build.rs` running `git describe --always --dirty`
- **Data directory**: `~/Library/Application Support/cckit/` (macOS) or `~/.local/share/cckit/` (Linux)
- **Config**: reads `~/.claude.json` for project list, `~/.claude/settings.json` for hooks
- Project-local config: `cckit.toml` (optional, for `disable_paths`)
- Uses `serde_json` with `preserve_order` feature for JSON field ordering
