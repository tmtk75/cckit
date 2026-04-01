# Mission Control — cckit UI Redesign

Vibe Island (vibeisland.app) にインスパイアされた、cckit 全 UI コンポーネントのデザインリフレッシュ。データダッシュボード（計器パネル）スタイルで、アニメーションとプログラマティック美学を全面に。

## Table of Contents

- [Design Principles](#design-principles)
- [Color System](#color-system)
- [Window App](#window-app)
- [TUI](#tui)
- [Notification Window](#notification-window)
- [Menubar](#menubar)
- [Animation Specification](#animation-specification)
- [Implementation Notes](#implementation-notes)

## Design Principles

1. **Mission Control metaphor** — NASA の管制室のように、複数セッションをリアルタイムで監視する緊張感と美しさ
2. **Living UI** — 静止した画面ではなく、呼吸し脈動する生きたインターフェース
3. **Agent identity** — エージェント種別（Claude Code / Codex / Gemini）が色で瞬時に識別できる
4. **Status at a glance** — ステータスの緊急度がアニメーション速度で伝わる
5. **Information density** — 監視ツールとして必要な情報を削らず、視覚的に整理する

## Color System

### Base Palette

| Role | Color | Hex | Usage |
|------|-------|-----|-------|
| Background | Almost-black navy | `#0a0a0f` | Main window / TUI background |
| Surface | Dark navy | `#12121a` | Card / panel background |
| Grid | Muted navy @ 10% | `#1a1a2e` | Background grid dots |
| Border | White @ 8% | `rgba(255,255,255,0.08)` | Panel borders |
| Text Primary | Soft white | `#e2e8f0` | Main text |
| Text Dim | Slate | `#64748b` | Secondary text, timestamps |

### Agent Accent Colors

| Agent | Color | Hex | Glow (20% opacity) |
|-------|-------|-----|---------------------|
| Claude Code | Terracotta | `#d97757` | `rgba(217,119,87,0.2)` |
| Codex | Green | `#22c55e` | `rgba(34,197,94,0.2)` |
| Gemini | Blue | `#3b82f6` | `rgba(59,130,246,0.2)` |
| Unknown | Purple | `#a855f7` | `rgba(168,85,247,0.2)` |

### Status Colors (unified across all components)

| Status | Color | Hex | Animation |
|--------|-------|-----|-----------|
| Running | Green | `#22c55e` | Breathing pulse, 1.5s cycle |
| AwaitingApproval | Red | `#ef4444` | Fast blink, 0.5s cycle |
| WaitingInput | Amber | `#f59e0b` | Slow fade, 3s cycle |
| Stopped | Slate | `#475569` | Static |

Currently TUI and Window use different color schemes; this redesign unifies them.

### Context Gauge Gradient

- 0–50%: Green (`#22c55e`) → Yellow (`#eab308`) linear interpolation
- 50–100%: Yellow (`#eab308`) → Red (`#ef4444`) linear interpolation

## Window App

### Layout: Card-based Session Monitor

```
┌─────────────────────────────────────────────────────┐
│  ◉ cckit mission control          2 active / 5 total │
│  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ │
│ ┌─────────────────────────────────────────────────┐ │
│ │ ● claude-code  myproject           3m  ▓▓▓░ 72%│ │
│ │   Bash  ⏐ 12p 34t 2c                           │ │
│ │   ████████████████████░░░░░░░  context 72%      │ │
│ └─────────────────────────────────────────────────┘ │
│ ┌─────────────────────────────────────────────────┐ │
│ │ ◆ codex  api-server               1m  ▓░░░ 15%│ │
│ │   Edit  ⏐ 5p 8t 0c                             │ │
│ │   ███░░░░░░░░░░░░░░░░░░░░░░░  context 15%      │ │
│ └─────────────────────────────────────────────────┘ │
│ ┌ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─┐ │
│   ○ claude-code  old-task           45m  stopped  │ │
│ └ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─┘ │
│                                                     │
│  auto-focus: ✓  ⏐  style: mission-control          │
└─────────────────────────────────────────────────────┘
```

### Card Design

Each session is rendered as a card panel:

- **Background**: Surface color (`#12121a`) with `corner_radius: 8.0`
- **Left accent bar**: 3px vertical bar in agent accent color
- **Running cards**: Border glows in agent accent color (sin wave opacity, 2s cycle)
- **AwaitingApproval cards**: Border pulses red (0.5s cycle)
- **Stopped cards**: Dashed border, reduced opacity (0.5), no glow

### Card Content (3 rows)

1. **Row 1**: Status dot + agent name + project name + elapsed time + context mini-bar
2. **Row 2**: Current tool + separator + prompt/tool/compact counts
3. **Row 3**: Full-width context gauge with percentage label

### Context Gauge

- Full-width progress bar at card bottom
- Gradient follows the unified context gauge gradient
- Leading edge pulses subtly (1s cycle, opacity 0.6–1.0)
- Right-aligned percentage text

### Background Grid

- Dot grid: `#1a1a2e` dots at 20px intervals on main background
- On session state change: ripple effect from the changed card — nearby dots briefly brighten (0.3s decay, radius ~80px)

### Window Dimensions

| Property | Current | New |
|----------|---------|-----|
| Width | 640 | 680 |
| Card height | 22 (single row) | 64 (3 rows) |
| Card spacing | 0 | 4 |
| Header height | 20 | 28 |
| Footer height | 22 | 28 |

### Subagent Display

Subagent sessions are indented within their parent card as a nested mini-card with reduced height (2 rows, no context gauge).

## TUI

### Layout

```
╔══════════════════════════════════════════════════════╗
║  ◉ CCKIT ─── MISSION CONTROL ─── 2 active / 5 total ║
╠══════════════════════════════════════════════════════╣
║                                                      ║
║  ● claude-code │ myproject      │ Bash  │ 3m │ 72%  ║
║    ████████████████████░░░░░░░░  12p 34t 2c          ║
║  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─    ║
║  ◆ codex      │ api-server     │ Edit  │ 1m │ 15%   ║
║    ███░░░░░░░░░░░░░░░░░░░░░░░░  5p 8t 0c            ║
║  ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─ ─    ║
║  ○ claude-code │ old-task       │       │ 45m│ done  ║
║                                                      ║
╠══════════════════════════════════════════════════════╣
║  q:quit  j/k:navigate  f:focus  a:auto-focus        ║
╚══════════════════════════════════════════════════════╝
```

### Changes from Current

- **Double-line border** (`╔╗╚╝═║`) for instrument panel feel
- **2-row per session**: Row 1 = metadata, Row 2 = context bar + stats
- **Status dot color pulse**: 200ms tick loop, brightness varies via sin wave
- **Header**: "CCKIT" in bold cyan, flanked by horizontal rules for mission control feel
- **Selected row**: Agent accent color at 10% opacity background (replaces DarkGray)
- **Stopped sessions**: DarkGray + dim style, single row (no context bar)
- **Dashed separators** between sessions (light gray `─ ─ ─`)

### TUI Animation Implementation

ratatui does not have native animation, but the existing tick loop (used for periodic refresh) can drive animations:

- **Tick interval**: 200ms (5 fps, sufficient for breathing effects)
- **Sin wave brightness**: `brightness = base + amplitude * sin(2π * t / period)`
- **Color interpolation**: Precompute 8-step color lookup table per status for efficiency

## Notification Window

### Changes from Current

| Property | Current | New |
|----------|---------|-----|
| Left accent bar color | Static purple `#6C5CE7` | Dynamic: status color, pulsing |
| Text appearance | Instant | Typewriter effect (20ms per char) |
| Corner radius | 8px | 12px |
| Background | `#1a1a2e` | `#1a1a2e` (unchanged, matches palette) |
| State change effect | None | Flash: background briefly tints status color, 0.3s decay |

### Typewriter Effect

- Characters appear one at a time with 20ms interval
- Title appears first (typewriter), then subtitle fades in, then message body (typewriter)
- If notification is dismissed before animation completes, skip to final state

### Accent Bar Pulse

- Bar color matches current session status color
- For Running/AwaitingApproval: pulses with same timing as status animations
- For Stopped: static slate

## Menubar

### New Style: "Mission Control" (5th style)

Added alongside existing Emoji / Terminal / Htop / Compact styles.

**Status bar format:**
```
◉ 2↑ 1⚠ 5
```

- `◉` = cckit indicator
- `2↑` = running count
- `1⚠` = awaiting approval count (omitted if 0)
- `5` = total sessions

**Menu item format:**
```
● claude-code  myproject   Bash  3m  ▓▓▓░ 72%
◆ codex        api-server  Edit  1m  ▓░░░ 15%
○ claude-code  old-task          45m  stopped
```

### Status Bar Animation

- When AwaitingApproval sessions exist: status bar icon alternates between `◉` and `⚠` with red tint (NSAttributedString color change on timer)
- Normal state: static `◉`

### Agent-colored Indicators

Menu item status indicators use agent accent colors instead of uniform color:
- `●` (Claude Code) in terracotta
- `◆` (Codex) in green
- `▲` (Gemini) in blue

## Animation Specification

### Shared Animation Parameters

| Animation | Type | Period | Easing |
|-----------|------|--------|--------|
| Breathing pulse | Sin wave opacity (0.4–1.0) | 1.5s | ease-in-out (sin) |
| Fast blink | On/off toggle | 0.5s | linear |
| Slow fade | Sin wave opacity (0.6–1.0) | 3.0s | ease-in-out (sin) |
| Card glow | Sin wave border opacity (0.0–0.3) | 2.0s | ease-in-out (sin) |
| Context bar pulse | Sin wave leading edge opacity (0.6–1.0) | 1.0s | ease-in-out (sin) |
| Grid ripple | Radial brightness decay | 0.3s | ease-out (exponential) |
| Card appear | Opacity 0→1 + translateY 8→0 | 0.2s | ease-out |
| Card disappear | Opacity 1→0 | 0.15s | ease-in |
| Typewriter | Per-character reveal | 20ms/char | linear |
| Status bar blink | Icon swap | 1.0s | step |

### Animation Framework (Window App)

macOS Core Animation (`CALayer`) for smooth GPU-accelerated animations:

- Each card is a `CALayer` with sublayers for accent bar, border glow, and context gauge
- Status dot uses `CABasicAnimation` for opacity cycling
- Grid ripple uses `CATransaction` with grouped layer updates
- Timer fires at 60fps for Core Animation, but most animations are declarative (set duration and let CA interpolate)

### Animation Framework (TUI)

ratatui tick-based animation:

- Main loop tick: 200ms
- Global animation clock: `Instant::now()` at each tick
- Sin wave computed per-tick: `(clock.elapsed().as_secs_f64() * 2π / period).sin()`
- Applied as color brightness modifier to status dots and context bar

## Implementation Notes

### File Changes

| File | Change Scope |
|------|-------------|
| `src/monitor/window.rs` | Major rewrite: card layout, animations, grid background, new constants |
| `src/monitor/tui.rs` | Significant changes: 2-row layout, double borders, animation loop, new colors |
| `src/monitor/notification.rs` | Moderate: typewriter effect, dynamic accent bar, flash effect |
| `src/monitor/menubar.rs` | Moderate: new Mission Control style, agent-colored indicators, status bar animation |
| `src/monitor/session.rs` | Minor: add agent type enum if not present (for accent color mapping) |
| New: `src/monitor/theme.rs` | New file: centralized color/animation constants shared across all components |

### Agent Type Detection

To assign accent colors, detect agent type from session data:

- **Claude Code**: model field contains "claude" or agent is "claude-code"
- **Codex**: model field contains "gpt" / "o1" / "o3" / "codex", or source is codex hooks
- **Gemini**: model field contains "gemini"
- **Unknown**: fallback purple

### Performance Considerations

- **Window app**: Core Animation handles interpolation on GPU — minimal CPU overhead
- **TUI**: 200ms tick is very lightweight; sin wave computation is negligible
- **Notification typewriter**: 20ms timer per character — trivial overhead, auto-cancelled on dismiss
- **Grid ripple**: Only triggers on state change events (not continuous), affects ~20 dots max

### Configuration

Add to `~/.config/cckit/window.toml`:

```toml
# Existing
background_opacity = 0.5

# New
animations_enabled = true      # Master toggle for all animations
grid_visible = true            # Background dot grid
```

### Backward Compatibility

- Menubar: existing 4 styles unchanged, "Mission Control" added as 5th option
- Window: layout changes are not configurable (full replacement)
- TUI: double-border and 2-row layout are the new default
- Notification: typewriter can be disabled via `animations_enabled = false`
