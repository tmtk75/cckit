//! Hover popover state machine, hit testing, and transcript caching for the
//! `cckit app` window. The pure logic lives here so it can be unit-tested
//! without spinning up a Cocoa runtime; the GUI integration into VIEW_CLASS
//! lives in `window.rs`.

use crate::history::{Role, SessionRecord};

/// Returns up to `max_lines` lines from the most recent assistant turn in
/// `record`. If the response has more than `max_lines` lines, the result is
/// truncated and a " …" suffix is appended to the last kept line. Returns
/// `None` if there is no assistant turn.
pub fn extract_last_assistant_truncated(
    record: &SessionRecord,
    max_lines: usize,
) -> Option<String> {
    let text = record
        .turns
        .iter()
        .rev()
        .find(|t| t.role == Role::Assistant)
        .map(|t| t.text.as_str())?;

    let mut lines: Vec<&str> = text.lines().take(max_lines + 1).collect();
    if lines.is_empty() {
        return None;
    }
    let truncated = lines.len() > max_lines;
    if truncated {
        lines.truncate(max_lines);
    }
    let mut out = lines.join("\n");
    if truncated {
        out.push_str(" …");
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Turn;
    use chrono::TimeZone;

    fn make_record(turns: Vec<(Role, &str)>) -> SessionRecord {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        SessionRecord {
            session_id: "id".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: "/tmp/x.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            turns: turns
                .into_iter()
                .map(|(r, s)| Turn {
                    role: r,
                    timestamp: ts,
                    text: s.into(),
                })
                .collect(),
        }
    }

    #[test]
    fn returns_none_when_no_assistant_turn() {
        let r = make_record(vec![(Role::User, "hi")]);
        assert!(extract_last_assistant_truncated(&r, 10).is_none());
    }

    #[test]
    fn returns_full_text_when_under_max_lines() {
        let r = make_record(vec![
            (Role::User, "q"),
            (Role::Assistant, "line1\nline2\nline3"),
        ]);
        assert_eq!(
            extract_last_assistant_truncated(&r, 10),
            Some("line1\nline2\nline3".to_string())
        );
    }

    #[test]
    fn truncates_with_ellipsis_when_over_max_lines() {
        let body = (1..=15)
            .map(|i| format!("line{}", i))
            .collect::<Vec<_>>()
            .join("\n");
        let r = make_record(vec![(Role::User, "q"), (Role::Assistant, &body)]);
        let got = extract_last_assistant_truncated(&r, 10).unwrap();
        let lines: Vec<&str> = got.lines().collect();
        assert_eq!(lines.len(), 10);
        assert_eq!(lines[0], "line1");
        assert_eq!(lines[9], "line10 …");
    }

    #[test]
    fn picks_last_assistant_turn_when_multiple_exist() {
        let r = make_record(vec![
            (Role::User, "q1"),
            (Role::Assistant, "a1"),
            (Role::User, "q2"),
            (Role::Assistant, "a2"),
        ]);
        assert_eq!(
            extract_last_assistant_truncated(&r, 10),
            Some("a2".to_string())
        );
    }
}
