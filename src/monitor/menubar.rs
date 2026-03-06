// macOS menubar implementation using objc2

use super::focus;
use super::notification::{MenubarPosition, save_menubar_position};
use super::session::{Session, SessionStatus};
use super::storage::Storage;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{ClassType, MainThreadOnly, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSFont, NSImage, NSMenu, NSMenuItem, NSStatusBar,
    NSStatusItem, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSDefaultRunLoopMode, NSObject, NSSize, NSString, NSTimer,
};
use std::collections::HashMap;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, Once};

// Global storage for session info indexed by menu item tag
static SESSION_TTYS: Mutex<Option<HashMap<isize, String>>> = Mutex::new(None);
static SESSION_CWDS: Mutex<Option<HashMap<isize, String>>> = Mutex::new(None);

// TUI TTY storage (tag = -1 reserved for TUI)
static TUI_TTY: Mutex<Option<String>> = Mutex::new(None);
const TUI_MENU_TAG: isize = -1;

// Cache for terminal app detection (TTY -> app name)
static TERMINAL_CACHE: Mutex<Option<HashMap<String, Option<String>>>> = Mutex::new(None);
static SHOULD_QUIT: AtomicBool = AtomicBool::new(false);

fn get_cached_terminal_app(tty: &str) -> Option<String> {
    let mut cache = TERMINAL_CACHE.lock().unwrap();
    if cache.is_none() {
        *cache = Some(HashMap::new());
    }
    let cache = cache.as_mut().unwrap();

    if let Some(cached) = cache.get(tty) {
        return cached.clone();
    }

    let result = detect_terminal_app(tty);
    cache.insert(tty.to_string(), result.clone());
    result
}

/// Detect which terminal app owns a TTY by checking parent processes
fn detect_terminal_app(tty: &str) -> Option<String> {
    // First, try to find tmux client TTY for this pane
    let client_tty = get_tmux_client_tty(tty).unwrap_or_else(|| tty.to_string());

    // Get the first process on the client TTY and trace its parents
    let tty_short = client_tty.trim_start_matches("/dev/");
    let output = Command::new("ps")
        .args(["-t", tty_short, "-o", "pid="])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let first_pid: i32 = stdout.lines().next()?.trim().parse().ok()?;

    // Trace parent processes to find terminal app
    trace_parent_for_terminal(first_pid)
}

/// Get the tmux client TTY for a pane TTY
fn get_tmux_client_tty(pane_tty: &str) -> Option<String> {
    // Get session for this pane TTY
    let output = Command::new("tmux")
        .args(["list-panes", "-a", "-F", "#{pane_tty}|#{session_name}"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut session_name = None;

    for line in stdout.lines() {
        let parts: Vec<&str> = line.split('|').collect();
        if parts.len() >= 2 && parts[0] == pane_tty {
            session_name = Some(parts[1].to_string());
            break;
        }
    }

    let session_name = session_name?;

    // Get client TTY for this session
    let output = Command::new("tmux")
        .args(["list-clients", "-t", &session_name, "-F", "#{client_tty}"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().next().map(|s| s.to_string())
}

/// Trace parent processes to find a terminal app
fn trace_parent_for_terminal(mut pid: i32) -> Option<String> {
    for _ in 0..10 {
        // Max 10 levels
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "ppid=,comm="])
            .output()
            .ok()?;

        if !output.status.success() {
            return None;
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.trim();

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            return None;
        }

        let ppid: i32 = parts[0].parse().ok()?;
        let comm = parts[1..].join(" ").to_lowercase();

        if comm.contains("ghostty") {
            return Some("Ghostty".to_string());
        } else if comm.contains("iterm") {
            return Some("iTerm".to_string());
        } else if comm.contains("terminal.app") || comm == "terminal" {
            return Some("Terminal".to_string());
        } else if comm.contains("alacritty") {
            return Some("Alacritty".to_string());
        } else if comm.contains("kitty") {
            return Some("kitty".to_string());
        } else if comm.contains("wezterm") {
            return Some("WezTerm".to_string());
        }

        if ppid <= 1 {
            break;
        }
        pid = ppid;
    }

    None
}

// Icon size configuration
static ICON_SIZE: Mutex<f64> = Mutex::new(16.0); // Default: 16px

// Menu update interval configuration
static MENU_UPDATE_INTERVAL_MS: Mutex<u64> = Mutex::new(2000); // Default: 2000ms

// Menubar display style
#[derive(Clone, Copy, PartialEq)]
pub enum MenubarStyle {
    Emoji,    // ▶️ 2/1/5   |  ▶️ project [Bash] ~/path
    Terminal, // CC 2R 1T 5  |  ● project    Bash  3m
    Htop,     // [2/1/5]     |  [RUN] project  Bash  3m
    Compact,  // ● 2·1·5     |  ● project:Bash 3m
}

impl MenubarStyle {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "emoji" => Some(Self::Emoji),
            "terminal" | "term" => Some(Self::Terminal),
            "htop" => Some(Self::Htop),
            "compact" => Some(Self::Compact),
            _ => None,
        }
    }

    fn all() -> &'static [MenubarStyle] {
        &[Self::Emoji, Self::Terminal, Self::Htop, Self::Compact]
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Emoji => "Emoji",
            Self::Terminal => "Terminal",
            Self::Htop => "htop",
            Self::Compact => "Compact",
        }
    }

    fn status_title(&self, running: usize, tooling: usize, waiting: usize, total: usize) -> String {
        match self {
            Self::Emoji => {
                let icon = if waiting > 0 {
                    "🔵"
                } else if tooling > 0 {
                    "🟡"
                } else if running > 0 {
                    "🟢"
                } else {
                    "⚫"
                };
                format!("{} {}/{}/{}", icon, running, tooling, total)
            }
            Self::Terminal => {
                let indicator = if waiting > 0 {
                    "◆"
                } else if tooling > 0 {
                    "◇"
                } else if running > 0 {
                    "●"
                } else {
                    "○"
                };
                format!("{} {}R {}T {}", indicator, running, tooling, total)
            }
            Self::Htop => {
                format!("[{}/{}/{}]", running, tooling, total)
            }
            Self::Compact => {
                let dot = if waiting > 0 {
                    "◆"
                } else if tooling > 0 {
                    "◇"
                } else if running > 0 {
                    "●"
                } else {
                    "○"
                };
                format!("{} {}·{}·{}", dot, running, tooling, total)
            }
        }
    }

    fn legend(&self) -> &'static str {
        match self {
            Self::Emoji => "run / approval / total",
            Self::Terminal => "R=run  T=approval  total",
            Self::Htop => "run / approval / total",
            Self::Compact => "run · approval · total",
        }
    }

    fn session_label(&self, session: &Session) -> String {
        let project = session.project_name();
        let tool = session.last_tool.as_deref().unwrap_or("-");
        let elapsed = format_menu_elapsed(session.updated_at);
        let stats = format_menu_stats(session);

        match self {
            Self::Emoji => {
                let icon = match session.status {
                    SessionStatus::Running => "🟢",
                    SessionStatus::AwaitingApproval => "🟡",
                    SessionStatus::WaitingInput => "🔵",
                    SessionStatus::Stopped => "⚫",
                };
                format!(
                    "{} {} [{}] {}{}",
                    icon,
                    project,
                    tool,
                    session.short_cwd(),
                    stats
                )
            }
            Self::Terminal => {
                let dot = match session.status {
                    SessionStatus::Running => "●",
                    SessionStatus::AwaitingApproval => "◆",
                    SessionStatus::WaitingInput => "◇",
                    SessionStatus::Stopped => "×",
                };
                format!(
                    "{} {:<14} {:<6} {:>4}{}",
                    dot, project, tool, elapsed, stats
                )
            }
            Self::Htop => {
                let tag = match session.status {
                    SessionStatus::Running => "[RUN]",
                    SessionStatus::AwaitingApproval => "[TOOL]",
                    SessionStatus::WaitingInput => "[WAIT]",
                    SessionStatus::Stopped => "[DONE]",
                };
                format!(
                    "{} {:<14} {:<6} {:>4}{}",
                    tag, project, tool, elapsed, stats
                )
            }
            Self::Compact => {
                let dot = match session.status {
                    SessionStatus::Running => "●",
                    SessionStatus::AwaitingApproval => "◆",
                    SessionStatus::WaitingInput => "◇",
                    SessionStatus::Stopped => "·",
                };
                format!("{} {}:{} {}{}", dot, project, tool, elapsed, stats)
            }
        }
    }
}

static MENUBAR_STYLE: Mutex<MenubarStyle> = Mutex::new(MenubarStyle::Terminal);

pub fn set_style(style: MenubarStyle) {
    *MENUBAR_STYLE.lock().unwrap() = style;
}

fn current_style() -> MenubarStyle {
    *MENUBAR_STYLE.lock().unwrap()
}

fn format_menu_elapsed(dt: chrono::DateTime<chrono::Utc>) -> String {
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

fn format_menu_stats(session: &Session) -> String {
    let mut parts = Vec::new();
    if session.prompt_count > 0 {
        parts.push(format!("{}p", session.prompt_count));
    }
    if session.compact_count > 0 {
        parts.push(format!("{}c", session.compact_count));
    }
    if parts.is_empty() {
        String::new()
    } else {
        format!(" ({})", parts.join("/"))
    }
}

pub fn set_icon_size(size: f64) {
    *ICON_SIZE.lock().unwrap() = size;
}

pub fn set_update_interval(interval_ms: u64) {
    *MENU_UPDATE_INTERVAL_MS.lock().unwrap() = interval_ms;
}

/// Get the icon for an application by name
fn get_app_icon(app_name: &str, _mtm: MainThreadMarker) -> Option<Retained<NSImage>> {
    let workspace = NSWorkspace::sharedWorkspace();

    // Try to find the app path
    let app_bundle_id = match app_name {
        "Ghostty" => "com.mitchellh.ghostty",
        "iTerm" => "com.googlecode.iterm2",
        "Terminal" => "com.apple.Terminal",
        "Alacritty" => "org.alacritty",
        "kitty" => "net.kovidgoyal.kitty",
        "WezTerm" => "com.github.wez.wezterm",
        _ => return None,
    };

    let bundle_id = NSString::from_str(app_bundle_id);
    let url = workspace.URLForApplicationWithBundleIdentifier(&bundle_id)?;
    let path = url.path()?;

    let icon = workspace.iconForFile(&path);

    // Resize icon
    let icon_size = *ICON_SIZE.lock().unwrap();
    let size = NSSize::new(icon_size, icon_size);
    icon.setSize(size);

    Some(icon)
}

fn store_session_info(ttys: HashMap<isize, String>, cwds: HashMap<isize, String>) {
    *SESSION_TTYS.lock().unwrap() = Some(ttys);
    *SESSION_CWDS.lock().unwrap() = Some(cwds);
}

fn get_session_tty(tag: isize) -> Option<String> {
    SESSION_TTYS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&tag).cloned())
}

fn get_session_cwd(tag: isize) -> Option<String> {
    SESSION_CWDS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(&tag).cloned())
}

fn store_tui_tty(tty: Option<String>) {
    *TUI_TTY.lock().unwrap() = tty;
}

fn get_tui_tty() -> Option<String> {
    TUI_TTY.lock().unwrap().clone()
}

// Action handler called when menu item is clicked
extern "C" fn focus_session_action(_this: *mut AnyObject, _cmd: Sel, sender: *mut AnyObject) {
    if sender.is_null() {
        return;
    }
    let sender: &NSMenuItem = unsafe { &*(sender as *const NSMenuItem) };
    let tag = sender.tag();

    // Handle TUI menu item
    if tag == TUI_MENU_TAG {
        if let Some(tty) = get_tui_tty() {
            let _ = focus::focus_ghostty_tab_by_tty(&tty);
        }
        return;
    }

    // Try TTY-based focus first (works with tmux)
    if let Some(tty) = get_session_tty(tag) {
        if let Ok(true) = focus::focus_ghostty_tab_by_tty(&tty) {
            return;
        }
    }

    // Fallback to project name matching
    if let Some(cwd) = get_session_cwd(tag) {
        let project_name = std::path::Path::new(&cwd)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&cwd);
        let _ = focus::focus_ghostty_tab(project_name);
    }
}

// Action handler for style switching (tag encodes style index)
extern "C" fn switch_style_action(_this: *mut AnyObject, _cmd: Sel, sender: *mut AnyObject) {
    if sender.is_null() {
        return;
    }
    let sender: &NSMenuItem = unsafe { &*(sender as *const NSMenuItem) };
    let tag = sender.tag() as usize;
    let styles = MenubarStyle::all();
    if tag < styles.len() {
        set_style(styles[tag]);
    }
}

// Action handler called when quit menu item is clicked
extern "C" fn quit_app_action(_this: *mut AnyObject, _cmd: Sel, _sender: *mut AnyObject) {
    SHOULD_QUIT.store(true, Ordering::SeqCst);
    if let Some(mtm) = MainThreadMarker::new() {
        let app = NSApplication::sharedApplication(mtm);
        unsafe {
            let _: () = msg_send![&app, terminate: std::ptr::null::<AnyObject>()];
        }
    }
}

static REGISTER_HANDLER: Once = Once::new();
static mut HANDLER_CLASS: Option<&'static AnyClass> = None;

fn get_handler_class() -> &'static AnyClass {
    REGISTER_HANDLER.call_once(|| {
        let superclass = NSObject::class();
        let mut builder = ClassBuilder::new(c"CCKitMenuHandler", superclass).unwrap();

        unsafe {
            builder.add_method(
                sel!(focusSession:),
                focus_session_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(quitApp:),
                quit_app_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
            builder.add_method(
                sel!(switchStyle:),
                switch_style_action as extern "C" fn(*mut AnyObject, Sel, *mut AnyObject),
            );
        }

        let cls = builder.register();
        unsafe {
            HANDLER_CLASS = Some(cls);
        }
    });

    unsafe { HANDLER_CLASS.unwrap() }
}

fn create_handler() -> Retained<NSObject> {
    let cls = get_handler_class();
    unsafe { msg_send![cls, new] }
}

pub struct MenubarApp {
    status_item: Retained<NSStatusItem>,
    storage: Storage,
    handler: Retained<NSObject>,
    mtm: MainThreadMarker,
    last_update: std::time::Instant,
}

impl MenubarApp {
    pub fn new(mtm: MainThreadMarker) -> Self {
        let status_bar = NSStatusBar::systemStatusBar();
        let status_item = status_bar.statusItemWithLength(-1.0); // NSVariableStatusItemLength

        let storage = Storage::new();
        let handler = create_handler();

        let app = Self {
            status_item,
            storage,
            handler,
            mtm,
            last_update: std::time::Instant::now(),
        };

        app.update_menu();
        app
    }

    fn get_status_title(&self) -> String {
        let store = self.storage.load();
        let sessions: Vec<&Session> = store.sessions.values().collect();
        let total = sessions.len();

        if sessions.is_empty() {
            return "CC".to_string();
        }

        let running = sessions
            .iter()
            .filter(|s| s.status == SessionStatus::Running)
            .count();
        let tooling = sessions
            .iter()
            .filter(|s| s.status == SessionStatus::AwaitingApproval)
            .count();
        let waiting = sessions
            .iter()
            .filter(|s| s.status == SessionStatus::WaitingInput)
            .count();

        current_style().status_title(running, tooling, waiting, total)
    }

    pub fn update_menu(&self) {
        let title = self.get_status_title();
        let title_ns = NSString::from_str(&title);

        if let Some(button) = self.status_item.button(self.mtm) {
            button.setTitle(&title_ns);
            set_button_mono_font(&button);
        }

        let menu = NSMenu::new(self.mtm);
        let store = self.storage.load();

        // Add TUI item if TUI is running
        if let Some(tui_state) = self.storage.load_tui_state() {
            store_tui_tty(Some(tui_state.tty.clone()));

            let item = create_menu_item(self.mtm, "📺 TUI", None);
            item.setTag(TUI_MENU_TAG);

            // Set terminal app icon
            if let Some(app_name) = get_cached_terminal_app(&tui_state.tty) {
                if let Some(icon) = get_app_icon(&app_name, self.mtm) {
                    item.setImage(Some(&icon));
                }
            }

            unsafe {
                item.setAction(Some(objc2::sel!(focusSession:)));
                item.setTarget(Some(&self.handler));
            }

            menu.addItem(&item);

            // Separator after TUI
            let separator = NSMenuItem::separatorItem(self.mtm);
            menu.addItem(&separator);
        } else {
            store_tui_tty(None);
        }

        // Legend item (disabled, explains status bar format)
        let style = current_style();
        let legend_item = create_menu_item(self.mtm, style.legend(), None);
        unsafe { let _: () = msg_send![&*legend_item, setEnabled: false]; }
        menu.addItem(&legend_item);
        let separator_legend = NSMenuItem::separatorItem(self.mtm);
        menu.addItem(&separator_legend);

        let mut sessions: Vec<&Session> = store.sessions.values().collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

        if sessions.is_empty() {
            let item = create_menu_item(self.mtm, "No active sessions", None);
            menu.addItem(&item);
        } else {
            let mut tty_map = HashMap::new();
            let mut cwd_map = HashMap::new();

            let style = current_style();
            for (idx, session) in sessions.iter().enumerate() {
                let label = style.session_label(session);

                let item = create_menu_item(self.mtm, &label, None);
                let tag = idx as isize;
                item.setTag(tag);
                tty_map.insert(tag, session.tty.clone());
                cwd_map.insert(tag, session.cwd.clone());

                // Set terminal app icon (terminal detection cached)
                if let Some(app_name) = get_cached_terminal_app(&session.tty) {
                    if let Some(icon) = get_app_icon(&app_name, self.mtm) {
                        item.setImage(Some(&icon));
                    }
                }

                // Set action and target for focus
                unsafe {
                    item.setAction(Some(objc2::sel!(focusSession:)));
                    item.setTarget(Some(&self.handler));
                }

                menu.addItem(&item);
            }

            store_session_info(tty_map, cwd_map);
        }

        // Separator
        let separator = NSMenuItem::separatorItem(self.mtm);
        menu.addItem(&separator);

        // Style submenu
        let style_menu = NSMenu::new(self.mtm);
        let cur = current_style();
        for (i, s) in MenubarStyle::all().iter().enumerate() {
            let check = if *s == cur { "✓ " } else { "   " };
            let label = format!("{}{}", check, s.label());
            let item = create_menu_item(self.mtm, &label, None);
            item.setTag(i as isize);
            unsafe {
                item.setAction(Some(objc2::sel!(switchStyle:)));
                item.setTarget(Some(&self.handler));
            }
            style_menu.addItem(&item);
        }
        let style_item = create_menu_item(self.mtm, "Style", None);
        style_item.setSubmenu(Some(&style_menu));
        menu.addItem(&style_item);

        // Separator before quit
        let separator2 = NSMenuItem::separatorItem(self.mtm);
        menu.addItem(&separator2);

        // Quit item
        let quit_item = create_menu_item(self.mtm, "Quit", Some("q"));
        unsafe {
            quit_item.setAction(Some(objc2::sel!(quitApp:)));
            quit_item.setTarget(Some(&self.handler));
        }
        menu.addItem(&quit_item);

        self.status_item.setMenu(Some(&menu));

        // Save menubar position for notification alignment
        self.save_position();
    }

    /// Save the current status item position to shared file
    fn save_position(&self) {
        if let Some(button) = self.status_item.button(self.mtm) {
            // Get button frame in window coordinates
            let frame = button.frame();

            // Get the window to convert to screen coordinates
            if let Some(window) = button.window() {
                // Convert frame to screen coordinates
                let screen_rect = window.convertRectToScreen(frame);

                // Calculate center x and bottom y of the button
                let center_x = screen_rect.origin.x + screen_rect.size.width / 2.0;
                let bottom_y = screen_rect.origin.y; // Bottom of menubar button

                let pos = MenubarPosition {
                    x: center_x,
                    y: bottom_y,
                    width: screen_rect.size.width,
                    timestamp: chrono::Utc::now().timestamp(),
                };

                // Save asynchronously (ignore errors)
                let _ = save_menubar_position(&pos);
            }
        }
    }
}

const MENU_FONT_SIZE: f64 = 11.0;
const STATUS_FONT_SIZE: f64 = 11.0;

fn create_menu_item(mtm: MainThreadMarker, title: &str, key: Option<&str>) -> Retained<NSMenuItem> {
    let title_ns = NSString::from_str(title);
    let key_ns = NSString::from_str(key.unwrap_or(""));
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &title_ns,
            None,
            &key_ns,
        )
    };

    // Apply monospace font via NSAttributedString
    set_menu_item_mono_font(&item, title, MENU_FONT_SIZE);
    item
}

fn set_menu_item_mono_font(item: &NSMenuItem, text: &str, size: f64) {
    let font = NSFont::monospacedSystemFontOfSize_weight(size, 0.0);
    let text_ns = NSString::from_str(text);
    let font_key = NSString::from_str("NSFont");

    unsafe {
        // NSDictionary with font attribute
        let dict_cls = AnyClass::get(c"NSDictionary").unwrap();
        let attrs: *mut AnyObject =
            msg_send![dict_cls, dictionaryWithObject: &*font, forKey: &*font_key];

        // NSAttributedString
        let attr_cls = AnyClass::get(c"NSAttributedString").unwrap();
        let attr_str: *mut AnyObject = msg_send![attr_cls, alloc];
        let attr_str: *mut AnyObject =
            msg_send![attr_str, initWithString: &*text_ns, attributes: attrs];

        let _: () = msg_send![item, setAttributedTitle: attr_str];
    }
}

fn set_button_mono_font(button: &objc2_app_kit::NSStatusBarButton) {
    let font = NSFont::monospacedSystemFontOfSize_weight(STATUS_FONT_SIZE, 0.0);
    button.setFont(Some(&font));
}

/// Initialize menubar without blocking. Returns MenubarApp that must be kept alive.
/// Call `poll_menubar()` periodically to process events.
pub fn init_menubar() -> Result<MenubarApp, Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("Must run on main thread")?;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    // Finish launching so menu bar icon appears
    app.finishLaunching();

    Ok(MenubarApp::new(mtm))
}

/// Process pending NSApplication events without blocking.
/// Call this periodically from your main loop.
pub fn poll_menubar(menubar: &mut MenubarApp) {
    let app = NSApplication::sharedApplication(menubar.mtm);

    // Process all pending events
    loop {
        let event = unsafe {
            app.nextEventMatchingMask_untilDate_inMode_dequeue(
                objc2_app_kit::NSEventMask::Any,
                None, // Don't wait
                NSDefaultRunLoopMode,
                true,
            )
        };

        match event {
            Some(e) => {
                app.sendEvent(&e);
            }
            None => break,
        }
    }

    // Update menu periodically
    let update_interval_ms = *MENU_UPDATE_INTERVAL_MS.lock().unwrap();
    if menubar.last_update.elapsed() >= std::time::Duration::from_millis(update_interval_ms) {
        menubar.update_menu();
        menubar.last_update = std::time::Instant::now();
    }
}

/// Standalone menubar mode - blocks forever
pub fn run_menubar() -> Result<(), Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("Must run on main thread")?;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    let _menubar_app = MenubarApp::new(mtm);

    app.run();

    Ok(())
}

/// Menubar mode with polling loop (supports Ctrl+C)
pub fn run_menubar_with_polling(poll_interval_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let should_quit = Arc::new(AtomicBool::new(false));
    let should_quit_ctrlc = should_quit.clone();

    // Handle Ctrl+C
    ctrlc::set_handler(move || {
        should_quit_ctrlc.store(true, Ordering::SeqCst);
    })?;

    let mut menubar = init_menubar()?;

    // Keep processing events until Ctrl+C or Quit menu
    while !should_quit.load(Ordering::SeqCst) && !SHOULD_QUIT.load(Ordering::SeqCst) {
        poll_menubar(&mut menubar);
        std::thread::sleep(std::time::Duration::from_millis(poll_interval_ms));
    }

    Ok(())
}

/// Menubar app mode (no Ctrl+C handler)
pub fn run_menubar_app(poll_interval_ms: u64) -> Result<(), Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("Must run on main thread")?;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.finishLaunching();

    let menubar = std::rc::Rc::new(MenubarApp::new(mtm));
    let menubar_for_timer = menubar.clone();
    let interval = (poll_interval_ms.max(100) as f64) / 1000.0;

    let block = block2::RcBlock::new(move |_timer: std::ptr::NonNull<NSTimer>| {
        if SHOULD_QUIT.load(Ordering::SeqCst) {
            return;
        }
        menubar_for_timer.update_menu();
    });

    let _timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(interval, true, &block) };

    app.run();

    Ok(())
}
