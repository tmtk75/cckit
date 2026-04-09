// Text and JSON output formatters for search hits.

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

    #[test]
    fn format_json_serializes_expected_fields() {
        let hit = hit_with(&[
            ("first user", Role::User),
            ("asst", Role::Assistant),
            ("last user", Role::User),
        ]);
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
}
