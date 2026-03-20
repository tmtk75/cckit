// macOS window app for session monitoring

use crate::monitor::focus;
use crate::monitor::session::{Session, SessionStatus};
use crate::monitor::storage::Storage;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{ClassType, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSColor, NSEvent, NSFont, NSImage, NSMenu, NSMenuItem, NSScreen, NSTextField, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString, NSTimer};

type CGFloat = f64;

use std::path::PathBuf;
use std::sync::{Mutex, Once};

// --- Config ---

#[derive(serde::Deserialize, Clone)]
#[serde(default)]
struct WindowConfig {
    background_opacity: f64,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            background_opacity: 0.5,
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
    *WINDOW_CONFIG.lock().unwrap() = Some(config);
    apply_config();
}

// --- Layout constants ---

const WINDOW_WIDTH: CGFloat = 560.0;
const MIN_WINDOW_HEIGHT: CGFloat = 120.0;
const ROW_HEIGHT: CGFloat = 22.0;
const HEADER_HEIGHT: CGFloat = 20.0;
const FOOTER_HEIGHT: CGFloat = 22.0;
const FONT_SIZE: CGFloat = 11.5;
const FONT_SIZE_SMALL: CGFloat = 10.0;
const HINT_FONT_SIZE: CGFloat = 10.5;
const DOT_SIZE: CGFloat = 6.0;
const LEFT_PAD: CGFloat = 10.0;
const TEXT_LEFT: CGFloat = 24.0;
const FIT_MIN_WIDTH: CGFloat = 560.0;
const FIT_MAX_WIDTH: CGFloat = 760.0;
const FIT_PATH_CHAR_WIDTH: CGFloat = 6.0;
const FIT_PATH_BASE_CHARS: usize = 18;
const FIT_PATH_MAX_CHARS: usize = 34;

// --- Colors ---

fn color_text() -> Retained<NSColor> {
    NSColor::colorWithRed_green_blue_alpha(0.945, 0.961, 0.976, 1.0) // #F1F5F9
}

fn color_dim() -> Retained<NSColor> {
    NSColor::colorWithRed_green_blue_alpha(0.392, 0.455, 0.545, 1.0) // #64748B
}

fn color_border() -> Retained<NSColor> {
    NSColor::colorWithRed_green_blue_alpha(1.0, 1.0, 1.0, 0.08)
}

fn color_selection() -> Retained<NSColor> {
    NSColor::colorWithRed_green_blue_alpha(0.145, 0.388, 0.922, 0.25) // #2563EB @ 25%
}

fn status_color(status: &SessionStatus) -> Retained<NSColor> {
    match status {
        SessionStatus::Running => {
            NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 1.0) // green - active
        }
        SessionStatus::AwaitingApproval => {
            NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 1.0) // red #EF4444 - urgent
        }
        SessionStatus::WaitingInput => {
            NSColor::colorWithRed_green_blue_alpha(0.475, 0.525, 0.596, 1.0) // slate #798798 - idle
        }
        SessionStatus::Stopped => {
            NSColor::colorWithRed_green_blue_alpha(0.345, 0.388, 0.447, 1.0) // dark slate #586072 - done
        }
    }
}

fn status_row_bg(status: &SessionStatus) -> Retained<NSColor> {
    match status {
        SessionStatus::Running => {
            NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 0.10) // green tint
        }
        SessionStatus::AwaitingApproval => {
            NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 0.20) // red tint - urgent
        }
        SessionStatus::WaitingInput => {
            NSColor::colorWithRed_green_blue_alpha(0.0, 0.0, 0.0, 0.0) // transparent - idle
        }
        SessionStatus::Stopped => {
            NSColor::colorWithRed_green_blue_alpha(0.0, 0.0, 0.0, 0.0) // transparent - done
        }
    }
}

// --- Data ---

static SESSION_LIST: Mutex<Vec<Session>> = Mutex::new(Vec::new());
static SELECTED_INDEX: Mutex<Option<usize>> = Mutex::new(None);
static CONTENT_VIEW_PTR: Mutex<Option<usize>> = Mutex::new(None);
static WINDOW_PTR: Mutex<Option<usize>> = Mutex::new(None);
static AF_LABEL_PTR: Mutex<Option<usize>> = Mutex::new(None);
static NOTIFIED_APPROVALS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

/// Per-project auto-focus disabled set (cwd paths). Projects in this set won't trigger auto-focus.
pub static AF_DISABLED_PROJECTS: std::sync::LazyLock<Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

fn load_sessions() {
    let storage = Storage::new();
    let store = storage.load();
    let mut sessions: Vec<Session> = store.sessions.into_values().collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    *SESSION_LIST.lock().unwrap() = sessions;
}

fn format_elapsed(dt: chrono::DateTime<chrono::Utc>) -> String {
    let secs = chrono::Utc::now()
        .signed_duration_since(dt)
        .num_seconds()
        .max(0);
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

fn format_session_stats(session: &Session) -> String {
    let mut parts = Vec::new();
    if session.prompt_count > 0 {
        parts.push(format!("{}p", session.prompt_count));
    }
    if session.tool_count > 0 {
        parts.push(format!("{}t", session.tool_count));
    }
    if session.compact_count > 0 {
        parts.push(format!("{}c", session.compact_count));
    }
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

fn format_tool_duration(session: &Session) -> String {
    // Show live elapsed time if tool is currently running
    if let Some(started) = session.tool_started_at {
        if session.status == SessionStatus::AwaitingApproval {
            let ms = chrono::Utc::now()
                .signed_duration_since(started)
                .num_milliseconds()
                .max(0);
            return format_duration_ms(ms);
        }
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

    (FIT_MIN_WIDTH + extra_chars * FIT_PATH_CHAR_WIDTH).clamp(FIT_MIN_WIDTH, FIT_MAX_WIDTH)
}

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
        drop(sessions);

        match focus::focus_ghostty_tab_by_tty(&tty) {
            Ok(true) => {}
            _ => {
                let _ = focus::focus_ghostty_tab(&project);
            }
        }
    }
}

// --- View helpers ---

fn create_mono_label(
    mtm: MainThreadMarker,
    text: &str,
    rect: NSRect,
    text_color: &NSColor,
    size: CGFloat,
) -> Retained<NSTextField> {
    let label = NSTextField::initWithFrame(NSTextField::alloc(mtm), rect);
    label.setStringValue(&NSString::from_str(text));
    let font = NSFont::monospacedSystemFontOfSize_weight(size, 0.0);
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

    match key_code {
        // Up arrow
        126 => {
            let idx = current.unwrap_or(1);
            *SELECTED_INDEX.lock().unwrap() = Some(idx.saturating_sub(1));
        }
        // Down arrow
        125 => {
            if session_count == 0 {
                return;
            }
            let idx = current.map(|i| i + 1).unwrap_or(0);
            *SELECTED_INDEX.lock().unwrap() = Some(idx.min(session_count - 1));
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
                let idx = current.unwrap_or(1);
                *SELECTED_INDEX.lock().unwrap() = Some(idx.saturating_sub(1));
            }
            "j" => {
                if session_count == 0 {
                    return;
                }
                let idx = current.map(|i| i + 1).unwrap_or(0);
                *SELECTED_INDEX.lock().unwrap() = Some(idx.min(session_count - 1));
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

fn request_redraw() {
    let ptr = *CONTENT_VIEW_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let view = unsafe { &*(ptr as *const NSView) };
        let view_width = unsafe { view.superview() }
            .map(|sv| sv.bounds().size.width)
            .unwrap_or_else(|| view.frame().size.width.max(WINDOW_WIDTH));
        let view_height = view.frame().size.height;
        view.setFrameSize(NSSize::new(view_width, view_height));
        let _: () = unsafe { msg_send![view, setNeedsDisplay: true] };
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
            ("AF:ON".to_string(), color_text())
        } else if off_count == total {
            ("AF:OFF".to_string(), color_dim())
        } else {
            (format!("AF:{}/{}", total - off_count, total), color_dim())
        };
        let label = ptr as *mut AnyObject;
        unsafe {
            let ns_str = NSString::from_str(&text);
            let _: () = msg_send![label, setStringValue: &*ns_str];
            let _: () = msg_send![label, setTextColor: &*color];
        }
    }
}

fn rebuild_view(view: &NSView) {
    let mtm = MainThreadMarker::new().unwrap();
    let sessions = SESSION_LIST.lock().unwrap();
    let selected = *SELECTED_INDEX.lock().unwrap();

    let subviews = view.subviews();
    for subview in subviews.iter() {
        subview.removeFromSuperview();
    }

    let view_width = unsafe { view.superview() }
        .map(|sv| sv.bounds().size.width)
        .unwrap_or(WINDOW_WIDTH);

    let row_count = sessions.len().max(1) as CGFloat;
    let total_height = HEADER_HEIGHT + 1.0 + row_count * ROW_HEIGHT;
    view.setFrameSize(NSSize::new(view_width, total_height));

    // Header (3-column)
    let hdr_left = format!("{:>2}  {:<4}  {}", "#", "STAT", "PROJECT");
    let hdr_left_rect = NSRect::new(
        NSPoint::new(TEXT_LEFT, 2.0),
        NSSize::new(220.0, HEADER_HEIGHT - 2.0),
    );
    view.addSubview(&create_mono_label(
        mtm,
        &hdr_left,
        hdr_left_rect,
        &color_dim(),
        FONT_SIZE,
    ));

    let path_x = TEXT_LEFT + 220.0;
    let hdr_path_rect = NSRect::new(
        NSPoint::new(path_x, 2.0),
        NSSize::new(100.0, HEADER_HEIGHT - 2.0),
    );
    view.addSubview(&create_mono_label(
        mtm,
        "PATH",
        hdr_path_rect,
        &color_dim(),
        FONT_SIZE,
    ));

    let right_w = 230.0;
    let hdr_right = format!("{:>12} {:>6}  {:>5}  {:<2}", "STAT", "TOOL", "AGE", "AF");
    let hdr_right_rect = NSRect::new(
        NSPoint::new(view_width - right_w - LEFT_PAD, 2.0),
        NSSize::new(right_w, HEADER_HEIGHT - 2.0),
    );
    let hdr_right_label = create_mono_label(
        mtm,
        &hdr_right,
        hdr_right_rect,
        &color_dim(),
        FONT_SIZE_SMALL,
    );
    let _: () = unsafe { msg_send![&*hdr_right_label, setAlignment: 1_isize] };
    view.addSubview(&hdr_right_label);

    // Header separator
    view.addSubview(&create_colored_view(
        mtm,
        NSRect::new(
            NSPoint::new(LEFT_PAD, HEADER_HEIGHT),
            NSSize::new(view_width - LEFT_PAD * 2.0, 1.0),
        ),
        &color_border(),
        0.0,
    ));

    let y_start = HEADER_HEIGHT + 1.0;

    if sessions.is_empty() {
        let rect = NSRect::new(
            NSPoint::new(TEXT_LEFT, y_start + 8.0),
            NSSize::new(view_width - TEXT_LEFT - LEFT_PAD, ROW_HEIGHT),
        );
        view.addSubview(&create_mono_label(
            mtm,
            "  no active sessions",
            rect,
            &color_dim(),
            FONT_SIZE,
        ));
        return;
    }

    for (i, session) in sessions.iter().enumerate() {
        let y = y_start + (i as CGFloat) * ROW_HEIGHT;

        // Row background: selection highlight or subtle status tint
        let row_rect = NSRect::new(
            NSPoint::new(4.0, y + 1.0),
            NSSize::new(view_width - 8.0, ROW_HEIGHT - 2.0),
        );
        if Some(i) == selected {
            view.addSubview(&create_colored_view(mtm, row_rect, &color_selection(), 4.0));
        } else {
            let tint = status_row_bg(&session.status);
            view.addSubview(&create_colored_view(mtm, row_rect, &tint, 4.0));
        }

        // Status dot (larger for states needing attention)
        let needs_attention = matches!(
            session.status,
            SessionStatus::AwaitingApproval | SessionStatus::WaitingInput
        );
        let dot = if needs_attention {
            DOT_SIZE + 2.0
        } else {
            DOT_SIZE
        };
        let dot_y = y + (ROW_HEIGHT - dot) / 2.0;
        view.addSubview(&create_colored_view(
            mtm,
            NSRect::new(NSPoint::new(LEFT_PAD, dot_y), NSSize::new(dot, dot)),
            &status_color(&session.status),
            dot / 2.0,
        ));

        let project = session.project_name();
        let path = session.short_cwd();
        let tool = session.last_tool.as_deref().unwrap_or("-");
        let elapsed = format_elapsed(session.updated_at);

        let text_color = if session.status == SessionStatus::Stopped {
            color_dim()
        } else {
            color_text()
        };

        // Left: index + status + project
        let left_text = format!(
            "{:>2}  {:<4}  {}",
            i + 1,
            status_label(&session.status),
            project,
        );
        let left_rect = NSRect::new(
            NSPoint::new(TEXT_LEFT, y + 2.0),
            NSSize::new(220.0, ROW_HEIGHT - 4.0),
        );
        view.addSubview(&create_mono_label(
            mtm,
            &left_text,
            left_rect,
            &text_color,
            FONT_SIZE,
        ));

        // Middle: path (dim, truncate middle)
        let path_x = TEXT_LEFT + 220.0;
        let path_w = (view_width - path_x - 230.0 - LEFT_PAD).max(40.0);
        let path_rect = NSRect::new(
            NSPoint::new(path_x, y + 2.0),
            NSSize::new(path_w, ROW_HEIGHT - 4.0),
        );
        let path_label = create_mono_label(mtm, &path, path_rect, &color_dim(), 9.5);
        let _: () = unsafe { msg_send![&*path_label, setLineBreakMode: 5_isize] };
        view.addSubview(&path_label);

        // Right: stats + tool + elapsed + AF (right-aligned)
        let stats = format_session_stats(session);
        let af_off = AF_DISABLED_PROJECTS.lock().unwrap().contains(&session.cwd);
        let af_col = if af_off { "⏸" } else { "✓" };
        let right_w = 230.0;
        let right_text = format!("{:>12} {:>6}  {:>5}  {:<2}", stats, tool, elapsed, af_col);
        let right_rect = NSRect::new(
            NSPoint::new(view_width - right_w - LEFT_PAD, y + 2.0),
            NSSize::new(right_w, ROW_HEIGHT - 4.0),
        );
        let right_label =
            create_mono_label(mtm, &right_text, right_rect, &text_color, FONT_SIZE_SMALL);
        let _: () = unsafe { msg_send![&*right_label, setAlignment: 1_isize] }; // right
        view.addSubview(&right_label);

        // Row separator
        if i + 1 < sessions.len() {
            view.addSubview(&create_colored_view(
                mtm,
                NSRect::new(
                    NSPoint::new(LEFT_PAD, y + ROW_HEIGHT - 1.0),
                    NSSize::new(view_width - LEFT_PAD * 2.0, 1.0),
                ),
                &color_border(),
                0.0,
            ));
        }
    }
}

extern "C" fn draw_rect(this: *mut AnyObject, _sel: Sel, _dirty_rect: NSRect) {
    let view: &NSView = unsafe { &*(this as *const NSView) };
    rebuild_view(view);
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
            if s.status == SessionStatus::AwaitingApproval && !notified.contains(&s.key()) {
                if let Some(started) = s.tool_started_at {
                    let elapsed_ms = now.signed_duration_since(started).num_milliseconds();
                    if elapsed_ms >= 3000 {
                        approval = true;
                        notified.insert(s.key());
                    }
                }
            }
            if let Some(prev_status) = prev.get(&s.key()) {
                if s.status == SessionStatus::WaitingInput
                    && matches!(
                        prev_status,
                        SessionStatus::Running | SessionStatus::AwaitingApproval
                    )
                {
                    done = true;
                }
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
    request_redraw();
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
    let max_h = sf.size.height * 0.8;
    let content_h = calculate_content_height().clamp(MIN_WINDOW_HEIGHT, max_h);
    let content_w = calculate_fit_window_width().min(max_w);
    let target_content_rect =
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(content_w, content_h));
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
    window.setFrame_display_animate(new_frame, true, true);
    save_window_frame();
}

fn calculate_content_height() -> CGFloat {
    let session_count = SESSION_LIST.lock().unwrap().len().max(1) as CGFloat;
    HEADER_HEIGHT + 1.0 + session_count * ROW_HEIGHT + FOOTER_HEIGHT
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
    let needed_h = calculate_content_height();
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

    let hint_rect = NSRect::new(
        NSPoint::new(LEFT_PAD, 3.0),
        NSSize::new(content_w - LEFT_PAD * 2.0, FOOTER_HEIGHT - 3.0),
    );
    let hint_label = create_mono_label(
        mtm,
        " \u{2191}\u{2193}/jk navigate   \u{23CE} focus   f autofocus   1-9 jump   esc hide",
        hint_rect,
        &color_dim(),
        HINT_FONT_SIZE,
    );
    footer.addSubview(&hint_label);

    // Auto Focus indicator (right side of footer, initially AF:ON)
    let af_rect = NSRect::new(
        NSPoint::new(content_w - 80.0 - LEFT_PAD, 3.0),
        NSSize::new(80.0, FOOTER_HEIGHT - 3.0),
    );
    let af_color = color_text();
    let af_label = create_mono_label(mtm, "AF:ON", af_rect, &af_color, HINT_FONT_SIZE);
    let _: () = unsafe { msg_send![&*af_label, setAlignment: 2_isize] }; // right-align
    af_label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
    *AF_LABEL_PTR.lock().unwrap() = Some(&*af_label as *const NSTextField as usize);
    footer.addSubview(&af_label);

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
    let doc_height =
        (HEADER_HEIGHT + 1.0 + session_count as CGFloat * ROW_HEIGHT).max(scroll_height);
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

    // Set up standard application menu (provides Cmd+H hide, Cmd+Q quit, Cmd+M minimize)
    setup_main_menu(mtm, app);

    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&doc_view));

    // Periodic refresh
    let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        update_sessions_and_redraw();
    });
    let _timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &block) };

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
