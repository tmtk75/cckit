# Mission Control UI Redesign — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign all cckit UI components (window, TUI, notification, menubar) with a "Mission Control" aesthetic — data dashboard style with animations, agent-colored accents, and unified color system.

**Architecture:** New `src/monitor/theme.rs` centralizes all color, animation, and layout constants. Each UI component (window.rs, tui.rs, notification.rs, menubar.rs) is updated to use the shared theme. Agent type detection is added to `session.rs`. Animation is driven by Core Animation (window/notification) and tick-based sin wave (TUI).

**Tech Stack:** Rust, ratatui (TUI), objc2/Core Animation (macOS window/notification/menubar)

**Spec:** `docs/superpowers/specs/2026-04-01-mission-control-design.md`

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/monitor/theme.rs` | Create | Centralized colors, animation params, layout constants, AgentType enum |
| `src/monitor/session.rs` | Modify | Add `agent_type()` method using model field |
| `src/monitor/mod.rs` | Modify | Add `pub mod theme;` |
| `src/monitor/window.rs` | Modify | Card-based layout, animations, background grid, use theme |
| `src/monitor/tui.rs` | Modify | Double-border, 2-row sessions, animated dots, use theme |
| `src/monitor/notification.rs` | Modify | Dynamic accent bar, typewriter effect, use theme |
| `src/monitor/menubar.rs` | Modify | New Mission Control style, agent-colored indicators, use theme |

---

## Task 1: Create theme.rs — Color System & Agent Types

**Files:**
- Create: `src/monitor/theme.rs`
- Modify: `src/monitor/mod.rs:1-14`
- Modify: `src/monitor/session.rs:68-98`
- Test: `src/monitor/theme.rs` (inline tests)

- [ ] **Step 1: Write the failing test for AgentType detection**

Add to a new file `src/monitor/theme.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_type_from_model() {
        assert_eq!(AgentType::from_model(Some("claude-sonnet-4-20250514")), AgentType::Claude);
        assert_eq!(AgentType::from_model(Some("claude-opus-4-20250514")), AgentType::Claude);
        assert_eq!(AgentType::from_model(Some("gpt-4o")), AgentType::Codex);
        assert_eq!(AgentType::from_model(Some("o3-pro")), AgentType::Codex);
        assert_eq!(AgentType::from_model(Some("codex-mini")), AgentType::Codex);
        assert_eq!(AgentType::from_model(Some("gemini-2.5-pro")), AgentType::Gemini);
        assert_eq!(AgentType::from_model(None), AgentType::Unknown);
        assert_eq!(AgentType::from_model(Some("something-else")), AgentType::Unknown);
    }

    #[test]
    fn test_status_color_hex() {
        assert_eq!(StatusColor::Running.hex(), "#22c55e");
        assert_eq!(StatusColor::AwaitingApproval.hex(), "#ef4444");
        assert_eq!(StatusColor::WaitingInput.hex(), "#f59e0b");
        assert_eq!(StatusColor::Stopped.hex(), "#475569");
    }

    #[test]
    fn test_agent_accent_hex() {
        assert_eq!(AgentType::Claude.accent_hex(), "#d97757");
        assert_eq!(AgentType::Codex.accent_hex(), "#22c55e");
        assert_eq!(AgentType::Gemini.accent_hex(), "#3b82f6");
        assert_eq!(AgentType::Unknown.accent_hex(), "#a855f7");
    }

    #[test]
    fn test_context_gauge_color() {
        let (r, g, b) = context_gauge_rgb(0.0);
        assert_eq!((r, g, b), (0x22, 0xc5, 0x5e)); // green at 0%
        let (r, g, b) = context_gauge_rgb(1.0);
        assert_eq!((r, g, b), (0xef, 0x44, 0x44)); // red at 100%
    }

    #[test]
    fn test_animation_pulse_value() {
        // At t=0, sin(0) = 0 → midpoint
        let v = breathing_pulse(0.0);
        assert!((v - 0.7).abs() < 0.01); // midpoint of 0.4..1.0
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib monitor::theme -- --nocapture`
Expected: FAIL — module `theme` does not exist

- [ ] **Step 3: Register theme module**

In `src/monitor/mod.rs`, add after line 1 (after `pub mod display;`):

```rust
pub mod theme;
```

- [ ] **Step 4: Implement theme.rs with all color/animation constants**

Create `src/monitor/theme.rs`:

```rust
/// Centralized theme: colors, animation parameters, and agent types for Mission Control UI.

// ── Agent Type ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Codex,
    Gemini,
    Unknown,
}

impl AgentType {
    pub fn from_model(model: Option<&str>) -> Self {
        match model {
            Some(m) if m.contains("claude") => AgentType::Claude,
            Some(m) if m.contains("gpt") || m.contains("o1") || m.contains("o3") || m.contains("codex") => AgentType::Codex,
            Some(m) if m.contains("gemini") => AgentType::Gemini,
            Some(_) => AgentType::Unknown,
            None => AgentType::Unknown,
        }
    }

    /// Accent color as (r, g, b) u8 tuple
    pub fn accent_rgb(&self) -> (u8, u8, u8) {
        match self {
            AgentType::Claude => (0xd9, 0x77, 0x57),  // #d97757 terracotta
            AgentType::Codex => (0x22, 0xc5, 0x5e),   // #22c55e green
            AgentType::Gemini => (0x3b, 0x82, 0xf6),  // #3b82f6 blue
            AgentType::Unknown => (0xa8, 0x55, 0xf7),  // #a855f7 purple
        }
    }

    /// Accent color as hex string
    pub fn accent_hex(&self) -> &'static str {
        match self {
            AgentType::Claude => "#d97757",
            AgentType::Codex => "#22c55e",
            AgentType::Gemini => "#3b82f6",
            AgentType::Unknown => "#a855f7",
        }
    }

    /// Accent color as (r, g, b) f64 tuple (0.0–1.0), for NSColor
    pub fn accent_f64(&self) -> (f64, f64, f64) {
        let (r, g, b) = self.accent_rgb();
        (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }
}

// ── Status Colors ───────────────────────────────────

pub enum StatusColor {
    Running,
    AwaitingApproval,
    WaitingInput,
    Stopped,
}

impl StatusColor {
    pub fn rgb(&self) -> (u8, u8, u8) {
        match self {
            StatusColor::Running => (0x22, 0xc5, 0x5e),
            StatusColor::AwaitingApproval => (0xef, 0x44, 0x44),
            StatusColor::WaitingInput => (0xf5, 0x9e, 0x0b),
            StatusColor::Stopped => (0x47, 0x55, 0x69),
        }
    }

    pub fn hex(&self) -> &'static str {
        match self {
            StatusColor::Running => "#22c55e",
            StatusColor::AwaitingApproval => "#ef4444",
            StatusColor::WaitingInput => "#f59e0b",
            StatusColor::Stopped => "#475569",
        }
    }

    pub fn f64(&self) -> (f64, f64, f64) {
        let (r, g, b) = self.rgb();
        (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }
}

// ── Base Palette ────────────────────────────────────

pub mod palette {
    /// Main background — almost-black navy
    pub const BG: (u8, u8, u8) = (0x0a, 0x0a, 0x0f);
    /// Card/panel background
    pub const SURFACE: (u8, u8, u8) = (0x12, 0x12, 0x1a);
    /// Background grid dots
    pub const GRID: (u8, u8, u8) = (0x1a, 0x1a, 0x2e);
    /// Primary text
    pub const TEXT: (u8, u8, u8) = (0xe2, 0xe8, 0xf0);
    /// Dim/secondary text
    pub const TEXT_DIM: (u8, u8, u8) = (0x64, 0x74, 0x8b);
    /// Border opacity (white at this fraction)
    pub const BORDER_ALPHA: f64 = 0.08;
}

// ── Context Gauge ───────────────────────────────────

/// Returns (r, g, b) for a context usage ratio (0.0 = empty, 1.0 = full).
/// 0–0.5: green→yellow, 0.5–1.0: yellow→red
pub fn context_gauge_rgb(ratio: f64) -> (u8, u8, u8) {
    let ratio = ratio.clamp(0.0, 1.0);
    let green: (u8, u8, u8) = (0x22, 0xc5, 0x5e);
    let yellow: (u8, u8, u8) = (0xea, 0xb3, 0x08);
    let red: (u8, u8, u8) = (0xef, 0x44, 0x44);

    let (from, to, t) = if ratio <= 0.5 {
        (green, yellow, ratio / 0.5)
    } else {
        (yellow, red, (ratio - 0.5) / 0.5)
    };

    let lerp = |a: u8, b: u8, t: f64| -> u8 {
        (a as f64 + (b as f64 - a as f64) * t).round() as u8
    };
    (lerp(from.0, to.0, t), lerp(from.1, to.1, t), lerp(from.2, to.2, t))
}

// ── Animation Parameters ────────────────────────────

pub mod anim {
    /// Breathing pulse period in seconds (Running status)
    pub const BREATHING_PERIOD: f64 = 1.5;
    /// Fast blink period in seconds (AwaitingApproval)
    pub const FAST_BLINK_PERIOD: f64 = 0.5;
    /// Slow fade period in seconds (WaitingInput)
    pub const SLOW_FADE_PERIOD: f64 = 3.0;
    /// Card border glow period in seconds
    pub const GLOW_PERIOD: f64 = 2.0;
    /// Context bar leading-edge pulse period
    pub const CONTEXT_PULSE_PERIOD: f64 = 1.0;
    /// Grid ripple decay duration
    pub const RIPPLE_DECAY: f64 = 0.3;
    /// Card appear duration
    pub const CARD_APPEAR: f64 = 0.2;
    /// Card disappear duration
    pub const CARD_DISAPPEAR: f64 = 0.15;
    /// Typewriter per-character delay in seconds
    pub const TYPEWRITER_DELAY: f64 = 0.02;
    /// Status bar blink period
    pub const STATUSBAR_BLINK_PERIOD: f64 = 1.0;
    /// TUI tick interval in milliseconds
    pub const TUI_TICK_MS: u64 = 200;
}

/// Compute breathing pulse value (0.4–1.0) from elapsed seconds.
/// Uses sin wave: midpoint at t=0, peaks at period/4.
pub fn breathing_pulse(elapsed_secs: f64) -> f64 {
    let phase = (elapsed_secs * 2.0 * std::f64::consts::PI / anim::BREATHING_PERIOD).sin();
    0.7 + 0.3 * phase // range: 0.4 to 1.0
}

/// Compute fast blink value (on/off) from elapsed seconds.
/// Returns 1.0 or 0.2.
pub fn fast_blink(elapsed_secs: f64) -> f64 {
    if (elapsed_secs / anim::FAST_BLINK_PERIOD) as u64 % 2 == 0 {
        1.0
    } else {
        0.2
    }
}

/// Compute slow fade value (0.6–1.0) from elapsed seconds.
pub fn slow_fade(elapsed_secs: f64) -> f64 {
    let phase = (elapsed_secs * 2.0 * std::f64::consts::PI / anim::SLOW_FADE_PERIOD).sin();
    0.8 + 0.2 * phase
}

// ── Window Layout Constants ─────────────────────────

pub mod window_layout {
    pub const WIDTH: f64 = 680.0;
    pub const MIN_HEIGHT: f64 = 140.0;
    pub const CARD_HEIGHT: f64 = 64.0;
    pub const CARD_SPACING: f64 = 4.0;
    pub const CARD_CORNER_RADIUS: f64 = 8.0;
    pub const CARD_ACCENT_BAR_WIDTH: f64 = 3.0;
    pub const HEADER_HEIGHT: f64 = 28.0;
    pub const FOOTER_HEIGHT: f64 = 28.0;
    pub const FONT_SIZE: f64 = 11.5;
    pub const FONT_SIZE_SMALL: f64 = 10.0;
    pub const DOT_SIZE: f64 = 8.0;
    pub const GRID_SPACING: f64 = 20.0;
    pub const GRID_DOT_RADIUS: f64 = 1.0;
}

// ── Notification Constants ──────────────────────────

pub mod notif_layout {
    pub const WIDTH: f64 = 340.0;
    pub const MIN_HEIGHT: f64 = 68.0;
    pub const MAX_HEIGHT: f64 = 320.0;
    pub const CORNER_RADIUS: f64 = 12.0;
    pub const PADDING: f64 = 14.0;
    pub const ACCENT_BAR_WIDTH: f64 = 3.0;
    pub const DEFAULT_OPACITY: f64 = 0.92;
    pub const BG_HEX: &str = "#1a1a2e";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_type_from_model() {
        assert_eq!(AgentType::from_model(Some("claude-sonnet-4-20250514")), AgentType::Claude);
        assert_eq!(AgentType::from_model(Some("claude-opus-4-20250514")), AgentType::Claude);
        assert_eq!(AgentType::from_model(Some("gpt-4o")), AgentType::Codex);
        assert_eq!(AgentType::from_model(Some("o3-pro")), AgentType::Codex);
        assert_eq!(AgentType::from_model(Some("codex-mini")), AgentType::Codex);
        assert_eq!(AgentType::from_model(Some("gemini-2.5-pro")), AgentType::Gemini);
        assert_eq!(AgentType::from_model(None), AgentType::Unknown);
        assert_eq!(AgentType::from_model(Some("something-else")), AgentType::Unknown);
    }

    #[test]
    fn test_status_color_hex() {
        assert_eq!(StatusColor::Running.hex(), "#22c55e");
        assert_eq!(StatusColor::AwaitingApproval.hex(), "#ef4444");
        assert_eq!(StatusColor::WaitingInput.hex(), "#f59e0b");
        assert_eq!(StatusColor::Stopped.hex(), "#475569");
    }

    #[test]
    fn test_agent_accent_hex() {
        assert_eq!(AgentType::Claude.accent_hex(), "#d97757");
        assert_eq!(AgentType::Codex.accent_hex(), "#22c55e");
        assert_eq!(AgentType::Gemini.accent_hex(), "#3b82f6");
        assert_eq!(AgentType::Unknown.accent_hex(), "#a855f7");
    }

    #[test]
    fn test_context_gauge_color() {
        let (r, g, b) = context_gauge_rgb(0.0);
        assert_eq!((r, g, b), (0x22, 0xc5, 0x5e));
        let (r, g, b) = context_gauge_rgb(1.0);
        assert_eq!((r, g, b), (0xef, 0x44, 0x44));
    }

    #[test]
    fn test_context_gauge_midpoint() {
        let (r, g, b) = context_gauge_rgb(0.5);
        assert_eq!((r, g, b), (0xea, 0xb3, 0x08)); // yellow at midpoint
    }

    #[test]
    fn test_animation_pulse_value() {
        let v = breathing_pulse(0.0);
        assert!((v - 0.7).abs() < 0.01);
    }

    #[test]
    fn test_fast_blink_alternates() {
        let a = fast_blink(0.0);
        let b = fast_blink(0.5);
        assert!((a - 1.0).abs() < 0.01);
        assert!((b - 0.2).abs() < 0.01);
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib monitor::theme -- --nocapture`
Expected: All 7 tests PASS

- [ ] **Step 6: Add agent_type() method to Session**

In `src/monitor/session.rs`, add after the `display_name()` method (after line 111):

```rust
    /// Detect agent type from model name for theme coloring.
    pub fn agent_type(&self) -> crate::monitor::theme::AgentType {
        crate::monitor::theme::AgentType::from_model(self.model.as_deref())
    }
```

- [ ] **Step 7: Run full test suite**

Run: `cargo test`
Expected: All existing tests + new theme tests PASS

- [ ] **Step 8: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings

- [ ] **Step 9: Commit**

```bash
git add src/monitor/theme.rs src/monitor/mod.rs src/monitor/session.rs
git commit -m "feat(theme): add centralized Mission Control theme with agent types, colors, and animation params"
```

---

## Task 2: Redesign Window App — Card Layout & Theme Integration

**Files:**
- Modify: `src/monitor/window.rs:95-164` (constants and colors)
- Modify: `src/monitor/window.rs:576-838` (rebuild_view)

- [ ] **Step 1: Replace window constants with theme imports**

At the top of `src/monitor/window.rs`, add:

```rust
use crate::monitor::theme::{self, AgentType, StatusColor, palette, anim, window_layout};
```

Replace the constant block (lines 95-112) with:

```rust
const WINDOW_WIDTH: CGFloat = window_layout::WIDTH;
const MIN_WINDOW_HEIGHT: CGFloat = window_layout::MIN_HEIGHT;
const CARD_HEIGHT: CGFloat = window_layout::CARD_HEIGHT;
const CARD_SPACING: CGFloat = window_layout::CARD_SPACING;
const CARD_CORNER_RADIUS: CGFloat = window_layout::CARD_CORNER_RADIUS;
const CARD_ACCENT_BAR_WIDTH: CGFloat = window_layout::CARD_ACCENT_BAR_WIDTH;
const HEADER_HEIGHT: CGFloat = window_layout::HEADER_HEIGHT;
const FOOTER_HEIGHT: CGFloat = window_layout::FOOTER_HEIGHT;
const FONT_SIZE: CGFloat = window_layout::FONT_SIZE;
const FONT_SIZE_SMALL: CGFloat = window_layout::FONT_SIZE_SMALL;
const DOT_SIZE: CGFloat = window_layout::DOT_SIZE;
const GRID_SPACING: CGFloat = window_layout::GRID_SPACING;
const GRID_DOT_RADIUS: CGFloat = window_layout::GRID_DOT_RADIUS;
```

- [ ] **Step 2: Replace color functions with theme-based versions**

Replace the color functions (lines 116-164) with:

```rust
fn color_bg() -> Retained<NSColor> {
    let (r, g, b) = palette::BG;
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0) }
}

fn color_surface() -> Retained<NSColor> {
    let (r, g, b) = palette::SURFACE;
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0) }
}

fn color_text() -> Retained<NSColor> {
    let (r, g, b) = palette::TEXT;
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0) }
}

fn color_dim() -> Retained<NSColor> {
    let (r, g, b) = palette::TEXT_DIM;
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0, 1.0) }
}

fn color_border() -> Retained<NSColor> {
    unsafe { NSColor::colorWithRed_green_blue_alpha_(1.0, 1.0, 1.0, palette::BORDER_ALPHA) }
}

fn status_color(status: &SessionStatus) -> Retained<NSColor> {
    let sc = match status {
        SessionStatus::Running => StatusColor::Running,
        SessionStatus::AwaitingApproval => StatusColor::AwaitingApproval,
        SessionStatus::WaitingInput => StatusColor::WaitingInput,
        SessionStatus::Stopped => StatusColor::Stopped,
    };
    let (r, g, b) = sc.f64();
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, 1.0) }
}

fn agent_accent_color(agent: AgentType) -> Retained<NSColor> {
    let (r, g, b) = agent.accent_f64();
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, 1.0) }
}

fn agent_accent_color_alpha(agent: AgentType, alpha: f64) -> Retained<NSColor> {
    let (r, g, b) = agent.accent_f64();
    unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, alpha) }
}
```

- [ ] **Step 3: Run build to verify compilation**

Run: `cargo build 2>&1 | head -30`
Expected: Compilation succeeds (or only unused-variable warnings from not-yet-updated rebuild_view)

- [ ] **Step 4: Rewrite rebuild_view() for card layout**

This is the largest change. Replace the session row loop (lines 691-837) in `rebuild_view()` with card-based rendering. Each session becomes a card with:

1. Rounded rect background (`color_surface()`)
2. Left accent bar (3px, `agent_accent_color(session.agent_type())`)
3. Row 1: status dot + agent type + project name + elapsed + mini context
4. Row 2: tool name + stats (prompt/tool/compact counts)
5. Row 3: full-width context gauge bar

Update window height calculation (line 601-603):
```rust
let content_h = HEADER_HEIGHT + 1.0
    + sessions.len() as f64 * (CARD_HEIGHT + CARD_SPACING)
    + FOOTER_HEIGHT;
```

For Stopped sessions: use dashed border (alternating short line segments) and 0.5 opacity on the card layer.

- [ ] **Step 5: Update header for Mission Control style**

Replace header drawing (lines 605-670):
- Left: "◉ cckit mission control" in `color_text()`, bold
- Right: "{active} active / {total} total" in `color_dim()`
- Separator: thin line in `color_border()`

- [ ] **Step 6: Update footer**

Replace footer drawing:
- Left: "auto-focus: ✓/⏸"
- Right: "mission control"
- Style: `color_dim()`, small font

- [ ] **Step 7: Run build and manual test**

Run: `cargo build --release --bins`
Expected: Compiles. Then manually test: `cargo run -- app`

- [ ] **Step 8: Commit**

```bash
git add src/monitor/window.rs
git commit -m "feat(window): card-based Mission Control layout with agent accents and new color scheme"
```

---

## Task 3: Window App — Background Grid & Animations

**Files:**
- Modify: `src/monitor/window.rs`

- [ ] **Step 1: Add animation state**

Add a static for tracking animation time:

```rust
use std::time::Instant;

static ANIMATION_START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

fn elapsed_secs() -> f64 {
    ANIMATION_START.get_or_init(Instant::now).elapsed().as_secs_f64()
}
```

- [ ] **Step 2: Draw background dot grid**

In `rebuild_view()`, before drawing cards, add grid rendering:

```rust
fn draw_grid(view: &NSView, width: CGFloat, height: CGFloat) {
    let elapsed = elapsed_secs();
    let (gr, gg, gb) = palette::GRID;
    let base_alpha = 0.1;

    let cols = (width / GRID_SPACING) as usize;
    let rows = (height / GRID_SPACING) as usize;

    for row in 0..rows {
        for col in 0..cols {
            let x = col as f64 * GRID_SPACING;
            let y = row as f64 * GRID_SPACING;
            // Create a small dot view at (x, y) with GRID_DOT_RADIUS
            // Color: palette::GRID at base_alpha
            // (NSView with rounded corners or layer-backed dot)
        }
    }
}
```

- [ ] **Step 3: Add status dot pulse animation**

In the card rendering, apply animation to the status dot opacity:

```rust
let dot_alpha = match session.status {
    SessionStatus::Running => theme::breathing_pulse(elapsed_secs()),
    SessionStatus::AwaitingApproval => theme::fast_blink(elapsed_secs()),
    SessionStatus::WaitingInput => theme::slow_fade(elapsed_secs()),
    SessionStatus::Stopped => 1.0,
};
```

- [ ] **Step 4: Add card border glow animation**

For Running/AwaitingApproval cards, animate border color alpha:

```rust
let glow_alpha = match session.status {
    SessionStatus::Running => {
        let phase = (elapsed_secs() * 2.0 * std::f64::consts::PI / anim::GLOW_PERIOD).sin();
        0.15 + 0.15 * phase // 0.0 to 0.3
    }
    SessionStatus::AwaitingApproval => theme::fast_blink(elapsed_secs()) * 0.3,
    _ => 0.0,
};
// Apply as border color: agent_accent_color_alpha(agent, glow_alpha)
```

- [ ] **Step 5: Add context gauge pulse**

For the context gauge leading edge:

```rust
let gauge_pulse = {
    let phase = (elapsed_secs() * 2.0 * std::f64::consts::PI / anim::CONTEXT_PULSE_PERIOD).sin();
    0.6 + 0.4 * phase.max(0.0) // only pulse brighter, not dimmer
};
```

- [ ] **Step 6: Increase timer frequency for smooth animation**

Change the NSTimer interval from 2.0s to 0.05s (20fps) for animation, but keep session data refresh at 2.0s:

```rust
// Animation timer: 50ms for smooth visuals
let _anim_timer = unsafe {
    NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.05, true, &anim_block)
};
// Data refresh timer: 2s for session updates (keep existing)
```

The animation timer only calls `request_redraw()`. The data timer calls `load_sessions()` + `update_menu()`.

- [ ] **Step 7: Run build and manual test**

Run: `cargo build --release --bins && cargo run -- app`
Expected: Smooth animations visible — dots pulsing, card borders glowing

- [ ] **Step 8: Commit**

```bash
git add src/monitor/window.rs
git commit -m "feat(window): add background grid, status dot pulse, card glow, and context gauge animations"
```

---

## Task 4: Redesign TUI — Double Border, 2-Row Layout, Animated Dots

**Files:**
- Modify: `src/monitor/tui.rs:374-532`

- [ ] **Step 1: Add theme imports to tui.rs**

```rust
use crate::monitor::theme::{self, palette, anim, StatusColor};
use std::time::Instant;
```

Add animation clock to App struct or as a local in the render loop:

```rust
let anim_start = Instant::now();
```

- [ ] **Step 2: Update draw() layout — double border**

Replace the Block in `draw()` (around line 383):

```rust
let outer = Block::default()
    .borders(Borders::ALL)
    .border_type(BorderType::Double)
    .border_style(Style::default().fg(Color::Rgb(palette::GRID.0, palette::GRID.1, palette::GRID.2)))
    .style(Style::default().bg(Color::Rgb(palette::BG.0, palette::BG.1, palette::BG.2)));
```

- [ ] **Step 3: Rewrite draw_header() with Mission Control style**

```rust
fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let active = app.sessions.iter().filter(|s| s.status != SessionStatus::Stopped).count();
    let total = app.sessions.len();

    let header = Line::from(vec![
        Span::styled("  ◉ CCKIT ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled("─── MISSION CONTROL ─── ", Style::default().fg(Color::Rgb(palette::TEXT_DIM.0, palette::TEXT_DIM.1, palette::TEXT_DIM.2))),
        Span::styled(format!("{active} active / {total} total"), Style::default().fg(Color::Rgb(palette::TEXT.0, palette::TEXT.1, palette::TEXT.2))),
    ]);
    f.render_widget(Paragraph::new(header), area);
}
```

- [ ] **Step 4: Rewrite draw_sessions_table() for 2-row card display**

Replace the table with a custom widget approach using Paragraph per session. Each session gets 3 lines (2 content + 1 separator):

```rust
fn draw_sessions(f: &mut Frame, area: Rect, app: &App, elapsed: f64) {
    let dim = Color::Rgb(palette::TEXT_DIM.0, palette::TEXT_DIM.1, palette::TEXT_DIM.2);
    let text_color = Color::Rgb(palette::TEXT.0, palette::TEXT.1, palette::TEXT.2);

    for (i, session) in app.sessions.iter().enumerate() {
        let y = area.y + (i as u16 * 3); // 3 lines per session
        if y + 2 >= area.y + area.height { break; }

        // Status dot with animation
        let dot_char = match session.status {
            SessionStatus::Running => "●",
            SessionStatus::AwaitingApproval => "◆",
            SessionStatus::WaitingInput => "◇",
            SessionStatus::Stopped => "○",
        };
        let sc = match session.status {
            SessionStatus::Running => StatusColor::Running,
            SessionStatus::AwaitingApproval => StatusColor::AwaitingApproval,
            SessionStatus::WaitingInput => StatusColor::WaitingInput,
            SessionStatus::Stopped => StatusColor::Stopped,
        };
        let (sr, sg, sb) = sc.rgb();
        let dot_style = Style::default().fg(Color::Rgb(sr, sg, sb));

        // Row 1: dot + agent + project + tool + elapsed + context%
        let row1 = Line::from(vec![
            Span::styled(format!("  {dot_char} "), dot_style),
            Span::styled(format!("{:<12}", session.display_name()), Style::default().fg(text_color)),
            Span::styled(" │ ", Style::default().fg(dim)),
            Span::styled(format!("{:<14}", session.project_name()), Style::default().fg(text_color)),
            Span::styled(" │ ", Style::default().fg(dim)),
            Span::styled(format!("{:<5}", session.last_tool.as_deref().unwrap_or("-")), Style::default().fg(Color::Cyan)),
            // ... elapsed, context%
        ]);

        // Row 2: context bar + stats
        // Use Unicode block chars: █ for filled, ░ for empty
        let ratio = context_ratio(session);
        let bar_width = (area.width as usize).saturating_sub(10);
        let filled = (ratio * bar_width as f64) as usize;
        let (cr, cg, cb) = theme::context_gauge_rgb(ratio);
        let row2 = Line::from(vec![
            Span::raw("    "),
            Span::styled("█".repeat(filled), Style::default().fg(Color::Rgb(cr, cg, cb))),
            Span::styled("░".repeat(bar_width - filled), Style::default().fg(dim)),
            Span::styled(format!("  {}p {}t {}c", session.prompt_count, session.tool_count, session.compact_count), Style::default().fg(dim)),
        ]);

        f.render_widget(Paragraph::new(row1), Rect::new(area.x, y, area.width, 1));
        f.render_widget(Paragraph::new(row2), Rect::new(area.x, y + 1, area.width, 1));

        // Separator
        if i < app.sessions.len() - 1 {
            let sep = "  ─ ".repeat((area.width as usize) / 4);
            f.render_widget(
                Paragraph::new(Span::styled(sep, Style::default().fg(dim))),
                Rect::new(area.x, y + 2, area.width, 1),
            );
        }
    }
}
```

- [ ] **Step 5: Update selected row highlight**

Use agent accent color at 10% as selection background:

```rust
let agent = session.agent_type();
let (ar, ag, ab) = agent.accent_rgb();
let selected_bg = Color::Rgb(ar / 10, ag / 10, ab / 10); // approximate 10% on dark bg
```

- [ ] **Step 6: Reduce event_timeout for animation**

Change `event_timeout_ms` in the TUI config from 500 to 200 for smoother animation:

```rust
event_timeout_ms: anim::TUI_TICK_MS as u64, // 200ms
```

- [ ] **Step 7: Run build and test**

Run: `cargo build && cargo test`
Expected: Compiles and all tests pass

- [ ] **Step 8: Manual test TUI**

Run: `cargo run -- session tui`
Expected: Double borders, 2-row sessions, animated status dots, context bars

- [ ] **Step 9: Commit**

```bash
git add src/monitor/tui.rs
git commit -m "feat(tui): Mission Control layout with double borders, 2-row sessions, and animated status dots"
```

---

## Task 5: Redesign Notification — Dynamic Accent & Typewriter

**Files:**
- Modify: `src/monitor/notification.rs:151-164` (constants)
- Modify: `src/monitor/notification.rs:166-430` (send_notify, rendering)

- [ ] **Step 1: Replace constants with theme imports**

```rust
use crate::monitor::theme::{self, palette, anim, notif_layout, StatusColor};
```

Replace constants (lines 151-164):

```rust
const DEFAULT_WIDTH: f64 = notif_layout::WIDTH;
const MIN_HEIGHT: f64 = notif_layout::MIN_HEIGHT;
const MAX_HEIGHT: f64 = notif_layout::MAX_HEIGHT;
const DEFAULT_MARGIN: f64 = 10.0;
const DEFAULT_OPACITY: f64 = notif_layout::DEFAULT_OPACITY;
const DEFAULT_BGCOLOR: &str = notif_layout::BG_HEX;
const CORNER_RADIUS: f64 = notif_layout::CORNER_RADIUS;
const PADDING: f64 = notif_layout::PADDING;
const ACCENT_BAR_WIDTH: f64 = notif_layout::ACCENT_BAR_WIDTH;
```

- [ ] **Step 2: Make accent bar color dynamic based on session status**

In the accent bar creation section of `send_notify()`, replace the static purple color:

```rust
// Determine accent color from session status (passed via notification context)
let accent_color = match status {
    Some(SessionStatus::Running) => {
        let (r, g, b) = StatusColor::Running.f64();
        unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, 1.0) }
    }
    Some(SessionStatus::AwaitingApproval) => {
        let (r, g, b) = StatusColor::AwaitingApproval.f64();
        unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, 1.0) }
    }
    Some(SessionStatus::WaitingInput) => {
        let (r, g, b) = StatusColor::WaitingInput.f64();
        unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, 1.0) }
    }
    _ => {
        // Default: title purple
        unsafe { NSColor::colorWithRed_green_blue_alpha_(0.424, 0.361, 0.906, 1.0) }
    }
};
```

This requires passing `SessionStatus` to the notification function. Check the current `send_notify()` signature and add an optional `status: Option<SessionStatus>` parameter.

- [ ] **Step 3: Add typewriter effect for message text**

After creating the message label, add a timer that reveals characters progressively:

```rust
fn apply_typewriter(label: &NSTextField, full_text: &str, delay_per_char: f64) {
    let chars: Vec<char> = full_text.chars().collect();
    let total = chars.len();
    // Start with empty text
    unsafe { label.setStringValue(&NSString::from_str("")) };

    for i in 0..total {
        let partial: String = chars[..=i].iter().collect();
        let delay = i as f64 * delay_per_char;
        // Schedule with dispatch_after or NSTimer
        // Use dispatch_after for simplicity:
        let label_clone = label.retain();
        dispatch_after(delay, move || {
            unsafe { label_clone.setStringValue(&NSString::from_str(&partial)) };
        });
    }
}
```

- [ ] **Step 4: Add flash effect on notification appear**

Briefly tint the background to the status color, then fade back:

```rust
// Flash: set background to status color at 30% opacity
// Then animate back to normal over 0.3s using NSAnimationContext
unsafe {
    NSAnimationContext::runAnimationGroup_completionHandler(
        &block,  // set background to status-tinted color
        Some(&completion_block),  // restore to normal bgcolor
    );
}
```

- [ ] **Step 5: Update corner radius**

Already handled by constant change (8→12).

- [ ] **Step 6: Run build and manual test**

Run: `cargo build --release --bins`
Test: Trigger a notification via `cargo run -- session hook` with test data, or manually call the notification function.

- [ ] **Step 7: Commit**

```bash
git add src/monitor/notification.rs
git commit -m "feat(notification): dynamic accent bar, typewriter text effect, and flash animation"
```

---

## Task 6: Redesign Menubar — Mission Control Style

**Files:**
- Modify: `src/monitor/menubar.rs:167-297` (MenubarStyle enum and formatters)
- Modify: `src/monitor/menubar.rs:555-733` (status/menu update)

- [ ] **Step 1: Add MissionControl variant to MenubarStyle**

In `src/monitor/menubar.rs`, add to the enum (line 167-173):

```rust
pub enum MenubarStyle {
    Emoji,
    Terminal,
    Htop,
    Compact,
    MissionControl,  // NEW
}
```

Update `all()` (line 176):
```rust
pub fn all() -> [MenubarStyle; 5] {
    [MenubarStyle::Emoji, MenubarStyle::Terminal, MenubarStyle::Htop, MenubarStyle::Compact, MenubarStyle::MissionControl]
}
```

Update `label()` (line 180):
```rust
MenubarStyle::MissionControl => "Mission Control",
```

- [ ] **Step 2: Implement status_title() for MissionControl**

Add to the `status_title()` match (after Compact, around line 228):

```rust
MenubarStyle::MissionControl => {
    let approval_str = if awaiting > 0 {
        format!(" {awaiting}⚠")
    } else {
        String::new()
    };
    format!("◉ {running}↑{approval_str} {total}")
}
```

- [ ] **Step 3: Implement session_label() for MissionControl**

Add to `session_label()` match (after Compact, around line 297):

```rust
MenubarStyle::MissionControl => {
    let indicator = match session.status {
        SessionStatus::Running => "●",
        SessionStatus::AwaitingApproval => "◆",
        SessionStatus::WaitingInput => "◇",
        SessionStatus::Stopped => "○",
    };
    let tool = session.last_tool.as_deref().unwrap_or("");
    let elapsed = display::format_elapsed_short(session.updated_at);

    // Context mini-bar
    let ctx = if let (Some(used), Some(max)) = (session.context_used_tokens, session.context_max_tokens) {
        let ratio = used as f64 / max as f64;
        let pct = (ratio * 100.0) as u8;
        let filled = (ratio * 4.0) as usize;
        let empty = 4 - filled;
        format!(" {}{}  {pct}%", "▓".repeat(filled), "░".repeat(empty))
    } else {
        String::new()
    };

    format!("{indicator} {:<12} {:<12} {:<5} {:>3}{ctx}",
        session.display_name(), session.project_name(), tool, elapsed)
}
```

- [ ] **Step 4: Implement legend() for MissionControl**

```rust
MenubarStyle::MissionControl => "● run  ◆ tool  ◇ wait  ○ done  ↑ active  ⚠ approval".to_string(),
```

- [ ] **Step 5: Add agent-colored NSAttributedString for menu items**

In `update_menu()`, when creating menu items for MissionControl style, use `NSAttributedString` with agent-specific colors:

```rust
if current_style == MenubarStyle::MissionControl {
    let agent = session.agent_type();
    let (r, g, b) = agent.accent_f64();
    let color = unsafe { NSColor::colorWithRed_green_blue_alpha_(r, g, b, 1.0) };
    // Create attributed string with this color for the indicator character
    // and monospace font for the rest
}
```

- [ ] **Step 6: Add status bar blink for AwaitingApproval**

In `get_status_title()` or the timer callback, when AwaitingApproval > 0, alternate the status bar icon:

```rust
if awaiting > 0 && current_style == MenubarStyle::MissionControl {
    let blink_on = (elapsed_secs() / anim::STATUSBAR_BLINK_PERIOD) as u64 % 2 == 0;
    if blink_on {
        // Show "⚠" with red tint
    } else {
        // Show normal "◉"
    }
}
```

- [ ] **Step 7: Run build and manual test**

Run: `cargo build --release --bins && cargo run -- app`
Expected: New "Mission Control" style appears in menubar style submenu. Selecting it shows the new format.

- [ ] **Step 8: Commit**

```bash
git add src/monitor/menubar.rs
git commit -m "feat(menubar): add Mission Control style with agent colors and approval blink"
```

---

## Task 7: Integration Testing & Polish

**Files:**
- All modified files
- Modify: `src/monitor/window.rs` (card appear/disappear animations)

- [ ] **Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

- [ ] **Step 2: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: No warnings. Fix any that appear.

- [ ] **Step 3: Run fmt check**

Run: `cargo fmt --check`
Expected: No formatting issues. Run `cargo fmt` if needed.

- [ ] **Step 4: Add card appear/disappear animations to window**

In `rebuild_view()`, track previous session list and animate:
- New sessions: start with opacity 0, translateY +8, animate to opacity 1, translateY 0
- Removed sessions: animate opacity to 0

This requires storing the previous session ID set in a static:

```rust
static PREV_SESSION_IDS: Mutex<Vec<String>> = Mutex::new(Vec::new());
```

Compare current vs previous to detect new/removed sessions, then apply CABasicAnimation.

- [ ] **Step 5: Manual end-to-end test**

Test checklist:
1. `cargo run -- app` — window shows with new card layout, grid background, animations
2. Start a Claude Code session in another terminal — card appears with fade-in
3. Session uses a tool — AwaitingApproval state: red blink on dot, card border pulses
4. Session completes — Stopped: dashed border, dim
5. Menubar → Style → Mission Control — new format shows
6. Notification triggers — typewriter text, status-colored accent bar
7. TUI: `cargo run -- session tui` — double borders, 2-row sessions, context bars

- [ ] **Step 6: Final commit**

```bash
git add -A
git commit -m "feat(ui): Mission Control redesign — polish and card animations"
```
