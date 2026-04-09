// Ratatui-based interactive search result browser.

use crate::history::{Role, SessionRecord, Turn, search};
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::io;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

const PREVIEW_EDGE: usize = 3;
/// How long to wait after the last keystroke before re-running the fuzzy
/// filter. Prevents recomputing on every keystroke when typing fast.
const DEBOUNCE: Duration = Duration::from_millis(770);
/// event::poll timeout when the query is dirty (awaiting debounce).
const POLL_BUSY: Duration = Duration::from_millis(20);
/// event::poll timeout when idle.
const POLL_IDLE: Duration = Duration::from_millis(250);

/// Interactive result browser with live fuzzy filtering.
///
/// Takes the full session list and lets the user type a query into an input
/// box; results update on every keystroke. Returns `Some(cwd)` when the user
/// picked an entry with Enter, or `None` when cancelled.
///
/// The TUI renders to stderr so that stdout can be captured by `$(...)` to
/// receive the selected `cwd` as a plain path.
pub fn run_tui(
    sessions: Vec<SessionRecord>,
    initial_query: &str,
    limit: usize,
    load_elapsed: Duration,
) -> io::Result<Option<PathBuf>> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(io::stderr());
    let mut terminal = Terminal::new(backend)?;

    let result = run_loop(&mut terminal, sessions, initial_query, limit, load_elapsed);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

struct AppState {
    sessions: Vec<SessionRecord>,
    haystacks: Vec<String>,
    query: String,
    visible: Vec<usize>,
    limit: usize,
    list_state: ListState,
    status_message: Option<String>,
    /// When set, the query has changed since the last refilter. The stored
    /// `Instant` is the time of the most recent mutation; `maybe_refilter`
    /// will apply the filter once at least `DEBOUNCE` has elapsed since.
    dirty_since: Option<Instant>,
}

impl AppState {
    fn new(sessions: Vec<SessionRecord>, initial_query: &str, limit: usize) -> Self {
        let haystacks: Vec<String> = sessions.iter().map(search::searchable_text).collect();
        let mut app = Self {
            sessions,
            haystacks,
            query: initial_query.to_string(),
            visible: Vec::new(),
            limit,
            list_state: ListState::default(),
            status_message: None,
            dirty_since: None,
        };
        // Apply the initial query synchronously so the first frame already
        // shows filtered results rather than blinking through "all sessions".
        app.refilter();
        app
    }

    fn refilter(&mut self) {
        self.visible =
            search::fuzzy_filter(&self.sessions, &self.haystacks, &self.query, self.limit);
        if self.visible.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.status_message = None;
        self.dirty_since = None;
    }

    fn mark_dirty(&mut self) {
        self.dirty_since = Some(Instant::now());
    }

    /// Run the debounced refilter if enough time has passed since the last
    /// mutation. Returns the remaining debounce time if still waiting.
    fn maybe_refilter(&mut self) -> Option<Duration> {
        let started = self.dirty_since?;
        let elapsed = started.elapsed();
        if elapsed >= DEBOUNCE {
            self.refilter();
            None
        } else {
            Some(DEBOUNCE - elapsed)
        }
    }

    fn selected_session(&self) -> Option<&SessionRecord> {
        let idx = self.list_state.selected()?;
        let session_idx = *self.visible.get(idx)?;
        self.sessions.get(session_idx)
    }

    fn move_selection(&mut self, delta: i32) {
        let len = self.visible.len();
        if len == 0 {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0) as i32;
        let next = (current + delta).clamp(0, len as i32 - 1);
        self.list_state.select(Some(next as usize));
    }
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    sessions: Vec<SessionRecord>,
    initial_query: &str,
    limit: usize,
    load_elapsed: Duration,
) -> io::Result<Option<PathBuf>> {
    let total = sessions.len();
    let mut app = AppState::new(sessions, initial_query, limit);

    loop {
        terminal.draw(|f| draw(f, &mut app, total, load_elapsed))?;

        // If the query is dirty, poll for events only until the debounce
        // expires so we apply the filter promptly when typing stops.
        let poll_timeout = match app.dirty_since {
            Some(_) => POLL_BUSY,
            None => POLL_IDLE,
        };

        if event::poll(poll_timeout)?
            && let Event::Key(key) = event::read()?
        {
            if key.kind != crossterm::event::KeyEventKind::Press {
                // Apply any pending filter before looping back.
                app.maybe_refilter();
                continue;
            }
            match (key.code, key.modifiers) {
                (KeyCode::Esc, _) | (KeyCode::Char('c'), KeyModifiers::CONTROL) => {
                    return Ok(None);
                }
                (KeyCode::Enter, _) => {
                    // Flush the debounce: apply the current query immediately.
                    // Enter does NOT exit the TUI — use Esc / Ctrl-C to quit.
                    if app.dirty_since.is_some() {
                        app.refilter();
                    }
                }
                (KeyCode::Up, _) => app.move_selection(-1),
                (KeyCode::Down, _) => app.move_selection(1),
                (KeyCode::PageUp, _) => app.move_selection(-10),
                (KeyCode::PageDown, _) => app.move_selection(10),
                (KeyCode::Backspace, _) => {
                    if app.query.pop().is_some() {
                        app.mark_dirty();
                    }
                }
                (KeyCode::Char('u'), KeyModifiers::CONTROL) => {
                    if !app.query.is_empty() {
                        app.query.clear();
                        app.mark_dirty();
                    }
                }
                (KeyCode::Char('w'), KeyModifiers::CONTROL) => {
                    // Delete last word (mirrors readline).
                    let trimmed = app.query.trim_end();
                    let cut = trimmed
                        .rfind(|c: char| c.is_whitespace())
                        .map_or(0, |i| i + 1);
                    if cut < app.query.len() {
                        app.query.truncate(cut);
                        app.mark_dirty();
                    }
                }
                (KeyCode::Char('o'), KeyModifiers::CONTROL) => {
                    if let Some(s) = app.selected_session() {
                        match Command::new("open").arg(&s.cwd).status() {
                            Ok(_) => {
                                app.status_message = Some(format!("opened {}", s.cwd.display()))
                            }
                            Err(e) => app.status_message = Some(format!("open failed: {e}")),
                        }
                    }
                }
                (KeyCode::Char(c), modifiers)
                    if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT =>
                {
                    app.query.push(c);
                    app.mark_dirty();
                }
                _ => {}
            }
        }

        // Run the pending filter if the debounce has elapsed (regardless of
        // whether an event fired this tick).
        app.maybe_refilter();
    }
}

fn draw(
    f: &mut ratatui::Frame<'_>,
    app: &mut AppState,
    total_sessions: usize,
    load_elapsed: Duration,
) {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // query input box
            Constraint::Min(1),    // results + preview
            Constraint::Length(1), // status bar
        ])
        .split(f.area());

    // Query input box
    let prompt = format!("> {}", app.query);
    let query_widget =
        Paragraph::new(prompt).block(Block::default().borders(Borders::ALL).title(format!(
            "cckit session search ({}/{}) [loaded in {:.2}s]",
            app.visible.len(),
            total_sessions,
            load_elapsed.as_secs_f64(),
        )));
    f.render_widget(query_widget, rows[0]);

    // Main area
    let main = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(rows[1]);

    let items: Vec<ListItem> = app
        .visible
        .iter()
        .filter_map(|i| app.sessions.get(*i))
        .map(|s| {
            let when = s.started_at.format("%Y-%m-%d %H:%M");
            let base = s
                .cwd
                .file_name()
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_else(|| s.cwd.display().to_string());
            let snippet = s
                .first_user_text()
                .map(flatten_snippet)
                .unwrap_or_else(|| "(no user text)".into());
            ListItem::new(vec![
                Line::from(format!("{when} {base}")),
                Line::from(Span::styled(
                    format!("    {snippet}"),
                    Style::default().fg(Color::DarkGray),
                )),
            ])
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title("results"))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    f.render_stateful_widget(list, main[0], &mut app.list_state);

    let preview_text = app
        .selected_session()
        .map(build_preview)
        .unwrap_or_else(|| vec![Line::from("(no selection)")]);
    let preview = Paragraph::new(preview_text)
        .block(Block::default().borders(Borders::ALL).title("preview"))
        .wrap(Wrap { trim: false });
    f.render_widget(preview, main[1]);

    let status_line = app.status_message.clone().unwrap_or_else(|| {
        "enter: filter now  ^o: open  ↑↓: move  ^u: clear  ^w: word  esc/^c: quit".into()
    });
    f.render_widget(Paragraph::new(status_line), rows[2]);
}

fn build_preview(session: &SessionRecord) -> Vec<Line<'static>> {
    let turns = &session.turns;
    let n = turns.len();
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("session {} ({} turns)", session.session_id, n),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(session.cwd.display().to_string()));
    if let Some(b) = session.git_branch.as_ref() {
        lines.push(Line::from(format!("branch: {b}")));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "== first turns ==",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let head_end = PREVIEW_EDGE.min(n);
    for turn in &turns[..head_end] {
        lines.push(format_turn_line(turn));
    }
    lines.push(Line::from(""));

    lines.push(Line::from(Span::styled(
        "== last turns ==",
        Style::default().add_modifier(Modifier::BOLD),
    )));
    let tail_start = n.saturating_sub(PREVIEW_EDGE).max(head_end);
    if tail_start < n {
        for turn in &turns[tail_start..] {
            lines.push(format_turn_line(turn));
        }
    }

    lines
}

/// Collapse whitespace and truncate a user text to a short, single-line
/// preview suitable for the result list.
fn flatten_snippet(s: &str) -> String {
    const MAX: usize = 120;
    let flat: String = s
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    // Collapse runs of spaces.
    let mut out = String::with_capacity(flat.len());
    let mut prev_space = false;
    for c in flat.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    let trimmed = out.trim();
    if trimmed.chars().count() <= MAX {
        trimmed.to_string()
    } else {
        let mut t: String = trimmed.chars().take(MAX).collect();
        t.push('…');
        t
    }
}

fn format_turn_line(turn: &Turn) -> Line<'static> {
    let tag = match turn.role {
        Role::User => "[user]",
        Role::Assistant => "[asst]",
    };
    let flat: String = turn
        .text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    Line::from(format!("{tag} {flat}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn make_session(id: &str, turn_count: usize) -> SessionRecord {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let turns: Vec<Turn> = (0..turn_count)
            .map(|i| Turn {
                role: if i % 2 == 0 {
                    Role::User
                } else {
                    Role::Assistant
                },
                timestamp: ts,
                text: format!("turn {i}"),
            })
            .collect();
        SessionRecord {
            session_id: id.into(),
            cwd: "/tmp/proj".into(),
            git_branch: None,
            file_path: format!("/tmp/{id}.jsonl").into(),
            started_at: ts,
            ended_at: ts,
            turns,
        }
    }

    #[test]
    fn preview_shows_first_and_last_three_turns_without_overlap() {
        let s = make_session("s", 10);
        let lines = build_preview(&s);
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.clone().into_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        for i in [0, 1, 2, 7, 8, 9] {
            assert!(text.contains(&format!("turn {i}")), "missing turn {i}");
        }
    }

    #[test]
    fn preview_handles_short_session_without_duplicating_turns() {
        let s = make_session("s", 4);
        let lines = build_preview(&s);
        let joined: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.clone().into_owned()))
            .collect::<Vec<_>>()
            .join("\n");
        for i in 0..4 {
            assert_eq!(
                joined.matches(&format!("turn {i}")).count(),
                1,
                "turn {i} duplicated"
            );
        }
    }

    #[test]
    fn flatten_snippet_collapses_whitespace_and_truncates() {
        let s = "hello\n\nworld   \tfoo";
        assert_eq!(flatten_snippet(s), "hello world foo");

        let long: String = "a".repeat(200);
        let out = flatten_snippet(&long);
        assert!(out.ends_with('…'));
        assert_eq!(out.chars().count(), 121);
    }

    #[test]
    fn appstate_initial_query_filters() {
        let a = SessionRecord {
            session_id: "osrm".into(),
            cwd: "/tmp/a".into(),
            git_branch: None,
            file_path: "/tmp/a.jsonl".into(),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap(),
            ended_at: chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap(),
            turns: vec![Turn {
                role: Role::User,
                timestamp: chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap(),
                text: "osrm jni work".into(),
            }],
        };
        let b = SessionRecord {
            session_id: "other".into(),
            cwd: "/tmp/b".into(),
            git_branch: None,
            file_path: "/tmp/b.jsonl".into(),
            started_at: chrono::Utc.with_ymd_and_hms(2026, 3, 11, 0, 0, 0).unwrap(),
            ended_at: chrono::Utc.with_ymd_and_hms(2026, 3, 11, 0, 0, 0).unwrap(),
            turns: vec![Turn {
                role: Role::User,
                timestamp: chrono::Utc.with_ymd_and_hms(2026, 3, 11, 0, 0, 0).unwrap(),
                text: "unrelated content".into(),
            }],
        };
        let app = AppState::new(vec![a, b], "osrm", 10);
        assert_eq!(app.visible.len(), 1);
        assert_eq!(app.selected_session().unwrap().session_id, "osrm");
    }

    #[test]
    fn mark_dirty_defers_refilter_and_maybe_refilter_applies_it() {
        let a = make_session("a", 2);
        let mut app = AppState::new(vec![a], "", 10);
        assert!(app.dirty_since.is_none());

        // Mutating the query without touching dirty_since should not change
        // the visible set (it is only applied via refilter / maybe_refilter).
        let visible_before = app.visible.clone();
        app.query.push_str("zzz");
        app.mark_dirty();
        assert!(app.dirty_since.is_some());
        assert_eq!(app.visible, visible_before);

        // Pretend enough time has passed by back-dating `dirty_since`.
        app.dirty_since = Some(Instant::now() - DEBOUNCE - Duration::from_millis(10));
        let remaining = app.maybe_refilter();
        assert!(remaining.is_none());
        assert!(app.dirty_since.is_none());
        assert!(app.visible.is_empty(), "query 'zzz' should match nothing");
    }

    #[test]
    fn maybe_refilter_waits_when_debounce_not_elapsed() {
        let a = make_session("a", 2);
        let mut app = AppState::new(vec![a], "", 10);
        app.mark_dirty();
        let remaining = app.maybe_refilter();
        assert!(remaining.is_some());
        assert!(app.dirty_since.is_some());
    }

    #[test]
    fn appstate_empty_query_shows_all_sorted_by_ended_at_desc() {
        let a = make_session("old", 2);
        let mut a = a;
        a.ended_at = chrono::Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let b = make_session("new", 2);
        let mut b = b;
        b.ended_at = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let app = AppState::new(vec![a, b], "", 10);
        assert_eq!(app.visible.len(), 2);
        assert_eq!(app.sessions[app.visible[0]].session_id, "new");
    }
}
