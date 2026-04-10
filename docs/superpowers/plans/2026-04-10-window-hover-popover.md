# Window Hover Popover Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `cckit app` のウィンドウ上で、Mission Control / Classic テーマのセッション行に 500ms 以上 hover したら、その行の最後の assistant 応答をフロート popover で表示する。

**Architecture:** 純ロジック（hit-test / 抽出 / 状態機械 / キャッシュ）を `src/monitor/window_hover.rs` に分離して TDD で実装し、最後に `window.rs` の VIEW_CLASS に NSTrackingArea + マウスイベントセレクタ + NSTimer を追加して接続する。トランスクリプト読み込みは `(path, file_size)` を key とするメモリキャッシュで再利用し、500ms hover 遅延の中で main thread 同期読み込みする。

**Tech Stack:** Rust, objc2 / objc2-app-kit (NSView, NSTrackingArea, NSEvent, NSTimer, NSPanel, NSTextField, NSColor), 既存 `src/history/loader.rs::parse_session_file`

---

## File Structure

| File | 役割 | 変更種別 |
|---|---|---|
| `src/history/mod.rs` | `SessionRecord::last_assistant_text()` を追加 | 修正 |
| `src/monitor/window_hover.rs` | hit-test / `HoverHit` / `HoverTracker` / `TranscriptCache` / `extract_last_assistant_truncated` / `HoverPopover` を含む新規モジュール | 新規 |
| `src/monitor/mod.rs` | `pub mod window_hover;`（macOS 限定） | 修正 |
| `src/monitor/window.rs` | NSTrackingArea + mouseMoved/Entered/Exited/cckitHoverTimerFired セレクタを VIEW_CLASS に登録、`HOVER_RUNTIME` static、setup_window で `updateTrackingAreas` 起動 | 修正 |

---

## Task 1: `SessionRecord::last_assistant_text()` を追加

**Files:**
- Modify: `src/history/mod.rs`

`SessionRecord` に最後の assistant ターンを返すメソッドを追加する。既存の `last_user_text` と対称形。

- [ ] **Step 1: 失敗するテストを書く**

`src/history/mod.rs` の `#[cfg(test)] mod tests` ブロックに以下を追加:

```rust
#[test]
fn last_assistant_text_returns_most_recent_assistant_turn() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
    let turns = vec![
        Turn { role: Role::User, timestamp: ts, text: "q1".into() },
        Turn { role: Role::Assistant, timestamp: ts, text: "a1".into() },
        Turn { role: Role::User, timestamp: ts, text: "q2".into() },
        Turn { role: Role::Assistant, timestamp: ts, text: "a2".into() },
    ];
    let rec = SessionRecord {
        session_id: "abc".into(),
        cwd: "/tmp".into(),
        git_branch: None,
        file_path: "/tmp/x.jsonl".into(),
        started_at: ts,
        ended_at: ts,
        turns,
    };
    assert_eq!(rec.last_assistant_text(), Some("a2"));
}

#[test]
fn last_assistant_text_returns_none_when_no_assistant_turns() {
    let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
    let rec = SessionRecord {
        session_id: "abc".into(),
        cwd: "/tmp".into(),
        git_branch: None,
        file_path: "/tmp/x.jsonl".into(),
        started_at: ts,
        ended_at: ts,
        turns: vec![Turn { role: Role::User, timestamp: ts, text: "q".into() }],
    };
    assert_eq!(rec.last_assistant_text(), None);
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p cckit --lib history::tests::last_assistant_text -- --nocapture`
Expected: 2 件 FAIL（`no method named 'last_assistant_text' found`）

- [ ] **Step 3: 最小実装**

`src/history/mod.rs` の `impl SessionRecord` に追記:

```rust
pub fn last_assistant_text(&self) -> Option<&str> {
    self.turns
        .iter()
        .rev()
        .find(|t| t.role == Role::Assistant)
        .map(|t| t.text.as_str())
}
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p cckit --lib history::tests::last_assistant_text -- --nocapture`
Expected: 2 件 PASS

- [ ] **Step 5: コミット**

```bash
git add src/history/mod.rs
git commit -m "$(cat <<'EOF'
feat(history): add SessionRecord::last_assistant_text helper

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 2: `window_hover` モジュールと `extract_last_assistant_truncated` を追加

**Files:**
- Create: `src/monitor/window_hover.rs`
- Modify: `src/monitor/mod.rs`

純粋関数 `extract_last_assistant_truncated` を新規モジュールに追加。後続のタスクで同じファイルに hit-test や HoverTracker を足していく。

- [ ] **Step 1: モジュール宣言を追加**

`src/monitor/mod.rs` に以下を追加（`pub mod window;` の下に）:

```rust
#[cfg(target_os = "macos")]
pub mod window_hover;
```

- [ ] **Step 2: 失敗するテストを含む新規ファイルを作成**

Create `src/monitor/window_hover.rs`:

```rust
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
                .map(|(r, s)| Turn { role: r, timestamp: ts, text: s.into() })
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
```

- [ ] **Step 3: テストが通ることを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests -- --nocapture`
Expected: 4 件 PASS

- [ ] **Step 4: lint と format**

Run: `cargo clippy -- -D warnings && cargo fmt --check`
Expected: エラーなし

- [ ] **Step 5: コミット**

```bash
git add src/monitor/mod.rs src/monitor/window_hover.rs
git commit -m "$(cat <<'EOF'
feat(window): add window_hover module with last-assistant truncation helper

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 3: Mission Control テーマの hit-test を追加

**Files:**
- Modify: `src/monitor/window_hover.rs`

座標 → 行 idx + row rect の純関数を追加する。レイアウト定数は `window.rs` から複製しないように `MissionControlLayout` 構造体経由で渡す（テスト容易性のため）。

- [ ] **Step 1: 失敗するテストを追加**

`src/monitor/window_hover.rs` の `#[cfg(test)] mod tests` ブロックに追加:

```rust
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
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests::mc_hit_test -- --nocapture`
Expected: 7 件 FAIL（型・関数未定義）

- [ ] **Step 3: 最小実装**

`src/monitor/window_hover.rs` の `extract_last_assistant_truncated` の上に追加:

```rust
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
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests -- --nocapture`
Expected: 11 件 PASS（既存4 + 新規7）

- [ ] **Step 5: lint**

Run: `cargo clippy -- -D warnings`
Expected: エラーなし

- [ ] **Step 6: コミット**

```bash
git add src/monitor/window_hover.rs
git commit -m "$(cat <<'EOF'
feat(window): add mission-control hit test for hover popover

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 4: Classic テーマの hit-test を追加

**Files:**
- Modify: `src/monitor/window_hover.rs`

- [ ] **Step 1: 失敗するテストを追加**

`#[cfg(test)] mod tests` に追加:

```rust
#[test]
fn classic_hit_test_header_returns_none() {
    let layout = ClassicLayout::default();
    assert!(hit_test_classic(50.0, 10.0, 640.0, 3, layout).is_none());
}

#[test]
fn classic_hit_test_first_row() {
    let layout = ClassicLayout::default();
    // header_height = 21, row_height = 22
    // first row: y in [21, 43)
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
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests::classic_hit_test`
Expected: 5 件 FAIL

- [ ] **Step 3: 最小実装**

`src/monitor/window_hover.rs` の `hit_test_mission_control` の下に追加:

```rust
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
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests`
Expected: 16 件 PASS

- [ ] **Step 5: コミット**

```bash
git add src/monitor/window_hover.rs
git commit -m "$(cat <<'EOF'
feat(window): add classic hit test for hover popover

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 5: `HoverTracker` 状態機械を追加

**Files:**
- Modify: `src/monitor/window_hover.rs`

マウス位置のイベントを `Entered / Unchanged / Cleared` に変換する状態機械。version は同じ行に対する複数の NSTimer 発火の重複検出に使う。

- [ ] **Step 1: 失敗するテストを追加**

`#[cfg(test)] mod tests` に追加:

```rust
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
    let hit = HoverHit { idx: 1, row_x: 0.0, row_y: 28.0, row_w: 600.0, row_h: 38.0 };
    let ev = t.on_mouse(Some((hit, "k1".into())), now);
    assert_eq!(ev, HoverEvent::Entered { idx: 1, version: 1 });
    assert_eq!(t.current_version(), Some(1));
    assert_eq!(t.current_idx(), Some(1));
}

#[test]
fn tracker_same_row_continued_is_unchanged() {
    let mut t = HoverTracker::new();
    let now = std::time::Instant::now();
    let hit = HoverHit { idx: 1, row_x: 0.0, row_y: 28.0, row_w: 600.0, row_h: 38.0 };
    t.on_mouse(Some((hit, "k1".into())), now);
    let ev = t.on_mouse(Some((hit, "k1".into())), now);
    assert_eq!(ev, HoverEvent::Unchanged);
    assert_eq!(t.current_version(), Some(1));
}

#[test]
fn tracker_different_row_emits_entered_with_new_version() {
    let mut t = HoverTracker::new();
    let now = std::time::Instant::now();
    let hit_a = HoverHit { idx: 1, row_x: 0.0, row_y: 28.0, row_w: 600.0, row_h: 38.0 };
    let hit_b = HoverHit { idx: 2, row_x: 0.0, row_y: 68.0, row_w: 600.0, row_h: 38.0 };
    t.on_mouse(Some((hit_a, "k1".into())), now);
    let ev = t.on_mouse(Some((hit_b, "k2".into())), now);
    assert_eq!(ev, HoverEvent::Entered { idx: 2, version: 2 });
}

#[test]
fn tracker_some_to_none_emits_cleared() {
    let mut t = HoverTracker::new();
    let now = std::time::Instant::now();
    let hit = HoverHit { idx: 1, row_x: 0.0, row_y: 28.0, row_w: 600.0, row_h: 38.0 };
    t.on_mouse(Some((hit, "k1".into())), now);
    let ev = t.on_mouse(None, now);
    assert_eq!(ev, HoverEvent::Cleared);
    assert_eq!(t.current_version(), None);
}

#[test]
fn tracker_session_key_change_at_same_idx_re_enters() {
    // Session list has been updated and the row at idx=1 is now a different
    // session — treat it as a new hover.
    let mut t = HoverTracker::new();
    let now = std::time::Instant::now();
    let hit = HoverHit { idx: 1, row_x: 0.0, row_y: 28.0, row_w: 600.0, row_h: 38.0 };
    t.on_mouse(Some((hit, "k1".into())), now);
    let ev = t.on_mouse(Some((hit, "k2".into())), now);
    assert_eq!(ev, HoverEvent::Entered { idx: 1, version: 2 });
}
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests::tracker`
Expected: 6 件 FAIL

- [ ] **Step 3: 最小実装**

`src/monitor/window_hover.rs` の `hit_test_classic` の下に追加（`use std::time::Instant;` を必要なら先頭に追加）:

```rust
use std::time::Instant;

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
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests`
Expected: 22 件 PASS

- [ ] **Step 5: コミット**

```bash
git add src/monitor/window_hover.rs
git commit -m "$(cat <<'EOF'
feat(window): add HoverTracker state machine for hover popover

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 6: `TranscriptCache` を追加

**Files:**
- Modify: `src/monitor/window_hover.rs`

`(path, file_size)` をキーにした薄いキャッシュ。File size が変わったら再読み込みする。

- [ ] **Step 1: 失敗するテストを追加**

`#[cfg(test)] mod tests` に追加:

```rust
#[test]
fn cache_returns_some_for_known_fixture() {
    let mut cache = TranscriptCache::new();
    let path = std::path::PathBuf::from(
        "tests/fixtures/history/assistant_text_and_tool_use.jsonl",
    );
    let text = cache.get_or_load(&path, 10);
    assert!(text.is_some(), "expected fixture to yield assistant text, got None");
    let text = text.unwrap();
    // The fixture's assistant text is "let me run a tooldone"
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
```

- [ ] **Step 2: テストが失敗することを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests::cache`
Expected: 3 件 FAIL

- [ ] **Step 3: 最小実装**

`src/monitor/window_hover.rs` の `HoverTracker` の下に追加（先頭の `use` ブロックにも必要なものを足す）:

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
            CacheEntry { file_size, text: text.clone() },
        );
        text
    }

    #[cfg(test)]
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }
}
```

- [ ] **Step 4: テストが通ることを確認**

Run: `cargo test -p cckit --lib monitor::window_hover::tests::cache`
Expected: 3 件 PASS

注意: テストは cwd がワークスペースルートで実行される前提（Cargo の通常動作）。

- [ ] **Step 5: 全 hover テストを再実行**

Run: `cargo test -p cckit --lib monitor::window_hover::tests`
Expected: 25 件 PASS

- [ ] **Step 6: コミット**

```bash
git add src/monitor/window_hover.rs
git commit -m "$(cat <<'EOF'
feat(window): add TranscriptCache for hover popover transcript reads

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 7: `HoverPopover` (NSPanel) を追加

**Files:**
- Modify: `src/monitor/window_hover.rs`

GUI 層のため自動テストはなし。Mission Control / Classic 共通の見た目で multi-line テキストを表示するボーダレス NSPanel。

- [ ] **Step 1: 必要な import を追加**

`src/monitor/window_hover.rs` の先頭の `use` ブロックに追加:

```rust
#[cfg(target_os = "macos")]
use objc2::rc::Retained;
#[cfg(target_os = "macos")]
use objc2::{ClassType, MainThreadOnly, msg_send};
#[cfg(target_os = "macos")]
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
#[cfg(target_os = "macos")]
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
```

- [ ] **Step 2: 構造体と show/hide 実装を追加**

`TranscriptCache` の下に追加:

```rust
#[cfg(target_os = "macos")]
const POPOVER_WIDTH: f64 = 480.0;
#[cfg(target_os = "macos")]
const POPOVER_PADDING: f64 = 10.0;
#[cfg(target_os = "macos")]
const POPOVER_GAP: f64 = 8.0;

/// Borderless floating panel that shows the truncated last-assistant text
/// next to the hovered row. Created lazily on first show.
#[cfg(target_os = "macos")]
#[derive(Default)]
pub struct HoverPopover {
    panel: Option<Retained<NSPanel>>,
    text_field: Option<Retained<NSTextField>>,
}

#[cfg(target_os = "macos")]
impl HoverPopover {
    pub fn new() -> Self {
        Self::default()
    }

    /// Show the popover next to `parent_view`'s row at `anchor` (view-local,
    /// flipped coordinates). The text wraps to `POPOVER_WIDTH` and the height
    /// is computed from the wrapped layout.
    pub fn show(&mut self, parent_view: &NSView, anchor: HoverHit, text: &str) {
        let mtm = match MainThreadMarker::new() {
            Some(m) => m,
            None => return,
        };
        self.ensure_panel(mtm);

        let panel = self
            .panel
            .as_ref()
            .expect("ensure_panel populated panel");
        let text_field = self
            .text_field
            .as_ref()
            .expect("ensure_panel populated text_field");

        // 1. Update text and compute fitted height.
        unsafe {
            let ns_text = NSString::from_str(text);
            let _: () = msg_send![&**text_field, setStringValue: &*ns_text];
        }

        let max_h: f64 = 600.0;
        let inner_w = POPOVER_WIDTH - POPOVER_PADDING * 2.0;
        let fit_size = unsafe {
            let bounds = NSSize::new(inner_w, max_h);
            let cell: *mut AnyObject = msg_send![&**text_field, cell];
            let size: NSSize = msg_send![cell, cellSizeForBounds: NSRect::new(NSPoint::new(0.0, 0.0), bounds)];
            size
        };
        let panel_h = (fit_size.height + POPOVER_PADDING * 2.0).clamp(40.0, max_h);

        // 2. Compute screen-space anchor by going view -> window -> screen.
        let parent_window = match unsafe { parent_view.window() } {
            Some(w) => w,
            None => return,
        };
        // anchor.row_y is in flipped (top-origin) coordinates; NSView::convertRect_toView
        // with toView=nil expects flipped if the view is flipped, which it is.
        let row_rect_view = NSRect::new(
            NSPoint::new(anchor.row_x, anchor.row_y),
            NSSize::new(anchor.row_w, anchor.row_h),
        );
        let row_rect_window =
            unsafe { parent_view.convertRect_toView(row_rect_view, None) };
        let row_rect_screen =
            unsafe { parent_window.convertRectToScreen(row_rect_window) };

        // 3. Decide left/right placement based on screen geometry.
        let screen_frame = unsafe {
            parent_window
                .screen()
                .map(|s| s.visibleFrame())
                .unwrap_or_else(|| NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1920.0, 1080.0)))
        };
        let right_x = row_rect_screen.origin.x + row_rect_screen.size.width + POPOVER_GAP;
        let mut origin_x = if right_x + POPOVER_WIDTH <= screen_frame.origin.x + screen_frame.size.width {
            right_x
        } else {
            row_rect_screen.origin.x - POPOVER_GAP - POPOVER_WIDTH
        };
        if origin_x < screen_frame.origin.x {
            origin_x = screen_frame.origin.x;
        }
        // Top-align with row in screen coords. NSScreen y is bottom-origin, so
        // top of row in screen = origin.y + size.height.
        let row_top_screen = row_rect_screen.origin.y + row_rect_screen.size.height;
        let mut origin_y = row_top_screen - panel_h;
        if origin_y < screen_frame.origin.y {
            origin_y = screen_frame.origin.y;
        }

        let frame = NSRect::new(NSPoint::new(origin_x, origin_y), NSSize::new(POPOVER_WIDTH, panel_h));
        unsafe {
            let _: () = msg_send![&**panel, setFrame: frame, display: true];
            let text_frame = NSRect::new(
                NSPoint::new(POPOVER_PADDING, POPOVER_PADDING),
                NSSize::new(inner_w, panel_h - POPOVER_PADDING * 2.0),
            );
            let _: () = msg_send![&**text_field, setFrame: text_frame];
            let _: () = msg_send![&**panel, orderFrontRegardless];
        }
    }

    pub fn hide(&mut self) {
        if let Some(panel) = &self.panel {
            unsafe {
                let _: () = msg_send![&**panel, orderOut: std::ptr::null_mut::<AnyObject>()];
            }
        }
    }

    fn ensure_panel(&mut self, mtm: MainThreadMarker) {
        if self.panel.is_some() {
            return;
        }
        let style_mask: NSWindowStyleMask =
            NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel;
        let initial_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(POPOVER_WIDTH, 100.0));
        let panel: Retained<NSPanel> = unsafe {
            let alloc = NSPanel::alloc(mtm);
            msg_send![
                alloc,
                initWithContentRect: initial_rect,
                styleMask: style_mask,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        unsafe {
            let _: () = msg_send![&*panel, setLevel: 3_isize]; // NSFloatingWindowLevel
            let _: () = msg_send![&*panel, setOpaque: false];
            let _: () = msg_send![&*panel, setHasShadow: true];
            let _: () = msg_send![&*panel, setHidesOnDeactivate: false];
            let _: () = msg_send![&*panel, setIgnoresMouseEvents: true];
            let bg = NSColor::colorWithRed_green_blue_alpha(0.10, 0.11, 0.13, 0.97);
            let _: () = msg_send![&*panel, setBackgroundColor: &*bg];
        }

        // Build content view + text field.
        let content_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(POPOVER_WIDTH, 100.0));
        let content_view = unsafe {
            let v: Retained<NSView> = msg_send![NSView::alloc(mtm), initWithFrame: content_rect];
            let _: () = msg_send![&*v, setWantsLayer: true];
            let layer: *mut AnyObject = msg_send![&*v, layer];
            let _: () = msg_send![layer, setCornerRadius: 8.0_f64];
            let _: () = msg_send![layer, setMasksToBounds: true];
            v
        };

        let text_field: Retained<NSTextField> = unsafe {
            let alloc = NSTextField::alloc(mtm);
            let tf: Retained<NSTextField> = msg_send![alloc, initWithFrame: content_rect];
            let _: () = msg_send![&*tf, setBezeled: false];
            let _: () = msg_send![&*tf, setEditable: false];
            let _: () = msg_send![&*tf, setSelectable: false];
            let _: () = msg_send![&*tf, setDrawsBackground: false];
            let font = objc2_app_kit::NSFont::monospacedSystemFontOfSize_weight(11.5, 0.0);
            let _: () = msg_send![&*tf, setFont: &*font];
            let fg = NSColor::colorWithRed_green_blue_alpha(0.92, 0.94, 0.96, 1.0);
            let _: () = msg_send![&*tf, setTextColor: &*fg];
            let cell: *mut AnyObject = msg_send![&*tf, cell];
            let _: () = msg_send![cell, setWraps: true];
            let _: () = msg_send![cell, setScrollable: false];
            tf
        };

        unsafe {
            content_view.addSubview(&text_field);
            let _: () = msg_send![&*panel, setContentView: &*content_view];
        }

        self.panel = Some(panel);
        self.text_field = Some(text_field);
    }
}
```

`AnyObject` を `objc2::runtime::AnyObject` から import 必要なら use 文に追加:

```rust
#[cfg(target_os = "macos")]
use objc2::runtime::AnyObject;
```

- [ ] **Step 3: コンパイル確認**

Run: `cargo build`
Expected: 警告のみで成功（hover popover はまだ呼び出されないので未使用警告が出る可能性あり → `#[allow(dead_code)]` を追加して回避してもよい）

`HoverPopover` 構造体宣言の上に必要なら追加:
```rust
#[cfg(target_os = "macos")]
#[allow(dead_code)]
```

- [ ] **Step 4: lint と format**

Run: `cargo clippy -- -D warnings && cargo fmt --check`
Expected: エラーなし

- [ ] **Step 5: コミット**

```bash
git add src/monitor/window_hover.rs
git commit -m "$(cat <<'EOF'
feat(window): add borderless NSPanel HoverPopover for hover popover

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 8: VIEW_CLASS にマウスセレクタと NSTrackingArea を登録

**Files:**
- Modify: `src/monitor/window.rs`

`mouseMoved:`, `mouseEntered:`, `mouseExited:`, `updateTrackingAreas`, `cckitHoverTimerFired:` の 5 つのセレクタを VIEW_CLASS に追加する。実装本体は次のタスクで足すので、まずはセレクタとセル本体だけ。

- [ ] **Step 1: import を追加**

`src/monitor/window.rs` 上部の `objc2_app_kit::{...}` インポートに `NSTrackingArea, NSTrackingAreaOptions` を追加:

```rust
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBezierPath, NSColor, NSEvent, NSFont, NSGraphicsContext, NSImage, NSMenu, NSMenuItem,
    NSScreen, NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowStyleMask,
};
```

window_hover モジュールから必要な型をインポート（同じファイルの先頭、`use crate::monitor::theme::*;` の近くに）:

```rust
use crate::monitor::window_hover::{
    self, ClassicLayout, HoverEvent, HoverHit, HoverPopover, HoverTracker, MissionControlLayout,
    TranscriptCache, hit_test_classic, hit_test_mission_control,
};
```

- [ ] **Step 2: HOVER_RUNTIME static を追加**

`SESSION_LIST` などの static 群の近く（`AF_LABEL_PTR` の下あたり）に追加:

```rust
struct HoverRuntime {
    tracker: HoverTracker,
    cache: TranscriptCache,
    popover: HoverPopover,
    pending_timer_version: Option<u64>,
}

impl HoverRuntime {
    const fn new() -> Self {
        Self {
            tracker: HoverTracker {
                // Default::default() is not const; use the explicit zero state.
                // We'll go through Mutex::lock once and reinit there.
                ..unsafe { std::mem::zeroed() }
            },
            cache: TranscriptCache { entries: std::collections::HashMap::new() },
            popover: HoverPopover { panel: None, text_field: None },
            pending_timer_version: None,
        }
    }
}

static HOVER_RUNTIME: Mutex<Option<HoverRuntime>> = Mutex::new(None);

fn with_hover_runtime<R>(f: impl FnOnce(&mut HoverRuntime) -> R) -> R {
    let mut guard = HOVER_RUNTIME.lock().unwrap();
    if guard.is_none() {
        *guard = Some(HoverRuntime {
            tracker: HoverTracker::new(),
            cache: TranscriptCache::new(),
            popover: HoverPopover::new(),
            pending_timer_version: None,
        });
    }
    f(guard.as_mut().unwrap())
}
```

注意: `HoverRuntime::new()` const fn のアプローチは `HashMap::new()` が const ではない問題で動かない。`Option<HoverRuntime>` + lazy init（上の `with_hover_runtime`）パターンで回避する。`HoverRuntime::new` メソッドは削除して、上の `with_hover_runtime` だけ残す。

`HoverTracker` / `TranscriptCache` / `HoverPopover` の内部フィールドは `pub(crate)` でなくてよい — `with_hover_runtime` は同じクレート内なので `HoverRuntime::default()` 風には組み立てない。直接 `HoverTracker::new()` などのコンストラクタを使う（既にある）。

- [ ] **Step 3: マウスセレクタの C 関数を追加**

`extern "C" fn key_down` の下あたりに追加:

```rust
extern "C" fn mouse_moved(this: *mut AnyObject, _sel: Sel, event: *mut AnyObject) {
    handle_mouse_event(this, event);
}

extern "C" fn mouse_entered(this: *mut AnyObject, _sel: Sel, event: *mut AnyObject) {
    handle_mouse_event(this, event);
}

extern "C" fn mouse_exited(_this: *mut AnyObject, _sel: Sel, _event: *mut AnyObject) {
    handle_mouse_clear();
}

extern "C" fn update_tracking_areas(this: *mut AnyObject, _sel: Sel) {
    let view: &NSView = unsafe { &*(this as *const NSView) };
    install_tracking_area(view);
    // chain to super
    unsafe {
        let _: () = msg_send![super(view, NSView::class()), updateTrackingAreas];
    }
}

extern "C" fn hover_timer_fired(this: *mut AnyObject, _sel: Sel, _timer: *mut AnyObject) {
    let view: &NSView = unsafe { &*(this as *const NSView) };
    on_hover_timer_fired(view);
}
```

注意: `super(...)` の構文は objc2 ではサポートされていない。代わりに `MsgSend` を使うか、シンプルに NSView の `addTrackingArea` を直接呼んで super への chain を省略できる（NSView の `updateTrackingAreas` のデフォルト実装は no-op）。

修正版:

```rust
extern "C" fn update_tracking_areas(this: *mut AnyObject, _sel: Sel) {
    let view: &NSView = unsafe { &*(this as *const NSView) };
    install_tracking_area(view);
}
```

- [ ] **Step 4: スタブ実装を追加**

同じファイルに以下を追加（次のタスクで中身を埋める）:

```rust
fn install_tracking_area(view: &NSView) {
    // Remove any existing tracking areas first.
    let existing = unsafe { view.trackingAreas() };
    for area in existing.iter() {
        unsafe { view.removeTrackingArea(area) };
    }
    let bounds = view.bounds();
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    let area = unsafe {
        let mtm = MainThreadMarker::new().unwrap();
        let alloc = NSTrackingArea::alloc(mtm);
        let area: Retained<NSTrackingArea> = msg_send![
            alloc,
            initWithRect: bounds,
            options: options,
            owner: view,
            userInfo: std::ptr::null_mut::<AnyObject>()
        ];
        area
    };
    unsafe { view.addTrackingArea(&area) };
}

fn handle_mouse_event(_view_ptr: *mut AnyObject, _event_ptr: *mut AnyObject) {
    // Filled in Task 9.
}

fn handle_mouse_clear() {
    // Filled in Task 9.
}

fn on_hover_timer_fired(_view: &NSView) {
    // Filled in Task 9.
}
```

- [ ] **Step 5: VIEW_CLASS の `get_view_class` にセレクタ登録を追加**

`fn get_view_class()` の `builder.add_method(...)` 群の末尾（`isFlipped` の下）に追加:

```rust
unsafe {
    builder.add_method(
        sel!(mouseMoved:),
        mouse_moved as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
    );
    builder.add_method(
        sel!(mouseEntered:),
        mouse_entered as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
    );
    builder.add_method(
        sel!(mouseExited:),
        mouse_exited as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
    );
    builder.add_method(
        sel!(updateTrackingAreas),
        update_tracking_areas as extern "C" fn(*mut AnyObject, Sel),
    );
    builder.add_method(
        sel!(cckitHoverTimerFired:),
        hover_timer_fired as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
    );
}
```

- [ ] **Step 6: コンパイル確認**

Run: `cargo build`
Expected: 警告のみで成功

- [ ] **Step 7: 既存テストが壊れていないことを確認**

Run: `cargo test`
Expected: 全件 PASS

- [ ] **Step 8: lint と format**

Run: `cargo clippy -- -D warnings && cargo fmt --check`
Expected: エラーなし

- [ ] **Step 9: コミット**

```bash
git add src/monitor/window.rs
git commit -m "$(cat <<'EOF'
feat(window): wire NSTrackingArea + mouse selectors into VIEW_CLASS

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 9: マウスイベント → HoverTracker → 500ms NSTimer → popover を接続

**Files:**
- Modify: `src/monitor/window.rs`

Task 8 で空にしたスタブの中身を実装する。

- [ ] **Step 1: `handle_mouse_event` を実装**

```rust
fn handle_mouse_event(view_ptr: *mut AnyObject, event_ptr: *mut AnyObject) {
    let view: &NSView = unsafe { &*(view_ptr as *const NSView) };
    let event: &NSEvent = unsafe { &*(event_ptr as *const NSEvent) };

    let theme = *CURRENT_THEME.lock().unwrap();
    if matches!(theme, WindowThemeId::Notch) {
        // Notch theme: hover popover not supported.
        with_hover_runtime(|rt| {
            if rt.tracker.current_idx().is_some() {
                rt.tracker.clear();
                rt.popover.hide();
                rt.pending_timer_version = None;
            }
        });
        return;
    }

    let window_point = unsafe { event.locationInWindow() };
    let view_point = unsafe { view.convertPoint_fromView(window_point, None) };
    let view_width = view.bounds().size.width;

    let sessions = SESSION_LIST.lock().unwrap();
    let session_count = sessions.len();
    let hit = match theme {
        WindowThemeId::MissionControl => hit_test_mission_control(
            view_point.x,
            view_point.y,
            view_width,
            session_count,
            MissionControlLayout::default(),
        ),
        WindowThemeId::Classic => hit_test_classic(
            view_point.x,
            view_point.y,
            view_width,
            session_count,
            ClassicLayout::default(),
        ),
        WindowThemeId::Notch => None,
    };
    let hit_with_key = hit.and_then(|h| sessions.get(h.idx).map(|s| (h, s.key())));
    drop(sessions);

    let action = with_hover_runtime(|rt| {
        let event = rt.tracker.on_mouse(hit_with_key, std::time::Instant::now());
        match event {
            HoverEvent::Entered { version, .. } => {
                rt.popover.hide();
                rt.pending_timer_version = Some(version);
                Some(version)
            }
            HoverEvent::Cleared => {
                rt.popover.hide();
                rt.pending_timer_version = None;
                None
            }
            HoverEvent::Unchanged => None,
        }
    });

    if let Some(_version) = action {
        schedule_hover_timer(view);
    }
}

fn handle_mouse_clear() {
    with_hover_runtime(|rt| {
        rt.tracker.clear();
        rt.popover.hide();
        rt.pending_timer_version = None;
    });
}

fn schedule_hover_timer(view: &NSView) {
    unsafe {
        let _: Retained<NSTimer> = msg_send![
            NSTimer::class(),
            scheduledTimerWithTimeInterval: 0.5_f64,
            target: view,
            selector: sel!(cckitHoverTimerFired:),
            userInfo: std::ptr::null_mut::<AnyObject>(),
            repeats: false
        ];
    }
}
```

- [ ] **Step 2: `on_hover_timer_fired` を実装**

```rust
fn on_hover_timer_fired(view: &NSView) {
    // Snapshot the bits we need under the hover lock without holding it
    // across NSPanel work.
    let snapshot = with_hover_runtime(|rt| {
        let scheduled = rt.pending_timer_version;
        let current = rt.tracker.current_version();
        if scheduled.is_none() || scheduled != current {
            rt.pending_timer_version = None;
            return None;
        }
        rt.pending_timer_version = None;
        let idx = rt.tracker.current_idx()?;
        let hit = rt.tracker.current_hit()?;
        Some((idx, hit))
    });
    let Some((idx, hit)) = snapshot else { return };

    let session = {
        let sessions = SESSION_LIST.lock().unwrap();
        sessions.get(idx).cloned()
    };
    let Some(session) = session else { return };
    let Some(transcript_path) = session.transcript_path.as_ref() else { return };
    let path = std::path::PathBuf::from(transcript_path);

    let text = with_hover_runtime(|rt| rt.cache.get_or_load(&path, 10));
    let Some(text) = text else { return };

    with_hover_runtime(|rt| {
        // Re-check that the hover is still on the same row before showing.
        if rt.tracker.current_idx() != Some(idx) {
            return;
        }
        rt.popover.show(view, hit, &text);
    });
}
```

- [ ] **Step 3: setup_window で初回 `updateTrackingAreas` を発火**

`fn setup_window(...)` 内で `view.addSubview(...)` 等の初期化のあとに以下を追加（VIEW_CLASS のインスタンスが作られた直後あたり）:

```rust
unsafe {
    let _: () = msg_send![&*content_view, updateTrackingAreas];
}
```

`content_view` の正確な変数名は `setup_window` の実装に合わせること（既存コードを Read で確認する）。

- [ ] **Step 4: コンパイル確認**

Run: `cargo build`
Expected: 警告のみで成功

- [ ] **Step 5: 全テスト実行**

Run: `cargo test`
Expected: 全件 PASS（既存 + window_hover の 25 件）

- [ ] **Step 6: lint と format**

Run: `cargo clippy -- -D warnings && cargo fmt --check`
Expected: エラーなし

- [ ] **Step 7: コミット**

```bash
git add src/monitor/window.rs
git commit -m "$(cat <<'EOF'
feat(window): wire hover popover end-to-end with 500ms delay timer

Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>
EOF
)"
```

---

## Task 10: 手動スモークテスト + 最終確認

**Files:** なし（実行と検証のみ）

- [ ] **Step 1: app バンドルをビルド**

Run: `mise run build-app`
Expected: `target/release/cckit.app` が生成される

- [ ] **Step 2: app を起動**

Run: `open target/release/cckit.app`
Expected: window が起動し、現在のセッションリストが表示される

- [ ] **Step 3: Mission Control テーマで hover 確認**

手順:
1. window が Mission Control テーマで表示されていることを確認（必要なら設定で切り替え）
2. transcript_path がある（assistant 応答が一度でも記録された）セッション行にマウスを乗せて 500ms 静止
3. 約 0.5 秒後にカードの右隣にフロート popover が出ることを確認
4. popover に最後の assistant 応答の最初の数行が表示されていることを確認
5. マウスを別の行に動かすと popover が即座に消え、新しい行で 500ms 後に表示し直されることを確認
6. マウスを window 外に出すと popover が消えることを確認

- [ ] **Step 4: Classic テーマで hover 確認**

手順:
1. テーマを Classic に切り替える（設定 or `~/Library/Application Support/cckit/window.toml` で `theme = "classic"`）
2. window を再起動
3. 行に hover して同じ挙動になることを確認

- [ ] **Step 5: Notch テーマで hover が無効であることを確認**

手順:
1. テーマを Notch に切り替えて window 再起動
2. 行に hover しても popover が出ないことを確認（クラッシュもしないこと）

- [ ] **Step 6: 画面右端での flip 確認**

手順:
1. window をディスプレイの右端ぎりぎりまで移動
2. セッション行に hover し、popover が行の左側に出ることを確認

- [ ] **Step 7: transcript_path が None の行で何も起きないことを確認**

手順:
1. 起動直後の真新しいセッション（まだ assistant 応答が記録されていない）に hover
2. popover が出ないことを確認（クラッシュなし、ログにエラーが出ないこと）

- [ ] **Step 8: 最終 lint + test**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: 全部 PASS / エラーなし

- [ ] **Step 9: 最終確認コミット（差分があれば）**

差分があれば追加コミット。差分がなければスキップ。

```bash
git status
git diff
```

---

## Self-Review

このプランを書き終えた後の自己レビュー結果:

### Spec Coverage

| Spec 要件 | カバーするタスク |
|---|---|
| Mission Control / Classic で 500ms 後に最後の assistant 応答を popover 表示 | Task 9（タイマ）, Task 7（popover）, Task 3/4（hit-test） |
| 行が変わったら即差し替え、外れたら即消す | Task 5（HoverTracker）, Task 9（handle_mouse_event）, Task 10 Step 3 |
| transcript 読み込みのキャッシュ | Task 6（TranscriptCache） |
| 純ロジックを window.rs から分離 | Task 2-6（全部 window_hover.rs） |
| Notch テーマでは hover 無効 | Task 9 Step 1 内の Notch 早期 return |
| popover 位置（右側、画面端で flip） | Task 7 の `show` 実装 |
| transcript 読めない / assistant 応答なし → 黙ってスキップ | Task 6（cache が None 返す）, Task 9 の `let Some(text) = ... else { return }` |
| 末尾 1 ターン、最大 10 行、超過で `…` | Task 2 の `extract_last_assistant_truncated` |
| hit_test の単体テスト | Task 3/4/5/6 各タスクで必須 |
| 新規 crate 依存なし | 全タスク標準ライブラリと既存 crate のみ |

### 既知のリスク

- `super(view, NSView::class())` 構文を使えないため `update_tracking_areas` で super 呼び出しを省略している。NSView のデフォルト実装は no-op なので影響なし。
- `setLevel: 3` は NSFloatingWindowLevel に対応する整数値。`objc2_app_kit::NSWindowLevel` 定数を使えるならそちらの方が型安全（Step 修正可）。
- HoverTracker は `Default` を `derive` しているが内部で `HashMap` を持たないので `HOVER_RUNTIME` の lazy init に問題なし。
- `cell.cellSizeForBounds:` API はラップされたテキストの実フィット高さを返すので、複数行に対応するはず。万一フィットしない場合は `NSTextField` の代わりに自前の `NSAttributedString.boundingRectWithSize:options:` 計算に切り替える。
