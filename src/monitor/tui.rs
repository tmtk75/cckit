use super::display;
use super::focus;
#[cfg(target_os = "macos")]
use super::menubar;
use super::session::{Session, SessionStatus, TuiState};
use super::storage::Storage;
use crate::monitor::theme::{self, StatusColor, anim, palette};
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
};
use std::io;
use std::time::{Duration, Instant};

struct App {
    sessions: Vec<Session>,
    selected_index: usize,
    should_quit: bool,
    message: Option<String>,
    anim_start: Instant,
}

impl App {
    fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected_index: 0,
            should_quit: false,
            message: None,
            anim_start: Instant::now(),
        }
    }

    fn update_sessions(&mut self, storage: &Storage) {
        let store = storage.load();
        let mut sessions: Vec<Session> = store.sessions.values().cloned().collect();
        sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.sessions = sessions;

        if !self.sessions.is_empty() && self.selected_index >= self.sessions.len() {
            self.selected_index = self.sessions.len() - 1;
        }
    }

    fn select_next(&mut self) {
        if !self.sessions.is_empty() {
            self.selected_index = (self.selected_index + 1) % self.sessions.len();
        }
    }

    fn select_previous(&mut self) {
        if !self.sessions.is_empty() {
            self.selected_index = self
                .selected_index
                .checked_sub(1)
                .unwrap_or(self.sessions.len() - 1);
        }
    }

    #[allow(dead_code)]
    fn selected_session(&self) -> Option<&Session> {
        self.sessions.get(self.selected_index)
    }
}

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Configuration for TUI and menubar polling intervals
#[derive(Clone)]
pub struct TuiConfig {
    /// Session check interval in milliseconds
    pub check_interval_ms: u64,
    /// Menubar poll interval in milliseconds
    pub poll_interval_ms: u64,
    /// Menu update interval in milliseconds
    pub menu_update_interval_ms: u64,
    /// Event poll timeout in milliseconds
    pub event_timeout_ms: u64,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            check_interval_ms: 2000,
            poll_interval_ms: 500,
            menu_update_interval_ms: 2000,
            event_timeout_ms: anim::TUI_TICK_MS,
        }
    }
}

pub fn run_tui(config: TuiConfig) -> Result<(), Box<dyn std::error::Error>> {
    let tty = get_current_tty();
    run_tui_core(config, None, tty)
}

#[cfg(target_os = "macos")]
pub fn run_tui_with_menubar(config: TuiConfig) -> Result<(), Box<dyn std::error::Error>> {
    let should_quit = Arc::new(AtomicBool::new(false));
    let should_quit_tui = should_quit.clone();
    let should_quit_ctrlc = should_quit.clone();

    let poll_interval_ms = config.poll_interval_ms;
    let menu_update_interval_ms = config.menu_update_interval_ms;

    // Get TTY on main thread before spawning TUI thread
    let tty = get_current_tty();

    // Handle Ctrl+C
    ctrlc::set_handler(move || {
        should_quit_ctrlc.store(true, Ordering::SeqCst);
    })?;

    // Run TUI in a separate thread
    let tui_thread = std::thread::spawn(move || {
        let result = run_tui_core(config, Some(should_quit_tui), tty);
        if let Err(e) = result {
            eprintln!("TUI error: {}", e);
            return Err(e.to_string());
        }
        Ok(())
    });

    // Run menubar on main thread
    let mut menubar = menubar::init_menubar()?;
    menubar::set_update_interval(menu_update_interval_ms);

    // Keep processing events until TUI thread exits or Ctrl+C
    while !should_quit.load(Ordering::SeqCst) {
        menubar::poll_menubar(&mut menubar);
        std::thread::sleep(Duration::from_millis(poll_interval_ms));

        // Check if TUI thread finished
        if tui_thread.is_finished() {
            break;
        }
    }

    // Signal quit and wait for TUI thread
    should_quit.store(true, Ordering::SeqCst);

    match tui_thread.join() {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.into()),
        Err(_) => Err("TUI thread panicked".into()),
    }
}

#[cfg(not(target_os = "macos"))]
pub fn run_tui_with_menubar(config: TuiConfig) -> Result<(), Box<dyn std::error::Error>> {
    let tty = get_current_tty();
    run_tui_core(config, None, tty)
}

struct TerminalGuard {
    active: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        Ok(Self { active: true })
    }

    fn exit(&mut self) -> io::Result<()> {
        if self.active {
            self.active = false;
            disable_raw_mode()?;
            let mut stdout = io::stdout();
            execute!(stdout, LeaveAlternateScreen)?;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let mut stdout = io::stdout();
            let _ = execute!(stdout, LeaveAlternateScreen);
        }
    }
}

fn run_tui_core(
    config: TuiConfig,
    external_quit: Option<Arc<AtomicBool>>,
    tty: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut guard = TerminalGuard::enter()?;
    let stdout = io::stdout();
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    let storage = Storage::new();

    // Save TUI state for menubar integration
    if let Some(ref tty) = tty {
        let tui_state = TuiState {
            tty: tty.clone(),
            pid: std::process::id(),
            started_at: chrono::Utc::now(),
        };
        let _ = storage.save_tui_state(&tui_state);
    }

    // Initial load
    let store = storage.load();
    let mut last_updated_at = store.updated_at;
    app.update_sessions(&storage);

    // Polling state
    let mut last_check = Instant::now();
    let check_interval = Duration::from_millis(config.check_interval_ms);
    let event_timeout = Duration::from_millis(config.event_timeout_ms);

    loop {
        terminal.draw(|f| draw(f, &app))?;

        // Auto-refresh: check timestamp every few seconds
        if last_check.elapsed() >= check_interval {
            last_check = Instant::now();
            let store = storage.load();
            if last_updated_at != store.updated_at {
                last_updated_at = store.updated_at;
                app.update_sessions(&storage);
            }
        }

        // Event polling with short timeout for responsive Ctrl+C
        if event::poll(event_timeout)?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.message = None; // Clear message on key press

            // Handle Ctrl+C
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                app.should_quit = true;
                continue;
            }

            match key.code {
                KeyCode::Esc => {
                    app.should_quit = true;
                }
                KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
                KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                KeyCode::Enter | KeyCode::Char('f') => {
                    if let Some(session) = app.selected_session() {
                        if session.tty == "unknown" {
                            // Skip focus for sessions without a known TTY (e.g. Codex Desktop)
                            app.message = Some("No TTY (desktop app)".to_string());
                        } else {
                            // Use TTY-based focus (works with tmux)
                            match focus::focus_ghostty_tab_by_tty(&session.tty) {
                                Ok(true) => {
                                    app.message = Some(format!("Focused: {}", session.short_cwd()));
                                }
                                Ok(false) => {
                                    // Fallback to project name matching
                                    let project_name = std::path::Path::new(&session.cwd)
                                        .file_name()
                                        .and_then(|n| n.to_str())
                                        .unwrap_or(&session.cwd);
                                    match focus::focus_ghostty_tab(project_name) {
                                        Ok(true) => {
                                            app.message =
                                                Some(format!("Focused: {}", project_name));
                                        }
                                        Ok(false) => {
                                            app.message = Some(format!("No tab: {}", project_name));
                                        }
                                        Err(e) => {
                                            app.message = Some(format!("Error: {}", e));
                                        }
                                    }
                                }
                                Err(e) => {
                                    app.message = Some(format!("Error: {}", e));
                                }
                            }
                        }
                    }
                }
                KeyCode::Char('r') => {
                    app.update_sessions(&storage);
                    app.message = Some("Refreshed".to_string());
                }
                KeyCode::Char('d') => {
                    if let Some(session) = app.selected_session() {
                        let key = session.key();
                        let path = session.short_cwd();
                        match storage.remove_session(&key) {
                            Ok(true) => {
                                app.message = Some(format!("Deleted: {}", path));
                                app.update_sessions(&storage);
                            }
                            Ok(false) => {
                                app.message = Some("Session not found".to_string());
                            }
                            Err(e) => {
                                app.message = Some(format!("Error: {}", e));
                            }
                        }
                    }
                }
                KeyCode::Char(c @ '1'..='9') => {
                    let idx = (c as usize) - ('1' as usize);
                    if idx < app.sessions.len() {
                        app.selected_index = idx;
                    }
                }
                _ => {}
            }
        }

        // Check for quit signal from external source (Ctrl+C handler)
        if let Some(ref quit_flag) = external_quit
            && quit_flag.load(Ordering::SeqCst)
        {
            app.should_quit = true;
        }

        if app.should_quit {
            break;
        }
    }

    // Clear TUI state on exit
    let _ = storage.clear_tui_state();

    guard.exit()?;

    Ok(())
}

/// Get current TTY device path
fn get_current_tty() -> Option<String> {
    use std::ffi::CStr;
    use std::os::unix::io::AsRawFd;

    // Try stdin first
    let fd = std::io::stdin().as_raw_fd();
    let tty_name = unsafe { libc::ttyname(fd) };

    if !tty_name.is_null() {
        let cstr = unsafe { CStr::from_ptr(tty_name) };
        return cstr.to_str().ok().map(|s| s.to_string());
    }

    // Fallback: try stdout
    let fd = std::io::stdout().as_raw_fd();
    let tty_name = unsafe { libc::ttyname(fd) };

    if !tty_name.is_null() {
        let cstr = unsafe { CStr::from_ptr(tty_name) };
        return cstr.to_str().ok().map(|s| s.to_string());
    }

    None
}

/// Apply brightness multiplier to an RGB tuple
fn apply_brightness(rgb: (u8, u8, u8), brightness: f64) -> (u8, u8, u8) {
    let r = (rgb.0 as f64 * brightness).round().min(255.0) as u8;
    let g = (rgb.1 as f64 * brightness).round().min(255.0) as u8;
    let b = (rgb.2 as f64 * brightness).round().min(255.0) as u8;
    (r, g, b)
}

fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Outer frame with double border
    let (gr, gg, gb) = palette::GRID;
    let (br, bg_r, bb) = palette::BG;
    let outer_block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(Color::Rgb(gr, gg, gb)))
        .style(Style::default().bg(Color::Rgb(br, bg_r, bb)));
    frame.render_widget(outer_block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(area);

    draw_header(frame, chunks[0], app);
    draw_sessions(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let active_count = app
        .sessions
        .iter()
        .filter(|s| s.status != SessionStatus::Stopped)
        .count();
    let total_count = app.sessions.len();

    let (tr, tg, tb) = palette::TEXT;
    let right_text = format!("{} active / {} total", active_count, total_count);

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "  \u{25C9} CCKIT \u{2500}\u{2500}\u{2500} MISSION CONTROL \u{2500}\u{2500}\u{2500} ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(right_text, Style::default().fg(Color::Rgb(tr, tg, tb))),
    ]));

    frame.render_widget(header, area);
}

fn draw_sessions(frame: &mut Frame, area: Rect, app: &App) {
    if app.sessions.is_empty() {
        let (dr, dg, db) = palette::TEXT_DIM;
        let empty = Paragraph::new(Line::from(Span::styled(
            "  No sessions",
            Style::default().fg(Color::Rgb(dr, dg, db)),
        )));
        frame.render_widget(empty, area);
        return;
    }

    let elapsed_secs = app.anim_start.elapsed().as_secs_f64();
    let mut lines: Vec<Line> = Vec::new();

    for (idx, session) in app.sessions.iter().enumerate() {
        let is_selected = idx == app.selected_index;
        let is_stopped = session.status == SessionStatus::Stopped;

        // Compute highlight background for selected row
        let bg_style = if is_selected {
            let (ar, ag, ab) = session.agent_type().accent_rgb();
            // Scale by /10 for subtle tint
            let (hr, hg, hb) = (ar / 10, ag / 10, ab / 10);
            Style::default().bg(Color::Rgb(hr, hg, hb))
        } else {
            Style::default()
        };

        // Status dot with animation
        let (dot_char, status_color) = match session.status {
            SessionStatus::Running => ("\u{25CF}", StatusColor::Running),
            SessionStatus::AwaitingApproval => ("\u{25C6}", StatusColor::AwaitingApproval),
            SessionStatus::WaitingInput => ("\u{25C7}", StatusColor::WaitingInput),
            SessionStatus::Stopped => ("\u{25CB}", StatusColor::Stopped),
        };

        let brightness = match session.status {
            SessionStatus::Running => theme::breathing_pulse(elapsed_secs),
            SessionStatus::AwaitingApproval => theme::fast_blink(elapsed_secs),
            SessionStatus::WaitingInput => theme::slow_fade(elapsed_secs),
            SessionStatus::Stopped => 1.0,
        };

        let dot_rgb = apply_brightness(status_color.rgb(), brightness);
        let dot_color = Color::Rgb(dot_rgb.0, dot_rgb.1, dot_rgb.2);

        let (tr, tg, tb) = palette::TEXT;
        let (dr, dg, db) = palette::TEXT_DIM;
        let text_color = Color::Rgb(tr, tg, tb);
        let dim_color = Color::Rgb(dr, dg, db);

        // Session display values
        let display_name = session.display_name();
        let project_name = session.project_name();
        let tool = session.last_tool.as_deref().unwrap_or("-");
        let elapsed = display::format_elapsed_short(session.updated_at);

        // Context ratio
        let context_ratio = match (session.context_used_tokens, session.context_max_tokens) {
            (Some(used), Some(max)) if max > 0 => used as f64 / max as f64,
            _ => 0.0,
        };
        let context_pct = format!("{}%", (context_ratio * 100.0).round() as u32);

        // Row 1: status dot + display_name + separators + tool + elapsed + context%
        let row1_style = if is_stopped {
            Style::default().fg(dim_color)
        } else {
            Style::default().fg(text_color)
        };

        let mut row1_spans = vec![
            Span::styled(format!("  {} ", dot_char), bg_style.fg(dot_color)),
            Span::styled(
                display_name.clone(),
                bg_style.patch(row1_style).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" \u{2502} ", bg_style.fg(dim_color)),
            Span::styled(project_name.to_string(), bg_style.patch(row1_style)),
            Span::styled(" \u{2502} ", bg_style.fg(dim_color)),
            Span::styled(tool.to_string(), bg_style.fg(Color::Cyan)),
            Span::styled(" \u{2502} ", bg_style.fg(dim_color)),
            Span::styled(elapsed.clone(), bg_style.fg(dim_color)),
        ];

        if !is_stopped {
            row1_spans.push(Span::styled(" \u{2502} ", bg_style.fg(dim_color)));
            row1_spans.push(Span::styled(
                context_pct.clone(),
                bg_style.patch(row1_style),
            ));
        }

        lines.push(Line::from(row1_spans));

        // Row 2: context bar + stats (only for non-stopped sessions)
        if !is_stopped {
            let bar_width = 20;
            let filled = (context_ratio * bar_width as f64).round() as usize;
            let empty = bar_width - filled;

            let gauge_rgb = theme::context_gauge_rgb(context_ratio);
            let gauge_color = Color::Rgb(gauge_rgb.0, gauge_rgb.1, gauge_rgb.2);

            let filled_str: String = "\u{2588}".repeat(filled);
            let empty_str: String = "\u{2591}".repeat(empty);

            let stats = display::session_count_parts(session).join(" ");

            let row2_spans = vec![
                Span::styled("    ", bg_style),
                Span::styled(filled_str, bg_style.fg(gauge_color)),
                Span::styled(empty_str, bg_style.fg(dim_color)),
                Span::styled(format!(" {}", stats), bg_style.fg(dim_color)),
            ];

            lines.push(Line::from(row2_spans));
        }

        // Separator between sessions (not after the last one)
        if idx < app.sessions.len() - 1 {
            lines.push(Line::from(Span::styled(
                "  \u{2500} \u{2500} \u{2500} \u{2500}",
                Style::default().fg(dim_color),
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let content = if let Some(ref msg) = app.message {
        Line::from(vec![Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        )])
    } else {
        Line::from(vec![
            Span::styled("\u{2191}\u{2193}/jk", Style::default().fg(Color::Cyan)),
            Span::raw(" Select  "),
            Span::styled("Enter/f", Style::default().fg(Color::Cyan)),
            Span::raw(" Focus  "),
            Span::styled("d", Style::default().fg(Color::Cyan)),
            Span::raw(" Delete  "),
            Span::styled("r", Style::default().fg(Color::Cyan)),
            Span::raw(" Refresh  "),
            Span::styled("q/^C", Style::default().fg(Color::Cyan)),
            Span::raw(" Quit"),
        ])
    };

    let footer = Paragraph::new(content);
    frame.render_widget(footer, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn create_test_session(id: &str) -> Session {
        Session {
            session_id: id.to_string(),
            cwd: format!("/test/{}", id),
            tty: "/dev/ttys001".to_string(),
            status: SessionStatus::Running,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_tool: None,
            last_tool_input: None,
            pid: Some(12345),
            prompt_count: 0,
            compact_count: 0,
            transcript_path: None,
            tool_started_at: None,
            last_tool_duration_ms: None,
            tool_count: 0,
            context_used_tokens: None,
            context_max_tokens: None,
            model: None,
            subagent_name: None,
        }
    }

    #[test]
    fn test_app_new() {
        let app = App::new();
        assert!(app.sessions.is_empty());
        assert_eq!(app.selected_index, 0);
        assert!(!app.should_quit);
        assert!(app.message.is_none());
    }

    #[test]
    fn test_app_select_next_empty() {
        let mut app = App::new();
        app.select_next(); // Should not panic
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_app_select_next() {
        let mut app = App::new();
        app.sessions = vec![
            create_test_session("1"),
            create_test_session("2"),
            create_test_session("3"),
        ];
        assert_eq!(app.selected_index, 0);

        app.select_next();
        assert_eq!(app.selected_index, 1);

        app.select_next();
        assert_eq!(app.selected_index, 2);

        app.select_next(); // Should wrap around
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_app_select_previous_empty() {
        let mut app = App::new();
        app.select_previous(); // Should not panic
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_app_select_previous() {
        let mut app = App::new();
        app.sessions = vec![
            create_test_session("1"),
            create_test_session("2"),
            create_test_session("3"),
        ];
        assert_eq!(app.selected_index, 0);

        app.select_previous(); // Should wrap to end
        assert_eq!(app.selected_index, 2);

        app.select_previous();
        assert_eq!(app.selected_index, 1);

        app.select_previous();
        assert_eq!(app.selected_index, 0);
    }

    #[test]
    fn test_app_selected_session() {
        let mut app = App::new();
        assert!(app.selected_session().is_none());

        app.sessions = vec![create_test_session("test")];
        assert!(app.selected_session().is_some());
        assert_eq!(app.selected_session().unwrap().session_id, "test");
    }

    #[test]
    fn test_apply_brightness() {
        assert_eq!(apply_brightness((100, 200, 50), 1.0), (100, 200, 50));
        assert_eq!(apply_brightness((100, 200, 50), 0.5), (50, 100, 25));
        assert_eq!(apply_brightness((255, 255, 255), 0.0), (0, 0, 0));
    }
}
