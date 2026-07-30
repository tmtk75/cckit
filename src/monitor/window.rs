// macOS window app for session monitoring

use crate::monitor::display;
use crate::monitor::focus;
use crate::monitor::session::{Session, SessionStatus};
use crate::monitor::storage::Storage;
use crate::monitor::theme::{self, AgentType, StatusColor, anim, palette, window_layout};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{AnyThread, ClassType, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSBezierPath, NSColor, NSEvent, NSFont, NSGraphicsContext, NSImage, NSMenu, NSMenuItem,
    NSScreen, NSTextField, NSTrackingArea, NSTrackingAreaOptions, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString, NSTimer};

use crate::monitor::window_hover::{
    ClassicLayout, HoverEvent, HoverPopover, HoverTracker, MissionControlLayout, TranscriptCache,
    hit_test_classic, hit_test_mission_control,
};

type CGFloat = f64;

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Mutex, Once, OnceLock};
use std::time::Instant;

// --- Theme ---

#[derive(Debug, Clone, Copy, Default, PartialEq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
enum WindowThemeId {
    Classic,
    #[default]
    MissionControl,
    Notch,
}

static CURRENT_THEME: Mutex<WindowThemeId> = Mutex::new(WindowThemeId::MissionControl);
static THEME_LABEL_PTR: Mutex<Option<usize>> = Mutex::new(None);

// --- Config ---

#[derive(serde::Deserialize, Clone)]
#[serde(default)]
struct WindowConfig {
    background_opacity: f64,
    theme: WindowThemeId,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            background_opacity: 0.5,
            theme: WindowThemeId::MissionControl,
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("cckit/window.toml")
}

fn load_config() -> WindowConfig {
    let path = config_path();
    if path.exists() {
        let content = std::fs::read_to_string(&path).unwrap_or_default();
        toml::from_str(&content).unwrap_or_default()
    } else {
        // Create default config file
        let dir = path.parent().unwrap();
        let _ = std::fs::create_dir_all(dir);
        let default = "# cckit window configuration\n\
                        # Reload: Cmd+Shift+,   Open: Cmd+,\n\
                        \n\
                        # Theme: \"classic\" or \"mission-control\"\n\
                        theme = \"mission-control\"\n\
                        \n\
                        # Background opacity (0.0 = fully transparent, 1.0 = opaque)\n\
                        background_opacity = 0.5\n";
        let _ = std::fs::write(&path, default);
        WindowConfig::default()
    }
}

static WINDOW_CONFIG: Mutex<Option<WindowConfig>> = Mutex::new(None);
static EFFECT_VIEW_PTR: Mutex<Option<usize>> = Mutex::new(None);

fn apply_config() {
    let config = WINDOW_CONFIG.lock().unwrap();
    let opacity = config.as_ref().map(|c| c.background_opacity).unwrap_or(0.5);
    drop(config);

    let ptr = *EFFECT_VIEW_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let view = ptr as *mut AnyObject;
        let _: () = unsafe { msg_send![view, setAlphaValue: opacity] };
    }
}

fn open_config_file() {
    let path = config_path();
    let _ = std::process::Command::new("open")
        .arg("-t")
        .arg(&path)
        .spawn();
}

fn reload_config() {
    let config = load_config();
    *CURRENT_THEME.lock().unwrap() = config.theme;
    *WINDOW_CONFIG.lock().unwrap() = Some(config);
    apply_config();
    update_theme_label();
    fit_window_to_content();
}

fn update_theme_label() {
    let ptr = *THEME_LABEL_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let theme = *CURRENT_THEME.lock().unwrap();
        let text = match theme {
            WindowThemeId::Classic => "classic",
            WindowThemeId::MissionControl => "mission control",
            WindowThemeId::Notch => "notch",
        };
        let label = ptr as *mut AnyObject;
        unsafe {
            let ns_str = NSString::from_str(text);
            let _: () = msg_send![label, setStringValue: &*ns_str];
        }
    }
}

// --- Animation state ---

static ANIMATION_START: OnceLock<Instant> = OnceLock::new();

fn elapsed_secs() -> f64 {
    let start = ANIMATION_START.get_or_init(Instant::now);
    start.elapsed().as_secs_f64()
}

// --- Layout constants (theme-based) ---

const WINDOW_WIDTH: CGFloat = window_layout::WIDTH;
const MIN_WINDOW_HEIGHT: CGFloat = window_layout::MIN_HEIGHT;
const CARD_HEIGHT: CGFloat = 38.0; // compact 2-row card
const CARD_SPACING: CGFloat = 2.0;
const CARD_CORNER_RADIUS: CGFloat = 6.0;
const HEADER_HEIGHT: CGFloat = window_layout::HEADER_HEIGHT;
const FOOTER_HEIGHT: CGFloat = window_layout::FOOTER_HEIGHT;
const FONT_SIZE: CGFloat = window_layout::FONT_SIZE;
const FONT_SIZE_SMALL: CGFloat = window_layout::FONT_SIZE_SMALL;
const DOT_SIZE: CGFloat = 6.0;
const GRID_SPACING: CGFloat = window_layout::GRID_SPACING;
const GRID_DOT_RADIUS: CGFloat = window_layout::GRID_DOT_RADIUS;
const LEFT_PAD: CGFloat = 8.0;
const CARD_CONTENT_LEFT: CGFloat = 8.0; // no accent bar
const FIT_MIN_WIDTH: CGFloat = 640.0;
const FIT_MAX_WIDTH: CGFloat = 840.0;
const FIT_PATH_CHAR_WIDTH: CGFloat = 6.0;
const FIT_PATH_BASE_CHARS: usize = 18;
const FIT_PATH_MAX_CHARS: usize = 34;

// --- Colors (theme-based) ---

fn rgb_to_f64(c: (u8, u8, u8)) -> (f64, f64, f64) {
    (c.0 as f64 / 255.0, c.1 as f64 / 255.0, c.2 as f64 / 255.0)
}

#[allow(dead_code)]
fn color_bg() -> Retained<NSColor> {
    let (r, g, b) = rgb_to_f64(palette::BG);
    NSColor::colorWithRed_green_blue_alpha(r, g, b, 1.0)
}

fn color_surface() -> Retained<NSColor> {
    let config = WINDOW_CONFIG.lock().unwrap();
    let opacity = config.as_ref().map(|c| c.background_opacity).unwrap_or(0.5);
    drop(config);
    let alpha = (opacity - 0.05).max(0.0);
    let (r, g, b) = rgb_to_f64(palette::SURFACE);
    NSColor::colorWithRed_green_blue_alpha(r, g, b, alpha)
}

fn color_text() -> Retained<NSColor> {
    let (r, g, b) = rgb_to_f64(palette::TEXT);
    NSColor::colorWithRed_green_blue_alpha(r, g, b, 1.0)
}

fn color_dim() -> Retained<NSColor> {
    let (r, g, b) = rgb_to_f64(palette::TEXT_DIM);
    NSColor::colorWithRed_green_blue_alpha(r, g, b, 1.0)
}

fn color_border() -> Retained<NSColor> {
    NSColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, palette::BORDER_ALPHA)
}

fn color_selection() -> Retained<NSColor> {
    NSColor::colorWithRed_green_blue_alpha(0.145, 0.388, 0.922, 0.25) // #2563EB @ 25%
}

fn status_color(status: &SessionStatus) -> Retained<NSColor> {
    let sc = match status {
        SessionStatus::Running => StatusColor::Running,
        SessionStatus::AwaitingApproval => StatusColor::AwaitingApproval,
        SessionStatus::WaitingInput => StatusColor::WaitingInput,
        SessionStatus::Stopped => StatusColor::Stopped,
    };
    let (r, g, b) = sc.f64();
    NSColor::colorWithRed_green_blue_alpha(r, g, b, 1.0)
}

#[allow(dead_code)]
fn agent_accent_color(agent: AgentType) -> Retained<NSColor> {
    let (r, g, b) = agent.accent_f64();
    NSColor::colorWithRed_green_blue_alpha(r, g, b, 1.0)
}

fn agent_accent_color_alpha(agent: AgentType, alpha: f64) -> Retained<NSColor> {
    let (r, g, b) = agent.accent_f64();
    NSColor::colorWithRed_green_blue_alpha(r, g, b, alpha)
}

// --- Data ---

static SESSION_LIST: Mutex<Vec<Session>> = Mutex::new(Vec::new());
static SELECTED_INDEX: Mutex<Option<usize>> = Mutex::new(None);
static CONTENT_VIEW_PTR: Mutex<Option<usize>> = Mutex::new(None);
static WINDOW_PTR: Mutex<Option<usize>> = Mutex::new(None);
static AF_LABEL_PTR: Mutex<Option<usize>> = Mutex::new(None);

// --- Hover popover runtime (main-thread-only, lazy-initialized) ---

struct HoverRuntime {
    tracker: HoverTracker,
    cache: TranscriptCache,
    popover: HoverPopover,
    pending_timer_version: Option<u64>,
}

thread_local! {
    static HOVER_RUNTIME: RefCell<Option<HoverRuntime>> = const { RefCell::new(None) };
}

fn with_hover_runtime<R>(f: impl FnOnce(&mut HoverRuntime) -> R) -> R {
    HOVER_RUNTIME.with(|cell| {
        let mut borrowed = cell.borrow_mut();
        if borrowed.is_none() {
            *borrowed = Some(HoverRuntime {
                tracker: HoverTracker::new(),
                cache: TranscriptCache::new(),
                popover: HoverPopover::new(),
                pending_timer_version: None,
            });
        }
        f(borrowed.as_mut().unwrap())
    })
}
static NOTIFIED_APPROVALS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Per-project auto-focus disabled set (cwd paths). Projects in this set won't trigger auto-focus.
pub static AF_DISABLED_PROJECTS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

static LAST_SYNC: Mutex<Option<Instant>> = Mutex::new(None);
static REDRAW_NEEDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static SESSION_FINGERPRINT: Mutex<u64> = Mutex::new(0);

fn load_sessions() {
    let storage = Storage::new();

    // Throttle sync_sessions (spawns ps per session) to once every 10 seconds
    let should_sync = {
        let mut last = LAST_SYNC.lock().unwrap();
        let now_inst = Instant::now();
        match *last {
            Some(prev) if now_inst.duration_since(prev).as_secs() < 10 => false,
            _ => {
                *last = Some(now_inst);
                true
            }
        }
    };
    if should_sync {
        let _ = storage.sync_sessions();
    }

    let store = storage.load();
    let mut sessions: Vec<Session> = store.sessions.into_values().collect();
    let now = chrono::Utc::now();
    for session in &mut sessions {
        if let Some(ref tp) = session.transcript_path {
            // Only re-read transcript if session was recently updated or has no cached data
            let recently_updated = now.signed_duration_since(session.updated_at).num_seconds() < 30;
            if recently_updated || session.context_used_tokens.is_none() {
                let ctx = crate::monitor::hook::read_context_usage(tp);
                // The stored model came from the hook payload and keeps the
                // "[1m]" suffix, so it outranks the transcript's stripped ID.
                let max = crate::monitor::hook::resolve_max_tokens(
                    session.model.as_deref(),
                    ctx.model.as_deref(),
                );
                session.context_used_tokens = ctx.used_tokens;
                session.context_max_tokens = max;
                if ctx.model.is_some()
                    && !session.model.as_deref().is_some_and(|m| m.contains("[1m]"))
                {
                    session.model = ctx.model;
                }
            }
            if session.subagent_name.is_none() && session.is_subagent() {
                session.subagent_name = crate::monitor::hook::extract_subagent_name(tp);
            }
        }
        // Subagents don't receive Stop events; mark as stopped if stale
        if session.is_subagent() && session.status == SessionStatus::Running {
            let idle_secs = now.signed_duration_since(session.updated_at).num_seconds();
            if idle_secs > 180 {
                session.status = SessionStatus::Stopped;
            }
        }
    }
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    // Compute a fingerprint to detect meaningful changes
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    sessions.len().hash(&mut hasher);
    for s in &sessions {
        s.session_id.hash(&mut hasher);
        std::mem::discriminant(&s.status).hash(&mut hasher);
        s.updated_at.timestamp().hash(&mut hasher);
        s.prompt_count.hash(&mut hasher);
        s.tool_count.hash(&mut hasher);
        s.context_used_tokens.hash(&mut hasher);
    }
    let new_fp = hasher.finish();
    let mut fp = SESSION_FINGERPRINT.lock().unwrap();
    if *fp != new_fp {
        *fp = new_fp;
        REDRAW_NEEDED.store(true, std::sync::atomic::Ordering::Relaxed);
    }

    *SESSION_LIST.lock().unwrap() = sessions;
}

fn format_elapsed(dt: chrono::DateTime<chrono::Utc>) -> String {
    display::format_elapsed_short(dt)
}

fn format_session_stats(session: &Session) -> String {
    let mut parts = display::session_count_parts(session);
    let dur = format_tool_duration(session);
    if !dur.is_empty() {
        parts.push(dur);
    }
    if parts.is_empty() {
        String::new()
    } else {
        parts.join("/")
    }
}

struct ContextBarInfo {
    ratio: f64,
    label: String,
}

/// Extract context window usage info for rendering
fn context_bar_info(session: &Session) -> Option<ContextBarInfo> {
    let used = session.context_used_tokens?;
    let max = session.context_max()?;
    if max == 0 {
        return None;
    }
    let ratio = (used as f64 / max as f64).clamp(0.0, 1.0);
    let pct = (ratio * 100.0) as u32;

    Some(ContextBarInfo {
        ratio,
        label: format!("{}%", pct),
    })
}

/// Color for context bar: green (low) → yellow (mid) → red (high)
fn context_bar_color(ratio: f64) -> Retained<NSColor> {
    if ratio < 0.5 {
        // green → yellow (0.0..0.5)
        let t = ratio / 0.5;
        NSColor::colorWithRed_green_blue_alpha(t, 0.8, 0.2 * (1.0 - t), 0.9)
    } else {
        // yellow → red (0.5..1.0)
        let t = (ratio - 0.5) / 0.5;
        NSColor::colorWithRed_green_blue_alpha(0.9, 0.8 * (1.0 - t), 0.0, 0.9)
    }
}

fn format_tool_duration(session: &Session) -> String {
    // Show live elapsed time if tool is currently running
    if let Some(started) = session.tool_started_at
        && session.status == SessionStatus::AwaitingApproval
    {
        let ms = chrono::Utc::now()
            .signed_duration_since(started)
            .num_milliseconds()
            .max(0);
        return format_duration_ms(ms);
    }
    // Otherwise show last completed tool duration
    match session.last_tool_duration_ms {
        Some(ms) => format_duration_ms(ms),
        None => String::new(),
    }
}

fn format_duration_ms(ms: i64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{:.0}m", ms as f64 / 60_000.0)
    }
}

fn calculate_fit_window_width() -> CGFloat {
    let sessions = SESSION_LIST.lock().unwrap();
    let longest_path_chars = sessions
        .iter()
        .map(|session| session.short_cwd().chars().count())
        .max()
        .unwrap_or(FIT_PATH_BASE_CHARS)
        .clamp(FIT_PATH_BASE_CHARS, FIT_PATH_MAX_CHARS);
    let extra_chars = longest_path_chars.saturating_sub(FIT_PATH_BASE_CHARS) as CGFloat;

    let theme = *CURRENT_THEME.lock().unwrap();
    match theme {
        WindowThemeId::Notch => {
            // Width scales with session count: ~80px per session chip, min 300
            let count = sessions.len().max(1) as CGFloat;
            (count * 80.0 + 40.0).clamp(300.0, FIT_MAX_WIDTH)
        }
        _ => {
            (FIT_MIN_WIDTH + extra_chars * FIT_PATH_CHAR_WIDTH).clamp(FIT_MIN_WIDTH, FIT_MAX_WIDTH)
        }
    }
}

#[allow(dead_code)]
fn status_label(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "run",
        SessionStatus::AwaitingApproval => "tool",
        SessionStatus::WaitingInput => "wait",
        SessionStatus::Stopped => "done",
    }
}

fn focus_selected() {
    let sessions = SESSION_LIST.lock().unwrap();
    let index = match *SELECTED_INDEX.lock().unwrap() {
        Some(i) => i,
        None => return,
    };

    if let Some(session) = sessions.get(index) {
        let tty = session.tty.clone();
        let project = session.project_name().to_string();
        let session_id = session.session_id.clone();
        let cwd = session.cwd.clone();
        drop(sessions);

        // Skip focus for sessions without a known TTY (e.g. Codex Desktop)
        if tty == "unknown" {
            eprintln!(
                "[cckit] focus skipped: tty=unknown session={} cwd={}",
                session_id, cwd
            );
            return;
        }

        match focus::focus_ghostty_tab_by_tty(&tty) {
            Ok(true) => {}
            _ => {
                let _ = focus::focus_ghostty_tab(&project);
            }
        }
    }
}

// --- View helpers ---

fn cached_mono_font(size: CGFloat) -> Retained<NSFont> {
    use std::collections::HashMap;
    thread_local! {
        static CACHE: RefCell<HashMap<u64, Retained<NSFont>>> = RefCell::new(HashMap::new());
    }
    CACHE.with(|cache| {
        let key = size.to_bits();
        let mut map = cache.borrow_mut();
        map.entry(key)
            .or_insert_with(|| NSFont::monospacedSystemFontOfSize_weight(size, 0.0))
            .clone()
    })
}

fn create_mono_label(
    mtm: MainThreadMarker,
    text: &str,
    rect: NSRect,
    text_color: &NSColor,
    size: CGFloat,
) -> Retained<NSTextField> {
    let label = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect);
    label.setStringValue(&NSString::from_str(text));
    let font = cached_mono_font(size);
    label.setFont(Some(&font));
    label.setTextColor(Some(text_color));
    label.setBordered(false);
    label.setEditable(false);
    label.setDrawsBackground(false);
    label
}

fn create_colored_view(
    mtm: MainThreadMarker,
    rect: NSRect,
    color: &NSColor,
    corner_radius: CGFloat,
) -> Retained<NSView> {
    let v = NSView::initWithFrame(NSView::alloc(mtm), rect);
    v.setWantsLayer(true);
    if let Some(layer) = v.layer() {
        layer.setBackgroundColor(Some(&color.CGColor()));
        if corner_radius > 0.0 {
            let _: () = unsafe { msg_send![&*layer, setCornerRadius: corner_radius] };
        }
    }
    v
}

// --- Custom NSView subclass ---

static REGISTER_VIEW_CLASS: Once = Once::new();
static mut VIEW_CLASS: Option<&'static AnyClass> = None;

extern "C" fn accepts_first_responder(_this: *mut AnyObject, _sel: Sel) -> Bool {
    Bool::YES
}

extern "C" fn key_down(_this: *mut AnyObject, _sel: Sel, event: *mut AnyObject) {
    let event: &NSEvent = unsafe { &*(event as *const NSEvent) };
    let key_code = event.keyCode();
    let chars = event.charactersIgnoringModifiers();
    let char_str = chars.map(|s| s.to_string()).unwrap_or_default();

    // Modifier flags: Command=0x100000, Shift=0x020000
    let raw_flags: usize = unsafe { msg_send![event, modifierFlags] };
    let cmd = raw_flags & 0x100000 != 0;
    let shift = raw_flags & 0x020000 != 0;

    // Cmd+Shift+, → reload config
    if cmd && shift && key_code == 43 {
        reload_config();
        return;
    }
    // Cmd+, → open config file
    if cmd && key_code == 43 {
        open_config_file();
        return;
    }
    // Cmd+0 → fit window height to content
    if cmd && char_str == "0" {
        fit_window_to_content();
        return;
    }
    // Cmd+1-9 → move window (numpad layout: 7=top-left, 8=top-center, 9=top-right, ...)
    if cmd && char_str.len() == 1 && char_str.as_bytes()[0].is_ascii_digit() {
        let n = char_str.as_bytes()[0] - b'0';
        if (1..=9).contains(&n) {
            move_window_to_position(n);
            return;
        }
    }

    let session_count = SESSION_LIST.lock().unwrap().len();
    let current = *SELECTED_INDEX.lock().unwrap();

    // Find next/prev focusable index (skip tty=unknown sessions)
    let find_focusable = |mut range: Box<dyn Iterator<Item = usize>>| -> Option<usize> {
        let sessions = SESSION_LIST.lock().unwrap();
        range.find(|&i| sessions.get(i).is_some_and(|s| s.tty != "unknown"))
    };

    match key_code {
        // Up arrow
        126 => {
            let from = current.unwrap_or(1);
            let target = find_focusable(Box::new((0..from).rev()))
                .or_else(|| find_focusable(Box::new(0..session_count)));
            if let Some(idx) = target {
                *SELECTED_INDEX.lock().unwrap() = Some(idx);
            }
        }
        // Down arrow
        125 => {
            let from = current.map(|i| i + 1).unwrap_or(0);
            let target = find_focusable(Box::new(from..session_count))
                .or_else(|| find_focusable(Box::new(0..session_count)));
            if let Some(idx) = target {
                *SELECTED_INDEX.lock().unwrap() = Some(idx);
            }
        }
        // Enter
        36 => {
            focus_selected();
            return;
        }
        // Esc - deselect and hide window
        53 => {
            *SELECTED_INDEX.lock().unwrap() = None;
            let mtm = MainThreadMarker::new().unwrap();
            let app = NSApplication::sharedApplication(mtm);
            app.hide(None);
            request_redraw();
            return;
        }
        _ => match char_str.as_str() {
            "k" => {
                let from = current.unwrap_or(1);
                let target = find_focusable(Box::new((0..from).rev()))
                    .or_else(|| find_focusable(Box::new(0..session_count)));
                if let Some(idx) = target {
                    *SELECTED_INDEX.lock().unwrap() = Some(idx);
                }
            }
            "j" => {
                let from = current.map(|i| i + 1).unwrap_or(0);
                let target = find_focusable(Box::new(from..session_count))
                    .or_else(|| find_focusable(Box::new(0..session_count)));
                if let Some(idx) = target {
                    *SELECTED_INDEX.lock().unwrap() = Some(idx);
                }
            }
            "d" => {
                if let Some(idx) = current {
                    let sessions = SESSION_LIST.lock().unwrap();
                    if let Some(session) = sessions.get(idx) {
                        let key = session.key();
                        drop(sessions);
                        let storage = Storage::new();
                        let _ = storage.remove_session(&key);
                        load_sessions();
                        let new_count = SESSION_LIST.lock().unwrap().len();
                        if new_count == 0 {
                            *SELECTED_INDEX.lock().unwrap() = None;
                        } else if idx >= new_count {
                            *SELECTED_INDEX.lock().unwrap() = Some(new_count - 1);
                        }
                    }
                }
            }
            "f" => {
                if let Some(idx) = current {
                    // Per-project toggle
                    let sessions = SESSION_LIST.lock().unwrap();
                    if let Some(session) = sessions.get(idx) {
                        let cwd = session.cwd.clone();
                        drop(sessions);
                        let mut disabled = AF_DISABLED_PROJECTS.lock().unwrap();
                        if !disabled.remove(&cwd) {
                            disabled.insert(cwd);
                        }
                    }
                } else {
                    // Bulk toggle: if any project is enabled, disable all; otherwise enable all
                    let sessions = SESSION_LIST.lock().unwrap();
                    let cwds: Vec<String> = sessions.iter().map(|s| s.cwd.clone()).collect();
                    drop(sessions);
                    let mut disabled = AF_DISABLED_PROJECTS.lock().unwrap();
                    let all_disabled = cwds.iter().all(|c| disabled.contains(c));
                    if all_disabled {
                        disabled.clear();
                    } else {
                        for c in cwds {
                            disabled.insert(c);
                        }
                    }
                }
                persist_af_disabled();
            }
            c if c.len() == 1 && c.as_bytes()[0].is_ascii_digit() => {
                let n = (c.as_bytes()[0] - b'0') as usize;
                if n >= 1 && n <= session_count {
                    *SELECTED_INDEX.lock().unwrap() = Some(n - 1);
                }
            }
            _ => return,
        },
    }

    request_redraw();
}

// --- Hover event handlers (wired into VIEW_CLASS) ---

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
}

extern "C" fn hover_timer_fired(this: *mut AnyObject, _sel: Sel, _timer: *mut AnyObject) {
    let view: &NSView = unsafe { &*(this as *const NSView) };
    on_hover_timer_fired(view);
}

fn install_tracking_area(view: &NSView) {
    // Remove any existing tracking areas first so the rect always matches the
    // current view bounds (called whenever the view resizes).
    let existing = view.trackingAreas();
    for area in existing.iter() {
        view.removeTrackingArea(&area);
    }
    let bounds = view.bounds();
    let options = NSTrackingAreaOptions::MouseEnteredAndExited
        | NSTrackingAreaOptions::MouseMoved
        | NSTrackingAreaOptions::ActiveAlways
        | NSTrackingAreaOptions::InVisibleRect;
    let area: Retained<NSTrackingArea> = unsafe {
        let alloc = NSTrackingArea::alloc();
        msg_send![
            alloc,
            initWithRect: bounds,
            options: options,
            owner: view,
            userInfo: std::ptr::null_mut::<AnyObject>()
        ]
    };
    view.addTrackingArea(&area);
}

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

    let window_point = event.locationInWindow();
    let view_point = view.convertPoint_fromView(window_point, None);
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

    let needs_timer = with_hover_runtime(|rt| {
        let event = rt.tracker.on_mouse(hit_with_key, Instant::now());
        match event {
            HoverEvent::Entered { version, .. } => {
                rt.popover.hide();
                rt.pending_timer_version = Some(version);
                true
            }
            HoverEvent::Cleared => {
                rt.popover.hide();
                rt.pending_timer_version = None;
                false
            }
            HoverEvent::Unchanged => false,
        }
    });

    if needs_timer {
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

fn on_hover_timer_fired(view: &NSView) {
    // Snapshot the bits we need under the hover lock without holding it
    // across NSWindow/cache work that may also lock.
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
    let Some(transcript_path) = session.transcript_path.as_ref() else {
        return;
    };
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

fn request_redraw() {
    let ptr = *CONTENT_VIEW_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let view = unsafe { &*(ptr as *const NSView) };
        let view_width = unsafe { view.superview() }
            .map(|sv| sv.bounds().size.width)
            .unwrap_or_else(|| view.frame().size.width.max(WINDOW_WIDTH));
        let view_height = view.frame().size.height;
        view.setFrameSize(NSSize::new(view_width, view_height));
        let theme = *CURRENT_THEME.lock().unwrap();
        if theme_rebuilds_subviews(theme) {
            objc2::rc::autoreleasepool(|_| {
                rebuild_view_for_theme(view, theme);
            });
        } else {
            let _: () = unsafe { msg_send![view, setNeedsDisplay: true] };
        }
    }
    update_af_label();
}

fn persist_af_disabled() {
    let disabled = AF_DISABLED_PROJECTS.lock().unwrap().clone();
    let storage = Storage::new();
    let _ = storage.save_af_disabled(&disabled);
}

fn update_af_label() {
    let ptr = *AF_LABEL_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let sessions = SESSION_LIST.lock().unwrap();
        let disabled = AF_DISABLED_PROJECTS.lock().unwrap();
        let total = sessions.len();
        let off_count = sessions
            .iter()
            .filter(|s| disabled.contains(&s.cwd))
            .count();
        drop(disabled);
        drop(sessions);
        let (text, color) = if total == 0 || off_count == 0 {
            ("auto-focus: \u{2713}".to_string(), color_text())
        } else if off_count == total {
            ("auto-focus: \u{23F8}".to_string(), color_dim())
        } else {
            (
                format!("auto-focus: {}/{}", total - off_count, total),
                color_dim(),
            )
        };
        let label = ptr as *mut AnyObject;
        unsafe {
            let ns_str = NSString::from_str(&text);
            let _: () = msg_send![label, setStringValue: &*ns_str];
            let _: () = msg_send![label, setTextColor: &*color];
        }
    }
}

fn rebuild_view_for_theme(view: &NSView, theme: WindowThemeId) {
    match theme {
        WindowThemeId::Classic => rebuild_view_classic(view),
        WindowThemeId::MissionControl => rebuild_view_mission_control(view),
        WindowThemeId::Notch => rebuild_view_notch(view),
    }
}

fn theme_rebuilds_subviews(theme: WindowThemeId) -> bool {
    matches!(theme, WindowThemeId::Classic | WindowThemeId::Notch)
}

// --- Notch theme (Dynamic Island style) ---

fn rebuild_view_notch(view: &NSView) {
    const NOTCH_HEIGHT: CGFloat = 44.0;
    const NOTCH_RADIUS: CGFloat = 20.0;
    const NOTCH_PAD: CGFloat = 8.0;
    const CHIP_HEIGHT: CGFloat = 28.0;
    const CHIP_RADIUS: CGFloat = 14.0;
    const CHIP_PAD: CGFloat = 4.0;
    const CHIP_DOT: CGFloat = 6.0;
    const CHIP_FONT: CGFloat = 10.5;

    let mtm = MainThreadMarker::new().unwrap();
    let sessions = SESSION_LIST.lock().unwrap();
    let selected = *SELECTED_INDEX.lock().unwrap();

    let subviews = view.subviews();
    for subview in subviews.iter() {
        subview.removeFromSuperview();
    }

    let view_width = unsafe { view.superview() }
        .map(|sv| sv.bounds().size.width)
        .unwrap_or(500.0);

    view.setFrameSize(NSSize::new(view_width, NOTCH_HEIGHT));

    // Notch pill background
    let notch_bg = create_colored_view(
        mtm,
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(view_width, NOTCH_HEIGHT),
        ),
        &NSColor::colorWithRed_green_blue_alpha(0.05, 0.05, 0.08, 0.9),
        NOTCH_RADIUS,
    );
    view.addSubview(&notch_bg);

    if sessions.is_empty() {
        let label = create_mono_label(
            mtm,
            "no sessions",
            NSRect::new(
                NSPoint::new(NOTCH_PAD + 8.0, (NOTCH_HEIGHT - 16.0) / 2.0),
                NSSize::new(view_width - NOTCH_PAD * 2.0, 16.0),
            ),
            &NSColor::colorWithRed_green_blue_alpha(0.4, 0.45, 0.55, 1.0),
            CHIP_FONT,
        );
        notch_bg.addSubview(&label);
        return;
    }

    // Calculate chip width to fit all sessions in one line
    let count = sessions.len() as CGFloat;
    let available = view_width - NOTCH_PAD * 2.0;
    let chip_w = ((available - (count - 1.0) * CHIP_PAD) / count).max(40.0);
    let chip_y = (NOTCH_HEIGHT - CHIP_HEIGHT) / 2.0;

    for (i, session) in sessions.iter().enumerate() {
        let chip_x = NOTCH_PAD + (i as CGFloat) * (chip_w + CHIP_PAD);
        let is_selected = Some(i) == selected;

        // Chip background
        let chip_bg_color = if is_selected {
            NSColor::colorWithRed_green_blue_alpha(0.2, 0.3, 0.5, 0.5)
        } else {
            NSColor::colorWithRed_green_blue_alpha(0.12, 0.12, 0.18, 0.7)
        };
        let chip_view = create_colored_view(
            mtm,
            NSRect::new(
                NSPoint::new(chip_x, chip_y),
                NSSize::new(chip_w, CHIP_HEIGHT),
            ),
            &chip_bg_color,
            CHIP_RADIUS,
        );

        // Status dot
        let dot_x = 8.0;
        let dot_y_inner = (CHIP_HEIGHT - CHIP_DOT) / 2.0;
        let dot_color = match session.status {
            SessionStatus::Running => {
                NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 1.0)
            }
            SessionStatus::AwaitingApproval => {
                NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 1.0)
            }
            SessionStatus::WaitingInput => {
                NSColor::colorWithRed_green_blue_alpha(0.961, 0.620, 0.043, 1.0)
            }
            SessionStatus::Stopped => NSColor::colorWithRed_green_blue_alpha(0.35, 0.39, 0.45, 1.0),
        };
        let dot_alpha: f64 = match session.status {
            SessionStatus::Running => theme::breathing_pulse(elapsed_secs()),
            SessionStatus::AwaitingApproval => theme::fast_blink(elapsed_secs()),
            SessionStatus::WaitingInput => theme::slow_fade(elapsed_secs()),
            SessionStatus::Stopped => 0.6,
        };
        let dot_view = create_colored_view(
            mtm,
            NSRect::new(
                NSPoint::new(dot_x, dot_y_inner),
                NSSize::new(CHIP_DOT, CHIP_DOT),
            ),
            &dot_color,
            CHIP_DOT / 2.0,
        );
        let _: () = unsafe { msg_send![&*dot_view, setAlphaValue: dot_alpha] };
        chip_view.addSubview(&dot_view);

        // Project name (truncated to fit)
        let label_x = dot_x + CHIP_DOT + 4.0;
        let label_w = chip_w - label_x - 6.0;
        let project = session.project_name();
        let text_color = if session.status == SessionStatus::Stopped {
            NSColor::colorWithRed_green_blue_alpha(0.4, 0.45, 0.5, 1.0)
        } else {
            NSColor::colorWithRed_green_blue_alpha(0.9, 0.92, 0.95, 1.0)
        };
        let label_rect = NSRect::new(
            NSPoint::new(label_x, (CHIP_HEIGHT - 14.0) / 2.0),
            NSSize::new(label_w.max(0.0), 14.0),
        );
        let label = create_mono_label(mtm, project, label_rect, &text_color, CHIP_FONT);
        let _: () = unsafe { msg_send![&*label, setLineBreakMode: 4_isize] }; // truncate tail
        chip_view.addSubview(&label);

        notch_bg.addSubview(&chip_view);
    }
}

// --- CG direct-drawing helpers (used by mission control theme) ---
// These draw into the current NSGraphicsContext without creating any NSView objects.

#[allow(dead_code)]
fn cg_fill_rect(rect: NSRect, color: &NSColor) {
    color.setFill();
    NSBezierPath::fillRect(rect);
}

fn cg_fill_rounded_rect(rect: NSRect, color: &NSColor, radius: CGFloat) {
    color.setFill();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
    path.fill();
}

fn cg_fill_circle(center_x: CGFloat, center_y: CGFloat, radius: CGFloat, color: &NSColor) {
    color.setFill();
    let rect = NSRect::new(
        NSPoint::new(center_x - radius, center_y - radius),
        NSSize::new(radius * 2.0, radius * 2.0),
    );
    let path = NSBezierPath::bezierPathWithOvalInRect(rect);
    path.fill();
}

/// Build a text attributes NSDictionary with font and color.
fn cg_text_attrs(font: &NSFont, color: &NSColor) -> *mut AnyObject {
    thread_local! {
        static FONT_KEY: Retained<NSString> = NSString::from_str("NSFont");
        static COLOR_KEY: Retained<NSString> = NSString::from_str("NSColor");
    }
    FONT_KEY.with(|fk| {
        COLOR_KEY.with(|ck| unsafe {
            msg_send![
                objc2::runtime::AnyClass::get(c"NSDictionary").unwrap(),
                dictionaryWithObjects: [font as *const NSFont as *const AnyObject, color as *const NSColor as *const AnyObject].as_ptr(),
                forKeys: [&**fk as *const NSString as *const AnyObject, &**ck as *const NSString as *const AnyObject].as_ptr(),
                count: 2_usize
            ]
        })
    })
}

fn cg_draw_text(text: &str, point: NSPoint, color: &NSColor, font: &NSFont) {
    unsafe {
        let ns_str = NSString::from_str(text);
        let dict = cg_text_attrs(font, color);
        let _: () = msg_send![&*ns_str, drawAtPoint: point, withAttributes: dict];
    }
}

fn cg_draw_text_right(text: &str, rect: NSRect, color: &NSColor, font: &NSFont) {
    // Measure text width, then draw at right-aligned position within rect
    unsafe {
        let ns_str = NSString::from_str(text);
        let dict = cg_text_attrs(font, color);
        let size: NSSize = msg_send![&*ns_str, sizeWithAttributes: dict];
        let x = rect.origin.x + rect.size.width - size.width;
        let point = NSPoint::new(x, rect.origin.y);
        let _: () = msg_send![&*ns_str, drawAtPoint: point, withAttributes: dict];
    }
}

fn cg_draw_text_truncated(text: &str, rect: NSRect, color: &NSColor, font: &NSFont) {
    // Draw text truncated to fit within rect (clip to rect)
    NSGraphicsContext::saveGraphicsState_class();
    NSBezierPath::clipRect(rect);
    cg_draw_text(text, rect.origin, color, font);
    NSGraphicsContext::restoreGraphicsState_class();
}

// Draw a rounded-rect border (stroke only, no fill)
fn cg_stroke_rounded_rect(rect: NSRect, color: &NSColor, radius: CGFloat, width: CGFloat) {
    color.setStroke();
    let path = NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius);
    path.setLineWidth(width);
    path.stroke();
}

fn rebuild_view_mission_control(view: &NSView) {
    let sessions = SESSION_LIST.lock().unwrap();
    let selected = *SELECTED_INDEX.lock().unwrap();

    // Remove all subviews (there should be none in CG mode, but clear any stragglers)
    let subviews = view.subviews();
    for subview in subviews.iter() {
        subview.removeFromSuperview();
    }

    let view_width = unsafe { view.superview() }
        .map(|sv| sv.bounds().size.width)
        .unwrap_or(WINDOW_WIDTH);

    // --- Background dot grid (drawn directly) ---
    {
        let (gr, gg, gb) = rgb_to_f64(palette::GRID);
        let grid_color = NSColor::colorWithRed_green_blue_alpha(gr, gg, gb, 0.10);
        let view_h = view.frame().size.height.max(400.0);
        let mut gx = GRID_SPACING;
        while gx < view_width {
            let mut gy = GRID_SPACING;
            while gy < view_h {
                cg_fill_circle(gx, gy, GRID_DOT_RADIUS, &grid_color);
                gy += GRID_SPACING;
            }
            gx += GRID_SPACING;
        }
    }

    let active_count = sessions
        .iter()
        .filter(|s| s.status != SessionStatus::Stopped)
        .count();
    let total_count = sessions.len();

    let card_count = sessions.len().max(1) as CGFloat;
    let total_height = HEADER_HEIGHT + card_count * (CARD_HEIGHT + CARD_SPACING);
    view.setFrameSize(NSSize::new(view_width, total_height));

    // --- Cached fonts (thread_local because NSFont is !Sync) ---
    fn mc_cached_font(size: CGFloat, weight: CGFloat) -> Retained<NSFont> {
        use std::collections::HashMap;
        thread_local! {
            static CACHE: RefCell<HashMap<u64, Retained<NSFont>>> = RefCell::new(HashMap::new());
        }
        CACHE.with(|cache| {
            let key = size.to_bits() ^ weight.to_bits().rotate_left(32);
            let mut map = cache.borrow_mut();
            map.entry(key)
                .or_insert_with(|| NSFont::monospacedSystemFontOfSize_weight(size, weight))
                .clone()
        })
    }
    let bold_font = mc_cached_font(FONT_SIZE, 0.7);
    let small_font = mc_cached_font(FONT_SIZE_SMALL, 0.0);
    let normal_font = mc_cached_font(FONT_SIZE, 0.0);
    let bold_font_row1 = mc_cached_font(FONT_SIZE, 0.5);

    // --- Header ---
    // Left: "◉ cckit mission control"
    cg_draw_text(
        "\u{25C9} cckit mission control",
        NSPoint::new(LEFT_PAD, 4.0),
        &color_text(),
        &bold_font,
    );

    // Right: "{active} active / {total} total"
    let hdr_right_text = format!("{} active / {} total", active_count, total_count);
    let hdr_right_rect = NSRect::new(
        NSPoint::new(view_width - 200.0 - LEFT_PAD, 4.0),
        NSSize::new(200.0, HEADER_HEIGHT - 4.0),
    );
    cg_draw_text_right(&hdr_right_text, hdr_right_rect, &color_dim(), &small_font);

    let y_start = HEADER_HEIGHT;

    if sessions.is_empty() {
        cg_draw_text(
            "no active sessions",
            NSPoint::new(LEFT_PAD + CARD_CONTENT_LEFT, y_start + 20.0),
            &color_dim(),
            &normal_font,
        );
        return;
    }

    for (i, session) in sessions.iter().enumerate() {
        let card_y = y_start + (i as CGFloat) * (CARD_HEIGHT + CARD_SPACING);
        let card_w = view_width - LEFT_PAD * 2.0;
        let agent = session.agent_type();
        let is_stopped = session.status == SessionStatus::Stopped;
        let inactive_factor = match session.status {
            SessionStatus::Running | SessionStatus::WaitingInput => {
                theme::inactivity_factor(session.updated_at, chrono::Utc::now())
            }
            _ => 0.0,
        };
        let is_inactive = inactive_factor > 0.0;
        let card_alpha: CGFloat = if is_stopped {
            0.5
        } else {
            theme::inactivity_alpha(inactive_factor)
        };

        // Save graphics state for per-card alpha
        NSGraphicsContext::saveGraphicsState_class();

        // --- Card background ---
        let card_rect = NSRect::new(
            NSPoint::new(LEFT_PAD, card_y),
            NSSize::new(card_w, CARD_HEIGHT),
        );
        let card_bg = if Some(i) == selected {
            color_selection()
        } else {
            color_surface()
        };

        // For stopped or inactive cards, use color with reduced alpha
        if card_alpha < 1.0 {
            let bg_with_alpha = unsafe {
                let r: CGFloat = msg_send![&*card_bg, redComponent];
                let g: CGFloat = msg_send![&*card_bg, greenComponent];
                let b: CGFloat = msg_send![&*card_bg, blueComponent];
                let a: CGFloat = msg_send![&*card_bg, alphaComponent];
                NSColor::colorWithRed_green_blue_alpha(r, g, b, a * card_alpha)
            };
            cg_fill_rounded_rect(card_rect, &bg_with_alpha, CARD_CORNER_RADIUS);
        } else {
            cg_fill_rounded_rect(card_rect, &card_bg, CARD_CORNER_RADIUS);
        }

        // Card border glow animation (suppressed for inactive sessions)
        let glow_alpha = if is_inactive {
            0.0
        } else {
            match session.status {
                SessionStatus::Running => {
                    let phase = (elapsed_secs() / anim::GLOW_PERIOD) * std::f64::consts::TAU;
                    ((phase.sin() + 1.0) / 2.0) * 0.3
                }
                SessionStatus::AwaitingApproval => theme::fast_blink(elapsed_secs()) * 0.3,
                _ => 0.0,
            }
        };
        if glow_alpha > 0.0 {
            let border_color = agent_accent_color_alpha(agent, glow_alpha);
            cg_stroke_rounded_rect(card_rect, &border_color, CARD_CORNER_RADIUS, 1.0);
        }

        // All card content is drawn at absolute positions (card_y offset)
        let content_x = LEFT_PAD + CARD_CONTENT_LEFT;

        // --- Row 1: status dot + agent + project + context bar + elapsed ---
        let row1_y: CGFloat = card_y + 4.0;
        let row1_h: CGFloat = 16.0;

        // Status dot with pulse animation
        let dot = DOT_SIZE;
        let dot_cy = row1_y + row1_h / 2.0;
        let dot_color = if session.tty == "unknown" {
            color_dim()
        } else {
            status_color(&session.status)
        };
        let dot_alpha: f64 = if is_inactive {
            1.0 // no animation, alpha comes from card_alpha
        } else {
            match session.status {
                SessionStatus::Running => theme::breathing_pulse(elapsed_secs()),
                SessionStatus::AwaitingApproval => theme::fast_blink(elapsed_secs()),
                SessionStatus::WaitingInput => theme::slow_fade(elapsed_secs()),
                SessionStatus::Stopped => 1.0,
            }
        };
        // Apply dot alpha by adjusting color
        let dot_color_alpha = {
            let (r, g, b) = if session.tty == "unknown" {
                rgb_to_f64(palette::TEXT_DIM)
            } else {
                let sc = match session.status {
                    SessionStatus::Running => StatusColor::Running,
                    SessionStatus::AwaitingApproval => StatusColor::AwaitingApproval,
                    SessionStatus::WaitingInput => StatusColor::WaitingInput,
                    SessionStatus::Stopped => StatusColor::Stopped,
                };
                sc.f64()
            };
            let effective_alpha = dot_alpha
                * if is_stopped || is_inactive {
                    card_alpha
                } else {
                    1.0
                };
            NSColor::colorWithRed_green_blue_alpha(r, g, b, effective_alpha)
        };
        cg_fill_circle(content_x + dot / 2.0, dot_cy, dot / 2.0, &dot_color_alpha);
        // Suppress unused variable warning
        let _ = dot_color;

        let project = session.display_name();
        let elapsed = format_elapsed(session.updated_at);
        let unfocusable = session.tty == "unknown";
        let text_color_effective = if is_stopped {
            let (r, g, b) = rgb_to_f64(palette::TEXT_DIM);
            NSColor::colorWithRed_green_blue_alpha(r, g, b, card_alpha)
        } else if is_inactive {
            let (r, g, b) = rgb_to_f64(palette::TEXT);
            NSColor::colorWithRed_green_blue_alpha(r, g, b, card_alpha)
        } else if unfocusable {
            color_dim()
        } else {
            color_text()
        };

        // Agent label + project name.
        // Prefer the short model label (e.g. "Opus 4.7") when the session's
        // last-seen model id maps to one; fall back to the agent-family name.
        let agent_label_text: String = session
            .model
            .as_deref()
            .and_then(theme::model_short_label)
            .unwrap_or_else(|| {
                match agent {
                    AgentType::Claude => "Claude",
                    AgentType::Codex => "Codex",
                    AgentType::Gemini => "Gemini",
                    AgentType::Unknown => "Agent",
                }
                .to_string()
            });
        let row1_text = format!("{} \u{2022} {}", agent_label_text, project);
        let label_x = content_x + dot + 4.0;
        let row1_rect = NSRect::new(
            NSPoint::new(label_x, row1_y),
            NSSize::new(card_w - (label_x - LEFT_PAD) - 120.0, row1_h),
        );
        cg_draw_text_truncated(
            &row1_text,
            row1_rect,
            &text_color_effective,
            &bold_font_row1,
        );

        // Elapsed + context % (right side of row 1)
        let ctx_info = context_bar_info(session);
        let mini_ctx = ctx_info
            .as_ref()
            .map(|c| format!("  {}", c.label))
            .unwrap_or_default();
        let row1_right_text = format!("{}{}", elapsed, mini_ctx);
        let dim_color = if is_stopped || is_inactive {
            let (r, g, b) = rgb_to_f64(palette::TEXT_DIM);
            NSColor::colorWithRed_green_blue_alpha(r, g, b, card_alpha)
        } else {
            color_dim()
        };
        let row1_right_rect = NSRect::new(
            NSPoint::new(LEFT_PAD + card_w - 110.0, row1_y),
            NSSize::new(100.0, row1_h),
        );
        cg_draw_text_right(&row1_right_text, row1_right_rect, &dim_color, &small_font);

        // --- Row 2: tool + stats + path + context bar + AF ---
        let row2_y: CGFloat = card_y + 20.0;
        let row2_h: CGFloat = 14.0;

        let tool = session.last_tool.as_deref().unwrap_or("-");
        let stats = format_session_stats(session);
        let path = session.short_cwd();
        let af_off = AF_DISABLED_PROJECTS.lock().unwrap().contains(&session.cwd);
        let af_icon = if af_off { "\u{23F8}" } else { "\u{2713}" };

        // Context bar inline (small, right-aligned before AF)
        let bar_w: CGFloat = 60.0;
        let bar_h: CGFloat = 3.0;
        let bar_x = LEFT_PAD + card_w - 80.0;
        let bar_y = row2_y + (row2_h - bar_h) / 2.0;

        let row2_text = format!(
            "{} \u{2022} {} \u{2022} {} \u{2022} AF:{}",
            tool, stats, path, af_icon
        );
        let row2_rect = NSRect::new(
            NSPoint::new(content_x, row2_y),
            NSSize::new(card_w - CARD_CONTENT_LEFT - 90.0, row2_h),
        );
        cg_draw_text_truncated(&row2_text, row2_rect, &dim_color, &small_font);

        // Inline context gauge bar
        if let Some(ref info) = ctx_info {
            // Track
            let track_rect = NSRect::new(NSPoint::new(bar_x, bar_y), NSSize::new(bar_w, bar_h));
            cg_fill_rounded_rect(
                track_rect,
                &NSColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 0.08),
                1.5,
            );
            // Fill
            let fill_w = (bar_w * info.ratio).max(1.0);
            let fill_rect = NSRect::new(NSPoint::new(bar_x, bar_y), NSSize::new(fill_w, bar_h));
            cg_fill_rounded_rect(fill_rect, &context_bar_color(info.ratio), 1.5);
            // Leading-edge pulse
            if fill_w > 4.0 && !is_stopped && !is_inactive {
                let pulse_w = 4.0_f64.min(fill_w);
                let phase = (elapsed_secs() * std::f64::consts::TAU / anim::CONTEXT_PULSE_PERIOD)
                    .sin()
                    .max(0.0);
                let pulse_alpha = 0.6 + 0.4 * phase;
                let pulse_rect = NSRect::new(
                    NSPoint::new(bar_x + fill_w - pulse_w, bar_y),
                    NSSize::new(pulse_w, bar_h),
                );
                cg_fill_rounded_rect(
                    pulse_rect,
                    &NSColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, pulse_alpha * 0.5),
                    1.5,
                );
            }
        }

        NSGraphicsContext::restoreGraphicsState_class();
    }
}

// --- Classic theme (v0.1.0 layout) ---

fn rebuild_view_classic(view: &NSView) {
    // v0.1.0 constants — completely independent from MC
    const CL_ROW_HEIGHT: CGFloat = 22.0;
    const CL_HEADER_HEIGHT: CGFloat = 20.0;
    const CL_FONT_SIZE: CGFloat = 11.5;
    const CL_FONT_SIZE_SMALL: CGFloat = 10.0;
    const CL_DOT_SIZE: CGFloat = 6.0;
    const CL_LEFT_PAD: CGFloat = 10.0;
    const CL_TEXT_LEFT: CGFloat = 24.0;
    const CL_PROJECT_COL_WIDTH: CGFloat = 220.0;
    const CL_PROJECT_COL_WIDTH_WIDE: CGFloat = 300.0;

    // v0.1.0 colors
    fn cl_color_text() -> Retained<NSColor> {
        NSColor::colorWithRed_green_blue_alpha(0.945, 0.961, 0.976, 1.0)
    }
    fn cl_color_dim() -> Retained<NSColor> {
        NSColor::colorWithRed_green_blue_alpha(0.392, 0.455, 0.545, 1.0)
    }
    fn cl_color_border() -> Retained<NSColor> {
        NSColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 0.08)
    }
    fn cl_color_selection() -> Retained<NSColor> {
        NSColor::colorWithRed_green_blue_alpha(0.145, 0.388, 0.922, 0.25)
    }
    fn cl_status_color(status: &SessionStatus) -> Retained<NSColor> {
        match status {
            SessionStatus::Running => {
                NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 1.0)
            }
            SessionStatus::AwaitingApproval => {
                NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 1.0)
            }
            SessionStatus::WaitingInput => {
                NSColor::colorWithRed_green_blue_alpha(0.475, 0.525, 0.596, 1.0)
            }
            SessionStatus::Stopped => {
                NSColor::colorWithRed_green_blue_alpha(0.345, 0.388, 0.447, 1.0)
            }
        }
    }
    fn cl_status_row_bg(status: &SessionStatus) -> Retained<NSColor> {
        match status {
            SessionStatus::Running => {
                NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 0.10)
            }
            SessionStatus::AwaitingApproval => {
                NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 0.20)
            }
            _ => NSColor::colorWithRed_green_blue_alpha(0.0, 0.0, 0.0, 0.0),
        }
    }

    let mtm = MainThreadMarker::new().unwrap();
    let sessions = SESSION_LIST.lock().unwrap();
    let selected = *SELECTED_INDEX.lock().unwrap();

    let subviews = view.subviews();
    for subview in subviews.iter() {
        subview.removeFromSuperview();
    }

    let view_width = unsafe { view.superview() }
        .map(|sv| sv.bounds().size.width)
        .unwrap_or(640.0);

    let any_has_context = sessions
        .iter()
        .any(|s| s.context_used_tokens.is_some() && s.context_max().is_some());

    let any_has_model = sessions.iter().any(|s| {
        s.model
            .as_deref()
            .and_then(theme::model_short_label)
            .is_some()
    });
    // Space allocated for the MODEL column (0 when no session reports one).
    let model_col_w: CGFloat = if any_has_model { 80.0 } else { 0.0 };

    let any_has_subagent_name = sessions.iter().any(|s| s.subagent_name.is_some());
    let proj_w = if any_has_subagent_name {
        CL_PROJECT_COL_WIDTH_WIDE
    } else {
        CL_PROJECT_COL_WIDTH
    };

    let row_count = sessions.len().max(1) as CGFloat;
    let total_height = CL_HEADER_HEIGHT + 1.0 + row_count * CL_ROW_HEIGHT;
    view.setFrameSize(NSSize::new(view_width, total_height));

    // Header
    let hdr_left = format!("{:>2}  {:<4}  {}", "#", "STAT", "PROJECT");
    let hdr_left_rect = NSRect::new(
        NSPoint::new(CL_TEXT_LEFT, 2.0),
        NSSize::new(proj_w, CL_HEADER_HEIGHT - 2.0),
    );
    view.addSubview(&create_mono_label(
        mtm,
        &hdr_left,
        hdr_left_rect,
        &cl_color_dim(),
        CL_FONT_SIZE,
    ));

    let path_x = CL_TEXT_LEFT + proj_w;
    let hdr_path_rect = NSRect::new(
        NSPoint::new(path_x, 2.0),
        NSSize::new(100.0, CL_HEADER_HEIGHT - 2.0),
    );
    view.addSubview(&create_mono_label(
        mtm,
        "PATH",
        hdr_path_rect,
        &cl_color_dim(),
        CL_FONT_SIZE,
    ));

    let hdr_ctx_w: CGFloat = if any_has_context { 70.0 } else { 0.0 };
    let hdr_stats_w: CGFloat = 230.0;
    let hdr_right = format!("{:>12} {:>6}  {:>5}  {:<2}", "STAT", "TOOL", "AGE", "AF");
    let hdr_right_rect = NSRect::new(
        NSPoint::new(view_width - hdr_stats_w - hdr_ctx_w - CL_LEFT_PAD, 2.0),
        NSSize::new(hdr_stats_w, CL_HEADER_HEIGHT - 2.0),
    );
    let hdr_right_label = create_mono_label(
        mtm,
        &hdr_right,
        hdr_right_rect,
        &cl_color_dim(),
        CL_FONT_SIZE_SMALL,
    );
    let _: () = unsafe { msg_send![&*hdr_right_label, setAlignment: 1_isize] };
    view.addSubview(&hdr_right_label);

    if any_has_model {
        let hdr_model_rect = NSRect::new(
            NSPoint::new(
                view_width - hdr_stats_w - hdr_ctx_w - model_col_w - CL_LEFT_PAD,
                2.0,
            ),
            NSSize::new(model_col_w - 6.0, CL_HEADER_HEIGHT - 2.0),
        );
        let hdr_model_label = create_mono_label(
            mtm,
            "MODEL",
            hdr_model_rect,
            &cl_color_dim(),
            CL_FONT_SIZE_SMALL,
        );
        let _: () = unsafe { msg_send![&*hdr_model_label, setAlignment: 1_isize] };
        view.addSubview(&hdr_model_label);
    }

    if any_has_context {
        let hdr_ctx_rect = NSRect::new(
            NSPoint::new(view_width - hdr_ctx_w - CL_LEFT_PAD, 2.0),
            NSSize::new(hdr_ctx_w, CL_HEADER_HEIGHT - 2.0),
        );
        let hdr_ctx_label = create_mono_label(
            mtm,
            "CONTEXT",
            hdr_ctx_rect,
            &cl_color_dim(),
            CL_FONT_SIZE_SMALL,
        );
        let _: () = unsafe { msg_send![&*hdr_ctx_label, setAlignment: 1_isize] };
        view.addSubview(&hdr_ctx_label);
    }

    // Header separator
    view.addSubview(&create_colored_view(
        mtm,
        NSRect::new(
            NSPoint::new(CL_LEFT_PAD, CL_HEADER_HEIGHT),
            NSSize::new(view_width - CL_LEFT_PAD * 2.0, 1.0),
        ),
        &cl_color_border(),
        0.0,
    ));

    let y_start = CL_HEADER_HEIGHT + 1.0;

    if sessions.is_empty() {
        let rect = NSRect::new(
            NSPoint::new(CL_TEXT_LEFT, y_start + 8.0),
            NSSize::new(view_width - CL_TEXT_LEFT - CL_LEFT_PAD, CL_ROW_HEIGHT),
        );
        view.addSubview(&create_mono_label(
            mtm,
            "  no active sessions",
            rect,
            &cl_color_dim(),
            CL_FONT_SIZE,
        ));
        return;
    }

    let right_col_w: CGFloat = 230.0 + if any_has_context { 70.0 } else { 0.0 } + model_col_w;

    for (i, session) in sessions.iter().enumerate() {
        let y = y_start + (i as CGFloat) * CL_ROW_HEIGHT;

        let row_rect = NSRect::new(
            NSPoint::new(4.0, y + 1.0),
            NSSize::new(view_width - 8.0, CL_ROW_HEIGHT - 2.0),
        );
        if Some(i) == selected {
            view.addSubview(&create_colored_view(
                mtm,
                row_rect,
                &cl_color_selection(),
                4.0,
            ));
        } else {
            let tint = cl_status_row_bg(&session.status);
            view.addSubview(&create_colored_view(mtm, row_rect, &tint, 4.0));
        }

        let cl_factor = match session.status {
            SessionStatus::Running | SessionStatus::WaitingInput => {
                theme::inactivity_factor(session.updated_at, chrono::Utc::now())
            }
            _ => 0.0,
        };
        let cl_alpha = theme::inactivity_alpha(cl_factor);

        let dot = CL_DOT_SIZE;
        let dot_y = y + (CL_ROW_HEIGHT - dot) / 2.0;
        let dot_color = if session.tty == "unknown" {
            cl_color_dim()
        } else if cl_factor > 0.0 {
            // Gradual fade: status color with reduced alpha
            let base = cl_status_color(&session.status);
            unsafe {
                let r: CGFloat = msg_send![&*base, redComponent];
                let g: CGFloat = msg_send![&*base, greenComponent];
                let b: CGFloat = msg_send![&*base, blueComponent];
                NSColor::colorWithRed_green_blue_alpha(r, g, b, cl_alpha)
            }
        } else {
            cl_status_color(&session.status)
        };
        view.addSubview(&create_colored_view(
            mtm,
            NSRect::new(NSPoint::new(CL_LEFT_PAD, dot_y), NSSize::new(dot, dot)),
            &dot_color,
            dot / 2.0,
        ));

        let project = session.display_name();
        let path = session.short_cwd();
        let tool = session.last_tool.as_deref().unwrap_or("-");
        let elapsed = format_elapsed(session.updated_at);
        let unfocusable = session.tty == "unknown";

        let text_color = if unfocusable || session.status == SessionStatus::Stopped {
            cl_color_dim()
        } else if cl_factor > 0.0 {
            // Gradual fade: text color with reduced alpha
            NSColor::colorWithRed_green_blue_alpha(0.945, 0.961, 0.976, cl_alpha)
        } else {
            cl_color_text()
        };

        let left_text = format!(
            "{:>2}  {:<4}  {}",
            i + 1,
            status_label(&session.status),
            project
        );
        let left_rect = NSRect::new(
            NSPoint::new(CL_TEXT_LEFT, y + 2.0),
            NSSize::new(proj_w, CL_ROW_HEIGHT - 4.0),
        );
        view.addSubview(&create_mono_label(
            mtm,
            &left_text,
            left_rect,
            &text_color,
            CL_FONT_SIZE,
        ));

        let path_x = CL_TEXT_LEFT + proj_w;
        let path_w = (view_width - path_x - right_col_w - CL_LEFT_PAD).max(40.0);
        let path_rect = NSRect::new(
            NSPoint::new(path_x, y + 2.0),
            NSSize::new(path_w, CL_ROW_HEIGHT - 4.0),
        );
        let path_label = create_mono_label(mtm, &path, path_rect, &cl_color_dim(), 9.5);
        let _: () = unsafe { msg_send![&*path_label, setLineBreakMode: 5_isize] };
        view.addSubview(&path_label);

        let stats = format_session_stats(session);
        let ctx_info = context_bar_info(session);
        let af_off = AF_DISABLED_PROJECTS.lock().unwrap().contains(&session.cwd);
        let af_col = if af_off { "\u{23F8}" } else { "\u{2713}" };
        let ctx_col_w: CGFloat = if any_has_context { 70.0 } else { 0.0 };
        let stats_w: CGFloat = 230.0;
        let right_text = format!("{:>12} {:>6}  {:>5}  {:<2}", stats, tool, elapsed, af_col);
        let right_rect = NSRect::new(
            NSPoint::new(view_width - stats_w - ctx_col_w - CL_LEFT_PAD, y + 2.0),
            NSSize::new(stats_w, CL_ROW_HEIGHT - 4.0),
        );
        let right_label = create_mono_label(
            mtm,
            &right_text,
            right_rect,
            &text_color,
            CL_FONT_SIZE_SMALL,
        );
        let _: () = unsafe { msg_send![&*right_label, setAlignment: 1_isize] };
        view.addSubview(&right_label);

        if any_has_model {
            let model_label_text = session
                .model
                .as_deref()
                .and_then(theme::model_short_label)
                .unwrap_or_else(|| match session.agent_type() {
                    AgentType::Claude => "Claude".to_string(),
                    AgentType::Codex => "Codex".to_string(),
                    AgentType::Gemini => "Gemini".to_string(),
                    AgentType::Unknown => "-".to_string(),
                });
            let model_rect = NSRect::new(
                NSPoint::new(
                    view_width - stats_w - ctx_col_w - model_col_w - CL_LEFT_PAD,
                    y + 2.0,
                ),
                NSSize::new(model_col_w - 6.0, CL_ROW_HEIGHT - 4.0),
            );
            let model_label = create_mono_label(
                mtm,
                &model_label_text,
                model_rect,
                &text_color,
                CL_FONT_SIZE_SMALL,
            );
            let _: () = unsafe { msg_send![&*model_label, setAlignment: 1_isize] };
            view.addSubview(&model_label);
        }

        if any_has_context {
            let col_x = view_width - ctx_col_w - CL_LEFT_PAD;
            let bar_total_w = 30.0_f64;
            let bar_h = 5.0_f64;
            let bar_y = y + (CL_ROW_HEIGHT - bar_h) / 2.0;

            if let Some(ref info) = ctx_info {
                let track_rect =
                    NSRect::new(NSPoint::new(col_x, bar_y), NSSize::new(bar_total_w, bar_h));
                view.addSubview(&create_colored_view(
                    mtm,
                    track_rect,
                    &NSColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 0.1),
                    3.0,
                ));

                let fill_w = (bar_total_w * info.ratio).max(1.0);
                let fill_rect = NSRect::new(NSPoint::new(col_x, bar_y), NSSize::new(fill_w, bar_h));
                view.addSubview(&create_colored_view(
                    mtm,
                    fill_rect,
                    &context_bar_color(info.ratio),
                    3.0,
                ));

                let label_x = col_x + bar_total_w + 4.0;
                let label_w = ctx_col_w - bar_total_w - 4.0;
                let label_rect = NSRect::new(
                    NSPoint::new(label_x, y + 2.0),
                    NSSize::new(label_w, CL_ROW_HEIGHT - 4.0),
                );
                view.addSubview(&create_mono_label(
                    mtm,
                    &info.label,
                    label_rect,
                    &text_color,
                    CL_FONT_SIZE_SMALL,
                ));
            }
        }

        // Row separator
        if i + 1 < sessions.len() {
            view.addSubview(&create_colored_view(
                mtm,
                NSRect::new(
                    NSPoint::new(CL_LEFT_PAD, y + CL_ROW_HEIGHT - 1.0),
                    NSSize::new(view_width - CL_LEFT_PAD * 2.0, 1.0),
                ),
                &cl_color_border(),
                0.0,
            ));
        }
    }
}

extern "C" fn draw_rect(this: *mut AnyObject, _sel: Sel, _dirty_rect: NSRect) {
    let view: &NSView = unsafe { &*(this as *const NSView) };
    let theme = *CURRENT_THEME.lock().unwrap();
    if !theme_rebuilds_subviews(theme) {
        objc2::rc::autoreleasepool(|_| {
            rebuild_view_for_theme(view, theme);
        });
    }
}

extern "C" fn is_flipped(_this: *mut AnyObject, _sel: Sel) -> Bool {
    Bool::YES
}

fn get_view_class() -> &'static AnyClass {
    REGISTER_VIEW_CLASS.call_once(|| {
        let superclass = NSView::class();
        let mut builder = ClassBuilder::new(c"CCKitSessionListView", superclass).unwrap();

        unsafe {
            builder.add_method(
                sel!(acceptsFirstResponder),
                accepts_first_responder as extern "C" fn(*mut AnyObject, Sel) -> Bool,
            );
            builder.add_method(
                sel!(keyDown:),
                key_down as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(drawRect:),
                draw_rect as extern "C" fn(*mut AnyObject, Sel, NSRect),
            );
            builder.add_method(
                sel!(isFlipped),
                is_flipped as extern "C" fn(*mut AnyObject, Sel) -> Bool,
            );
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

        let cls = builder.register();
        unsafe {
            VIEW_CLASS = Some(cls);
        }
    });

    unsafe { VIEW_CLASS.unwrap() }
}

// --- Window delegate ---

static REGISTER_DELEGATE_CLASS: Once = Once::new();
static mut DELEGATE_CLASS: Option<&'static AnyClass> = None;

extern "C" fn window_will_close(_this: *mut AnyObject, _sel: Sel, _notification: *mut AnyObject) {
    let mtm = MainThreadMarker::new().unwrap();
    let app = NSApplication::sharedApplication(mtm);
    app.terminate(None);
}

extern "C" fn window_did_move(_this: *mut AnyObject, _sel: Sel, _notification: *mut AnyObject) {
    save_window_frame();
}

extern "C" fn window_did_resize(_this: *mut AnyObject, _sel: Sel, _notification: *mut AnyObject) {
    save_window_frame();
}

fn save_window_frame() {
    let ptr = *WINDOW_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let window = unsafe { &*(ptr as *const NSWindow) };
        let f = window.frame();
        let storage = Storage::new();
        let _ = storage.save_window_frame((f.origin.x, f.origin.y, f.size.width, f.size.height));
    }
}

fn get_delegate_class() -> &'static AnyClass {
    REGISTER_DELEGATE_CLASS.call_once(|| {
        let superclass = NSObject::class();
        let mut builder = ClassBuilder::new(c"CCKitWindowDelegate", superclass).unwrap();

        unsafe {
            builder.add_method(
                sel!(windowWillClose:),
                window_will_close as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(windowDidMove:),
                window_did_move as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(windowDidResize:),
                window_did_resize as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
        }

        let cls = builder.register();
        unsafe {
            DELEGATE_CLASS = Some(cls);
        }
    });

    unsafe { DELEGATE_CLASS.unwrap() }
}

// --- Timer callback ---

fn update_sessions_and_redraw() {
    // Snapshot previous statuses before loading new data
    let prev = {
        let sessions = SESSION_LIST.lock().unwrap();
        let mut map = std::collections::HashMap::new();
        for s in sessions.iter() {
            map.insert(s.key(), s.status.clone());
        }
        map
    };

    load_sessions();

    // Detect state transitions
    let (needs_approval, finished) = {
        let sessions = SESSION_LIST.lock().unwrap();
        let now = chrono::Utc::now();
        let mut notified = NOTIFIED_APPROVALS.lock().unwrap();
        let mut approval = false;
        let mut done = false;
        // Clean up keys for sessions no longer awaiting approval
        notified.retain(|k| {
            sessions
                .iter()
                .any(|s| s.key() == *k && s.status == SessionStatus::AwaitingApproval)
        });
        let disabled = AF_DISABLED_PROJECTS.lock().unwrap();
        for s in sessions.iter() {
            if disabled.contains(&s.cwd) {
                continue;
            }
            if s.status == SessionStatus::AwaitingApproval
                && !notified.contains(&s.key())
                && let Some(started) = s.tool_started_at
            {
                let elapsed_ms = now.signed_duration_since(started).num_milliseconds();
                if elapsed_ms >= 3000 {
                    approval = true;
                    notified.insert(s.key());
                }
            }
            if let Some(prev_status) = prev.get(&s.key())
                && s.status == SessionStatus::WaitingInput
                && matches!(
                    prev_status,
                    SessionStatus::Running | SessionStatus::AwaitingApproval
                )
            {
                done = true;
            }
        }
        drop(disabled);
        (approval, done)
    };

    if needs_approval || finished {
        bring_window_to_front();
    }

    let count = SESSION_LIST.lock().unwrap().len();
    let mut idx = SELECTED_INDEX.lock().unwrap();
    if let Some(i) = *idx {
        if i >= count && count > 0 {
            *idx = Some(count - 1);
        } else if count == 0 {
            *idx = None;
        }
    }
    drop(idx);
    if REDRAW_NEEDED.swap(false, std::sync::atomic::Ordering::Relaxed) {
        request_redraw();
    }
}

fn bring_window_to_front() {
    let ptr = *WINDOW_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let window = ptr as *mut AnyObject;
        unsafe {
            let _: () = msg_send![window, orderFrontRegardless];
        }
        if let Some(mtm) = MainThreadMarker::new() {
            let app = NSApplication::sharedApplication(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }
    }
}

#[allow(dead_code)]
fn bounce_dock_icon() {
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        // NSInformationalRequest = 10: single bounce
        unsafe {
            let _: isize = msg_send![&*app, requestUserAttention: 10_isize];
        }
    }
}

/// Move window to a grid position (numpad layout: 1=bottom-left .. 9=top-right)
fn move_window_to_position(position: u8) {
    let ptr = *WINDOW_PTR.lock().unwrap();
    let ptr = match ptr {
        Some(p) => p,
        None => return,
    };
    let window = unsafe { &*(ptr as *const NSWindow) };
    let mtm = match MainThreadMarker::new() {
        Some(m) => m,
        None => return,
    };
    let screen = match NSScreen::mainScreen(mtm) {
        Some(s) => s,
        None => return,
    };
    let sf = screen.visibleFrame();
    let win_frame = window.frame();
    let w = win_frame.size.width;
    let h = win_frame.size.height;

    let (col, row) = match position {
        1 => (0, 2), // top-left
        2 => (1, 2), // top-center
        3 => (2, 2), // top-right
        4 => (0, 1), // middle-left
        5 => (1, 1), // center
        6 => (2, 1), // middle-right
        7 => (0, 0), // bottom-left
        8 => (1, 0), // bottom-center
        9 => (2, 0), // bottom-right
        _ => return,
    };

    let margin = 20.0;
    let x = match col {
        0 => sf.origin.x + margin,
        1 => sf.origin.x + (sf.size.width - w) / 2.0,
        _ => sf.origin.x + sf.size.width - w - margin,
    };
    let y = match row {
        0 => sf.origin.y + margin,
        1 => sf.origin.y + (sf.size.height - h) / 2.0,
        _ => sf.origin.y + sf.size.height - h - margin,
    };

    let new_frame = NSRect::new(NSPoint::new(x, y), win_frame.size);
    window.setFrame_display(new_frame, true);
    save_window_frame();
}

/// Resize window to fit height and a path-aware width.
fn fit_window_to_content() {
    let ptr = *WINDOW_PTR.lock().unwrap();
    let ptr = match ptr {
        Some(p) => p,
        None => return,
    };
    let window = unsafe { &*(ptr as *const NSWindow) };
    let mtm = match MainThreadMarker::new() {
        Some(m) => m,
        None => return,
    };

    let screen = match NSScreen::mainScreen(mtm) {
        Some(s) => s,
        None => return,
    };
    let sf = screen.visibleFrame();
    let max_w = sf.size.width * 0.9;
    let content_w = calculate_fit_window_width().min(max_w);
    resize_window_to_layout(
        window,
        calculate_layout_height(),
        content_w,
        sf.size.height * 0.8,
        true,
    );
    save_window_frame();
}

fn calculate_layout_height() -> CGFloat {
    let theme = *CURRENT_THEME.lock().unwrap();
    let session_count = SESSION_LIST.lock().unwrap().len().max(1) as CGFloat;
    match theme {
        WindowThemeId::Classic => 20.0 + 1.0 + session_count * 22.0 + FOOTER_HEIGHT,
        WindowThemeId::MissionControl => {
            HEADER_HEIGHT + session_count * (CARD_HEIGHT + CARD_SPACING) + FOOTER_HEIGHT
        }
        WindowThemeId::Notch => 44.0, // single pill bar, fixed height
    }
}

fn calculate_target_content_height(
    target_layout_h: CGFloat,
    layout_inset_h: CGFloat,
    max_content_h: CGFloat,
) -> CGFloat {
    let min_layout_h = (MIN_WINDOW_HEIGHT - layout_inset_h).max(0.0);
    let max_layout_h = (max_content_h - layout_inset_h).max(min_layout_h);
    let clamped_layout_h = target_layout_h.clamp(min_layout_h, max_layout_h);
    (clamped_layout_h + layout_inset_h).clamp(MIN_WINDOW_HEIGHT, max_content_h)
}

fn resize_window_to_layout(
    window: &NSWindow,
    target_layout_h: CGFloat,
    target_content_w: CGFloat,
    max_content_h: CGFloat,
    animate: bool,
) {
    let mtm = match MainThreadMarker::new() {
        Some(m) => m,
        None => return,
    };
    let layout_rect: NSRect = unsafe { msg_send![window, contentLayoutRect] };
    let content_h = window
        .contentView()
        .map(|view| view.bounds().size.height)
        .unwrap_or(layout_rect.size.height);
    let layout_inset_h = (content_h - layout_rect.size.height).max(0.0);
    let target_content_h =
        calculate_target_content_height(target_layout_h, layout_inset_h, max_content_h);
    let target_content_rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(target_content_w, target_content_h),
    );
    let target_frame =
        NSWindow::frameRectForContentRect_styleMask(target_content_rect, window.styleMask(), mtm);

    let old_frame = window.frame();
    // Keep top edge fixed and preserve the center horizontally.
    let dx = (target_frame.size.width - old_frame.size.width) / 2.0;
    let dy = target_frame.size.height - old_frame.size.height;
    let new_frame = NSRect::new(
        NSPoint::new(old_frame.origin.x - dx, old_frame.origin.y - dy),
        target_frame.size,
    );
    window.setFrame_display_animate(new_frame, true, animate);
}

// --- Main entry point ---

/// Unified app entry point: shows window + menubar (default), or one of them.
fn set_app_icon(app: &NSApplication) {
    static ICON_PNG: &[u8] = include_bytes!("../../assets/icon_512.png");
    unsafe {
        let data_cls = objc2::runtime::AnyClass::get(c"NSData").unwrap();
        let bytes_ptr: *const std::ffi::c_void = ICON_PNG.as_ptr() as *const std::ffi::c_void;
        let data: *mut AnyObject =
            msg_send![data_cls, dataWithBytes: bytes_ptr, length: ICON_PNG.len()];
        if data.is_null() {
            return;
        }
        let image_cls = objc2::runtime::AnyClass::get(c"NSImage").unwrap();
        let alloc: *mut AnyObject = msg_send![image_cls, alloc];
        let image: *mut AnyObject = msg_send![alloc, initWithData: data];
        if !image.is_null() {
            let image_ref: &NSImage = &*(image as *const NSImage);
            app.setApplicationIconImage(Some(image_ref));
        }
    }
}

pub fn run_app(menubar_only: bool, window_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Load persisted AF disabled set
    {
        let storage = Storage::new();
        let loaded = storage.load_af_disabled();
        *AF_DISABLED_PROJECTS.lock().unwrap() = loaded;
    }

    let mtm = MainThreadMarker::new().ok_or("Must run on main thread")?;
    let app = NSApplication::sharedApplication(mtm);

    let show_window = !menubar_only;
    let show_menubar = !window_only;

    if show_window {
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    } else {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }

    set_app_icon(&app);
    app.finishLaunching();

    // Menubar (kept alive via _menubar)
    let _menubar = if show_menubar {
        let menubar = std::rc::Rc::new(super::menubar::MenubarApp::new(mtm));
        let menubar_for_timer = menubar.clone();
        let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
            menubar_for_timer.update_menu();
        });
        let _timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &block) };
        Some((menubar, _timer))
    } else {
        None
    };

    if show_window {
        setup_window(mtm, &app)?;
    }

    app.run();
    Ok(())
}

pub fn run_window_app() -> Result<(), Box<dyn std::error::Error>> {
    run_app(false, true)
}

fn setup_main_menu(mtm: MainThreadMarker, app: &NSApplication) {
    unsafe {
        let menu_bar = NSMenu::new(mtm);

        // Application menu (first item is the app menu)
        let app_menu_item = NSMenuItem::new(mtm);
        let app_menu = NSMenu::new(mtm);

        let hide = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Hide cckit"),
            Some(sel!(hide:)),
            &NSString::from_str("h"),
        );
        app_menu.addItem(&hide);

        let hide_others = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Hide Others"),
            Some(sel!(hideOtherApplications:)),
            &NSString::from_str("h"),
        );
        let _: () = msg_send![&*hide_others, setKeyEquivalentModifierMask: 0x180000_usize]; // Cmd+Option
        app_menu.addItem(&hide_others);

        let show_all = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Show All"),
            Some(sel!(unhideAllApplications:)),
            &NSString::from_str(""),
        );
        app_menu.addItem(&show_all);

        app_menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Quit cckit"),
            Some(sel!(terminate:)),
            &NSString::from_str("q"),
        );
        app_menu.addItem(&quit);

        app_menu_item.setSubmenu(Some(&app_menu));
        menu_bar.addItem(&app_menu_item);

        // Window menu
        let window_menu_item = NSMenuItem::new(mtm);
        let window_menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str("Window"));

        let minimize = NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str("Minimize"),
            Some(sel!(performMiniaturize:)),
            &NSString::from_str("m"),
        );
        window_menu.addItem(&minimize);

        window_menu_item.setSubmenu(Some(&window_menu));
        menu_bar.addItem(&window_menu_item);

        app.setMainMenu(Some(&menu_bar));
    }
}

fn setup_window(
    mtm: MainThreadMarker,
    app: &NSApplication,
) -> Result<(), Box<dyn std::error::Error>> {
    load_sessions();

    let screen = NSScreen::mainScreen(mtm).ok_or("No main screen")?;
    let sf = screen.visibleFrame();
    let max_window_h = sf.size.height * 0.8;
    let style_mask = NSWindowStyleMask::Titled
        .union(NSWindowStyleMask::Closable)
        .union(NSWindowStyleMask::Resizable)
        .union(NSWindowStyleMask::Miniaturizable)
        .union(NSWindowStyleMask::FullSizeContentView);
    let needed_h = calculate_layout_height();
    let content_rect_h = needed_h.clamp(MIN_WINDOW_HEIGHT, max_window_h);

    // Restore saved window frame, or center on screen
    let storage = Storage::new();
    let (x, y, win_w, win_h) = if let Some((sx, sy, sw, sh)) = storage.load_window_frame() {
        // Validate saved frame is within current screen bounds
        if sx >= sf.origin.x - 100.0
            && sx <= sf.origin.x + sf.size.width
            && sy >= sf.origin.y - 100.0
            && sy <= sf.origin.y + sf.size.height
            && sw >= 200.0
            && sh >= MIN_WINDOW_HEIGHT
        {
            (sx, sy, sw, sh)
        } else {
            let frame_probe = NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(WINDOW_WIDTH, content_rect_h),
            );
            let frame_rect =
                NSWindow::frameRectForContentRect_styleMask(frame_probe, style_mask, mtm);
            (
                sf.origin.x + (sf.size.width - WINDOW_WIDTH) / 2.0,
                sf.origin.y + (sf.size.height - frame_rect.size.height) / 2.0,
                WINDOW_WIDTH,
                content_rect_h,
            )
        }
    } else {
        let frame_probe = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WINDOW_WIDTH, content_rect_h),
        );
        let frame_rect = NSWindow::frameRectForContentRect_styleMask(frame_probe, style_mask, mtm);
        (
            sf.origin.x + (sf.size.width - WINDOW_WIDTH) / 2.0,
            sf.origin.y + (sf.size.height - frame_rect.size.height) / 2.0,
            WINDOW_WIDTH,
            content_rect_h,
        )
    };

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(x, y), NSSize::new(win_w, win_h)),
            style_mask,
            NSBackingStoreType(2),
            false,
        )
    };

    window.setTitle(&NSString::from_str("cckit"));
    window.setMinSize(NSSize::new(480.0, MIN_WINDOW_HEIGHT));

    // Dark appearance + transparent title bar
    let dark_name = NSString::from_str("NSAppearanceNameDarkAqua");
    let appearance: *mut AnyObject = unsafe {
        msg_send![
            AnyClass::get(c"NSAppearance").unwrap(),
            appearanceNamed: &*dark_name
        ]
    };
    let _: () = unsafe { msg_send![&*window, setAppearance: appearance] };
    let _: () = unsafe { msg_send![&*window, setTitlebarAppearsTransparent: Bool::YES] };
    let _: () = unsafe { msg_send![&*window, setTitleVisibility: 1_isize] }; // Hidden
    let _: () = unsafe { msg_send![&*window, setOpaque: Bool::NO] };
    let _: () = unsafe { msg_send![&*window, setBackgroundColor: &*NSColor::clearColor()] };

    // Window delegate
    let delegate_cls = get_delegate_class();
    let delegate: Retained<NSObject> = unsafe { msg_send![delegate_cls, new] };
    let _: () = unsafe { msg_send![&*window, setDelegate: &*delegate] };

    // With FullSizeContentView, content view fills the entire frame (including title bar area).
    // Use it directly as root — visual effect view covers title bar for seamless blur.
    let root = window.contentView().ok_or("No content view")?;
    let root_bounds = root.bounds();

    // NSVisualEffectView: fills entire content view (including behind title bar)
    let ve_cls = AnyClass::get(c"NSVisualEffectView").unwrap();
    let effect_view: Retained<NSView> = unsafe {
        let obj: *mut AnyObject = msg_send![ve_cls, alloc];
        let obj: *mut AnyObject = msg_send![obj, initWithFrame: root_bounds];
        Retained::from_raw(obj as *mut NSView).unwrap()
    };
    let _: () = unsafe { msg_send![&*effect_view, setMaterial: 21_isize] }; // UnderWindowBackground
    let _: () = unsafe { msg_send![&*effect_view, setBlendingMode: 0_isize] }; // BehindWindow
    let _: () = unsafe { msg_send![&*effect_view, setState: 1_isize] }; // Active
    effect_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    *EFFECT_VIEW_PTR.lock().unwrap() = Some(&*effect_view as *const NSView as usize);
    root.addSubview(&effect_view);

    // Load and apply config
    let config = load_config();
    *CURRENT_THEME.lock().unwrap() = config.theme;
    *WINDOW_CONFIG.lock().unwrap() = Some(config);
    apply_config();

    // contentLayoutRect = usable area not obscured by title bar
    let layout_rect: NSRect = unsafe { msg_send![&*window, contentLayoutRect] };
    let usable_h = layout_rect.size.height;
    let content_w = layout_rect.size.width.max(WINDOW_WIDTH);
    // Footer at the bottom of the usable area
    let footer = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(content_w, FOOTER_HEIGHT),
        ),
    );
    footer.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewMaxYMargin,
    );

    // Footer left: auto-focus indicator
    let af_rect = NSRect::new(
        NSPoint::new(LEFT_PAD, 3.0),
        NSSize::new(120.0, FOOTER_HEIGHT - 3.0),
    );
    let af_color = color_text();
    let af_label = create_mono_label(
        mtm,
        "auto-focus: \u{2713}",
        af_rect,
        &af_color,
        FONT_SIZE_SMALL,
    );
    *AF_LABEL_PTR.lock().unwrap() = Some(&*af_label as *const NSTextField as usize);
    footer.addSubview(&af_label);

    // Footer right: theme name
    let theme = *CURRENT_THEME.lock().unwrap();
    let theme_text = match theme {
        WindowThemeId::Classic => "classic",
        WindowThemeId::MissionControl => "mission control",
        WindowThemeId::Notch => "notch",
    };
    let footer_right_rect = NSRect::new(
        NSPoint::new(content_w - 140.0 - LEFT_PAD, 3.0),
        NSSize::new(140.0, FOOTER_HEIGHT - 3.0),
    );
    let footer_right_label = create_mono_label(
        mtm,
        theme_text,
        footer_right_rect,
        &color_dim(),
        FONT_SIZE_SMALL,
    );
    let _: () = unsafe { msg_send![&*footer_right_label, setAlignment: 2_isize] };
    footer_right_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
    *THEME_LABEL_PTR.lock().unwrap() = Some(&*footer_right_label as *const NSTextField as usize);
    footer.addSubview(&footer_right_label);

    let footer_sep = create_colored_view(
        mtm,
        NSRect::new(
            NSPoint::new(0.0, FOOTER_HEIGHT - 1.0),
            NSSize::new(content_w, 1.0),
        ),
        &color_border(),
        0.0,
    );
    footer_sep.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable);
    footer.addSubview(&footer_sep);

    root.addSubview(&footer);

    // Scroll view: above footer, within usable area (below title bar)
    let scroll_y = layout_rect.origin.y + FOOTER_HEIGHT;
    let scroll_height = (usable_h - FOOTER_HEIGHT).max(0.0);
    let scroll_rect = NSRect::new(
        NSPoint::new(0.0, scroll_y),
        NSSize::new(content_w, scroll_height),
    );
    let scroll_view = objc2_app_kit::NSScrollView::initWithFrame(
        objc2_app_kit::NSScrollView::alloc(mtm),
        scroll_rect,
    );
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let _: () = unsafe { msg_send![&*scroll_view, setDrawsBackground: Bool::NO] };

    // Document view (custom subclass)
    let view_cls = get_view_class();
    let session_count = SESSION_LIST.lock().unwrap().len();
    let doc_height = (HEADER_HEIGHT + session_count as CGFloat * (CARD_HEIGHT + CARD_SPACING))
        .max(scroll_height);
    let doc_view: Retained<NSView> = unsafe {
        let obj: *mut AnyObject = msg_send![view_cls, alloc];
        let obj: *mut AnyObject = msg_send![obj, initWithFrame: NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(content_w, doc_height),
        )];
        Retained::from_raw(obj as *mut NSView).unwrap()
    };

    *CONTENT_VIEW_PTR.lock().unwrap() = Some(&*doc_view as *const NSView as usize);

    scroll_view.setDocumentView(Some(&doc_view));
    root.addSubview(&scroll_view);

    // Install the initial tracking area for hover popover. AppKit will call
    // `updateTrackingAreas` automatically on subsequent geometry changes.
    unsafe {
        let _: () = msg_send![&*doc_view, updateTrackingAreas];
    }
    request_redraw();

    // Set up standard application menu (provides Cmd+H hide, Cmd+Q quit, Cmd+M minimize)
    setup_main_menu(mtm, app);

    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&doc_view));
    resize_window_to_layout(
        &window,
        calculate_layout_height(),
        win_w,
        max_window_h,
        false,
    );

    // Periodic data refresh (2s interval)
    let data_block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        update_sessions_and_redraw();
    });
    let _data_timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &data_block) };

    // Animation timer — only redraws when data has changed (dirty flag set by load_sessions)
    let anim_block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        if REDRAW_NEEDED.swap(false, std::sync::atomic::Ordering::Relaxed) {
            request_redraw();
        }
    });
    let _anim_timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(1.0, true, &anim_block) };

    // Store window pointer for bring-to-front on state transitions
    *WINDOW_PTR.lock().unwrap() = Some(&*window as *const NSWindow as usize);

    // Keep delegate and window alive for the lifetime of the app.
    // They are moved into static storage since setup_window returns before app.run().
    std::mem::forget(delegate);
    std::mem::forget(window);

    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_content_height_includes_layout_inset() {
        let target = calculate_target_content_height(120.0, 28.0, 240.0);
        assert_eq!(target, 148.0);
    }

    #[test]
    fn target_content_height_respects_max_height() {
        let target = calculate_target_content_height(300.0, 28.0, 240.0);
        assert_eq!(target, 240.0);
    }

    #[test]
    fn target_content_height_respects_min_height() {
        let target = calculate_target_content_height(40.0, 28.0, 240.0);
        assert_eq!(target, MIN_WINDOW_HEIGHT);
    }
}
