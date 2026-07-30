//! Centralized Mission Control theme: colors, agent types, animation parameters, and layout constants.

// ---------------------------------------------------------------------------
// AgentType
// ---------------------------------------------------------------------------

/// The AI agent/model type detected from a session's model string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentType {
    Claude,
    Codex,
    Gemini,
    Unknown,
}

impl AgentType {
    /// Detect the agent type from a model name string.
    pub fn from_model(model: Option<&str>) -> Self {
        match model {
            Some(m) if m.contains("claude") => Self::Claude,
            Some(m)
                if m.contains("gpt")
                    || m.contains("o1")
                    || m.contains("o3")
                    || m.contains("codex") =>
            {
                Self::Codex
            }
            Some(m) if m.contains("gemini") => Self::Gemini,
            _ => Self::Unknown,
        }
    }

    /// Accent color as `(r, g, b)` u8 tuple.
    pub fn accent_rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Claude => (0xd9, 0x77, 0x57),  // #d97757 terracotta
            Self::Codex => (0x22, 0xc5, 0x5e),   // #22c55e green
            Self::Gemini => (0x3b, 0x82, 0xf6),  // #3b82f6 blue
            Self::Unknown => (0xa8, 0x55, 0xf7), // #a855f7 purple
        }
    }

    /// Accent color as a CSS hex string.
    pub fn accent_hex(self) -> &'static str {
        match self {
            Self::Claude => "#d97757",
            Self::Codex => "#22c55e",
            Self::Gemini => "#3b82f6",
            Self::Unknown => "#a855f7",
        }
    }

    /// Accent color as `(r, g, b)` f64 values in `[0.0, 1.0]`.
    pub fn accent_f64(self) -> (f64, f64, f64) {
        let (r, g, b) = self.accent_rgb();
        (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }
}

/// Claude model families, paired with their display name.
const CLAUDE_FAMILIES: [(&str, &str); 4] = [
    ("opus", "Opus"),
    ("sonnet", "Sonnet"),
    ("haiku", "Haiku"),
    ("fable", "Fable"),
];

/// Read the version that follows a family name in a modern Claude model id.
///
/// `-4-7-20250101` -> `"4.7"`, `-5` -> `"5"`, `-4-20250514` -> `"4"` (a trailing
/// date stamp is not a minor version). Returns `None` when no version follows,
/// as in the bare `opus` alias or the legacy `claude-3-5-sonnet-…` ordering.
fn claude_version(rest: &str) -> Option<String> {
    let mut parts = rest
        .trim_start_matches('-')
        .split('-')
        .take_while(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let major = parts.next()?;
    if major.len() >= 5 {
        return None; // a date stamp, not a version
    }
    match parts.next() {
        Some(minor) if minor.len() <= 2 => Some(format!("{major}.{minor}")),
        _ => Some(major.to_string()),
    }
}

/// Read the version segment that follows a `gpt-` prefix.
///
/// `gpt-5.6-sol` -> `"5.6"`, `gpt-5-codex` -> `"5"`, `gpt-4o-mini` -> `"4o"`.
/// Returns `None` when that segment is not a version, as in `gpt-oss-120b`.
fn gpt_version(m: &str) -> Option<&str> {
    let seg = m.strip_prefix("gpt-")?.split('-').next()?;
    seg.starts_with(|c: char| c.is_ascii_digit()).then_some(seg)
}

/// Return a short, human-readable label for a model id (e.g. "Opus 4.7").
/// Returns `None` if the id is unrecognized; callers fall back to the
/// `AgentType` family label. Matching is case-insensitive: the version is
/// parsed from the id where the layout allows, otherwise ordered substring
/// rules apply — more specific patterns come first.
pub fn model_short_label(model: &str) -> Option<String> {
    let lowered = model.to_ascii_lowercase();
    // Drop a trailing context-window marker such as "[1m]".
    let m = lowered.split('[').next().unwrap_or(&lowered);
    if m.is_empty() {
        return None;
    }

    // Modern ids look like "claude-<family>-<major>[-<minor>][-<date>]". Parse
    // the version rather than enumerating releases, so a model newer than this
    // code still shows its version instead of degrading to the bare family.
    for (family, display) in CLAUDE_FAMILIES {
        if let Some(pos) = m.find(family)
            && let Some(v) = claude_version(&m[pos + family.len()..])
        {
            return Some(format!("{display} {v}"));
        }
    }

    // Same idea for OpenAI ids: "gpt-<version>[-<variant>]".
    if let Some(v) = gpt_version(m) {
        return Some(format!("GPT-{v}"));
    }

    let label = if m.contains("opus-4-8") {
        "Opus 4.8"
    } else if m.contains("opus-4-7") {
        "Opus 4.7"
    } else if m.contains("opus-4") {
        "Opus 4"
    } else if m.contains("sonnet-5") {
        "Sonnet 5"
    } else if m.contains("sonnet-4-5") {
        "Sonnet 4.5"
    } else if m.contains("sonnet-4") {
        "Sonnet 4"
    } else if m.contains("sonnet-3-5") || m.contains("3-5-sonnet") {
        "Sonnet 3.5"
    } else if m.contains("haiku-4-5") {
        "Haiku 4.5"
    } else if m.contains("haiku") {
        "Haiku"
    } else if m.contains("fable-5") {
        "Fable 5"
    } else if m.contains("fable") {
        "Fable"
    } else if m.contains("sonnet") {
        "Sonnet"
    } else if m.contains("opus") {
        "Opus"
    } else if m.starts_with("gpt-") {
        "GPT"
    } else if m.contains("codex-mini") {
        "Codex mini"
    } else if m.starts_with("codex-") {
        "Codex"
    } else if m.contains("gemini-2.5") {
        "Gemini 2.5"
    } else if m.contains("gemini-2.0") {
        "Gemini 2.0"
    } else if m.contains("gemini-1.5") {
        "Gemini 1.5"
    } else if m.contains("gemini") {
        "Gemini"
    } else {
        return None;
    };
    Some(label.to_string())
}

// ---------------------------------------------------------------------------
// StatusColor
// ---------------------------------------------------------------------------

/// Semantic color for each session status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusColor {
    Running,
    AwaitingApproval,
    WaitingInput,
    Stopped,
}

impl StatusColor {
    /// Color as `(r, g, b)` u8 tuple.
    pub fn rgb(self) -> (u8, u8, u8) {
        match self {
            Self::Running => (0x22, 0xc5, 0x5e),          // green #22c55e
            Self::AwaitingApproval => (0xef, 0x44, 0x44), // red #ef4444
            Self::WaitingInput => (0xf5, 0x9e, 0x0b),     // amber #f59e0b
            Self::Stopped => (0x47, 0x55, 0x69),          // slate #475569
        }
    }

    /// Color as a CSS hex string.
    pub fn hex(self) -> &'static str {
        match self {
            Self::Running => "#22c55e",
            Self::AwaitingApproval => "#ef4444",
            Self::WaitingInput => "#f59e0b",
            Self::Stopped => "#475569",
        }
    }

    /// Color as `(r, g, b)` f64 values in `[0.0, 1.0]`.
    pub fn f64(self) -> (f64, f64, f64) {
        let (r, g, b) = self.rgb();
        (r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0)
    }
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

/// Base palette constants shared across all UI surfaces.
pub mod palette {
    /// Background — deepest dark.
    pub const BG: (u8, u8, u8) = (0x0a, 0x0a, 0x0f);
    /// Surface — card / panel background.
    pub const SURFACE: (u8, u8, u8) = (0x12, 0x12, 0x1a);
    /// Grid dot color.
    pub const GRID: (u8, u8, u8) = (0x1a, 0x1a, 0x2e);
    /// Primary text.
    pub const TEXT: (u8, u8, u8) = (0xe2, 0xe8, 0xf0);
    /// Dimmed / secondary text.
    pub const TEXT_DIM: (u8, u8, u8) = (0x64, 0x74, 0x8b);
    /// Border alpha (applied on top of surface).
    pub const BORDER_ALPHA: f64 = 0.08;
}

// ---------------------------------------------------------------------------
// Context gauge gradient
// ---------------------------------------------------------------------------

/// Return an RGB color for a context-usage gauge, interpolating
/// green (#22c55e) → yellow (#eab308) → red (#ef4444) as `ratio` goes from 0.0 to 1.0.
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

    let lerp =
        |a: u8, b: u8, t: f64| -> u8 { (a as f64 + (b as f64 - a as f64) * t).round() as u8 };
    (
        lerp(from.0, to.0, t),
        lerp(from.1, to.1, t),
        lerp(from.2, to.2, t),
    )
}

// ---------------------------------------------------------------------------
// Inactivity fade
// ---------------------------------------------------------------------------

/// How long (seconds) before inactivity fade begins.
pub const INACTIVE_START_SECS: f64 = 3600.0; // 1 hour
/// How long (seconds) until inactivity fade bottoms out.
pub const INACTIVE_END_SECS: f64 = 86400.0; // 24 hours
/// Minimum alpha / brightness multiplier at full inactivity.
pub const INACTIVE_MIN_ALPHA: f64 = 0.55;

/// Returns an "inactivity factor" in `[0.0, 1.0]` based on how long since
/// `updated_at`.  0.0 = active (≤ 1 h), 1.0 = fully inactive (≥ 24 h),
/// linearly interpolated in between.
pub fn inactivity_factor(
    updated_at: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> f64 {
    let idle_secs = now.signed_duration_since(updated_at).num_seconds().max(0) as f64;
    ((idle_secs - INACTIVE_START_SECS) / (INACTIVE_END_SECS - INACTIVE_START_SECS)).clamp(0.0, 1.0)
}

/// Maps an inactivity factor to an alpha / brightness multiplier.
/// factor = 0.0 → 1.0  (fully visible)
/// factor = 1.0 → `INACTIVE_MIN_ALPHA`  (heavily faded)
pub fn inactivity_alpha(factor: f64) -> f64 {
    1.0 - factor * (1.0 - INACTIVE_MIN_ALPHA)
}

// ---------------------------------------------------------------------------
// Animation timing constants and helpers
// ---------------------------------------------------------------------------

/// Animation timing parameters (all in seconds unless noted).
pub mod anim {
    /// Period for the breathing / pulse animation.
    pub const BREATHING_PERIOD: f64 = 1.5;
    /// Period for fast-blink indicators (e.g. AwaitingApproval dot).
    pub const FAST_BLINK_PERIOD: f64 = 0.5;
    /// Period for slow opacity fade.
    pub const SLOW_FADE_PERIOD: f64 = 3.0;
    /// Period for glow ring animation.
    pub const GLOW_PERIOD: f64 = 2.0;
    /// Period for context-bar pulse.
    pub const CONTEXT_PULSE_PERIOD: f64 = 1.0;
    /// Ripple decay half-life.
    pub const RIPPLE_DECAY: f64 = 0.3;
    /// Card appear animation duration.
    pub const CARD_APPEAR: f64 = 0.2;
    /// Card disappear animation duration.
    pub const CARD_DISAPPEAR: f64 = 0.15;
    /// Delay between each character in typewriter animations.
    pub const TYPEWRITER_DELAY: f64 = 0.02;
    /// Status-bar blink period.
    pub const STATUSBAR_BLINK_PERIOD: f64 = 1.0;
    /// TUI tick interval in milliseconds.
    pub const TUI_TICK_MS: u64 = 200;
}

/// Breathing pulse: returns a value in `[0.4, 1.0]` that oscillates
/// sinusoidally with period `BREATHING_PERIOD`.
pub fn breathing_pulse(elapsed: f64) -> f64 {
    let phase = (elapsed / anim::BREATHING_PERIOD) * std::f64::consts::TAU;
    0.7 + 0.3 * phase.sin() // range: 0.4 to 1.0
}

/// Fast blink: returns `1.0` or `0.2` alternating each half `FAST_BLINK_PERIOD`.
pub fn fast_blink(elapsed: f64) -> f64 {
    let phase = elapsed % anim::FAST_BLINK_PERIOD;
    if phase < anim::FAST_BLINK_PERIOD * 0.5 {
        1.0
    } else {
        0.2
    }
}

/// Slow fade: returns a value in `[0.6, 1.0]` with period `SLOW_FADE_PERIOD`.
pub fn slow_fade(elapsed: f64) -> f64 {
    let phase = (elapsed / anim::SLOW_FADE_PERIOD) * std::f64::consts::TAU;
    0.8 + 0.2 * phase.sin() // range: 0.6 to 1.0
}

// ---------------------------------------------------------------------------
// Window layout constants
// ---------------------------------------------------------------------------

/// Layout constants for the macOS session monitor window.
pub mod window_layout {
    /// Default window width in points.
    pub const WIDTH: f64 = 680.0;
    /// Minimum window height in points.
    pub const MIN_HEIGHT: f64 = 140.0;
    /// Height of each session card.
    pub const CARD_HEIGHT: f64 = 64.0;
    /// Vertical spacing between cards.
    pub const CARD_SPACING: f64 = 4.0;
    /// Corner radius of each card.
    pub const CARD_CORNER_RADIUS: f64 = 8.0;
    /// Width of the accent color bar on the left edge of a card.
    pub const CARD_ACCENT_BAR_WIDTH: f64 = 3.0;
    /// Height of the window header.
    pub const HEADER_HEIGHT: f64 = 28.0;
    /// Height of the window footer.
    pub const FOOTER_HEIGHT: f64 = 28.0;
    /// Primary font size in points.
    pub const FONT_SIZE: f64 = 11.5;
    /// Small / secondary font size in points.
    pub const FONT_SIZE_SMALL: f64 = 10.0;
    /// Diameter of status indicator dots.
    pub const DOT_SIZE: f64 = 8.0;
    /// Grid dot spacing.
    pub const GRID_SPACING: f64 = 20.0;
    /// Radius of each grid dot.
    pub const GRID_DOT_RADIUS: f64 = 1.0;
}

// ---------------------------------------------------------------------------
// Notification layout constants
// ---------------------------------------------------------------------------

/// Layout constants for the macOS notification overlay window.
pub mod notif_layout {
    /// Notification window width in points.
    pub const WIDTH: f64 = 340.0;
    /// Minimum height in points.
    pub const MIN_HEIGHT: f64 = 68.0;
    /// Maximum height in points (for multi-line messages).
    pub const MAX_HEIGHT: f64 = 320.0;
    /// Corner radius in points.
    pub const CORNER_RADIUS: f64 = 12.0;
    /// Internal padding.
    pub const PADDING: f64 = 14.0;
    /// Width of the accent bar.
    pub const ACCENT_BAR_WIDTH: f64 = 3.0;
    /// Default window opacity.
    pub const DEFAULT_OPACITY: f64 = 0.92;
    /// Background color as CSS hex.
    pub const BG_HEX: &str = "#1a1a2e";
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- AgentType ---

    #[test]
    fn test_agent_type_from_model_claude() {
        assert_eq!(
            AgentType::from_model(Some("claude-3-5-sonnet-20241022")),
            AgentType::Claude
        );
    }

    #[test]
    fn test_agent_type_from_model_codex() {
        assert_eq!(
            AgentType::from_model(Some("codex-mini-latest")),
            AgentType::Codex
        );
    }

    #[test]
    fn test_agent_type_from_model_gpt() {
        assert_eq!(AgentType::from_model(Some("gpt-4o")), AgentType::Codex);
    }

    #[test]
    fn test_agent_type_from_model_gemini() {
        assert_eq!(
            AgentType::from_model(Some("gemini-2.0-flash")),
            AgentType::Gemini
        );
    }

    #[test]
    fn test_agent_type_from_model_none() {
        assert_eq!(AgentType::from_model(None), AgentType::Unknown);
    }

    #[test]
    fn test_agent_type_from_model_unknown_string() {
        assert_eq!(
            AgentType::from_model(Some("some-future-model")),
            AgentType::Unknown
        );
    }

    // --- model_short_label ---

    #[test]
    fn test_model_short_label_opus_variants() {
        assert_eq!(
            model_short_label("claude-opus-4-8"),
            Some("Opus 4.8".to_string())
        );
        assert_eq!(
            model_short_label("claude-opus-4-7-20241022"),
            Some("Opus 4.7".to_string())
        );
        assert_eq!(
            model_short_label("claude-opus-4-20240101"),
            Some("Opus 4".to_string())
        );
    }

    #[test]
    fn test_model_short_label_sonnet_variants() {
        assert_eq!(
            model_short_label("claude-sonnet-4-5-20250101"),
            Some("Sonnet 4.5".to_string())
        );
        assert_eq!(
            model_short_label("claude-sonnet-4-20250101"),
            Some("Sonnet 4".to_string())
        );
        assert_eq!(
            model_short_label("claude-3-5-sonnet-20241022"),
            Some("Sonnet 3.5".to_string())
        );
    }

    #[test]
    fn test_model_short_label_haiku() {
        assert_eq!(
            model_short_label("claude-haiku-4-5-20251001"),
            Some("Haiku 4.5".to_string())
        );
        assert_eq!(
            model_short_label("claude-haiku-3"),
            Some("Haiku 3".to_string())
        );
        // Legacy ordering puts the version before the family; no version is
        // parsed there, so it falls through to the enumerated rules.
        assert_eq!(
            model_short_label("claude-3-haiku-20240307"),
            Some("Haiku".to_string())
        );
    }

    #[test]
    fn test_model_short_label_fable() {
        assert_eq!(
            model_short_label("claude-fable-5"),
            Some("Fable 5".to_string())
        );
        assert_eq!(model_short_label("claude-fable"), Some("Fable".to_string()));
    }

    #[test]
    fn test_model_short_label_sonnet_5() {
        assert_eq!(
            model_short_label("claude-sonnet-5"),
            Some("Sonnet 5".to_string())
        );
    }

    #[test]
    fn test_model_short_label_opus_5() {
        assert_eq!(
            model_short_label("claude-opus-5"),
            Some("Opus 5".to_string())
        );
    }

    #[test]
    fn test_model_short_label_strips_context_suffix() {
        assert_eq!(
            model_short_label("claude-opus-5[1m]"),
            Some("Opus 5".to_string())
        );
        assert_eq!(
            model_short_label("claude-opus-4-8[1m]"),
            Some("Opus 4.8".to_string())
        );
    }

    /// Versions are parsed, not enumerated, so releases that postdate this
    /// code still render with a version instead of degrading to the family.
    #[test]
    fn test_model_short_label_unenumerated_versions() {
        assert_eq!(
            model_short_label("claude-opus-4-6"),
            Some("Opus 4.6".to_string())
        );
        assert_eq!(
            model_short_label("claude-sonnet-4-6"),
            Some("Sonnet 4.6".to_string())
        );
        assert_eq!(
            model_short_label("claude-opus-4-5-20251101"),
            Some("Opus 4.5".to_string())
        );
        assert_eq!(
            model_short_label("claude-opus-9-3"),
            Some("Opus 9.3".to_string())
        );
    }

    #[test]
    fn test_model_short_label_gpt() {
        assert_eq!(model_short_label("gpt-5-turbo"), Some("GPT-5".to_string()));
        assert_eq!(model_short_label("gpt-4o"), Some("GPT-4o".to_string()));
        assert_eq!(model_short_label("gpt-4"), Some("GPT-4".to_string()));
    }

    #[test]
    fn test_model_short_label_gpt_minor_versions() {
        assert_eq!(
            model_short_label("gpt-5.6-sol"),
            Some("GPT-5.6".to_string())
        );
        assert_eq!(model_short_label("gpt-5-codex"), Some("GPT-5".to_string()));
        assert_eq!(model_short_label("gpt-4o-mini"), Some("GPT-4o".to_string()));
        assert_eq!(
            model_short_label("gpt-3.5-turbo"),
            Some("GPT-3.5".to_string())
        );
    }

    #[test]
    fn test_model_short_label_gpt_without_version() {
        // The segment after "gpt-" is not a version; fall back to the family.
        assert_eq!(model_short_label("gpt-oss-120b"), Some("GPT".to_string()));
    }

    #[test]
    fn test_model_short_label_codex() {
        assert_eq!(
            model_short_label("codex-mini-latest"),
            Some("Codex mini".to_string())
        );
        assert_eq!(
            model_short_label("codex-experimental"),
            Some("Codex".to_string())
        );
    }

    #[test]
    fn test_model_short_label_gemini() {
        assert_eq!(
            model_short_label("gemini-2.5-pro"),
            Some("Gemini 2.5".to_string())
        );
        assert_eq!(
            model_short_label("gemini-2.0-flash"),
            Some("Gemini 2.0".to_string())
        );
        assert_eq!(
            model_short_label("gemini-1.5-pro"),
            Some("Gemini 1.5".to_string())
        );
        assert_eq!(model_short_label("gemini-nano"), Some("Gemini".to_string()));
    }

    #[test]
    fn test_model_short_label_case_insensitive() {
        assert_eq!(
            model_short_label("Claude-Opus-4-7-20241022"),
            Some("Opus 4.7".to_string())
        );
    }

    #[test]
    fn test_model_short_label_unknown() {
        assert_eq!(model_short_label("some-future-model-x"), None);
        assert_eq!(model_short_label(""), None);
    }

    #[test]
    fn test_agent_type_accent_rgb_claude() {
        let (r, g, b) = AgentType::Claude.accent_rgb();
        assert_eq!((r, g, b), (0xd9, 0x77, 0x57));
    }

    #[test]
    fn test_agent_type_accent_hex() {
        assert_eq!(AgentType::Claude.accent_hex(), "#d97757");
        assert_eq!(AgentType::Codex.accent_hex(), "#22c55e");
        assert_eq!(AgentType::Gemini.accent_hex(), "#3b82f6");
        assert_eq!(AgentType::Unknown.accent_hex(), "#a855f7");
    }

    #[test]
    fn test_agent_type_accent_f64_range() {
        for agent in [
            AgentType::Claude,
            AgentType::Codex,
            AgentType::Gemini,
            AgentType::Unknown,
        ] {
            let (r, g, b) = agent.accent_f64();
            assert!((0.0..=1.0).contains(&r));
            assert!((0.0..=1.0).contains(&g));
            assert!((0.0..=1.0).contains(&b));
        }
    }

    #[test]
    fn test_agent_type_accent_f64_consistency() {
        // f64 values should be consistent with rgb() / 255
        let agent = AgentType::Claude;
        let (r8, g8, b8) = agent.accent_rgb();
        let (rf, gf, bf) = agent.accent_f64();
        assert!((rf - r8 as f64 / 255.0).abs() < 1e-10);
        assert!((gf - g8 as f64 / 255.0).abs() < 1e-10);
        assert!((bf - b8 as f64 / 255.0).abs() < 1e-10);
    }

    // --- StatusColor ---

    #[test]
    fn test_status_color_rgb() {
        assert_eq!(StatusColor::Running.rgb(), (0x22, 0xc5, 0x5e));
        assert_eq!(StatusColor::AwaitingApproval.rgb(), (0xef, 0x44, 0x44));
    }

    #[test]
    fn test_status_color_hex() {
        assert_eq!(StatusColor::Running.hex(), "#22c55e");
        assert_eq!(StatusColor::AwaitingApproval.hex(), "#ef4444");
        assert_eq!(StatusColor::WaitingInput.hex(), "#f59e0b");
        assert_eq!(StatusColor::Stopped.hex(), "#475569");
    }

    #[test]
    fn test_status_color_f64_range() {
        for sc in [
            StatusColor::Running,
            StatusColor::AwaitingApproval,
            StatusColor::WaitingInput,
            StatusColor::Stopped,
        ] {
            let (r, g, b) = sc.f64();
            assert!((0.0..=1.0).contains(&r));
            assert!((0.0..=1.0).contains(&g));
            assert!((0.0..=1.0).contains(&b));
        }
    }

    // --- context_gauge_rgb ---

    #[test]
    fn test_context_gauge_rgb_zero() {
        let (r, g, b) = context_gauge_rgb(0.0);
        assert_eq!((r, g, b), (0x22, 0xc5, 0x5e)); // green
    }

    #[test]
    fn test_context_gauge_rgb_half() {
        let (r, g, b) = context_gauge_rgb(0.5);
        assert_eq!((r, g, b), (0xea, 0xb3, 0x08)); // yellow
    }

    #[test]
    fn test_context_gauge_rgb_full() {
        let (r, g, b) = context_gauge_rgb(1.0);
        assert_eq!((r, g, b), (0xef, 0x44, 0x44)); // red
    }

    #[test]
    fn test_context_gauge_rgb_clamp() {
        // Values outside [0,1] should clamp without panic
        let _ = context_gauge_rgb(-0.5);
        let _ = context_gauge_rgb(1.5);
    }

    // --- Animation functions ---

    #[test]
    fn test_breathing_pulse_range() {
        for i in 0..100 {
            let t = i as f64 * 0.05;
            let v = breathing_pulse(t);
            assert!((0.39..=1.01).contains(&v), "breathing_pulse({t}) = {v}");
        }
    }

    #[test]
    fn test_breathing_pulse_midpoint() {
        let v = breathing_pulse(0.0);
        assert!((v - 0.7).abs() < 0.01); // midpoint at t=0
    }

    #[test]
    fn test_fast_blink_values() {
        assert!((fast_blink(0.0) - 1.0).abs() < 0.01);
        assert!((fast_blink(0.5) - 1.0).abs() < 0.01); // next period start
        let past = anim::FAST_BLINK_PERIOD * 0.5 + 0.001;
        assert!((fast_blink(past) - 0.2).abs() < 0.01);
    }

    #[test]
    fn test_slow_fade_range() {
        for i in 0..100 {
            let t = i as f64 * 0.1;
            let v = slow_fade(t);
            assert!((0.59..=1.01).contains(&v), "slow_fade({t}) = {v}");
        }
    }

    // --- window_layout ---

    #[test]
    fn test_window_layout_positive() {
        const {
            assert!(window_layout::WIDTH > 0.0);
            assert!(window_layout::CARD_HEIGHT > 0.0);
            assert!(window_layout::HEADER_HEIGHT > 0.0);
            assert!(window_layout::FOOTER_HEIGHT > 0.0);
        }
    }

    // --- notif_layout ---

    #[test]
    fn test_notif_layout_bounds() {
        const {
            assert!(notif_layout::MIN_HEIGHT < notif_layout::MAX_HEIGHT);
            assert!(notif_layout::DEFAULT_OPACITY > 0.0);
            assert!(notif_layout::DEFAULT_OPACITY <= 1.0);
        }
        assert_eq!(notif_layout::BG_HEX, "#1a1a2e");
    }

    // --- palette ---

    #[test]
    fn test_palette_border_alpha() {
        const {
            assert!(palette::BORDER_ALPHA > 0.0);
            assert!(palette::BORDER_ALPHA < 1.0);
        }
    }

    // --- inactivity ---

    #[test]
    fn test_inactivity_factor_before_start() {
        let now = chrono::Utc::now();
        let updated = now - chrono::Duration::minutes(30);
        assert_eq!(inactivity_factor(updated, now), 0.0);
    }

    #[test]
    fn test_inactivity_factor_at_start() {
        let now = chrono::Utc::now();
        let updated = now - chrono::Duration::hours(1);
        assert!((inactivity_factor(updated, now) - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_inactivity_factor_midpoint() {
        let now = chrono::Utc::now();
        // midpoint = (1h + 24h) / 2 = 12.5h = 45000s
        let updated = now - chrono::Duration::seconds(45000);
        assert!((inactivity_factor(updated, now) - 0.5).abs() < 0.01);
    }

    #[test]
    fn test_inactivity_factor_at_end() {
        let now = chrono::Utc::now();
        let updated = now - chrono::Duration::hours(24);
        assert!((inactivity_factor(updated, now) - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_inactivity_factor_beyond_end() {
        let now = chrono::Utc::now();
        let updated = now - chrono::Duration::hours(24);
        assert_eq!(inactivity_factor(updated, now), 1.0);
    }

    #[test]
    fn test_inactivity_factor_future_clamps() {
        let now = chrono::Utc::now();
        let updated = now + chrono::Duration::minutes(5);
        assert_eq!(inactivity_factor(updated, now), 0.0);
    }

    #[test]
    fn test_inactivity_alpha_active() {
        assert!((inactivity_alpha(0.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_inactivity_alpha_full_inactive() {
        assert!((inactivity_alpha(1.0) - INACTIVE_MIN_ALPHA).abs() < 1e-10);
    }

    #[test]
    fn test_inactivity_alpha_midpoint() {
        let mid = inactivity_alpha(0.5);
        assert!(mid > INACTIVE_MIN_ALPHA && mid < 1.0);
        // Should be 1.0 - 0.5 * 0.45 = 0.775
        assert!((mid - 0.775).abs() < 1e-10);
    }
}
