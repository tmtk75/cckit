// Session history loader for ~/.claude/projects/**/*.jsonl.

use crate::history::{Role, SessionRecord, Turn};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// What Claude Code writes as `gitBranch` when git has no current branch.
pub(crate) const DETACHED_HEAD: &str = "HEAD";

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
    #[allow(dead_code)]
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

        if cwd.is_none()
            && let Some(c) = raw.cwd.as_ref()
        {
            cwd = Some(PathBuf::from(c));
        }
        if session_id.is_none() {
            session_id = raw.session_id.clone();
        }
        // A jj-colocated repo leaves git detached, so its transcript records "HEAD" as the
        // branch. Keep the first real branch name the session ever saw and let "HEAD" stand
        // only when no line offers anything better.
        if let Some(branch) = raw.git_branch.as_ref() {
            let keep = match git_branch.as_deref() {
                None => true,
                Some(DETACHED_HEAD) => branch != DETACHED_HEAD,
                Some(_) => false,
            };
            if keep {
                git_branch = Some(branch.clone());
            }
        }

        if let Some(turn) = turn_of(&raw) {
            turns.push(turn);
        }
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
        bookmark: None,
    })
}

/// The conversation turn a transcript line records, if it records one: a `user` or
/// `assistant` line with a timestamp and some text once Claude Code's own noise is
/// stripped. Shared by the full parse and the tail read so "last turn" means the same
/// thing in `session recent` and `bookmark ls`.
fn turn_of(raw: &RawLine) -> Option<Turn> {
    let role = match raw.r#type.as_deref()? {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let ts = DateTime::parse_from_rfc3339(raw.timestamp.as_deref()?).ok()?;
    let content = raw.message.as_ref()?.content.as_ref()?;
    let mut text = extract_text(role, content);
    if role == Role::User {
        text = strip_claude_code_noise(&text);
    }
    if text.trim().is_empty() {
        return None;
    }
    Some(Turn {
        role,
        timestamp: ts.with_timezone(&Utc),
        text,
    })
}

/// When the last conversation turn in a transcript happened, found by reading the file
/// backwards from its end rather than parsing it front to back.
///
/// The file's mtime will not do: Claude Code rewrites trailer lines (`pr-link`,
/// `worktree-state`, `atis-latch`) long after the last turn, so a session nobody has
/// talked to for a day can carry this morning's mtime. The turn timestamps are the
/// only honest signal, and the last one sits a few lines from the end, so this reads
/// 64 KiB chunks from the tail and stops at the first turn it completes. A line longer
/// than a chunk (a tool result pasted into a user turn) is carried across reads until
/// its start is found.
pub fn last_turn_at(path: &Path) -> Option<DateTime<Utc>> {
    use std::io::{Read, Seek, SeekFrom};
    const CHUNK: u64 = 64 * 1024;

    let mut file = File::open(path).ok()?;
    let mut end = file.metadata().ok()?.len();
    // The unfinished first line of the previous read, waiting for its beginning.
    let mut carry: Vec<u8> = Vec::new();
    while end > 0 {
        let start = end.saturating_sub(CHUNK);
        let mut buf = vec![0u8; (end - start) as usize];
        file.seek(SeekFrom::Start(start)).ok()?;
        file.read_exact(&mut buf).ok()?;
        buf.extend_from_slice(&carry);
        // Everything after the first newline is whole lines; before it is the tail of a
        // line whose start is still unread, unless this read reached the file's start.
        let complete_from = if start == 0 {
            0
        } else {
            match buf.iter().position(|&b| b == b'\n') {
                Some(i) => i + 1,
                None => {
                    carry = buf;
                    end = start;
                    continue;
                }
            }
        };
        for line in buf[complete_from..].rsplit(|&b| b == b'\n') {
            let Ok(line) = std::str::from_utf8(line) else {
                continue;
            };
            if line.trim().is_empty() {
                continue;
            }
            if let Some(turn) = serde_json::from_str::<RawLine>(line)
                .ok()
                .and_then(|raw| turn_of(&raw))
            {
                return Some(turn.timestamp);
            }
        }
        carry = buf[..complete_from.saturating_sub(1)].to_vec();
        end = start;
    }
    None
}

/// Remove Claude Code system-inserted blocks that appear in user-role messages when
/// the user runs slash commands or when the session is initialized. These are not
/// real user typing and should not show up in search previews.
fn strip_claude_code_noise(s: &str) -> String {
    const TAGS: &[(&str, &str)] = &[
        ("<local-command-caveat>", "</local-command-caveat>"),
        ("<local-command-stdout>", "</local-command-stdout>"),
        ("<command-name>", "</command-name>"),
        ("<command-message>", "</command-message>"),
        ("<command-args>", "</command-args>"),
        ("<system-reminder>", "</system-reminder>"),
    ];
    let mut out = s.to_string();
    for (open, close) in TAGS {
        while let Some(start) = out.find(open) {
            match out[start..].find(close) {
                Some(end) => {
                    let end_pos = start + end + close.len();
                    out.replace_range(start..end_pos, "");
                }
                None => {
                    out.truncate(start);
                    break;
                }
            }
        }
    }
    out.trim().to_string()
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

use rayon::prelude::*;
use walkdir::WalkDir;

/// Collect every transcript path under `root`. Files under any `subagents/` or
/// `tool-results/` directory are ignored (they are supplementary sidecar logs for their
/// parent session). The walk is sequential because it is cheap next to parsing.
///
/// Exposed separately from parsing so a caller can learn the total up front and report
/// progress against it.
pub fn collect_session_paths(root: &Path) -> Vec<std::path::PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .flatten()
        .map(|entry| entry.into_path())
        .filter(|path| {
            path.is_file()
                && path.extension().and_then(|s| s.to_str()) == Some("jsonl")
                && !path.components().any(|c| {
                    matches!(
                        c.as_os_str().to_str(),
                        Some("subagents") | Some("tool-results")
                    )
                })
        })
        .collect()
}

/// Parse every path into a SessionRecord across rayon's pool, calling `on_file` once per
/// file. `par_iter().filter_map().collect()` preserves the input order, so the result
/// matches a sequential parse.
pub fn parse_paths(
    paths: &[std::path::PathBuf],
    on_file: impl Fn() + Sync + Send,
) -> Vec<SessionRecord> {
    paths
        .par_iter()
        .filter_map(|path| {
            let rec = parse_session_file(path);
            on_file();
            rec
        })
        .collect()
}

/// Walk `root` recursively and parse every reachable `*.jsonl` file into a SessionRecord.
pub fn scan_sessions_in(root: &Path) -> Vec<SessionRecord> {
    parse_paths(&collect_session_paths(root), || {})
}

/// Scan the default Claude Code history directory `~/.claude/projects`.
/// Returns an empty vec (and no error) when the directory does not exist.
pub fn scan_all_sessions() -> Vec<SessionRecord> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    scan_sessions_in(&home.join(".claude/projects"))
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
    fn last_turn_at_agrees_with_the_full_parse() {
        let path = fixture("user_string.jsonl");
        let rec = parse_session_file(&path).unwrap();
        assert_eq!(last_turn_at(&path), Some(rec.ended_at));
    }

    #[test]
    fn last_turn_at_ignores_trailer_lines_written_after_the_last_turn() {
        // What a real transcript ends with: the last turn, then bookkeeping lines Claude
        // Code rewrites later. Their timestamps (or mtime) must not pass for activity.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(
            &path,
            concat!(
                r#"{"type":"user","cwd":"/p","sessionId":"s","timestamp":"2026-03-12T06:12:48.000Z","message":{"role":"user","content":"hi"}}"#,
                "\n",
                r#"{"type":"assistant","cwd":"/p","sessionId":"s","timestamp":"2026-03-12T06:13:00.000Z","message":{"role":"assistant","content":[{"type":"text","text":"hello"}]}}"#,
                "\n",
                r#"{"type":"last-prompt","sessionId":"s"}"#,
                "\n",
                r#"{"type":"pr-link","sessionId":"s","prNumber":1,"timestamp":"2026-03-13T09:00:00.000Z"}"#,
                "\n",
            ),
        )
        .unwrap();
        let at = last_turn_at(&path).unwrap();
        assert_eq!(at.to_rfc3339(), "2026-03-12T06:13:00+00:00");
    }

    #[test]
    fn last_turn_at_handles_a_line_longer_than_one_read() {
        // A tool result pasted into the last user turn can run past 64 KiB, so the line
        // starts in an earlier chunk than it ends in.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        let big = "x".repeat(200 * 1024);
        std::fs::write(
            &path,
            format!(
                concat!(
                    r#"{{"type":"user","cwd":"/p","sessionId":"s","timestamp":"2026-03-12T06:12:48.000Z","message":{{"role":"user","content":"first"}}}}"#,
                    "\n",
                    r#"{{"type":"user","cwd":"/p","sessionId":"s","timestamp":"2026-03-12T07:00:00.000Z","message":{{"role":"user","content":"{}"}}}}"#,
                    "\n",
                    r#"{{"type":"pr-link","sessionId":"s"}}"#,
                    "\n",
                ),
                big
            ),
        )
        .unwrap();
        let at = last_turn_at(&path).unwrap();
        assert_eq!(at.to_rfc3339(), "2026-03-12T07:00:00+00:00");
    }

    #[test]
    fn last_turn_at_is_none_for_a_transcript_with_no_turns() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, "{\"type\":\"summary\"}\n").unwrap();
        assert_eq!(last_turn_at(&path), None);
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
    fn a_real_branch_later_in_the_transcript_wins_over_a_leading_head() {
        let rec = parse_session_file(&fixture("branch_head_then_real.jsonl")).unwrap();
        assert_eq!(rec.git_branch.as_deref(), Some("worktree-fizzy-avalanche"));
    }

    #[test]
    fn head_is_kept_when_the_transcript_never_records_a_real_branch() {
        let rec = parse_session_file(&fixture("branch_head_only.jsonl")).unwrap();
        assert_eq!(rec.git_branch.as_deref(), Some("HEAD"));
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

    #[test]
    fn strip_claude_code_noise_drops_known_tags() {
        let input = "<local-command-caveat>ignore</local-command-caveat>\n<command-name>/clear</command-name>real typing here";
        assert_eq!(strip_claude_code_noise(input), "real typing here");
    }

    #[test]
    fn strip_claude_code_noise_keeps_plain_text() {
        assert_eq!(
            strip_claude_code_noise("please test osrm jni"),
            "please test osrm jni"
        );
    }

    #[test]
    fn scan_sessions_in_finds_all_fixture_files() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/history");
        let mut sessions = scan_sessions_in(&dir);
        sessions.sort_by(|a, b| a.session_id.cmp(&b.session_id));
        let ids: Vec<&str> = sessions.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["s1", "s2", "s3", "s4", "s5", "s6", "s7"]);
    }

    #[test]
    fn scan_sessions_in_returns_empty_when_dir_missing() {
        let sessions = scan_sessions_in(&PathBuf::from("/nonexistent/path/for/test"));
        assert!(sessions.is_empty());
    }
}
