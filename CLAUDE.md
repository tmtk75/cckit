# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

cckit (Claude Code Kit) is a Rust CLI toolkit for managing AI coding tool environments (Claude Code and OpenAI Codex). It provides session monitoring (TUI/menubar/window app), project inspection, and cleanup tools. macOS-specific features include a menubar app and desktop notifications via native Objective-C bindings.

## Build & Development Commands

**Prerequisites:** `mise` (manages Rust toolchain and tasks — see `mise.toml`)

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

# Run during development
cargo run -- session ls          # list sessions
cargo run -- app                 # run macOS window app

# macOS app bundle
mise run build-app              # runs scripts/macos/build_app.sh
```

## Architecture

**Single binary** `cckit` (`src/main.rs` → `src/cli.rs`):
- CLI mode: all subcommands including TUI (`cckit session ls`)
- App mode: `cckit app` runs macOS window + menubar (also auto-detected when launched from .app bundle)

**CLI layer** (`src/cli.rs`, ~2000 lines): All subcommand definitions (clap derive), project scanning logic (`ls`, `prune`, `config`, `doctor`, `status`), and YAML frontmatter parsing for skills/agents/commands.

**Monitor module** (`src/monitor/`): Session tracking and UI components.

| File | Role |
|------|------|
| `session.rs` | `Session`, `SessionStatus`, `SessionStore` data models |
| `storage.rs` | File-based storage with `fs2` file locking, atomic writes (tmp + rename) |
| `hook.rs` | Hook event handler for Claude Code and Codex (reads stdin JSON) |
| `setup.rs` | Install/uninstall hooks in `~/.claude/settings.json` or `~/.codex/hooks.json` |
| `tui.rs` | ratatui-based interactive TUI |
| `menubar.rs` | macOS NSStatusBar/NSMenu via objc2 |
| `window.rs` | macOS NSWindow session monitor app via objc2 (`run_app` unifies window + menubar) |
| `notification.rs` | macOS custom notification window via objc2 |
| `focus.rs` | Terminal focus via AppleScript (iTerm2, Terminal.app, Ghostty) |

**Data flow**: Claude Code / Codex hooks → `cckit session hook` (stdin JSON) → `storage.rs` (sessions.json with file lock) → TUI/menubar/window reads and displays.

**Subagent detection**: Sessions with `prompt_count == 0 && tool_count > 0` and a Claude model are detected as subagents. Agent names are extracted from the transcript's `agentName` field. Non-Claude models (Codex) are excluded from this heuristic.

**Stale session cleanup**: `load_sessions()` in window.rs calls `sync_sessions()` to remove sessions whose TTY is gone or process has exited. For `tty=unknown` sessions (Codex Desktop), PID liveness is checked via `kill -0`.

## Key Conventions

- **CI**: GitHub Actions runs `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on all branches. Release workflow triggers on `v*` tags.
- **Rust edition 2024**, targets macOS primarily (conditional deps for macOS-only features)
- **Version**: embedded via `build.rs` running `git describe --always --dirty`
- **Data directory**: `~/Library/Application Support/cckit/` (macOS) or `~/.local/share/cckit/` (Linux)
- **Config**: reads `~/.claude.json` for project list, `~/.claude/settings.json` for Claude Code hooks, `~/.codex/hooks.json` for Codex hooks
- Project-local config: `cckit.toml` (optional, for `disable_paths`)
- Uses `serde_json` with `preserve_order` feature for JSON field ordering
