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
| `bookmark.rs` | `session bookmark`: the `bookmarks.json` store (`Bookmark`, `BookmarkBook`, `BookmarkStore`), id-prefix resolution, `--current` terminal lookup, and the `set`/`ls`/`id`/`tag`/`rm` entry points |
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

`session bookmark` answers "what was that session about": a name and tags keyed by session
id, kept in `<data_dir>/bookmarks.json` rather than on the `Session` records in
`sessions.json`, which `sync_sessions` wipes once the TTY or process is gone.

**Writing is CLI-only, by design.** A first version opened a picker over every transcript
and it was unusable: this corpus is thousands of files, and a list of them all is not
something a person reads. The workflow is `session search` to find the session, then
`bookmark set <ID>` with a leading fragment of the id. Do not reintroduce a picker, and do
not put bookmark editing inside the search TUI either: the point is that writing a
bookmark costs one command, not a session in an interactive app.

`attach_bookmarks` is applied inside `scan_all_sessions_with_progress`, so every reader
(`search`, `recent`, the interactive browser) sees bookmarks without its own call.
`recent` prints the bookmark at the head of the phrase column (`★name #tag`, magenta) so
no column is added; `search` matches name and tags as well as turn text and reports a
bookmark-only hit as `matches: bookmark`. The bookmark is also folded into
`searchable_text`, which is what the interactive fuzzy filter reads. `--current` finds the
session by terminal: `current_tty` walks up to five levels of the process tree because a
command run from a tool call sits in a shell with no controlling terminal, and a `tty` of
`unknown` matches nothing (Codex Desktop sessions record `unknown` too, so matching on it
would bookmark an unrelated session). `bookmark ls` and `bookmark id` read only `bookmarks.json`, which is why `cwd` and
`session_started_at` are copied into each bookmark when it is written. `lookup` casts
three nets in order (name substring, tag, id prefix) and the first that catches anything
decides; name beats tag because a tag is the broader net and would turn a unique answer
ambiguous. Several matches are never narrowed to one: `bookmark id` is built for `$(...)`,
where resuming the wrong session silently is worse than failing. `upsert` returns what it
displaced so `set` can print `renamed … from … to …`, which is what makes a `--current`
fired in the wrong terminal visible. `set` replaces the whole bookmark, so `edit_tags`
(`bookmark tag`) exists for adding or dropping one tag without retyping the name; it
reports tags that were already set or were not there to remove, and refuses an edit that
would leave a nameless bookmark with no tags (`rm` is how you delete one). The listing
ends with the store path so an empty result is not mistaken for cckit reading a different
file. `--here` filters with `is_under`, which compares path segments rather than text so
a worktree beside the repo (`proj-wt` next to `proj`) is not swept in; `--group-by`
buckets with `group_by_path` / `group_by_tag`, both of which sort newest-first themselves
rather than trusting the caller order. A bookmark appears under every tag it carries
(collapsing it would make `#design` fail to find something tagged `#design`), so the
grouped footer counts rows, not bookmarks. Under `--group-by path` the rows drop the cwd
column because the heading already carries it, and `--json` stays flat under every
grouping so consumers do not track a shape that moves with a display flag.

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
| `hook.rs` | Hook event handler for Claude Code, Codex, Antigravity and Grok (reads stdin JSON) |
| `setup.rs` | Install/uninstall hooks in `~/.claude/settings.json`, `~/.codex/hooks.json` or `~/.gemini/config/hooks.json` |
| `tui.rs` | ratatui-based interactive TUI |
| `menubar.rs` | macOS NSStatusBar/NSMenu via objc2 |
| `window.rs` | macOS NSWindow session monitor app via objc2 (`run_app` unifies window + menubar) |
| `window_hover.rs` | Hover popover hit-testing and transcript preview extraction |
| `notification.rs` | macOS custom notification window via objc2 |
| `focus.rs` | Terminal focus via AppleScript (iTerm2, Terminal.app, Ghostty) |
| `theme.rs` | Agent type colors, status colors, context gauge, inactivity fade, animations |
| `display.rs` | Shared display helpers (relative time, elapsed, session count formatting) |
| `paths.rs` | Data directory path resolution |

**Data flow**: Claude Code / Codex / Antigravity / Grok hooks → `cckit session hook` (stdin JSON) → `storage.rs` (sessions.json with file lock) → TUI/menubar/window reads and displays.

**Grok hooks**: nothing is installed for grok. Its Claude Code compatibility mode reads
`~/.claude/settings.json`, so cckit's existing hooks already fire. grok sends every field
under **both** spellings (`sessionId` and `session_id`, `toolName` and `tool_name`,
`transcriptPath` and `transcript_path`), which is why those camelCase keys are separate
`*_camel` fields folded in by `normalize` rather than serde aliases: an alias makes serde
reject the whole payload with "duplicate field". `hook_event_name` carries Claude's
PascalCase value, so dispatch is unchanged. The one key without a twin is `hookEventName`
(grok's own `"pre_tool_use"` spelling); its presence is how `normalize` tells a grok
session from a Claude Code one arriving through the same hooks. The payload has no model
id, so `normalize` records `"grok"` and `handle_hook` then upgrades it from
`summary.json`'s `current_model_id`, which sits beside the transcript the payload points
at (`~/.grok/sessions/<url-encoded cwd>/<session id>/`). Re-read on every event so a
mid-session model switch is picked up. No context gauge: grok's transcript does not carry
the token counts `read_context_usage` looks for.

**Antigravity (`agy`) hooks**: a session only appears once it makes its first model call,
because Antigravity has no `SessionStart` event and every installed hook fires at or after
`PreInvocation`; launching `agy` and leaving it idle shows nothing. Beyond that, the third
tool differs from the other two in four ways, all
handled in `hook.rs`. Its payload is camelCase protojson (`conversationId`, `modelName`,
`transcriptPath`, `workspacePaths`, nested `toolCall`), which `HookInput` absorbs through
serde aliases plus `normalize`. The payload carries no event name, so the hooks pass it as
`cckit session hook --event <NAME>`; that flag's presence also marks the Antigravity path,
which must print `{}` on stdout because a silent hook is read as a refusal. `workspacePaths`
is empty under `agy -p` (print mode) and must not overwrite a known cwd. Finally, **no
`PreToolUse` hook is installed**: that event demands a permission decision
(`allow`/`deny`/`ask`/`force_ask`) and treats no answer as `deny`, so a monitor there would
either rubber-stamp or block every tool call. `PostToolUse` therefore names and counts the
call itself, but only when `tool_started_at` is unset, which keeps Claude Code's two hooks
per call from counting it twice. Prompt counting uses the invocation that follows a `Stop`,
since Antigravity calls the model several times per turn. Hooks live in
`~/.gemini/config/hooks.json` under the name `cckit` (`install_agy_entry` replaces that
entry wholesale and leaves other named hooks alone); a project's `.agents/hooks.json` is
not read by the CLI.

**Subagent detection**: Sessions with `prompt_count == 0 && tool_count > 0` and a Claude model are detected as subagents. Agent names are extracted from the transcript's `agentName` field. Non-Claude models (Codex) are excluded from this heuristic.

**Stale session cleanup**: `load_sessions()` in window.rs calls `sync_sessions()` to remove sessions whose TTY is gone or process has exited. For `tty=unknown` sessions (Codex Desktop), PID liveness is checked via `kill -0`.

## Key Conventions

- **CI**: GitHub Actions runs `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on all branches. Release workflow triggers on `v*` tags.
- **Rust edition 2024**, targets macOS primarily (conditional deps for macOS-only features)
- **Version**: embedded via `build.rs` running `git describe --always --dirty`
- **Data directory**: `~/Library/Application Support/cckit/` (macOS) or `~/.local/share/cckit/` (Linux)
- **Config**: reads `~/.claude.json` for project list, `~/.claude/settings.json` for Claude Code hooks, `~/.codex/hooks.json` for Codex hooks, `~/.gemini/config/hooks.json` for Antigravity hooks
- Config: `./config.toml`, `~/.config/cckit/config.toml`, or the platform config dir (read by `load_cckit_config`; first found wins, no merging). Keys: `disable_paths` (project paths to skip; glob or prefix, leading `~/` expands) and `skill_stale_exclude` (skills hidden from `skill stale`; every entry is a glob matched against both the skill name and the skill directory path, so `github.com/org/**` excludes a whole org, and a wildcard-free glob is an exact name match)
- Uses `serde_json` with `preserve_order` feature for JSON field ordering
