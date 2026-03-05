// macOS custom notification window implementation

use objc2::rc::Retained;
use objc2::{MainThreadOnly, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSFont, NSScreen,
    NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSDefaultRunLoopMode, NSPoint, NSRect, NSSize, NSString};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Menubar position info saved by menubar app
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MenubarPosition {
    /// X coordinate of menubar button center (screen coordinates)
    pub x: f64,
    /// Y coordinate of menubar button bottom (screen coordinates)
    pub y: f64,
    /// Width of the menubar button
    pub width: f64,
    /// Timestamp when this was saved
    pub timestamp: i64,
}

const MENUBAR_POSITION_FILE: &str = "menubar_position.json";

fn get_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not find home directory")
                .join(".local/share")
        })
        .join("cckit")
}

/// Load menubar position from shared file
pub fn load_menubar_position() -> Option<MenubarPosition> {
    let path = get_data_dir().join(MENUBAR_POSITION_FILE);
    let content = std::fs::read_to_string(&path).ok()?;
    let pos: MenubarPosition = serde_json::from_str(&content).ok()?;

    // Check if position is stale (older than 60 seconds)
    let now = chrono::Utc::now().timestamp();
    if now - pos.timestamp > 60 {
        return None;
    }

    Some(pos)
}

/// Save menubar position to shared file
pub fn save_menubar_position(pos: &MenubarPosition) -> std::io::Result<()> {
    let dir = get_data_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(MENUBAR_POSITION_FILE);
    let content = serde_json::to_string(pos)?;
    std::fs::write(path, content)
}

type CGFloat = f64;

#[derive(Debug, Clone, Copy, Default)]
pub enum Position {
    LeftTop,
    CenterTop,
    #[default]
    RightTop,
    LeftCenter,
    CenterCenter,
    RightCenter,
    LeftBottom,
    CenterBottom,
    RightBottom,
    /// Position below the menubar icon (reads position from shared file)
    Menubar,
}

impl Position {
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.to_lowercase().as_str() {
            "left-top" | "lt" => Ok(Position::LeftTop),
            "center-top" | "ct" => Ok(Position::CenterTop),
            "right-top" | "rt" => Ok(Position::RightTop),
            "left-center" | "lc" => Ok(Position::LeftCenter),
            "center-center" | "center" | "cc" => Ok(Position::CenterCenter),
            "right-center" | "rc" => Ok(Position::RightCenter),
            "left-bottom" | "lb" => Ok(Position::LeftBottom),
            "center-bottom" | "cb" => Ok(Position::CenterBottom),
            "right-bottom" | "rb" => Ok(Position::RightBottom),
            "menubar" | "mb" => Ok(Position::Menubar),
            _ => Err(format!(
                "Invalid position: '{}'. Valid values: left-top, center-top, right-top, left-center, center-center, right-center, left-bottom, center-bottom, right-bottom, menubar (or abbreviations: lt, ct, rt, lc, cc, rc, lb, cb, rb, mb)",
                s
            )),
        }
    }
}

#[allow(dead_code)]
pub struct NotifyOptions {
    pub title: String,
    pub subtitle: Option<String>,
    pub message: String,
    pub sound: Option<String>,
    pub duration_ms: u64,
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub position: Position,
    pub margin: Option<f64>,
    pub opacity: Option<f64>,
    pub bgcolor: Option<String>,
}

pub fn parse_hex_color(s: &str) -> Result<(f64, f64, f64), String> {
    let s = s.trim_start_matches('#');
    if s.len() != 3 && s.len() != 6 {
        return Err(format!(
            "Invalid hex color: '{}'. Expected #RGB or #RRGGBB",
            s
        ));
    }

    let (r, g, b) = if s.len() == 3 {
        let r = u8::from_str_radix(&s[0..1], 16).map_err(|_| "Invalid red")? * 17;
        let g = u8::from_str_radix(&s[1..2], 16).map_err(|_| "Invalid green")? * 17;
        let b = u8::from_str_radix(&s[2..3], 16).map_err(|_| "Invalid blue")? * 17;
        (r, g, b)
    } else {
        let r = u8::from_str_radix(&s[0..2], 16).map_err(|_| "Invalid red")?;
        let g = u8::from_str_radix(&s[2..4], 16).map_err(|_| "Invalid green")?;
        let b = u8::from_str_radix(&s[4..6], 16).map_err(|_| "Invalid blue")?;
        (r, g, b)
    };

    Ok((r as f64 / 255.0, g as f64 / 255.0, b as f64 / 255.0))
}

impl Default for NotifyOptions {
    fn default() -> Self {
        Self {
            title: "cckit".to_string(),
            subtitle: None,
            message: String::new(),
            sound: None,
            duration_ms: 3000,
            width: None,
            height: None,
            position: Position::default(),
            margin: None,
            opacity: None,
            bgcolor: None,
        }
    }
}

const DEFAULT_WIDTH: CGFloat = 320.0;
const MIN_HEIGHT: CGFloat = 60.0;
const MAX_HEIGHT: CGFloat = 300.0;
const DEFAULT_MARGIN: CGFloat = 8.0;
const DEFAULT_OPACITY: CGFloat = 0.84;
const DEFAULT_BGCOLOR: &str = "#333";
const CORNER_RADIUS: CGFloat = 4.0;
const PADDING: CGFloat = 8.0;

const TITLE_HEIGHT: CGFloat = 20.0;
const SUBTITLE_HEIGHT: CGFloat = 16.0;
const TITLE_FONT_SIZE: CGFloat = 14.0;
const SUBTITLE_FONT_SIZE: CGFloat = 12.0;
const MESSAGE_FONT_SIZE: CGFloat = 12.0;

pub fn send_notify(opts: NotifyOptions) -> Result<(), Box<dyn std::error::Error>> {
    let mtm = MainThreadMarker::new().ok_or("Must run on main thread")?;

    let window_width = opts.width.unwrap_or(DEFAULT_WIDTH);
    let margin = opts.margin.unwrap_or(DEFAULT_MARGIN);
    let opacity = opts.opacity.unwrap_or(DEFAULT_OPACITY);
    let bgcolor_str = opts.bgcolor.as_deref().unwrap_or(DEFAULT_BGCOLOR);
    let (bg_r, bg_g, bg_b) = parse_hex_color(bgcolor_str)?;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);

    // Calculate content width for text measurement
    let content_width = window_width - PADDING * 2.0;

    // Calculate message height based on text content
    let msg_height = calculate_text_height(mtm, &opts.message, content_width, MESSAGE_FONT_SIZE);

    // Calculate total window height
    let has_subtitle = opts.subtitle.as_ref().is_some_and(|s| !s.is_empty());
    let header_height = if has_subtitle {
        TITLE_HEIGHT + SUBTITLE_HEIGHT + PADDING
    } else {
        TITLE_HEIGHT + PADDING
    };

    let window_height = if let Some(h) = opts.height {
        h // Use explicit height if provided
    } else {
        let calculated = header_height + msg_height + PADDING * 2.0;
        calculated.clamp(MIN_HEIGHT, MAX_HEIGHT)
    };

    // Get screen size
    let screen = NSScreen::mainScreen(mtm).ok_or("No main screen")?;
    let screen_frame = screen.frame();
    let visible_frame = screen.visibleFrame();

    // Calculate window position based on Position
    let (x, y) = calculate_position(
        opts.position,
        screen_frame,
        visible_frame,
        window_width,
        window_height,
        margin,
    );

    let window_rect = NSRect::new(NSPoint::new(x, y), NSSize::new(window_width, window_height));

    // Create borderless window
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            window_rect,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType(2), // NSBackingStoreBuffered
            false,
        )
    };

    // Configure window
    window.setLevel(25); // NSStatusWindowLevel (floating above everything)
    window.setOpaque(false);
    window.setBackgroundColor(Some(&NSColor::clearColor()));
    window.setHasShadow(true);
    window.setMovableByWindowBackground(true);

    // Create content view with solid background color
    let content_rect = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(window_width, window_height),
    );
    let content_view = NSView::initWithFrame(NSView::alloc(mtm), content_rect);
    content_view.setWantsLayer(true);

    // Set corner radius and background color
    if let Some(layer) = content_view.layer() {
        layer.setCornerRadius(CORNER_RADIUS);
        layer.setMasksToBounds(true);
        // Set background color
        let bg_color = NSColor::colorWithRed_green_blue_alpha(bg_r, bg_g, bg_b, 1.0);
        let cg_color = bg_color.CGColor();
        layer.setBackgroundColor(Some(&cg_color));
    }

    // Calculate vertical positions
    let title_y = window_height - TITLE_HEIGHT - PADDING;

    // Create title label
    let title_rect = NSRect::new(
        NSPoint::new(PADDING, title_y),
        NSSize::new(content_width, TITLE_HEIGHT),
    );
    let title_label = create_label(mtm, &opts.title, title_rect, TITLE_FONT_SIZE, true);
    content_view.addSubview(&title_label);

    // Create subtitle label if present
    let subtitle_y = title_y - SUBTITLE_HEIGHT;
    if let Some(ref subtitle) = opts.subtitle {
        if !subtitle.is_empty() {
            let subtitle_rect = NSRect::new(
                NSPoint::new(PADDING, subtitle_y),
                NSSize::new(content_width, SUBTITLE_HEIGHT),
            );
            let subtitle_label =
                create_label(mtm, subtitle, subtitle_rect, SUBTITLE_FONT_SIZE, false);
            content_view.addSubview(&subtitle_label);
        }
    }

    // Create message label - fill remaining space
    let msg_top = if has_subtitle { subtitle_y } else { title_y } - PADDING;
    let msg_y = PADDING;
    let actual_msg_height = msg_top - msg_y;
    let msg_rect = NSRect::new(
        NSPoint::new(PADDING, msg_y),
        NSSize::new(content_width, actual_msg_height),
    );
    let msg_label =
        create_label_with_wrap(mtm, &opts.message, msg_rect, MESSAGE_FONT_SIZE, false, true);
    content_view.addSubview(&msg_label);

    // Set content view
    window.setContentView(Some(&content_view));

    // Set initial alpha for fade-in
    window.setAlphaValue(0.0);

    // Show window
    window.makeKeyAndOrderFront(None);

    play_notification_sound(opts.sound.as_deref());

    // Animate fade-in
    animate_alpha(&window, opacity, 0.3);

    // Run loop for display duration
    let start = std::time::Instant::now();
    let duration = Duration::from_millis(opts.duration_ms);
    let fade_out_duration = Duration::from_secs_f64(0.3);

    // Wait for display duration
    while start.elapsed() < duration {
        unsafe {
            let date = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.05);
            let run_loop = objc2_foundation::NSRunLoop::currentRunLoop();
            run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &date);
        }
    }

    // Start fade-out animation
    animate_alpha(&window, 0.0, fade_out_duration.as_secs_f64());

    // Wait for fade-out to complete
    let fade_start = std::time::Instant::now();
    while fade_start.elapsed() < fade_out_duration {
        unsafe {
            let date = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.05);
            let run_loop = objc2_foundation::NSRunLoop::currentRunLoop();
            run_loop.runMode_beforeDate(NSDefaultRunLoopMode, &date);
        }
    }

    // Ensure window is closed
    window.close();

    Ok(())
}

fn play_notification_sound(sound: Option<&str>) {
    let Some(name) = sound.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };

    if name.eq_ignore_ascii_case("default") {
        let _ = Command::new("osascript").args(["-e", "beep"]).status();
        return;
    }

    if play_named_sound(name) {
        return;
    }

    // Fallback to generic beep when named sound is unavailable.
    let _ = Command::new("osascript").args(["-e", "beep"]).status();
}

fn play_named_sound(name: &str) -> bool {
    let mut candidates = vec![name.to_string()];

    if !name.contains('.') {
        candidates.push(format!("{}.aiff", name));
        candidates.push(format!("{}.wav", name));
        candidates.push(format!("{}.caf", name));
    }

    let mut roots = vec![
        PathBuf::from("/System/Library/Sounds"),
        PathBuf::from("/Library/Sounds"),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join("Library/Sounds"));
    }

    for root in &roots {
        for candidate in &candidates {
            let path = root.join(candidate);
            if path.exists() {
                let Ok(status) = Command::new("afplay").arg(&path).status() else {
                    continue;
                };
                if status.success() {
                    return true;
                }
            }
        }
    }

    false
}

fn create_label(
    mtm: MainThreadMarker,
    text: &str,
    frame: NSRect,
    font_size: CGFloat,
    bold: bool,
) -> Retained<NSTextField> {
    create_label_with_wrap(mtm, text, frame, font_size, bold, false)
}

fn create_label_with_wrap(
    mtm: MainThreadMarker,
    text: &str,
    frame: NSRect,
    font_size: CGFloat,
    bold: bool,
    wrap: bool,
) -> Retained<NSTextField> {
    let label = NSTextField::initWithFrame(NSTextField::alloc(mtm), frame);

    let ns_string = NSString::from_str(text);
    label.setStringValue(&ns_string);

    label.setBezeled(false);
    label.setDrawsBackground(false);
    label.setEditable(false);
    label.setSelectable(false);

    // Set text color to white
    label.setTextColor(Some(&NSColor::whiteColor()));

    // Set font
    let font = if bold {
        NSFont::boldSystemFontOfSize(font_size)
    } else {
        NSFont::systemFontOfSize(font_size)
    };
    label.setFont(Some(&font));

    // Enable word wrap if requested
    if wrap {
        unsafe {
            if let Some(cell) = label.cell() {
                let _: () = msg_send![&cell, setWraps: true];
            }
        }
        label.setMaximumNumberOfLines(0); // 0 = unlimited lines
    }

    label
}

fn calculate_text_height(
    _mtm: MainThreadMarker,
    text: &str,
    width: CGFloat,
    font_size: CGFloat,
) -> CGFloat {
    // Estimate line height (font size * 1.2 for line spacing)
    let line_height = font_size * 1.4;

    // Estimate characters per line based on width and average char width
    // Average char width is roughly 0.6 * font_size for system font
    let avg_char_width = font_size * 0.55;
    let chars_per_line = (width / avg_char_width).floor() as usize;

    if chars_per_line == 0 {
        return line_height;
    }

    // Count lines: explicit newlines + wrapped lines
    let mut total_lines = 0;
    for line in text.lines() {
        let line_chars = line.chars().count();
        if line_chars == 0 {
            total_lines += 1; // Empty line
        } else {
            // Calculate wrapped lines for this line
            total_lines += (line_chars + chars_per_line - 1) / chars_per_line;
        }
    }

    // Handle case where text doesn't end with newline
    if total_lines == 0 {
        total_lines = 1;
    }

    (total_lines as CGFloat) * line_height
}

fn animate_alpha(window: &NSWindow, target: CGFloat, duration: f64) {
    unsafe {
        let context = objc2_app_kit::NSAnimationContext::currentContext();
        context.setDuration(duration);
        let animator: Retained<NSWindow> = msg_send![window, animator];
        animator.setAlphaValue(target);
    }
}

fn calculate_position(
    position: Position,
    screen_frame: NSRect,
    visible_frame: NSRect,
    window_width: CGFloat,
    window_height: CGFloat,
    margin: CGFloat,
) -> (CGFloat, CGFloat) {
    // Handle menubar position specially
    if let Position::Menubar = position {
        if let Some(mb_pos) = load_menubar_position() {
            // Position below the menubar icon, centered on it
            let x = mb_pos.x - window_width / 2.0;
            // mb_pos.y is the bottom of the menubar button (top of visible area)
            let y = mb_pos.y - window_height - margin;
            return (x, y);
        }
        // Fallback to right-top if no menubar position available
        let vis_x = visible_frame.origin.x;
        let vis_y = visible_frame.origin.y;
        let vis_w = visible_frame.size.width;
        let vis_h = visible_frame.size.height;
        let x = vis_x + vis_w - window_width - margin;
        let y = vis_y + vis_h - window_height - margin;
        return (x, y);
    }

    // Use visible_frame to respect menubar and dock
    let vis_x = visible_frame.origin.x;
    let vis_y = visible_frame.origin.y;
    let vis_w = visible_frame.size.width;
    let vis_h = visible_frame.size.height;

    // For center calculations, use full screen
    let screen_x = screen_frame.origin.x;
    let screen_w = screen_frame.size.width;

    // Horizontal position
    let x = match position {
        Position::LeftTop | Position::LeftCenter | Position::LeftBottom => vis_x + margin,
        Position::CenterTop | Position::CenterCenter | Position::CenterBottom => {
            screen_x + (screen_w - window_width) / 2.0
        }
        Position::RightTop | Position::RightCenter | Position::RightBottom => {
            vis_x + vis_w - window_width - margin
        }
        Position::Menubar => unreachable!(), // Handled above
    };

    // Vertical position (macOS coordinates: 0 is bottom)
    // visible_frame already excludes menubar at top and dock
    let y = match position {
        Position::LeftTop | Position::CenterTop | Position::RightTop => {
            vis_y + vis_h - window_height - margin
        }
        Position::LeftCenter | Position::CenterCenter | Position::RightCenter => {
            vis_y + (vis_h - window_height) / 2.0
        }
        Position::LeftBottom | Position::CenterBottom | Position::RightBottom => vis_y + margin,
        Position::Menubar => unreachable!(), // Handled above
    };

    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_position_parse_full_names() {
        assert!(matches!(Position::parse("left-top"), Ok(Position::LeftTop)));
        assert!(matches!(
            Position::parse("center-top"),
            Ok(Position::CenterTop)
        ));
        assert!(matches!(
            Position::parse("right-top"),
            Ok(Position::RightTop)
        ));
        assert!(matches!(
            Position::parse("left-center"),
            Ok(Position::LeftCenter)
        ));
        assert!(matches!(
            Position::parse("center-center"),
            Ok(Position::CenterCenter)
        ));
        assert!(matches!(
            Position::parse("right-center"),
            Ok(Position::RightCenter)
        ));
        assert!(matches!(
            Position::parse("left-bottom"),
            Ok(Position::LeftBottom)
        ));
        assert!(matches!(
            Position::parse("center-bottom"),
            Ok(Position::CenterBottom)
        ));
        assert!(matches!(
            Position::parse("right-bottom"),
            Ok(Position::RightBottom)
        ));
    }

    #[test]
    fn test_position_parse_abbreviations() {
        assert!(matches!(Position::parse("lt"), Ok(Position::LeftTop)));
        assert!(matches!(Position::parse("ct"), Ok(Position::CenterTop)));
        assert!(matches!(Position::parse("rt"), Ok(Position::RightTop)));
        assert!(matches!(Position::parse("lc"), Ok(Position::LeftCenter)));
        assert!(matches!(Position::parse("cc"), Ok(Position::CenterCenter)));
        assert!(matches!(Position::parse("rc"), Ok(Position::RightCenter)));
        assert!(matches!(Position::parse("lb"), Ok(Position::LeftBottom)));
        assert!(matches!(Position::parse("cb"), Ok(Position::CenterBottom)));
        assert!(matches!(Position::parse("rb"), Ok(Position::RightBottom)));
        assert!(matches!(Position::parse("mb"), Ok(Position::Menubar)));
    }

    #[test]
    fn test_position_parse_menubar() {
        assert!(matches!(Position::parse("menubar"), Ok(Position::Menubar)));
        assert!(matches!(Position::parse("mb"), Ok(Position::Menubar)));
        assert!(matches!(Position::parse("MENUBAR"), Ok(Position::Menubar)));
        assert!(matches!(Position::parse("MB"), Ok(Position::Menubar)));
    }

    #[test]
    fn test_position_parse_case_insensitive() {
        assert!(matches!(Position::parse("LEFT-TOP"), Ok(Position::LeftTop)));
        assert!(matches!(
            Position::parse("Right-Top"),
            Ok(Position::RightTop)
        ));
        assert!(matches!(
            Position::parse("CENTER"),
            Ok(Position::CenterCenter)
        ));
    }

    #[test]
    fn test_position_parse_invalid() {
        assert!(Position::parse("invalid").is_err());
        assert!(Position::parse("top-left").is_err());
        assert!(Position::parse("").is_err());
    }

    #[test]
    fn test_parse_hex_color_6digit() {
        assert_eq!(parse_hex_color("#000000"), Ok((0.0, 0.0, 0.0)));
        assert_eq!(parse_hex_color("#ffffff"), Ok((1.0, 1.0, 1.0)));
        assert_eq!(parse_hex_color("#ff0000"), Ok((1.0, 0.0, 0.0)));
        assert_eq!(parse_hex_color("#00ff00"), Ok((0.0, 1.0, 0.0)));
        assert_eq!(parse_hex_color("#0000ff"), Ok((0.0, 0.0, 1.0)));
    }

    #[test]
    fn test_parse_hex_color_3digit() {
        assert_eq!(parse_hex_color("#000"), Ok((0.0, 0.0, 0.0)));
        assert_eq!(parse_hex_color("#fff"), Ok((1.0, 1.0, 1.0)));
        assert_eq!(parse_hex_color("#f00"), Ok((1.0, 0.0, 0.0)));
        assert_eq!(
            parse_hex_color("#222"),
            Ok((34.0 / 255.0, 34.0 / 255.0, 34.0 / 255.0))
        );
    }

    #[test]
    fn test_parse_hex_color_no_hash() {
        assert_eq!(parse_hex_color("ff0000"), Ok((1.0, 0.0, 0.0)));
        assert_eq!(parse_hex_color("fff"), Ok((1.0, 1.0, 1.0)));
    }

    #[test]
    fn test_parse_hex_color_invalid() {
        assert!(parse_hex_color("#gg0000").is_err());
        assert!(parse_hex_color("#12345").is_err());
        assert!(parse_hex_color("#1234567").is_err());
        assert!(parse_hex_color("").is_err());
    }
}
