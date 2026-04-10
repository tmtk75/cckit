//! Hover popover state machine, hit testing, and transcript caching for the
//! `cckit app` window. The pure logic lives here so it can be unit-tested
//! without spinning up a Cocoa runtime; the GUI integration into VIEW_CLASS
//! lives in `window.rs`.

use crate::history::{Role, SessionRecord};

/// Layout constants for the Mission Control theme. Defaults match the
/// constants currently in use in `src/monitor/window.rs`. The values are
/// passed in explicitly so the hit-test logic can be unit tested without
/// depending on `window.rs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MissionControlLayout {
    pub header_height: f64,
    pub card_height: f64,
    pub card_spacing: f64,
    pub left_pad: f64,
}

impl MissionControlLayout {
    pub const fn default() -> Self {
        Self {
            header_height: 28.0,
            card_height: 38.0,
            card_spacing: 2.0,
            left_pad: 8.0,
        }
    }
}

/// Result of hit-testing a mouse point against the rendered session list.
/// Coordinates are in the (flipped) view's local coordinate space.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HoverHit {
    pub idx: usize,
    pub row_x: f64,
    pub row_y: f64,
    pub row_w: f64,
    pub row_h: f64,
}

/// Returns the row hit by `(point_x, point_y)` for the Mission Control theme,
/// or `None` if the point is outside any card (header, gap between cards,
/// padding, etc.).
pub fn hit_test_mission_control(
    point_x: f64,
    point_y: f64,
    view_width: f64,
    session_count: usize,
    layout: MissionControlLayout,
) -> Option<HoverHit> {
    if session_count == 0 || point_y < layout.header_height {
        return None;
    }
    let stride = layout.card_height + layout.card_spacing;
    let rel_y = point_y - layout.header_height;
    let idx = (rel_y / stride).floor() as usize;
    if idx >= session_count {
        return None;
    }
    let in_card = rel_y - (idx as f64) * stride;
    if in_card >= layout.card_height {
        return None;
    }
    let row_x = layout.left_pad;
    let row_w = view_width - layout.left_pad * 2.0;
    if point_x < row_x || point_x > row_x + row_w {
        return None;
    }
    Some(HoverHit {
        idx,
        row_x,
        row_y: layout.header_height + (idx as f64) * stride,
        row_w,
        row_h: layout.card_height,
    })
}

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

    // ---- Mission Control hit-test ----

    #[test]
    fn mc_hit_test_header_returns_none() {
        let layout = MissionControlLayout::default();
        assert!(hit_test_mission_control(50.0, 10.0, 680.0, 3, layout).is_none());
    }

    #[test]
    fn mc_hit_test_first_card_center() {
        let layout = MissionControlLayout::default();
        // first card occupies y in [28, 66)
        let hit = hit_test_mission_control(50.0, 40.0, 680.0, 3, layout).unwrap();
        assert_eq!(hit.idx, 0);
        assert_eq!(hit.row_y, 28.0);
        assert_eq!(hit.row_h, 38.0);
    }

    #[test]
    fn mc_hit_test_second_card() {
        let layout = MissionControlLayout::default();
        // second card occupies y in [68, 106) (28 + 38 + 2)
        let hit = hit_test_mission_control(50.0, 80.0, 680.0, 3, layout).unwrap();
        assert_eq!(hit.idx, 1);
        assert_eq!(hit.row_y, 68.0);
    }

    #[test]
    fn mc_hit_test_gap_between_cards_returns_none() {
        let layout = MissionControlLayout::default();
        // gap: y in [66, 68)
        assert!(hit_test_mission_control(50.0, 67.0, 680.0, 3, layout).is_none());
    }

    #[test]
    fn mc_hit_test_below_last_card_returns_none() {
        let layout = MissionControlLayout::default();
        // 3 cards, last card ends ~y=146; y=200 is past it
        assert!(hit_test_mission_control(50.0, 200.0, 680.0, 3, layout).is_none());
    }

    #[test]
    fn mc_hit_test_left_padding_returns_none() {
        let layout = MissionControlLayout::default();
        assert!(hit_test_mission_control(2.0, 40.0, 680.0, 3, layout).is_none());
    }

    #[test]
    fn mc_hit_test_empty_session_list_returns_none() {
        let layout = MissionControlLayout::default();
        assert!(hit_test_mission_control(50.0, 100.0, 680.0, 0, layout).is_none());
    }
}
