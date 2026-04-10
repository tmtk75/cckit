//! Hover popover state machine, hit testing, and transcript caching for the
//! `cckit app` window. The pure logic lives here so it can be unit-tested
//! without spinning up a Cocoa runtime; the GUI integration into VIEW_CLASS
//! lives in `window.rs`.

use crate::history::{Role, SessionRecord};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// Layout constants for the Classic theme. Defaults match the constants
/// currently used inside `rebuild_view_classic` in `src/monitor/window.rs`
/// (`CL_HEADER_HEIGHT + 1.0` separator and `CL_ROW_HEIGHT`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClassicLayout {
    pub header_height: f64,
    pub row_height: f64,
    pub left_pad: f64,
}

impl ClassicLayout {
    pub const fn default() -> Self {
        Self {
            header_height: 21.0, // CL_HEADER_HEIGHT (20.0) + 1.0 separator
            row_height: 22.0,
            left_pad: 4.0,
        }
    }
}

/// Returns the row hit by `(point_x, point_y)` for the Classic theme.
pub fn hit_test_classic(
    point_x: f64,
    point_y: f64,
    view_width: f64,
    session_count: usize,
    layout: ClassicLayout,
) -> Option<HoverHit> {
    if session_count == 0 || point_y < layout.header_height {
        return None;
    }
    let rel_y = point_y - layout.header_height;
    let idx = (rel_y / layout.row_height).floor() as usize;
    if idx >= session_count {
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
        row_y: layout.header_height + (idx as f64) * layout.row_height,
        row_w,
        row_h: layout.row_height,
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

/// State machine that converts a stream of mouse positions (already mapped to
/// `Option<HoverHit>`) into hover lifecycle events. The session_key bound to
/// each entered row guards against the session list being reordered while a
/// hover is in progress.
#[derive(Debug, Default)]
pub struct HoverTracker {
    current: Option<HoverState>,
    next_version: u64,
}

#[derive(Debug, Clone)]
struct HoverState {
    session_idx: usize,
    session_key: String,
    #[allow(dead_code)]
    started_at: Instant,
    version: u64,
    hit: HoverHit,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HoverEvent {
    Entered { idx: usize, version: u64 },
    Unchanged,
    Cleared,
}

impl HoverTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_mouse(
        &mut self,
        hit: Option<(HoverHit, String)>,
        now: Instant,
    ) -> HoverEvent {
        match (hit, self.current.as_ref()) {
            (None, None) => HoverEvent::Unchanged,
            (None, Some(_)) => {
                self.current = None;
                HoverEvent::Cleared
            }
            (Some((h, key)), Some(state))
                if state.session_idx == h.idx && state.session_key == key =>
            {
                HoverEvent::Unchanged
            }
            (Some((h, key)), _) => {
                self.next_version += 1;
                let version = self.next_version;
                self.current = Some(HoverState {
                    session_idx: h.idx,
                    session_key: key,
                    started_at: now,
                    version,
                    hit: h,
                });
                HoverEvent::Entered { idx: h.idx, version }
            }
        }
    }

    pub fn current_version(&self) -> Option<u64> {
        self.current.as_ref().map(|s| s.version)
    }

    pub fn current_idx(&self) -> Option<usize> {
        self.current.as_ref().map(|s| s.session_idx)
    }

    #[allow(dead_code)]
    pub fn current_session_key(&self) -> Option<&str> {
        self.current.as_ref().map(|s| s.session_key.as_str())
    }

    pub fn current_hit(&self) -> Option<HoverHit> {
        self.current.as_ref().map(|s| s.hit)
    }

    pub fn clear(&mut self) {
        self.current = None;
    }
}

/// Memoizes the result of `extract_last_assistant_truncated` keyed by file
/// path. The cache key includes the file size so the entry is naturally
/// invalidated when new turns are appended.
#[derive(Debug, Default)]
pub struct TranscriptCache {
    entries: HashMap<PathBuf, CacheEntry>,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    file_size: u64,
    text: Option<String>,
}

impl TranscriptCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the cached truncated text for `path`, loading and parsing the
    /// transcript on cache miss. Returns `None` if the file is missing,
    /// unreadable, or contains no assistant turn.
    pub fn get_or_load(&mut self, path: &Path, max_lines: usize) -> Option<String> {
        let metadata = std::fs::metadata(path).ok()?;
        let file_size = metadata.len();

        if let Some(entry) = self.entries.get(path)
            && entry.file_size == file_size
        {
            return entry.text.clone();
        }

        let record = crate::history::loader::parse_session_file(path)?;
        let text = extract_last_assistant_truncated(&record, max_lines);
        self.entries.insert(
            path.to_path_buf(),
            CacheEntry {
                file_size,
                text: text.clone(),
            },
        );
        text
    }

    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
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

    // ---- Classic hit-test ----

    #[test]
    fn classic_hit_test_header_returns_none() {
        let layout = ClassicLayout::default();
        assert!(hit_test_classic(50.0, 10.0, 640.0, 3, layout).is_none());
    }

    #[test]
    fn classic_hit_test_first_row() {
        let layout = ClassicLayout::default();
        // header_height = 21, row_height = 22 → first row: y in [21, 43)
        let hit = hit_test_classic(50.0, 30.0, 640.0, 3, layout).unwrap();
        assert_eq!(hit.idx, 0);
        assert_eq!(hit.row_y, 21.0);
        assert_eq!(hit.row_h, 22.0);
    }

    #[test]
    fn classic_hit_test_third_row() {
        let layout = ClassicLayout::default();
        // third row: y in [65, 87)
        let hit = hit_test_classic(50.0, 70.0, 640.0, 3, layout).unwrap();
        assert_eq!(hit.idx, 2);
        assert_eq!(hit.row_y, 65.0);
    }

    #[test]
    fn classic_hit_test_below_last_row_returns_none() {
        let layout = ClassicLayout::default();
        // 3 rows, last ends at y=87; y=200 is past it
        assert!(hit_test_classic(50.0, 200.0, 640.0, 3, layout).is_none());
    }

    #[test]
    fn classic_hit_test_empty_session_list_returns_none() {
        let layout = ClassicLayout::default();
        assert!(hit_test_classic(50.0, 30.0, 640.0, 0, layout).is_none());
    }

    // ---- HoverTracker ----

    fn dummy_hit(idx: usize) -> HoverHit {
        HoverHit {
            idx,
            row_x: 0.0,
            row_y: 28.0 + (idx as f64) * 40.0,
            row_w: 600.0,
            row_h: 38.0,
        }
    }

    #[test]
    fn tracker_none_to_none_is_unchanged() {
        let mut t = HoverTracker::new();
        let now = std::time::Instant::now();
        assert_eq!(t.on_mouse(None, now), HoverEvent::Unchanged);
    }

    #[test]
    fn tracker_none_to_some_emits_entered_with_version() {
        let mut t = HoverTracker::new();
        let now = std::time::Instant::now();
        let ev = t.on_mouse(Some((dummy_hit(1), "k1".into())), now);
        assert_eq!(ev, HoverEvent::Entered { idx: 1, version: 1 });
        assert_eq!(t.current_version(), Some(1));
        assert_eq!(t.current_idx(), Some(1));
    }

    #[test]
    fn tracker_same_row_continued_is_unchanged() {
        let mut t = HoverTracker::new();
        let now = std::time::Instant::now();
        t.on_mouse(Some((dummy_hit(1), "k1".into())), now);
        let ev = t.on_mouse(Some((dummy_hit(1), "k1".into())), now);
        assert_eq!(ev, HoverEvent::Unchanged);
        assert_eq!(t.current_version(), Some(1));
    }

    #[test]
    fn tracker_different_row_emits_entered_with_new_version() {
        let mut t = HoverTracker::new();
        let now = std::time::Instant::now();
        t.on_mouse(Some((dummy_hit(1), "k1".into())), now);
        let ev = t.on_mouse(Some((dummy_hit(2), "k2".into())), now);
        assert_eq!(ev, HoverEvent::Entered { idx: 2, version: 2 });
    }

    #[test]
    fn tracker_some_to_none_emits_cleared() {
        let mut t = HoverTracker::new();
        let now = std::time::Instant::now();
        t.on_mouse(Some((dummy_hit(1), "k1".into())), now);
        let ev = t.on_mouse(None, now);
        assert_eq!(ev, HoverEvent::Cleared);
        assert_eq!(t.current_version(), None);
    }

    #[test]
    fn tracker_session_key_change_at_same_idx_re_enters() {
        // Session list reordered: row at idx=1 is now a different session.
        let mut t = HoverTracker::new();
        let now = std::time::Instant::now();
        t.on_mouse(Some((dummy_hit(1), "k1".into())), now);
        let ev = t.on_mouse(Some((dummy_hit(1), "k2".into())), now);
        assert_eq!(ev, HoverEvent::Entered { idx: 1, version: 2 });
    }

    // ---- TranscriptCache ----

    #[test]
    fn cache_returns_some_for_known_fixture() {
        let mut cache = TranscriptCache::new();
        let path = std::path::PathBuf::from(
            "tests/fixtures/history/assistant_text_and_tool_use.jsonl",
        );
        let text = cache.get_or_load(&path, 10);
        assert!(
            text.is_some(),
            "expected fixture to yield assistant text, got None"
        );
        let text = text.unwrap();
        assert!(text.contains("let me run a tool"));
    }

    #[test]
    fn cache_returns_none_for_missing_file() {
        let mut cache = TranscriptCache::new();
        let path = std::path::PathBuf::from("tests/fixtures/history/__does_not_exist__.jsonl");
        assert!(cache.get_or_load(&path, 10).is_none());
    }

    #[test]
    fn cache_serves_subsequent_calls_without_re_parse() {
        let mut cache = TranscriptCache::new();
        let path = std::path::PathBuf::from(
            "tests/fixtures/history/assistant_text_and_tool_use.jsonl",
        );
        let first = cache.get_or_load(&path, 10);
        let second = cache.get_or_load(&path, 10);
        assert_eq!(first, second);
        assert_eq!(cache.entry_count(), 1);
    }
}
