// macOS window app for session monitoring

use crate::monitor::focus;
use crate::monitor::session::{Session, SessionStatus};
use crate::monitor::storage::Storage;

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
use objc2::{msg_send, sel, ClassType, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions,
    NSBackingStoreType, NSColor, NSEvent, NSFont, NSScreen, NSTextField, NSView,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSObject, NSPoint, NSRect, NSSize, NSString, NSTimer,
};

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
    let _ = std::process::Command::new("open").arg("-t").arg(&path).spawn();
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
const HINT_FONT_SIZE: CGFloat = 10.5;
const DOT_SIZE: CGFloat = 6.0;
const LEFT_PAD: CGFloat = 10.0;
const TEXT_LEFT: CGFloat = 24.0;

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
            NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 1.0) // green
        }
        SessionStatus::AwaitingApproval => {
            NSColor::colorWithRed_green_blue_alpha(0.984, 0.749, 0.141, 1.0) // yellow #FBBF24
        }
        SessionStatus::WaitingInput => {
            NSColor::colorWithRed_green_blue_alpha(0.961, 0.620, 0.043, 1.0) // amber #F59E0B
        }
        SessionStatus::Stopped => {
            NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 1.0) // red #EF4444
        }
    }
}

fn status_row_bg(status: &SessionStatus) -> Retained<NSColor> {
    match status {
        SessionStatus::Running => {
            NSColor::colorWithRed_green_blue_alpha(0.133, 0.773, 0.369, 0.10) // green tint
        }
        SessionStatus::AwaitingApproval => {
            NSColor::colorWithRed_green_blue_alpha(0.984, 0.749, 0.141, 0.10) // yellow tint
        }
        SessionStatus::Stopped => {
            NSColor::colorWithRed_green_blue_alpha(0.937, 0.267, 0.267, 0.07) // red tint
        }
        SessionStatus::WaitingInput => {
            NSColor::colorWithRed_green_blue_alpha(0.0, 0.0, 0.0, 0.0) // transparent
        }
    }
}

// --- Data ---

static SESSION_LIST: Mutex<Vec<Session>> = Mutex::new(Vec::new());
static SELECTED_INDEX: Mutex<usize> = Mutex::new(0);
static CONTENT_VIEW_PTR: Mutex<Option<usize>> = Mutex::new(None);

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

fn status_label(status: &SessionStatus) -> &'static str {
    match status {
        SessionStatus::Running => "run",
        SessionStatus::AwaitingApproval => "pend",
        SessionStatus::WaitingInput => "wait",
        SessionStatus::Stopped => "done",
    }
}

fn focus_selected() {
    let sessions = SESSION_LIST.lock().unwrap();
    let index = *SELECTED_INDEX.lock().unwrap();

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

    let session_count = SESSION_LIST.lock().unwrap().len();
    if session_count == 0 {
        return;
    }

    let mut index = *SELECTED_INDEX.lock().unwrap();

    match key_code {
        126 => {
            if index > 0 {
                index -= 1;
            }
        }
        125 => {
            if index + 1 < session_count {
                index += 1;
            }
        }
        36 => {
            focus_selected();
            return;
        }
        53 => {
            let mtm = MainThreadMarker::new().unwrap();
            let app = NSApplication::sharedApplication(mtm);
            app.terminate(None);
            return;
        }
        _ => match char_str.as_str() {
            "k" => {
                if index > 0 {
                    index -= 1;
                }
            }
            "j" => {
                if index + 1 < session_count {
                    index += 1;
                }
            }
            "q" => {
                let mtm = MainThreadMarker::new().unwrap();
                let app = NSApplication::sharedApplication(mtm);
                app.terminate(None);
                return;
            }
            c if c.len() == 1 && c.as_bytes()[0].is_ascii_digit() => {
                let n = (c.as_bytes()[0] - b'0') as usize;
                if n >= 1 && n <= session_count {
                    index = n - 1;
                }
            }
            _ => return,
        },
    }

    *SELECTED_INDEX.lock().unwrap() = index;
    request_redraw();
}

fn request_redraw() {
    let ptr = *CONTENT_VIEW_PTR.lock().unwrap();
    if let Some(ptr) = ptr {
        let view = ptr as *mut AnyObject;
        let _: () = unsafe { msg_send![view, setNeedsDisplay: true] };
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
    view.addSubview(&create_mono_label(mtm, &hdr_left, hdr_left_rect, &color_dim(), FONT_SIZE));

    let path_x = TEXT_LEFT + 220.0;
    let hdr_path_rect = NSRect::new(
        NSPoint::new(path_x, 2.0),
        NSSize::new(100.0, HEADER_HEIGHT - 2.0),
    );
    view.addSubview(&create_mono_label(mtm, "PATH", hdr_path_rect, &color_dim(), FONT_SIZE));

    let right_w = 110.0;
    let hdr_right = format!("{:>6}  {:>5}", "TOOL", "AGE");
    let hdr_right_rect = NSRect::new(
        NSPoint::new(view_width - right_w - LEFT_PAD, 2.0),
        NSSize::new(right_w, HEADER_HEIGHT - 2.0),
    );
    let hdr_right_label = create_mono_label(mtm, &hdr_right, hdr_right_rect, &color_dim(), FONT_SIZE);
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
        if i == selected {
            view.addSubview(&create_colored_view(mtm, row_rect, &color_selection(), 4.0));
        } else if session.status != SessionStatus::WaitingInput {
            let tint = status_row_bg(&session.status);
            view.addSubview(&create_colored_view(mtm, row_rect, &tint, 4.0));
        }

        // Status dot
        let dot_y = y + (ROW_HEIGHT - DOT_SIZE) / 2.0;
        view.addSubview(&create_colored_view(
            mtm,
            NSRect::new(
                NSPoint::new(LEFT_PAD, dot_y),
                NSSize::new(DOT_SIZE, DOT_SIZE),
            ),
            &status_color(&session.status),
            DOT_SIZE / 2.0,
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
        view.addSubview(&create_mono_label(mtm, &left_text, left_rect, &text_color, FONT_SIZE));

        // Middle: path (dim, truncate middle)
        let path_x = TEXT_LEFT + 220.0;
        let right_w = 110.0;
        let path_w = (view_width - path_x - right_w - LEFT_PAD).max(40.0);
        let path_rect = NSRect::new(
            NSPoint::new(path_x, y + 2.0),
            NSSize::new(path_w, ROW_HEIGHT - 4.0),
        );
        let path_label = create_mono_label(mtm, &path, path_rect, &color_dim(), 9.5);
        let _: () = unsafe { msg_send![&*path_label, setLineBreakMode: 5_isize] };
        view.addSubview(&path_label);

        // Right: tool + elapsed (right-aligned)
        let right_text = format!("{:>6}  {:>5}", tool, elapsed);
        let right_rect = NSRect::new(
            NSPoint::new(view_width - right_w - LEFT_PAD, y + 2.0),
            NSSize::new(right_w, ROW_HEIGHT - 4.0),
        );
        let right_label = create_mono_label(mtm, &right_text, right_rect, &text_color, FONT_SIZE);
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
        let mut builder =
            ClassBuilder::new(c"CCKitSessionListView", superclass).unwrap();

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

fn get_delegate_class() -> &'static AnyClass {
    REGISTER_DELEGATE_CLASS.call_once(|| {
        let superclass = NSObject::class();
        let mut builder =
            ClassBuilder::new(c"CCKitWindowDelegate", superclass).unwrap();

        unsafe {
            builder.add_method(
                sel!(windowWillClose:),
                window_will_close as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
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
    load_sessions();
    let count = SESSION_LIST.lock().unwrap().len();
    let mut idx = SELECTED_INDEX.lock().unwrap();
    if *idx >= count && count > 0 {
        *idx = count - 1;
    }
    drop(idx);
    request_redraw();
}

fn calculate_content_height() -> CGFloat {
    let session_count = SESSION_LIST.lock().unwrap().len().max(1) as CGFloat;
    HEADER_HEIGHT + 1.0 + session_count * ROW_HEIGHT + FOOTER_HEIGHT
}

// --- Main entry point ---

/// Unified app entry point: shows window + menubar (default), or one of them.
pub fn run_app(menubar_only: bool, window_only: bool) -> Result<(), Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("Must run on main thread")?;
    let app = NSApplication::sharedApplication(mtm);

    let show_window = !menubar_only;
    let show_menubar = !window_only;

    if show_window {
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    } else {
        app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    }
    app.finishLaunching();

    // Menubar (kept alive via _menubar)
    let _menubar = if show_menubar {
        let menubar = std::rc::Rc::new(super::menubar::MenubarApp::new(mtm));
        let menubar_for_timer = menubar.clone();
        let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
            menubar_for_timer.update_menu();
        });
        let _timer = unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &block)
        };
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

fn setup_window(mtm: MainThreadMarker, app: &NSApplication) -> Result<(), Box<dyn std::error::Error>> {
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

    // With FullSizeContentView, frameRectForContentRect returns frame==content (titlebar_h=0).
    // Probe WITHOUT FullSizeContentView to get the actual title bar height.
    let probe_style = NSWindowStyleMask::Titled;
    let probe = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WINDOW_WIDTH, 100.0));
    let probe_frame = NSWindow::frameRectForContentRect_styleMask(probe, probe_style, mtm);
    let titlebar_h = probe_frame.size.height - 100.0;

    let content_rect_h = (needed_h + titlebar_h).clamp(MIN_WINDOW_HEIGHT, max_window_h);

    // Frame height for centering on screen
    let frame_probe = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WINDOW_WIDTH, content_rect_h));
    let frame_rect = NSWindow::frameRectForContentRect_styleMask(frame_probe, style_mask, mtm);
    let frame_h = frame_rect.size.height;

    let x = sf.origin.x + (sf.size.width - WINDOW_WIDTH) / 2.0;
    let y = sf.origin.y + (sf.size.height - frame_h) / 2.0;

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(x, y), NSSize::new(WINDOW_WIDTH, content_rect_h)),
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
        NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable,
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
    // Footer at the bottom of the usable area
    let footer = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(WINDOW_WIDTH, FOOTER_HEIGHT),
        ),
    );
    footer.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewMaxYMargin,
    );

    let hint_rect = NSRect::new(
        NSPoint::new(LEFT_PAD, 3.0),
        NSSize::new(WINDOW_WIDTH - LEFT_PAD * 2.0, FOOTER_HEIGHT - 3.0),
    );
    footer.addSubview(&create_mono_label(
        mtm,
        " \u{2191}\u{2193}/jk navigate   \u{23CE} focus   1-9 jump   q quit",
        hint_rect,
        &color_dim(),
        HINT_FONT_SIZE,
    ));

    let footer_sep = create_colored_view(
        mtm,
        NSRect::new(
            NSPoint::new(0.0, FOOTER_HEIGHT - 1.0),
            NSSize::new(WINDOW_WIDTH, 1.0),
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
        NSSize::new(WINDOW_WIDTH, scroll_height),
    );
    let scroll_view = objc2_app_kit::NSScrollView::initWithFrame(
        objc2_app_kit::NSScrollView::alloc(mtm),
        scroll_rect,
    );
    scroll_view.setHasVerticalScroller(true);
    scroll_view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable
            | NSAutoresizingMaskOptions::ViewHeightSizable,
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
            NSSize::new(WINDOW_WIDTH, doc_height),
        )];
        Retained::from_raw(obj as *mut NSView).unwrap()
    };

    *CONTENT_VIEW_PTR.lock().unwrap() = Some(&*doc_view as *const NSView as usize);

    scroll_view.setDocumentView(Some(&doc_view));
    root.addSubview(&scroll_view);
    window.makeKeyAndOrderFront(None);
    window.makeFirstResponder(Some(&doc_view));

    // Periodic refresh
    let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        update_sessions_and_redraw();
    });
    let _timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(2.0, true, &block) };

    // Keep delegate and window alive for the lifetime of the app.
    // They are moved into static storage since setup_window returns before app.run().
    std::mem::forget(delegate);
    std::mem::forget(window);

    #[allow(deprecated)]
    app.activateIgnoringOtherApps(true);

    Ok(())
}
