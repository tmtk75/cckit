// Session history loader for ~/.claude/projects/**/*.jsonl.

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

        let mut text = extract_text(role, content);
        if role == Role::User {
            text = strip_claude_code_noise(&text);
        }
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
        if path.components().any(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some("subagents") | Some("tool-results")
            )
        }) {
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
        assert_eq!(ids, vec!["s1", "s2", "s3", "s4", "s5"]);
    }

    #[test]
    fn scan_sessions_in_returns_empty_when_dir_missing() {
        let sessions = scan_sessions_in(&PathBuf::from("/nonexistent/path/for/test"));
        assert!(sessions.is_empty());
    }
}
