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

**CLI layer** (`src/cli.rs`, ~7900 lines): All subcommand definitions (clap derive), project scanning logic (`ls`, `prune`, `config`, `doctor`, `status`, `tidy-up`, `permissions`), YAML frontmatter parsing for skills/agents/commands, and dedicated `skill`/`mcp`/`agent` management subcommands (`ls`, `copy`, `promote`, `prune`, `remove`, `stale`, `how-to-remove`, `validate`). `mcp prune` removes stale MCP residue (orphaned `enabledMcpjsonServers`/`disabledMcpjsonServers` approvals and empty `.mcp.json` files); `mcp remove` deletes matching MCP server definitions across project/user/global scopes (plugin entries are reported, not removed); both are dry-run by default with `--execute` to apply and `.bak` backups. `skill remove` moves matching directory skills to a reversible trash (`<data_dir>/trash/`; marketplace/plugin untouched). `skill stale` mines Skill tool invocations from `~/.claude/projects/**/*.jsonl` to flag skills not fired in `--days N` (default 90), with an origin label (`self` / `external:marketplace` / `external:installed`).

**History module** (`src/history/`): Session search and browsing across past Claude Code transcripts.

| File | Role |
|------|------|
| `mod.rs` | `SearchOpts`, `SessionRecord`, `Turn`, `Hit` data models |
| `loader.rs` | Scan and parse session JSONL transcript files |
| `search.rs` | AND-term and fuzzy search over session text |
| `format.rs` | Plain text and JSON output formatting |
| `tui.rs` | Interactive ratatui browser for search results |
| `skill_usage.rs` | Aggregate Skill tool invocations from transcripts (last-fired/count) for `skill stale` |

**Marketplace module** (`src/marketplace.rs`): Plugin marketplace inspection and validation (`marketplace summary`, `marketplace doctor`).

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
| `window_hover.rs` | Hover popover hit-testing and transcript preview extraction |
| `notification.rs` | macOS custom notification window via objc2 |
| `focus.rs` | Terminal focus via AppleScript (iTerm2, Terminal.app, Ghostty) |
| `theme.rs` | Agent type colors, status colors, context gauge, inactivity fade, animations |
| `display.rs` | Shared display helpers (relative time, elapsed, session count formatting) |
| `paths.rs` | Data directory path resolution |

**Data flow**: Claude Code / Codex hooks → `cckit session hook` (stdin JSON) → `storage.rs` (sessions.json with file lock) → TUI/menubar/window reads and displays.

**Subagent detection**: Sessions with `prompt_count == 0 && tool_count > 0` and a Claude model are detected as subagents. Agent names are extracted from the transcript's `agentName` field. Non-Claude models (Codex) are excluded from this heuristic.

**Stale session cleanup**: `load_sessions()` in window.rs calls `sync_sessions()` to remove sessions whose TTY is gone or process has exited. For `tty=unknown` sessions (Codex Desktop), PID liveness is checked via `kill -0`.

## Key Conventions

- **CI**: GitHub Actions runs `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on all branches. Release workflow triggers on `v*` tags.
- **Rust edition 2024**, targets macOS primarily (conditional deps for macOS-only features)
- **Version**: embedded via `build.rs` running `git describe --always --dirty`
- **Data directory**: `~/Library/Application Support/cckit/` (macOS) or `~/.local/share/cckit/` (Linux)
- **Config**: reads `~/.claude.json` for project list, `~/.claude/settings.json` for Claude Code hooks, `~/.codex/hooks.json` for Codex hooks
- Config for `disable_paths`: `./config.toml`, `~/.config/cckit/config.toml`, or the platform config dir (read by `load_cckit_config`); patterns support a leading `~/`
- Uses `serde_json` with `preserve_order` feature for JSON field ordering
