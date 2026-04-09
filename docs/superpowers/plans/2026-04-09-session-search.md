# Session Search Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `cckit session search <TERMS...>` that scans `~/.claude/projects/**/*.jsonl`, finds sessions whose user messages match all query terms (AND, case-insensitive), and shows results as a one-shot listing or an interactive ratatui TUI with first/last turn previews.

**Architecture:** New `src/history/` module, isolated from the live-session `src/monitor/` module. A loader streams jsonl files into `SessionRecord`s, a matcher filters by AND-terms, and two output backends (text formatter + ratatui TUI) render the results. CLI wires `SessionCommands::Search` into `history::run_search`.

**Tech Stack:** Rust 2024, serde / serde_json, walkdir (new), chrono, ratatui + crossterm, clap, dirs.

**Reference spec:** `docs/superpowers/specs/2026-04-09-session-search-design.md`

---

## File Structure

```
Cargo.toml                               # add walkdir dep
src/lib.rs                               # add `pub mod history;`
src/cli.rs                               # add SessionCommands::Search variant + dispatch
src/history/
├── mod.rs                               # types (Role, Turn, SessionRecord, Hit, SearchOpts) + run_search() entry
├── loader.rs                            # parse_session_file, scan_all_sessions
├── search.rs                            # search_sessions(query, sessions) -> Vec<Hit>
├── format.rs                            # format_oneshot, format_json
└── tui.rs                               # run_tui(hits) -> io::Result<Option<PathBuf>>
tests/fixtures/history/
├── user_string.jsonl
├── user_blocks.jsonl
├── user_tool_result_only.jsonl
├── assistant_text_and_tool_use.jsonl
└── broken.jsonl
```

---

## Task 1: Module skeleton and dependency

**Files:**
- Modify: `Cargo.toml` (dependencies section)
- Modify: `src/lib.rs`
- Create: `src/history/mod.rs`
- Create: `src/history/loader.rs`
- Create: `src/history/search.rs`
- Create: `src/history/format.rs`
- Create: `src/history/tui.rs`

- [ ] **Step 1: Add walkdir dependency**

Edit `Cargo.toml`, after the `ctrlc = "3.4"` line in `[dependencies]`:

```toml
# Directory walker for history search
walkdir = "2"
```

- [ ] **Step 2: Register the module in lib.rs**

Edit `src/lib.rs` to add the module after the existing declarations:

```rust
pub mod cli;
pub mod history;
pub mod marketplace;
pub mod monitor;
```

- [ ] **Step 3: Create empty submodule stubs**

Create `src/history/loader.rs`:

```rust
// Session history loader for ~/.claude/projects/**/*.jsonl.
```

Create `src/history/search.rs`:

```rust
// AND matcher over SessionRecord user turns.
```

Create `src/history/format.rs`:

```rust
// Text and JSON output formatters for search hits.
```

Create `src/history/tui.rs`:

```rust
// Ratatui-based interactive search result browser.
```

- [ ] **Step 4: Create src/history/mod.rs with a public run_search stub**

```rust
pub mod format;
pub mod loader;
pub mod search;
pub mod tui;

use std::io;

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub terms: Vec<String>,
    pub interactive: bool,
    pub limit: usize,
    pub json: bool,
}

pub fn run_search(_opts: SearchOpts) -> io::Result<i32> {
    unimplemented!("wired up in Task 8")
}
```

- [ ] **Step 5: Build to verify the skeleton compiles**

Run: `cargo build`
Expected: compiles cleanly (may emit dead_code warnings for unused items — acceptable for this step).

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/lib.rs src/history/
git commit -m "feat(history): scaffold session history module"
```

---

## Task 2: Data model types

**Files:**
- Modify: `src/history/mod.rs`

- [ ] **Step 1: Write failing test for basic type construction**

Append to `src/history/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn session_record_constructs_and_reports_turn_count() {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 6, 12, 48).unwrap();
        let turns = vec![
            Turn {
                role: Role::User,
                timestamp: ts,
                text: "hello".into(),
            },
            Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "world".into(),
            },
        ];
        let rec = SessionRecord {
            session_id: "abc".into(),
            cwd: "/tmp/foo".into(),
            git_branch: Some("main".into()),
            file_path: "/tmp/foo.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            turns,
        };
        assert_eq!(rec.turns.len(), 2);
        assert_eq!(rec.first_user_text(), Some("hello"));
        assert_eq!(rec.last_user_text(), Some("hello"));
    }
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p cckit history::tests`
Expected: compile error — `Role`, `Turn`, `SessionRecord`, `first_user_text`, `last_user_text` not defined.

- [ ] **Step 3: Add the type definitions above the tests module**

Insert after the `SearchOpts` struct in `src/history/mod.rs`:

```rust
use chrono::{DateTime, Utc};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub role: Role,
    pub timestamp: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
    pub file_path: PathBuf,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub turns: Vec<Turn>,
}

impl SessionRecord {
    pub fn first_user_text(&self) -> Option<&str> {
        self.turns
            .iter()
            .find(|t| t.role == Role::User)
            .map(|t| t.text.as_str())
    }

    pub fn last_user_text(&self) -> Option<&str> {
        self.turns
            .iter()
            .rev()
            .find(|t| t.role == Role::User)
            .map(|t| t.text.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub session: SessionRecord,
    pub matched_turn_indices: Vec<usize>,
}
```

- [ ] **Step 4: Run the test and verify it passes**

Run: `cargo test -p cckit history::tests`
Expected: PASS `session_record_constructs_and_reports_turn_count`.

- [ ] **Step 5: Commit**

```bash
git add src/history/mod.rs
git commit -m "feat(history): add core data model types"
```

---

## Task 3: Loader — parse a single jsonl file

**Files:**
- Create: `tests/fixtures/history/user_string.jsonl`
- Create: `tests/fixtures/history/user_blocks.jsonl`
- Create: `tests/fixtures/history/user_tool_result_only.jsonl`
- Create: `tests/fixtures/history/assistant_text_and_tool_use.jsonl`
- Create: `tests/fixtures/history/broken.jsonl`
- Modify: `src/history/loader.rs`

- [ ] **Step 1: Create fixture — user content as plain string**

Create `tests/fixtures/history/user_string.jsonl` (one JSON object per line, single trailing newline):

```jsonl
{"type":"user","cwd":"/tmp/proj","sessionId":"s1","gitBranch":"main","timestamp":"2026-03-12T06:12:48.000Z","message":{"role":"user","content":"osrm jni please test"}}
{"type":"assistant","cwd":"/tmp/proj","sessionId":"s1","gitBranch":"main","timestamp":"2026-03-12T06:12:50.000Z","message":{"role":"assistant","content":[{"type":"text","text":"sure"}]}}
{"type":"user","cwd":"/tmp/proj","sessionId":"s1","gitBranch":"main","timestamp":"2026-03-12T06:13:00.000Z","message":{"role":"user","content":"thanks"}}
```

- [ ] **Step 2: Create fixture — user content as text blocks**

Create `tests/fixtures/history/user_blocks.jsonl`:

```jsonl
{"type":"user","cwd":"/tmp/proj","sessionId":"s2","timestamp":"2026-03-12T06:12:48.000Z","message":{"role":"user","content":[{"type":"text","text":"first part "},{"type":"text","text":"second part"}]}}
```

- [ ] **Step 3: Create fixture — user content that is only a tool_result**

Create `tests/fixtures/history/user_tool_result_only.jsonl`:

```jsonl
{"type":"user","cwd":"/tmp/proj","sessionId":"s3","timestamp":"2026-03-12T06:12:48.000Z","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"ignored output"}]}}
{"type":"user","cwd":"/tmp/proj","sessionId":"s3","timestamp":"2026-03-12T06:12:50.000Z","message":{"role":"user","content":"real user text"}}
```

- [ ] **Step 4: Create fixture — assistant mixed content**

Create `tests/fixtures/history/assistant_text_and_tool_use.jsonl`:

```jsonl
{"type":"user","cwd":"/tmp/proj","sessionId":"s4","timestamp":"2026-03-12T06:12:48.000Z","message":{"role":"user","content":"hello"}}
{"type":"assistant","cwd":"/tmp/proj","sessionId":"s4","timestamp":"2026-03-12T06:12:50.000Z","message":{"role":"assistant","content":[{"type":"text","text":"let me run a tool"},{"type":"tool_use","id":"t1","name":"bash","input":{"cmd":"ls"}},{"type":"text","text":"done"}]}}
```

- [ ] **Step 5: Create fixture — file containing a broken line**

Create `tests/fixtures/history/broken.jsonl`:

```jsonl
{"type":"user","cwd":"/tmp/proj","sessionId":"s5","timestamp":"2026-03-12T06:12:48.000Z","message":{"role":"user","content":"good line"}}
not a json line at all {{{
{"type":"user","cwd":"/tmp/proj","sessionId":"s5","timestamp":"2026-03-12T06:12:52.000Z","message":{"role":"user","content":"another good line"}}
```

- [ ] **Step 6: Write failing tests for parse_session_file**

Replace `src/history/loader.rs` with:

```rust
use crate::history::{Role, SessionRecord, Turn};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct RawLine {
    #[serde(default)]
    r#type: Option<String>,
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default, rename = "gitBranch")]
    git_branch: Option<String>,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<RawMessage>,
}

#[derive(Debug, Deserialize)]
struct RawMessage {
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    content: Option<serde_json::Value>,
}

/// Parse one `.jsonl` file into a SessionRecord, skipping broken lines.
/// Returns None if the file contains no text turns.
pub fn parse_session_file(path: &Path) -> Option<SessionRecord> {
    let file = File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut session_id: Option<String> = None;
    let mut cwd: Option<PathBuf> = None;
    let mut git_branch: Option<String> = None;
    let mut turns: Vec<Turn> = Vec::new();

    for line in reader.lines() {
        let Ok(line) = line else { continue };
        if line.trim().is_empty() {
            continue;
        }
        let raw: RawLine = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        if cwd.is_none() {
            if let Some(c) = raw.cwd.as_ref() {
                cwd = Some(PathBuf::from(c));
            }
        }
        if session_id.is_none() {
            session_id = raw.session_id.clone();
        }
        if git_branch.is_none() {
            git_branch = raw.git_branch.clone();
        }

        let Some(ty) = raw.r#type.as_deref() else {
            continue;
        };
        let role = match ty {
            "user" => Role::User,
            "assistant" => Role::Assistant,
            _ => continue,
        };

        let Some(ts_str) = raw.timestamp.as_deref() else {
            continue;
        };
        let Ok(ts) = DateTime::parse_from_rfc3339(ts_str) else {
            continue;
        };
        let ts: DateTime<Utc> = ts.with_timezone(&Utc);

        let Some(msg) = raw.message.as_ref() else {
            continue;
        };
        let Some(content) = msg.content.as_ref() else {
            continue;
        };

        let text = extract_text(role, content);
        if text.trim().is_empty() {
            continue;
        }

        turns.push(Turn {
            role,
            timestamp: ts,
            text,
        });
    }

    let session_id = session_id?;
    let cwd = cwd?;
    if turns.is_empty() {
        return None;
    }
    let started_at = turns.iter().map(|t| t.timestamp).min()?;
    let ended_at = turns.iter().map(|t| t.timestamp).max()?;

    Some(SessionRecord {
        session_id,
        cwd,
        git_branch,
        file_path: path.to_path_buf(),
        started_at,
        ended_at,
        turns,
    })
}

/// Extract display text for a message `content` value according to role-specific rules.
/// - For User: string content is used directly; list content uses `text` blocks only
///   and drops rows whose blocks are all `tool_result`.
/// - For Assistant: only `text` blocks inside the list are kept.
fn extract_text(role: Role, content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::String(s) if role == Role::User => s.clone(),
        serde_json::Value::Array(items) => {
            let mut out = String::new();
            for item in items {
                let Some(obj) = item.as_object() else {
                    continue;
                };
                let Some(t) = obj.get("type").and_then(|v| v.as_str()) else {
                    continue;
                };
                if t != "text" {
                    continue;
                }
                if let Some(text) = obj.get("text").and_then(|v| v.as_str()) {
                    out.push_str(text);
                }
            }
            out
        }
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/history")
            .join(name)
    }

    #[test]
    fn parses_user_string_and_assistant_blocks() {
        let rec = parse_session_file(&fixture("user_string.jsonl")).unwrap();
        assert_eq!(rec.session_id, "s1");
        assert_eq!(rec.cwd, PathBuf::from("/tmp/proj"));
        assert_eq!(rec.turns.len(), 3);
        assert_eq!(rec.turns[0].role, Role::User);
        assert_eq!(rec.turns[0].text, "osrm jni please test");
        assert_eq!(rec.turns[1].role, Role::Assistant);
        assert_eq!(rec.turns[1].text, "sure");
    }

    #[test]
    fn concatenates_user_text_blocks() {
        let rec = parse_session_file(&fixture("user_blocks.jsonl")).unwrap();
        assert_eq!(rec.turns.len(), 1);
        assert_eq!(rec.turns[0].text, "first part second part");
    }

    #[test]
    fn drops_user_rows_that_are_only_tool_result() {
        let rec = parse_session_file(&fixture("user_tool_result_only.jsonl")).unwrap();
        assert_eq!(rec.turns.len(), 1);
        assert_eq!(rec.turns[0].text, "real user text");
    }

    #[test]
    fn assistant_text_blocks_keep_only_text() {
        let rec = parse_session_file(&fixture("assistant_text_and_tool_use.jsonl")).unwrap();
        let assistant = rec
            .turns
            .iter()
            .find(|t| t.role == Role::Assistant)
            .unwrap();
        assert_eq!(assistant.text, "let me run a tooldone");
    }

    #[test]
    fn broken_lines_are_skipped_without_panic() {
        let rec = parse_session_file(&fixture("broken.jsonl")).unwrap();
        assert_eq!(rec.turns.len(), 2);
        assert_eq!(rec.turns[0].text, "good line");
        assert_eq!(rec.turns[1].text, "another good line");
    }
}
```

- [ ] **Step 7: Run the tests and verify they pass**

Run: `cargo test -p cckit history::loader::tests`
Expected: all 5 tests PASS.

- [ ] **Step 8: Commit**

```bash
git add tests/fixtures/history/ src/history/loader.rs
git commit -m "feat(history): parse jsonl session files"
```

---

## Task 4: Loader — directory walk

**Files:**
- Modify: `src/history/loader.rs`

- [ ] **Step 1: Write a failing test for scan_sessions_in**

Append to the `tests` module in `src/history/loader.rs`:

```rust
#[test]
fn scan_sessions_in_finds_all_fixture_files() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/history");
    let mut sessions = scan_sessions_in(&dir);
    sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
    let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
    assert_eq!(ids, vec!["s1", "s2", "s3", "s4", "s5"]);
}

#[test]
fn scan_sessions_in_returns_empty_when_dir_missing() {
    let sessions = scan_sessions_in(&PathBuf::from("/nonexistent/path/for/test"));
    assert!(sessions.is_empty());
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p cckit history::loader::tests::scan_sessions_in`
Expected: compile error — `scan_sessions_in` not defined.

- [ ] **Step 3: Implement scan_sessions_in and scan_all_sessions**

Add to `src/history/loader.rs` above the `#[cfg(test)]` block:

```rust
use walkdir::WalkDir;

/// Walk `root` recursively and parse every reachable `*.jsonl` file into a SessionRecord.
/// Files under any `subagents/` or `tool-results/` directory are ignored (they are
/// supplementary sidecar logs for their parent session).
pub fn scan_sessions_in(root: &Path) -> Vec<SessionRecord> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root).follow_links(false).into_iter().flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }
        if path
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some("subagents") | Some("tool-results")))
        {
            continue;
        }
        if let Some(rec) = parse_session_file(path) {
            out.push(rec);
        }
    }
    out
}

/// Scan the default Claude Code history directory `~/.claude/projects`.
/// Returns an empty vec (and no error) when the directory does not exist.
pub fn scan_all_sessions() -> Vec<SessionRecord> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    scan_sessions_in(&home.join(".claude/projects"))
}
```

- [ ] **Step 4: Run the tests and verify they pass**

Run: `cargo test -p cckit history::loader::tests`
Expected: all 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/history/loader.rs
git commit -m "feat(history): walk projects dir for jsonl files"
```

---

## Task 5: Search — AND matcher

**Files:**
- Modify: `src/history/search.rs`

- [ ] **Step 1: Write failing tests for search_sessions**

Replace `src/history/search.rs` with:

```rust
use crate::history::{Hit, Role, SessionRecord};

/// Return all sessions whose any user-turn text contains every query term (AND),
/// compared case-insensitively. Results are sorted by `ended_at` descending and
/// truncated to `limit` entries.
pub fn search_sessions(
    sessions: Vec<SessionRecord>,
    terms: &[String],
    limit: usize,
) -> Vec<Hit> {
    let lower_terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let mut hits: Vec<Hit> = sessions
        .into_iter()
        .filter_map(|session| match_session(session, &lower_terms))
        .collect();
    hits.sort_by(|a, b| b.session.ended_at.cmp(&a.session.ended_at));
    hits.truncate(limit);
    hits
}

fn match_session(session: SessionRecord, lower_terms: &[String]) -> Option<Hit> {
    if lower_terms.is_empty() {
        return None;
    }
    let mut matched: Vec<usize> = Vec::new();
    for (idx, turn) in session.turns.iter().enumerate() {
        if turn.role != Role::User {
            continue;
        }
        let lower_text = turn.text.to_lowercase();
        if lower_terms.iter().all(|t| lower_text.contains(t)) {
            matched.push(idx);
        }
    }
    if matched.is_empty() {
        None
    } else {
        Some(Hit {
            session,
            matched_turn_indices: matched,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Turn;
    use chrono::TimeZone;

    fn sample(id: &str, ended_ymd: (i32, u32, u32), user_texts: &[&str]) -> SessionRecord {
        let ts = chrono::Utc
            .with_ymd_and_hms(ended_ymd.0, ended_ymd.1, ended_ymd.2, 0, 0, 0)
            .unwrap();
        let turns = user_texts
            .iter()
            .map(|t| Turn {
                role: Role::User,
                timestamp: ts,
                text: (*t).to_string(),
            })
            .collect::<Vec<_>>();
        SessionRecord {
            session_id: id.into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: format!("/tmp/{id}.jsonl").into(),
            started_at: ts,
            ended_at: ts,
            turns,
        }
    }

    #[test]
    fn and_match_requires_all_terms_in_same_turn() {
        let a = sample("a", (2026, 3, 12), &["osrm jni test"]);
        let b = sample("b", (2026, 3, 11), &["osrm only", "jni only"]);
        let hits = search_sessions(
            vec![a, b],
            &["osrm".into(), "jni".into()],
            10,
        );
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn case_insensitive_and_substring() {
        let a = sample("a", (2026, 3, 12), &["OSRM JNI compile"]);
        let hits = search_sessions(vec![a], &["osrm".into(), "jni".into()], 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched_turn_indices, vec![0]);
    }

    #[test]
    fn sorts_by_ended_at_descending_and_applies_limit() {
        let a = sample("old", (2024, 1, 1), &["foo"]);
        let b = sample("new", (2026, 6, 1), &["foo"]);
        let c = sample("mid", (2025, 1, 1), &["foo"]);
        let hits = search_sessions(vec![a, b, c], &["foo".into()], 2);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "mid"]);
    }

    #[test]
    fn no_match_returns_empty() {
        let a = sample("a", (2026, 3, 12), &["hello world"]);
        let hits = search_sessions(vec![a], &["zzz".into()], 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn assistant_turns_are_not_searched() {
        use chrono::TimeZone;
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let rec = SessionRecord {
            session_id: "a".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: "/tmp/a.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            turns: vec![Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "osrm jni".into(),
            }],
        };
        let hits = search_sessions(vec![rec], &["osrm".into(), "jni".into()], 10);
        assert!(hits.is_empty());
    }
}
```

- [ ] **Step 2: Run the tests and verify they pass**

Run: `cargo test -p cckit history::search::tests`
Expected: all 5 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/history/search.rs
git commit -m "feat(history): AND search matcher over user turns"
```

---

## Task 6: Format — one-shot text output

**Files:**
- Modify: `src/history/format.rs`

- [ ] **Step 1: Write a failing test for format_oneshot**

Replace `src/history/format.rs` with:

```rust
use crate::history::Hit;
use std::time::Duration;

const MAX_TEXT_LEN: usize = 120;

/// Format a single hit as four lines (no trailing newline).
pub fn format_hit(hit: &Hit) -> String {
    let session = &hit.session;
    let branch = session
        .git_branch
        .as_ref()
        .map(|b| format!(" ({b})"))
        .unwrap_or_default();
    let started = session.started_at.format("%Y-%m-%d %H:%M");
    let first = session
        .first_user_text()
        .map(truncate_oneline)
        .unwrap_or_else(|| "(no user text)".into());
    let last = session
        .last_user_text()
        .map(truncate_oneline)
        .unwrap_or_else(|| "(no user text)".into());

    format!(
        "{started}  {cwd}{branch}\n  session: {sid}  turns: {n}  matches: {m}\n  first> {first}\n  last>  {last}",
        started = started,
        cwd = session.cwd.display(),
        branch = branch,
        sid = session.session_id,
        n = session.turns.len(),
        m = hit.matched_turn_indices.len(),
        first = first,
        last = last,
    )
}

/// Full one-shot output for a result set, including the trailing statistics line.
pub fn format_oneshot(hits: &[Hit], scanned_files: usize, elapsed: Duration) -> String {
    let mut out = String::new();
    for hit in hits {
        out.push_str(&format_hit(hit));
        out.push_str("\n\n");
    }
    if hits.is_empty() {
        out.push_str("No sessions matched.\n");
    }
    out.push_str(&format!(
        "Found {} sessions (scanned {} jsonl files in {:.1}s)\n",
        hits.len(),
        scanned_files,
        elapsed.as_secs_f64(),
    ));
    out
}

fn truncate_oneline(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    if flat.chars().count() <= MAX_TEXT_LEN {
        flat
    } else {
        let mut truncated: String = flat.chars().take(MAX_TEXT_LEN).collect();
        truncated.push('…');
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::TimeZone;

    fn hit_with(texts: &[(&str, Role)]) -> Hit {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 6, 12, 0).unwrap();
        let turns = texts
            .iter()
            .map(|(t, r)| Turn {
                role: *r,
                timestamp: ts,
                text: (*t).to_string(),
            })
            .collect::<Vec<_>>();
        Hit {
            session: SessionRecord {
                session_id: "abc-1".into(),
                cwd: "/Users/tomo/proj".into(),
                git_branch: Some("main".into()),
                file_path: "/tmp/abc-1.jsonl".into(),
                started_at: ts,
                ended_at: ts,
                turns,
            },
            matched_turn_indices: vec![0],
        }
    }

    #[test]
    fn format_hit_produces_four_lines_with_branch() {
        let hit = hit_with(&[("hello", Role::User), ("world", Role::User)]);
        let text = format_hit(&hit);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 4);
        assert!(lines[0].contains("/Users/tomo/proj"));
        assert!(lines[0].contains("(main)"));
        assert!(lines[1].contains("session: abc-1"));
        assert!(lines[1].contains("turns: 2"));
        assert!(lines[1].contains("matches: 1"));
        assert_eq!(lines[2], "  first> hello");
        assert_eq!(lines[3], "  last>  world");
    }

    #[test]
    fn format_hit_hides_branch_when_absent() {
        let mut hit = hit_with(&[("hi", Role::User)]);
        hit.session.git_branch = None;
        let text = format_hit(&hit);
        assert!(!text.contains("()"));
        assert!(!text.contains("(main)"));
    }

    #[test]
    fn truncates_long_text_and_replaces_newlines() {
        let long: String = "a".repeat(200);
        let text = format!("{long}\nsecond line");
        let out = truncate_oneline(&text);
        assert!(out.ends_with('…'));
        assert!(!out.contains('\n'));
        assert_eq!(out.chars().count(), MAX_TEXT_LEN + 1);
    }

    #[test]
    fn format_oneshot_includes_stats_line_and_empty_hint() {
        let out = format_oneshot(&[], 42, Duration::from_millis(1500));
        assert!(out.contains("No sessions matched."));
        assert!(out.contains("Found 0 sessions (scanned 42 jsonl files in 1.5s)"));
    }
}
```

- [ ] **Step 2: Run the tests and verify they pass**

Run: `cargo test -p cckit history::format::tests`
Expected: all 4 tests PASS.

- [ ] **Step 3: Commit**

```bash
git add src/history/format.rs
git commit -m "feat(history): one-shot text formatter"
```

---

## Task 7: Format — JSON output

**Files:**
- Modify: `src/history/format.rs`

- [ ] **Step 1: Write a failing test for format_json**

Append to the `tests` module of `src/history/format.rs`:

```rust
#[test]
fn format_json_serializes_expected_fields() {
    let hit = hit_with(&[("first user", Role::User), ("asst", Role::Assistant), ("last user", Role::User)]);
    let json = format_json(&[hit]);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let e = &arr[0];
    assert_eq!(e["session_id"], "abc-1");
    assert_eq!(e["cwd"], "/Users/tomo/proj");
    assert_eq!(e["git_branch"], "main");
    assert_eq!(e["turns"], 3);
    assert_eq!(e["matches"], 1);
    assert_eq!(e["first_user_text"], "first user");
    assert_eq!(e["last_user_text"], "last user");
    assert!(e["started_at"].is_string());
    assert!(e["ended_at"].is_string());
}
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `cargo test -p cckit history::format::tests::format_json_serializes_expected_fields`
Expected: compile error — `format_json` not defined.

- [ ] **Step 3: Implement format_json**

Add to `src/history/format.rs` below `format_oneshot`:

```rust
/// Serialize hits as pretty JSON. Text fields are NOT truncated.
pub fn format_json(hits: &[Hit]) -> String {
    let entries: Vec<serde_json::Value> = hits
        .iter()
        .map(|h| {
            let s = &h.session;
            serde_json::json!({
                "session_id": s.session_id,
                "cwd": s.cwd.display().to_string(),
                "git_branch": s.git_branch,
                "started_at": s.started_at.to_rfc3339(),
                "ended_at": s.ended_at.to_rfc3339(),
                "turns": s.turns.len(),
                "matches": h.matched_turn_indices.len(),
                "first_user_text": s.first_user_text(),
                "last_user_text": s.last_user_text(),
                "file_path": s.file_path.display().to_string(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".into())
}
```

- [ ] **Step 4: Run the test and verify it passes**

Run: `cargo test -p cckit history::format::tests`
Expected: all 5 format tests PASS.

- [ ] **Step 5: Commit**

```bash
git add src/history/format.rs
git commit -m "feat(history): JSON output formatter"
```

---

## Task 8: CLI wiring — run_search entry (non-interactive)

**Files:**
- Modify: `src/history/mod.rs`
- Modify: `src/cli.rs`

- [ ] **Step 1: Implement run_search for the non-interactive path**

In `src/history/mod.rs`, replace the `run_search` stub with:

```rust
pub fn run_search(opts: SearchOpts) -> io::Result<i32> {
    if opts.terms.is_empty() {
        eprintln!("error: at least one search term is required");
        return Ok(2);
    }
    if opts.interactive && opts.json {
        eprintln!("error: --json cannot be combined with --interactive");
        return Ok(2);
    }

    let start = std::time::Instant::now();
    let sessions = loader::scan_all_sessions();
    let scanned = sessions.len();
    let hits = search::search_sessions(sessions, &opts.terms, opts.limit);
    let elapsed = start.elapsed();

    if opts.interactive {
        match tui::run_tui(&hits)? {
            Some(cwd) => {
                println!("{}", cwd.display());
                Ok(0)
            }
            None => Ok(1),
        }
    } else if opts.json {
        println!("{}", format::format_json(&hits));
        Ok(0)
    } else {
        print!("{}", format::format_oneshot(&hits, scanned, elapsed));
        Ok(0)
    }
}
```

- [ ] **Step 2: Add a placeholder run_tui so the file compiles**

Replace `src/history/tui.rs` with:

```rust
use crate::history::Hit;
use std::io;
use std::path::PathBuf;

/// Interactive result browser. Returns `Some(cwd)` when the user picked an entry,
/// or `None` when the user cancelled.
///
/// Non-interactive stub for now — implemented in Task 9.
pub fn run_tui(_hits: &[Hit]) -> io::Result<Option<PathBuf>> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "interactive TUI not yet implemented",
    ))
}
```

- [ ] **Step 3: Add the Search variant to SessionCommands in src/cli.rs**

Find the `enum SessionCommands` block (starts around line 210) and add this variant just before the `Hook` variant:

```rust
    /// Search past Claude Code sessions by user-message text (AND terms)
    Search {
        /// Space-separated search terms; all must match (case-insensitive)
        #[arg(required = true, num_args = 1..)]
        terms: Vec<String>,

        /// Launch an interactive ratatui browser instead of plain text output
        #[arg(short, long)]
        interactive: bool,

        /// Maximum number of results to display
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Emit results as JSON (cannot be combined with --interactive)
        #[arg(long)]
        json: bool,
    },
```

- [ ] **Step 4: Dispatch Search to history::run_search**

In the `match command` block for `SessionCommands` in `src/cli.rs` (starts around line 6128), add this arm just before `Some(SessionCommands::Hook) =>`:

```rust
                Some(SessionCommands::Search {
                    terms,
                    interactive,
                    limit,
                    json,
                }) => {
                    let opts = crate::history::SearchOpts {
                        terms,
                        interactive,
                        limit,
                        json,
                    };
                    match crate::history::run_search(opts) {
                        Ok(code) => std::process::exit(code),
                        Err(e) => {
                            eprintln!("{}: {}", "Error running session search".red(), e);
                            std::process::exit(1);
                        }
                    }
                }
```

- [ ] **Step 5: Build and run the non-interactive command end-to-end**

Run: `cargo build`
Expected: clean compile.

Run: `cargo run -- session search osrm jni --limit 3`
Expected: up to 3 matching sessions printed with the 4-line format, ending in a `Found N sessions (scanned M jsonl files in T.Ts)` line. No panics. If you have previously-discussed osrm jni work, one of the entries should reference `~/.ghq/github.com/kiicorp/varanus-osrm`.

- [ ] **Step 6: Sanity-check the JSON mode**

Run: `cargo run -- session search --json --limit 2 osrm jni | jq '.[0].cwd'`
Expected: a double-quoted absolute path printed. No error.

- [ ] **Step 7: Commit**

```bash
git add src/history/mod.rs src/history/tui.rs src/cli.rs
git commit -m "feat(history): wire session search subcommand"
```

---

## Task 9: TUI implementation

**Files:**
- Modify: `src/history/tui.rs`

- [ ] **Step 1: Replace the stub with a full ratatui implementation**

Replace `src/history/tui.rs` with:

```rust
use crate::history::{Hit, Role};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use std::io;
use std::path::PathBuf;
use std::process::Command;

const PREVIEW_EDGE: usize = 3;

/// Interactive result browser. Returns `Some(cwd)` when the user picked an entry,
/// or `None` when the user cancelled.
///
/// The TUI renders to stderr so that stdout can be captured by `$(...)` to
/// receive the selected `cwd` as a plain path.
pub fn run_tui(hits: &[Hit]) -> io::Result<Option<PathBuf>> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(io::stderr());
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, hits);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    hits: &[Hit],
) -> io::Result<Option<PathBuf>> {
    let mut state = ListState::default();
    if !hits.is_empty() {
        state.select(Some(0));
    }
    let mut status_message: Option<String> = None;

    loop {
        terminal.draw(|f| draw(f, hits, &mut state, status_message.as_deref()))?;

        if !event::poll(std::time::Duration::from_millis(250))? {
            continue;
        }

        if let Event::Key(key) = event::read()? {
            match (key.code, key.modifiers) {
                (KeyCode::Char('q') | KeyCode::Esc, _) => return Ok(None),
                (KeyCode::Char('c'), KeyModifiers::CONTROL) => return Ok(None),
                (KeyCode::Down | KeyCode::Char('j'), _) => move_selection(&mut state, hits.len(), 1),
                (KeyCode::Up | KeyCode::Char('k'), _) => move_selection(&mut state, hits.len(), -1),
                (KeyCode::PageDown, _) => move_selection(&mut state, hits.len(), 10),
                (KeyCode::PageUp, _) => move_selection(&mut state, hits.len(), -10),
                (KeyCode::Char('g'), _) => {
                    if !hits.is_empty() {
                        state.select(Some(0));
                    }
                }
                (KeyCode::Char('G'), _) => {
                    if !hits.is_empty() {
                        state.select(Some(hits.len() - 1));
                    }
                }
                (KeyCode::Enter, _) => {
                    if let Some(idx) = state.selected() {
                        if let Some(hit) = hits.get(idx) {
                            return Ok(Some(hit.session.cwd.clone()));
                        }
                    }
                }
                (KeyCode::Char('o'), _) => {
                    if let Some(idx) = state.selected() {
                        if let Some(hit) = hits.get(idx) {
                            match Command::new("open").arg(&hit.session.cwd).status() {
                                Ok(_) => status_message = Some(format!("opened {}", hit.session.cwd.display())),
                                Err(e) => status_message = Some(format!("open failed: {e}")),
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn move_selection(state: &mut ListState, len: usize, delta: i32) {
    if len == 0 {
        return;
    }
    let current = state.selected().unwrap_or(0) as i32;
    let next = (current + delta).clamp(0, len as i32 - 1);
    state.select(Some(next as usize));
}

fn draw(
    f: &mut ratatui::Frame<'_>,
    hits: &[Hit],
    state: &mut ListState,
    status: Option<&str>,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(f.area());

    f.render_widget(Paragraph::new("cckit session search"), rows[0]);

    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
        .split(rows[1]);

    let items: Vec<ListItem> = hits
        .iter()
        .map(|h| {
            let date = h.session.started_at.format("%Y-%m-%d");
            let base = h
                .session
                .cwd
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| h.session.cwd.display().to_string());
            ListItem::new(format!("{date} {base}"))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("results"))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
        .highlight_symbol("> ");
    f.render_stateful_widget(list, main[0], state);

    let preview_text = state
        .selected()
        .and_then(|idx| hits.get(idx))
        .map(build_preview)
        .unwrap_or_else(|| vec![Line::from("(no selection)")]);
    let preview = Paragraph::new(preview_text)
        .block(Block::default().borders(Borders::ALL).title("preview"))
        .wrap(Wrap { trim: false });
    f.render_widget(preview, main[1]);

    let status_line = status.unwrap_or("enter: print cwd  o: open  j/k: move  g/G: jump  q: quit");
    f.render_widget(Paragraph::new(status_line), rows[2]);
}

fn build_preview(hit: &Hit) -> Vec<Line<'static>> {
    let turns = &hit.session.turns;
    let n = turns.len();
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("session {} ({} turns)", hit.session.session_id, n),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(hit.session.cwd.display().to_string()));
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "== first turns ==",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let head_end = PREVIEW_EDGE.min(n);
    for turn in &turns[..head_end] {
        lines.push(format_turn_line(turn));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "== last turns ==",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let tail_start = n.saturating_sub(PREVIEW_EDGE).max(head_end);
    if tail_start < n {
        for turn in &turns[tail_start..] {
            lines.push(format_turn_line(turn));
        }
    }

    lines
}

fn format_turn_line(turn: &crate::history::Turn) -> Line<'static> {
    let tag = match turn.role {
        Role::User => "[user]",
        Role::Assistant => "[asst]",
    };
    let flat: String = turn
        .text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    Line::from(format!("{tag} {flat}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::TimeZone;

    fn make_hit(turn_count: usize) -> Hit {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let turns: Vec<Turn> = (0..turn_count)
            .map(|i| Turn {
                role: if i % 2 == 0 { Role::User } else { Role::Assistant },
                timestamp: ts,
                text: format!("turn {i}"),
            })
            .collect();
        Hit {
            session: SessionRecord {
                session_id: "s".into(),
                cwd: "/tmp/proj".into(),
                git_branch: None,
                file_path: "/tmp/s.jsonl".into(),
                started_at: ts,
                ended_at: ts,
                turns,
            },
            matched_turn_indices: vec![0],
        }
    }

    #[test]
    fn preview_shows_first_and_last_three_turns_without_overlap() {
        let hit = make_hit(10);
        let lines = build_preview(&hit);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.clone().into_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("turn 0"));
        assert!(text.contains("turn 1"));
        assert!(text.contains("turn 2"));
        assert!(text.contains("turn 7"));
        assert!(text.contains("turn 8"));
        assert!(text.contains("turn 9"));
    }

    #[test]
    fn preview_handles_short_session_without_duplicating_turns() {
        let hit = make_hit(4);
        let lines = build_preview(&hit);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.clone().into_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        // first turns = 0,1,2; last turns = 3 (since tail_start = max(1, 3) = 3).
        assert_eq!(joined.matches("turn 0").count(), 1);
        assert_eq!(joined.matches("turn 1").count(), 1);
        assert_eq!(joined.matches("turn 2").count(), 1);
        assert_eq!(joined.matches("turn 3").count(), 1);
    }
}
```

- [ ] **Step 2: Run the TUI unit tests**

Run: `cargo test -p cckit history::tui::tests`
Expected: both tests PASS.

- [ ] **Step 3: Manually test the TUI**

Run: `cargo run -- session search -i osrm jni`
Expected: alternate screen with results on the left, preview on the right. Keys `j`/`k` move the selection, `Enter` prints the selected cwd to stdout after leaving the alternate screen, `q` / `Esc` exits with status 1 and no stdout output.

- [ ] **Step 4: Commit**

```bash
git add src/history/tui.rs
git commit -m "feat(history): ratatui search browser TUI"
```

---

## Task 10: Final verification

**Files:** (none modified — verification only)

- [ ] **Step 1: Run the full test suite**

Run: `cargo test`
Expected: all tests pass, including the new `history::*` tests.

- [ ] **Step 2: Run clippy with the repo's strict warning gate**

Run: `cargo clippy -- -D warnings`
Expected: clean exit (no warnings, no errors).

- [ ] **Step 3: Check formatting**

Run: `cargo fmt --check`
Expected: no diff output.

- [ ] **Step 4: End-to-end smoke test against live data**

Run: `cargo run --release -- session search osrm jni`
Expected: at least one hit, first entry's cwd is a real directory that exists (spot-check with `ls`). Scanned file count is reasonable (hundreds to low thousands). Elapsed time under a few seconds.

- [ ] **Step 5: Shell integration smoke test**

Run: `cd "$(cargo run --release -- session search -i osrm jni)"`
Expected: TUI opens, you can select an entry, press Enter, and the shell cd's into that directory. Verify with `pwd`.

- [ ] **Step 6: Commit any formatting fixups (if any)**

Only if `cargo fmt --check` found something in Step 3:

```bash
cargo fmt
git add -u
git commit -m "style(history): apply cargo fmt"
```

---

## Spec Coverage Checklist

- Goals (search / context display / TUI preview / cd-friendly output) — Tasks 3-9
- Non-Goals — honored by scope of tasks; no Codex / no regex / no filters / no index
- CLI interface (`<TERMS>`, `-i`, `--limit`, `--json`) — Task 8 Step 3
- Data source (`~/.claude/projects`, subagents / tool-results excluded) — Task 4
- JSONL schema & turn extraction rules — Task 3
- Data model (Role / Turn / SessionRecord / Hit) — Task 2
- Search logic (AND, case-insensitive, sort + limit) — Task 5
- One-shot output format — Task 6
- JSON output format — Task 7
- TUI layout, keybindings, preview window — Task 9
- Module structure (`src/history/*`, separate from monitor) — Task 1
- Dependencies (walkdir new) — Task 1 Step 1
- Error handling (missing dir, broken lines, empty query, `-i` + `--json` conflict) — Tasks 3, 4, 8
- Testing (loader fixtures, search, format unit tests; TUI preview unit tests) — Tasks 3, 5, 6, 7, 9
- CI compatibility (fmt / clippy / test) — Task 10
