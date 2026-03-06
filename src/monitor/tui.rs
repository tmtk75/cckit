use super::focus;
#[cfg(target_os = "macos")]
use super::menubar;
use super::session::{Session, SessionStatus, TuiState};
use super::storage::Storage;
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
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, TableState},
};
use std::io;
use std::time::{Duration, Instant};

struct App {
    sessions: Vec<Session>,
    selected_index: usize,
    should_quit: bool,
    message: Option<String>,
}

impl App {
    fn new() -> Self {
        Self {
            sessions: Vec::new(),
            selected_index: 0,
            should_quit: false,
            message: None,
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
            event_timeout_ms: 500,
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
        if event::poll(event_timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.message = None; // Clear message on key press

                    // Handle Ctrl+C
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        app.should_quit = true;
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            app.should_quit = true;
                        }
                        KeyCode::Up | KeyCode::Char('k') => app.select_previous(),
                        KeyCode::Down | KeyCode::Char('j') => app.select_next(),
                        KeyCode::Enter | KeyCode::Char('f') => {
                            if let Some(session) = app.selected_session() {
                                // Use TTY-based focus (works with tmux)
                                match focus::focus_ghostty_tab_by_tty(&session.tty) {
                                    Ok(true) => {
                                        app.message =
                                            Some(format!("Focused: {}", session.short_cwd()));
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
                                                app.message =
                                                    Some(format!("No tab: {}", project_name));
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
            }
        }

        // Check for quit signal from external source (Ctrl+C handler)
        if let Some(ref quit_flag) = external_quit {
            if quit_flag.load(Ordering::SeqCst) {
                app.should_quit = true;
            }
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

fn draw(frame: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .margin(1)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(5),
            Constraint::Length(2),
        ])
        .split(frame.area());

    draw_header(frame, chunks[0], app);
    draw_sessions_table(frame, chunks[1], app);
    draw_footer(frame, chunks[2], app);
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App) {
    let active_count = app
        .sessions
        .iter()
        .filter(|s| s.status == SessionStatus::Running)
        .count();

    let header = Paragraph::new(Line::from(vec![
        Span::styled(
            "cckit",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" - "),
        Span::styled(
            format!("{} sessions", app.sessions.len()),
            Style::default().fg(Color::Green),
        ),
        Span::raw(" ("),
        Span::styled(
            format!("{} active", active_count),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw(")"),
    ]));

    frame.render_widget(header, area);
}

fn draw_sessions_table(frame: &mut Frame, area: Rect, app: &App) {
    let header_cells = ["", "Status", "Path", "Tool", "PID", "Created", "Updated"]
        .iter()
        .map(|h| Cell::from(*h).style(Style::default().fg(Color::Yellow)));
    let header = Row::new(header_cells).height(1);

    let rows: Vec<Row> = app
        .sessions
        .iter()
        .enumerate()
        .map(|(idx, session)| {
            let status_style = match session.status {
                SessionStatus::Running => Style::default().fg(Color::Green),
                SessionStatus::AwaitingApproval => Style::default().fg(Color::Rgb(255, 165, 0)),
                SessionStatus::WaitingInput => Style::default().fg(Color::Yellow),
                SessionStatus::Stopped => Style::default().fg(Color::DarkGray),
            };

            let status_text = match session.status {
                SessionStatus::Running => "● run",
                SessionStatus::AwaitingApproval => "◐ tool",
                SessionStatus::WaitingInput => "○ wait",
                SessionStatus::Stopped => "× done",
            };

            let tool_display = match (&session.last_tool, &session.last_tool_input) {
                (Some(tool), Some(input)) => format!("{}: {}", tool, input),
                (Some(tool), None) => tool.clone(),
                _ => "-".to_string(),
            };
            let pid_display = session
                .pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "-".to_string());
            let created = format_relative_time(session.created_at);
            let updated = format_relative_time(session.updated_at);

            Row::new(vec![
                Cell::from(format!("{}", idx + 1)).style(Style::default().fg(Color::Gray)),
                Cell::from(status_text).style(status_style),
                Cell::from(session.short_cwd()),
                Cell::from(tool_display).style(Style::default().fg(Color::Cyan)),
                Cell::from(pid_display).style(Style::default().fg(Color::Gray)),
                Cell::from(created).style(Style::default().fg(Color::Gray)),
                Cell::from(updated).style(Style::default().fg(Color::Gray)),
            ])
        })
        .collect();

    let widths = [
        Constraint::Length(2),
        Constraint::Length(8),
        Constraint::Percentage(25),
        Constraint::Percentage(35),
        Constraint::Length(7),
        Constraint::Length(10),
        Constraint::Length(10),
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Sessions ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        )
        .row_highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );

    let mut state = TableState::default();
    state.select(Some(app.selected_index));

    frame.render_stateful_widget(table, area, &mut state);
}

fn draw_footer(frame: &mut Frame, area: Rect, app: &App) {
    let content = if let Some(ref msg) = app.message {
        Line::from(vec![Span::styled(
            msg.as_str(),
            Style::default().fg(Color::Yellow),
        )])
    } else {
        Line::from(vec![
            Span::styled("↑↓/jk", Style::default().fg(Color::Cyan)),
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

fn format_relative_time(dt: chrono::DateTime<chrono::Utc>) -> String {
    let now = chrono::Utc::now();
    let duration = now.signed_duration_since(dt);

    if duration.num_seconds() < 60 {
        format!("{}s ago", duration.num_seconds())
    } else if duration.num_minutes() < 60 {
        format!("{}m ago", duration.num_minutes())
    } else if duration.num_hours() < 24 {
        format!("{}h ago", duration.num_hours())
    } else {
        format!("{}d ago", duration.num_days())
    }
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
}
