pub mod format;
pub mod loader;
pub mod search;
pub mod tui;

use chrono::{DateTime, Utc};
use std::io;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct SearchOpts {
    pub terms: Vec<String>,
    pub interactive: bool,
    pub limit: usize,
    pub json: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone)]
pub struct Turn {
    pub role: Role,
    pub timestamp: DateTime<Utc>,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    pub session_id: String,
    pub cwd: PathBuf,
    pub git_branch: Option<String>,
    pub file_path: PathBuf,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub turns: Vec<Turn>,
}

impl SessionRecord {
    pub fn first_user_text(&self) -> Option<&str> {
        self.turns
            .iter()
            .find(|t| t.role == Role::User)
            .map(|t| t.text.as_str())
    }

    pub fn last_user_text(&self) -> Option<&str> {
        self.turns
            .iter()
            .rev()
            .find(|t| t.role == Role::User)
            .map(|t| t.text.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub session: SessionRecord,
    pub matched_turn_indices: Vec<usize>,
}

pub fn run_search(opts: SearchOpts) -> io::Result<i32> {
    if opts.interactive && opts.json {
        eprintln!("error: --json cannot be combined with --interactive");
        return Ok(2);
    }
    if !opts.interactive && opts.terms.is_empty() {
        eprintln!("error: at least one search term is required (or use --interactive)");
        return Ok(2);
    }

    if opts.interactive {
        // Interactive mode: load all sessions, pass to TUI which filters live.
        let load_start = std::time::Instant::now();
        let sessions = loader::scan_all_sessions();
        let load_elapsed = load_start.elapsed();
        let initial_query = opts.terms.join(" ");
        match tui::run_tui(sessions, &initial_query, opts.limit, load_elapsed)? {
            Some(cwd) => {
                println!("{}", cwd.display());
                Ok(0)
            }
            None => Ok(1),
        }
    } else {
        let start = std::time::Instant::now();
        let sessions = loader::scan_all_sessions();
        let scanned = sessions.len();
        let hits = search::search_sessions(sessions, &opts.terms, opts.limit);
        let elapsed = start.elapsed();

        if opts.json {
            println!("{}", format::format_json(&hits));
        } else {
            print!("{}", format::format_oneshot(&hits, scanned, elapsed));
        }
        Ok(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn session_record_constructs_and_reports_turn_count() {
        let ts = chrono::Utc
            .with_ymd_and_hms(2026, 3, 12, 6, 12, 48)
            .unwrap();
        let turns = vec![
            Turn {
                role: Role::User,
                timestamp: ts,
                text: "hello".into(),
            },
            Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "world".into(),
            },
        ];
        let rec = SessionRecord {
            session_id: "abc".into(),
            cwd: "/tmp/foo".into(),
            git_branch: Some("main".into()),
            file_path: "/tmp/foo.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            turns,
        };
        assert_eq!(rec.turns.len(), 2);
        assert_eq!(rec.first_user_text(), Some("hello"));
        assert_eq!(rec.last_user_text(), Some("hello"));
    }
}
