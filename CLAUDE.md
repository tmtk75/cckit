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
| `mod.rs` | `SearchOpts`, `SessionRecord`, `Turn`, `Hit`, `MatchMode` data models; `run_search` and the progress-reporting scan entry points |
| `loader.rs` | Scan and parse session JSONL transcript files (`collect_session_paths` + `parse_paths`, parsed in parallel via rayon) |
| `search.rs` | AND-term and fuzzy search over session text |
| `format.rs` | Plain text and JSON output formatting |
| `progress.rs` | Determinate scan progress on stderr, drawn only on a TTY and only after a 250ms delay |
| `recent.rs` | `session recent`: age filter, project/branch grouping, one-line-per-session text and flat JSON output |
| `tui.rs` | Interactive ratatui browser for search results |
| `skill_usage.rs` | Aggregate Skill tool invocations from transcripts (last-fired/count) for `skill stale` |
| `monitor_usage.rs` | `monitor ls`: mine Monitor tool launches from transcripts, group by project and description, cross-check the process table for `--alive`, text and flat JSON output |

`session search` runs the AND-term matcher first and, when it finds nothing, retries the same
corpus with the fuzzy matcher so typos still land (`--exact` disables the retry). The notice is
inline in text output and on stderr under `--json`, which keeps stdout a bare JSON array.

`session recent` answers "where was I working lately" without a search term: it keeps sessions whose
last activity falls inside `--days N` (default 7), nests them under working directory then git branch,
and prints one line per session (time range, turn count, full session id, first and last user phrase).
The phrase column comes last so the line can be cut to the terminal width without losing the id.
`--limit` caps the session count (0 = no cap) and `--json` emits a flat array for `jq`.

`monitor ls` answers "which projects run background monitors, how often, and when last": it reads the
`Monitor` tool_use blocks out of the same transcripts, groups them by working directory then by the
description they were launched with, and prints one line per monitor (last launch, relative age,
launch count, description). `--days N` sizes the window (default 90, `0` = full history), `--filter`
keeps only monitors whose description or command matches, and `--json` emits a flat array carrying
the last command and session id. Projects whose directory is gone (a removed git worktree, usually)
are dropped, with the count reported in the summary line; `--include-missing` keeps them, marked
`(gone)`, and `cwd_exists` carries the same fact in `--json`. This is launch history, not liveness: a `persistent: false` launch
expires after its timeout and a monitor stopped with `TaskStop` leaves the same trace as one still
streaming, so only the process table can confirm a monitor is still running.

`--alive` adds that process-table check. Claude Code starts every background task as
`zsh -c source <shell snapshot> … && eval '<command>' < /dev/null && …`, so `task_command` recovers
the launched command from the wrapper `ps` reports and matches it against the commands each monitor
was launched with (the whole set, since a relaunch carries a new session id inside the command). A
matched row is marked `●` inside the existing indent, so checked and unchecked runs stay aligned;
`alive` is `None`/`null` when nobody looked and `Some(0)`/`0` when the check found nothing. MCP
servers are children of the same `claude` process but skip the wrapper, so they never match.
Processes whose command no listed monitor claims are printed under `running but not listed above:`
(on stderr under `--json`) when the transcripts recorded them as a monitor launch, which keeps a
running monitor from disappearing just because `--days`, `--filter` or a gone directory excluded
its launch. The gone-directory drop happens before the liveness check for exactly that reason.

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
- Config: `./config.toml`, `~/.config/cckit/config.toml`, or the platform config dir (read by `load_cckit_config`; first found wins, no merging). Keys: `disable_paths` (project paths to skip; glob or prefix, leading `~/` expands) and `skill_stale_exclude` (skills hidden from `skill stale`; every entry is a glob matched against both the skill name and the skill directory path, so `github.com/org/**` excludes a whole org, and a wildcard-free glob is an exact name match)
- Uses `serde_json` with `preserve_order` feature for JSON field ordering
