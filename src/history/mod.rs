pub mod bookmark;
pub mod format;
pub mod loader;
pub mod monitor_usage;
pub mod progress;
pub mod recent;
pub mod search;
pub mod skill_usage;
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
    /// Report no matches instead of retrying fuzzily when the exact search comes up empty.
    pub exact: bool,
}

/// Which matcher produced the results being reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMode {
    Exact,
    Fuzzy,
}

impl MatchMode {
    pub fn as_str(self) -> &'static str {
        match self {
            MatchMode::Exact => "exact",
            MatchMode::Fuzzy => "fuzzy",
        }
    }
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
    /// Name and tags the user attached to this session, when it has any.
    pub bookmark: Option<bookmark::Bookmark>,
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

    pub fn last_assistant_text(&self) -> Option<&str> {
        self.turns
            .iter()
            .rev()
            .find(|t| t.role == Role::Assistant)
            .map(|t| t.text.as_str())
    }
}

#[derive(Debug, Clone)]
pub struct Hit {
    pub session: SessionRecord,
    pub matched_turn_indices: Vec<usize>,
    /// True when the session was found through its bookmark rather than through its turns.
    pub bookmark_matched: bool,
}

/// Load every session transcript, drawing a progress line while the scan runs.
/// Returns an empty vec (and no error) when the history directory does not exist.
pub fn scan_all_sessions_with_progress() -> Vec<SessionRecord> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let paths = loader::collect_session_paths(&home.join(".claude/projects"));
    let progress = progress::ScanProgress::start("scanning transcripts", paths.len());
    let mut sessions = loader::parse_paths(&paths, || progress.tick());
    progress.finish();
    // Every reader of this list (search, recent, the interactive browser) wants the
    // bookmarks, so they are attached here rather than at each call site.
    bookmark::attach_bookmarks(
        &mut sessions,
        &bookmark::BookmarkStore::new().load_or_warn(),
    );
    sessions
}

/// Aggregate Skill firings from every transcript, drawing a progress line while the scan
/// runs. Returns an empty map when the history directory does not exist.
pub fn scan_skill_firings_with_progress()
-> std::collections::HashMap<String, skill_usage::SkillFiring> {
    let Some(home) = dirs::home_dir() else {
        return std::collections::HashMap::new();
    };
    let paths = skill_usage::collect_jsonl_paths(&home.join(".claude/projects"));
    let progress = progress::ScanProgress::start("scanning transcripts", paths.len());
    let firings = skill_usage::aggregate_paths(&paths, || progress.tick());
    progress.finish();
    firings
}

/// Collect the Monitor launches recorded in every transcript, drawing a progress line
/// while the scan runs. Returns the launches and how many files were read, so the caller
/// can report the corpus it covered. Empty when the history directory does not exist.
pub fn scan_monitor_launches_with_progress() -> (Vec<monitor_usage::MonitorLaunch>, usize) {
    let Some(home) = dirs::home_dir() else {
        return (Vec::new(), 0);
    };
    let paths = skill_usage::collect_jsonl_paths(&home.join(".claude/projects"));
    let progress = progress::ScanProgress::start("scanning transcripts", paths.len());
    let launches = monitor_usage::collect_paths(&paths, || progress.tick());
    progress.finish();
    (launches, paths.len())
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
        let sessions = scan_all_sessions_with_progress();
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
        let sessions = scan_all_sessions_with_progress();
        let scanned = sessions.len();

        // Exact first. When it finds nothing, retry the same corpus fuzzily rather than
        // reporting an empty result: typos are the common reason for a miss. The retry
        // costs nothing extra, the transcripts are already in memory.
        let mut hits = search::search_sessions(&sessions, &opts.terms, opts.limit);
        let mut mode = MatchMode::Exact;
        if hits.is_empty() && !opts.exact {
            let query = opts.terms.join(" ");
            let fuzzy = search::fuzzy_search_sessions(&sessions, &query, opts.limit);
            if !fuzzy.is_empty() {
                hits = fuzzy;
                mode = MatchMode::Fuzzy;
            }
        }
        let elapsed = start.elapsed();

        if opts.json {
            // Keep stdout a bare JSON array for consumers; the notice goes to stderr.
            if mode == MatchMode::Fuzzy {
                eprintln!("{}", format::fuzzy_fallback_notice(&opts.terms));
            }
            println!("{}", format::format_json(&hits));
        } else {
            print!(
                "{}",
                format::format_oneshot(&hits, scanned, elapsed, mode, &opts.terms)
            );
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
            bookmark: None,
            turns,
        };
        assert_eq!(rec.turns.len(), 2);
        assert_eq!(rec.first_user_text(), Some("hello"));
        assert_eq!(rec.last_user_text(), Some("hello"));
    }

    #[test]
    fn last_assistant_text_returns_most_recent_assistant_turn() {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let turns = vec![
            Turn {
                role: Role::User,
                timestamp: ts,
                text: "q1".into(),
            },
            Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "a1".into(),
            },
            Turn {
                role: Role::User,
                timestamp: ts,
                text: "q2".into(),
            },
            Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "a2".into(),
            },
        ];
        let rec = SessionRecord {
            session_id: "abc".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: "/tmp/x.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            bookmark: None,
            turns,
        };
        assert_eq!(rec.last_assistant_text(), Some("a2"));
    }

    #[test]
    fn last_assistant_text_returns_none_when_no_assistant_turns() {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let rec = SessionRecord {
            session_id: "abc".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: "/tmp/x.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            bookmark: None,
            turns: vec![Turn {
                role: Role::User,
                timestamp: ts,
                text: "q".into(),
            }],
        };
        assert_eq!(rec.last_assistant_text(), None);
    }
}
