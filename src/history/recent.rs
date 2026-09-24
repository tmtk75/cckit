// Recent-session listing: filter transcripts by age, group them by project and branch.

use crate::history::SessionRecord;
use crate::history::loader::DETACHED_HEAD;
use chrono::{DateTime, Duration, FixedOffset, Local, Offset, Utc};
use colored::{ColoredString, Colorize};
use std::io::{self, IsTerminal};
use std::path::PathBuf;

/// Label for sessions whose transcript recorded no git branch at all.
const NO_BRANCH: &str = "(no branch)";

/// Keep the sessions whose last activity falls within the last `days` days of `now`.
pub fn filter_recent(
    sessions: Vec<SessionRecord>,
    now: DateTime<Utc>,
    days: i64,
) -> Vec<SessionRecord> {
    let cutoff = now - Duration::days(days);
    sessions
        .into_iter()
        .filter(|s| s.ended_at >= cutoff)
        .collect()
}

/// Cap the list to the `limit` most recently active sessions. `0` means no cap.
pub fn take_newest(mut sessions: Vec<SessionRecord>, limit: usize) -> Vec<SessionRecord> {
    if limit == 0 || sessions.len() <= limit {
        return sessions;
    }
    sessions.sort_by_key(|s| std::cmp::Reverse(s.ended_at));
    sessions.truncate(limit);
    sessions
}

/// Sessions of one project that share a git branch (`None` when the transcript had none).
#[derive(Debug, Clone)]
pub struct BranchGroup {
    pub branch: Option<String>,
    pub sessions: Vec<SessionRecord>,
}

/// One working directory and the branches worked on inside it.
#[derive(Debug, Clone)]
pub struct ProjectGroup {
    pub cwd: PathBuf,
    pub branches: Vec<BranchGroup>,
}

impl ProjectGroup {
    pub fn session_count(&self) -> usize {
        self.branches.iter().map(|b| b.sessions.len()).sum()
    }
}

/// Nest sessions under project then branch. Every level is ordered by its newest
/// session first, which falls out of sorting the input before grouping it.
pub fn group_sessions(mut sessions: Vec<SessionRecord>) -> Vec<ProjectGroup> {
    sessions.sort_by_key(|s| std::cmp::Reverse(s.ended_at));

    let mut groups: Vec<ProjectGroup> = Vec::new();
    for session in sessions {
        let project = match groups.iter_mut().find(|p| p.cwd == session.cwd) {
            Some(p) => p,
            None => {
                groups.push(ProjectGroup {
                    cwd: session.cwd.clone(),
                    branches: Vec::new(),
                });
                groups.last_mut().expect("just pushed")
            }
        };
        match project
            .branches
            .iter_mut()
            .find(|b| b.branch == session.git_branch)
        {
            Some(b) => b.sessions.push(session),
            None => project.branches.push(BranchGroup {
                branch: session.git_branch.clone(),
                sessions: vec![session],
            }),
        }
    }
    groups
}

/// One line per session: when it ran, how long it was, its full id, and the first and
/// last thing the user said. The phrases come last so the line can be cut to the
/// terminal width without losing the identifying columns.
pub fn format_groups(
    groups: &[ProjectGroup],
    now: DateTime<Utc>,
    tz: &FixedOffset,
    width: usize,
    color: bool,
) -> String {
    let mut out = String::new();
    for (i, project) in groups.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let path = project.cwd.display().to_string();
        out.push_str(&format!("{}\n", paint(&path, color, |s| s.cyan().bold())));
        for branch in &project.branches {
            let name = branch.branch.as_deref().unwrap_or(NO_BRANCH);
            let count = branch.sessions.len();
            let unit = if count == 1 { "session" } else { "sessions" };
            let last = branch
                .sessions
                .iter()
                .map(|s| s.ended_at)
                .max()
                .unwrap_or(now);
            // A detached HEAD or a missing branch names no work, so it stays muted.
            let named = name != NO_BRANCH && name != DETACHED_HEAD;
            let tail = format!("({count} {unit}, last {})", relative_since(now, last));
            out.push_str(&format!(
                "  {}  {}\n",
                paint(name, color, |s| if named { s.green() } else { s.dimmed() }),
                paint(&tail, color, |s| s.dimmed()),
            ));
            for session in &branch.sessions {
                out.push_str(&session_line(session, tz, width, color));
                out.push('\n');
            }
        }
    }
    out
}

/// Apply `style` only when color is wanted, so the plain output stays byte-for-byte the
/// text a redirected run would produce.
fn paint(s: &str, color: bool, style: impl Fn(ColoredString) -> ColoredString) -> String {
    if color {
        style(s.into()).to_string()
    } else {
        s.to_string()
    }
}

/// Closing statistics line: how much was found and how much was read to find it.
pub fn format_summary(
    groups: &[ProjectGroup],
    scanned_files: usize,
    elapsed: std::time::Duration,
    days: i64,
) -> String {
    let sessions: usize = groups.iter().map(|p| p.session_count()).sum();
    format!(
        "{} session{} in {} project{} (last {} day{}, scanned {} jsonl files in {:.1}s)\n",
        sessions,
        plural(sessions),
        groups.len(),
        plural(groups.len()),
        days,
        plural(days as usize),
        scanned_files,
        elapsed.as_secs_f64(),
    )
}

/// Flat array, one entry per session, so `jq` consumers do not have to walk the groups.
/// Text fields are NOT truncated.
pub fn format_json(groups: &[ProjectGroup]) -> String {
    let entries: Vec<serde_json::Value> = groups
        .iter()
        .flat_map(|p| p.branches.iter().map(move |b| (p, b)))
        .flat_map(|(p, b)| {
            b.sessions.iter().map(move |s| {
                serde_json::json!({
                    "session_id": s.session_id,
                    "cwd": p.cwd.display().to_string(),
                    "git_branch": b.branch,
                    "started_at": s.started_at.to_rfc3339(),
                    "ended_at": s.ended_at.to_rfc3339(),
                    "turns": s.turns.len(),
                    "bookmark": s.bookmark,
                    "first_user_text": s.first_user_text(),
                    "last_user_text": s.last_user_text(),
                    "file_path": s.file_path.display().to_string(),
                })
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".into())
}

fn plural(n: usize) -> &'static str {
    if n == 1 { "" } else { "s" }
}

/// Coarse "how long ago" label. Takes `now` as an argument so it stays testable.
fn relative_since(now: DateTime<Utc>, dt: DateTime<Utc>) -> String {
    let d = now.signed_duration_since(dt);
    if d.num_minutes() < 1 {
        "just now".into()
    } else if d.num_hours() < 1 {
        format!("{}m ago", d.num_minutes())
    } else if d.num_days() < 1 {
        format!("{}h ago", d.num_hours())
    } else {
        format!("{}d ago", d.num_days())
    }
}

fn session_line(session: &SessionRecord, tz: &FixedOffset, width: usize, color: bool) -> String {
    let start_local = session.started_at.with_timezone(tz);
    let end_local = session.ended_at.with_timezone(tz);
    let started = start_local.format("%m-%d %H:%M");
    // A session left open overnight would otherwise end at a bare time with no day.
    let ended = if start_local.date_naive() == end_local.date_naive() {
        end_local.format("%H:%M").to_string()
    } else {
        end_local.format("%m-%d %H:%M").to_string()
    };
    let turns = format!("{}t", session.turns.len());

    // The columns left of the phrases identify the session, so they are measured and
    // never cut; the phrases take whatever width is left.
    let head = format!(
        "    {started}→{ended}  {turns:>5}  {id}  ",
        id = session.session_id
    );
    let budget = width.saturating_sub(head.chars().count());
    if budget == 0 {
        return truncate_to_width(&head, width);
    }
    let phrases = truncate_to_width(&phrases(session), budget);

    if !color {
        return format!("{head}{phrases}");
    }
    format!(
        "    {}  {}  {}  {}",
        format!("{started}→{ended}").yellow(),
        format!("{turns:>5}").dimmed(),
        session.session_id.blue(),
        paint_label_prefix(&phrases, session),
    )
}

/// Colour only the leading bookmark so it stands out from the phrases sharing its column.
/// A bookmark cut short by the width budget is left alone rather than half-painted.
fn paint_label_prefix(phrases: &str, session: &SessionRecord) -> String {
    let Some(bookmark) = session.bookmark.as_ref() else {
        return phrases.to_string();
    };
    let prefix = format!("★{}", crate::history::format::bookmark_oneline(bookmark));
    match phrases.strip_prefix(&prefix) {
        Some(rest) => format!("{}{}", prefix.magenta(), rest),
        None => phrases.to_string(),
    }
}

/// The bookmark, then "first › last". The bookmark leads because it is what the user chose to
/// remember the session by; it shares the phrase column so no column is added.
fn phrases(session: &SessionRecord) -> String {
    let said = spoken_phrases(session);
    match session.bookmark.as_ref() {
        Some(bookmark) => format!(
            "★{} {}",
            crate::history::format::bookmark_oneline(bookmark),
            said
        ),
        None => said,
    }
}

/// "first › last", collapsed to one phrase when the session has a single user turn.
fn spoken_phrases(session: &SessionRecord) -> String {
    let Some(first) = session.first_user_text() else {
        return "(no user text)".into();
    };
    let first = flatten(first);
    match session.last_user_text().map(flatten) {
        Some(last) if last != first => format!("{first} › {last}"),
        _ => first,
    }
}

fn flatten(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

fn truncate_to_width(line: &str, width: usize) -> String {
    if line.chars().count() <= width {
        return line.to_string();
    }
    let mut out: String = line.chars().take(width.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[derive(Debug, Clone)]
pub struct RecentOpts {
    /// Size of the lookback window in days.
    pub days: i64,
    /// Cap on the number of sessions listed; 0 means no cap.
    pub limit: usize,
    pub json: bool,
}

/// Fall back to a wide fixed width when stdout is a pipe, so redirected output keeps
/// the phrases instead of being cut to whatever the last terminal happened to be.
/// Width used when there is no terminal to measure, or when what it reports is too narrow
/// to hold even the identifying columns of a session line.
const FALLBACK_WIDTH: usize = 200;

/// Narrowest terminal still worth honouring: a session line is 36 characters of id plus
/// its time and turn columns, so anything below this would leave no room for a phrase.
const MIN_WIDTH: usize = 60;

fn usable_width(reported: Option<u16>) -> usize {
    match reported.map(usize::from) {
        Some(cols) if cols >= MIN_WIDTH => cols,
        _ => FALLBACK_WIDTH,
    }
}

fn output_width() -> usize {
    if !std::io::stdout().is_terminal() {
        return FALLBACK_WIDTH;
    }
    usable_width(crossterm::terminal::size().ok().map(|(cols, _)| cols))
}

pub fn run_recent(opts: RecentOpts) -> io::Result<i32> {
    let start = std::time::Instant::now();
    let sessions = super::scan_all_sessions_with_progress();
    let scanned = sessions.len();

    let now = Utc::now();
    let recent = take_newest(filter_recent(sessions, now, opts.days), opts.limit);
    let groups = group_sessions(recent);

    if opts.json {
        println!("{}", format_json(&groups));
    } else {
        // `colored` already folds in the tty check and NO_COLOR / CLICOLOR_FORCE.
        let color = colored::control::SHOULD_COLORIZE.should_colorize();
        let tz = Local::now().offset().fix();
        print!(
            "{}",
            format_groups(&groups, now, &tz, output_width(), color)
        );
        if !groups.is_empty() {
            println!();
        }
        print!(
            "{}",
            format_summary(&groups, scanned, start.elapsed(), opts.days)
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::{DateTime, Duration, FixedOffset, TimeZone, Utc};
    use std::path::PathBuf;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap()
    }

    fn session(id: &str, cwd: &str, branch: Option<&str>, ended: DateTime<Utc>) -> SessionRecord {
        let started = ended - Duration::minutes(30);
        SessionRecord {
            session_id: id.into(),
            cwd: PathBuf::from(cwd),
            git_branch: branch.map(|b| b.to_string()),
            file_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            started_at: started,
            ended_at: ended,
            bookmark: None,
            turns: vec![Turn {
                role: Role::User,
                timestamp: started,
                text: "hello".into(),
            }],
        }
    }

    #[test]
    fn filter_recent_keeps_sessions_inside_the_window_and_drops_older_ones() {
        let inside = session("in", "/p", None, now() - Duration::days(6));
        let outside = session("out", "/p", None, now() - Duration::days(8));
        let kept = filter_recent(vec![inside, outside], now(), 7);
        let ids: Vec<&str> = kept.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["in"]);
    }

    #[test]
    fn take_newest_keeps_the_most_recent_sessions_and_zero_means_no_cap() {
        let sessions = vec![
            session("old", "/p", None, now() - Duration::days(3)),
            session("new", "/p", None, now() - Duration::hours(1)),
            session("mid", "/p", None, now() - Duration::days(1)),
        ];

        let capped = take_newest(sessions.clone(), 2);
        let ids: Vec<&str> = capped.iter().map(|s| s.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "mid"]);

        assert_eq!(take_newest(sessions, 0).len(), 3);
    }

    fn utc_offset() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn with_turns(mut s: SessionRecord, texts: &[(&str, Role)]) -> SessionRecord {
        let ts = s.started_at;
        s.turns = texts
            .iter()
            .map(|(t, r)| Turn {
                role: *r,
                timestamp: ts,
                text: (*t).to_string(),
            })
            .collect();
        s
    }

    #[test]
    fn session_line_holds_times_turn_count_full_id_and_both_phrases() {
        let s = with_turns(
            session(
                "45af2a37-96eb-406c-bd96-36bd6e4d804c",
                "/a",
                Some("feat"),
                now() - Duration::hours(2),
            ),
            &[
                ("recent を作りたい", Role::User),
                ("ok", Role::Assistant),
                ("テスト通ったのでcommitして", Role::User),
            ],
        );

        let out = format_groups(&group_sessions(vec![s]), now(), &utc_offset(), 200, false);
        let line = out
            .lines()
            .find(|l| l.contains("45af2a37-96eb-406c-bd96-36bd6e4d804c"))
            .expect("session line");

        assert!(line.contains("09-18 09:30"), "start time: {line}");
        assert!(line.contains("10:00"), "end time: {line}");
        assert!(line.contains("3t"), "turn count: {line}");
        assert!(
            line.contains("recent を作りたい › テスト通ったのでcommitして"),
            "phrases: {line}"
        );
    }

    #[test]
    fn headers_show_the_project_path_then_each_branch_with_its_count_and_last_activity() {
        let groups = group_sessions(vec![
            session(
                "a",
                "/Users/t/proj",
                Some("feat/x"),
                now() - Duration::hours(2),
            ),
            session(
                "b",
                "/Users/t/proj",
                Some("feat/x"),
                now() - Duration::days(1),
            ),
        ]);

        let out = format_groups(&groups, now(), &utc_offset(), 200, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(lines[0], "/Users/t/proj");
        assert_eq!(lines[1], "  feat/x  (2 sessions, last 2h ago)");
    }

    #[test]
    fn branch_header_names_sessions_without_a_git_branch() {
        let groups = group_sessions(vec![session(
            "a",
            "/p",
            None,
            now() - Duration::minutes(90),
        )]);

        let out = format_groups(&groups, now(), &utc_offset(), 200, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(lines[1], "  (no branch)  (1 session, last 1h ago)");
    }

    #[test]
    fn a_session_spanning_days_dates_its_end_time_too() {
        let mut s = session("sid", "/p", None, now());
        s.started_at = now() - Duration::hours(23);

        let out = format_groups(&group_sessions(vec![s]), now(), &utc_offset(), 200, false);
        let line = out.lines().last().unwrap();

        assert!(line.contains("09-17 13:00→09-18 12:00"), "{line}");
    }

    fn strip_ansi(s: &str) -> String {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            if c != '\u{1b}' {
                out.push(c);
                continue;
            }
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        }
        out
    }

    #[test]
    fn an_unusable_terminal_width_falls_back_to_the_wide_default() {
        assert_eq!(usable_width(Some(120)), 120);
        // A pty with no real geometry (`script`, some CI runners) reports 0 columns, and
        // anything under a session id plus its columns would cut the line to an ellipsis.
        assert_eq!(usable_width(Some(0)), FALLBACK_WIDTH);
        assert_eq!(usable_width(Some(20)), FALLBACK_WIDTH);
        assert_eq!(usable_width(None), FALLBACK_WIDTH);
    }

    #[test]
    fn colored_output_carries_escapes_but_the_same_visible_text() {
        let s = with_turns(
            session("sid", "/p", Some("main"), now() - Duration::hours(1)),
            &[("hello", Role::User), ("bye", Role::User)],
        );
        let groups = group_sessions(vec![s]);

        // `colored` suppresses styling when stdout is not a tty, which it never is under
        // `cargo test`; the override lets the colored path actually emit escapes.
        colored::control::set_override(true);
        let plain = format_groups(&groups, now(), &utc_offset(), 200, false);
        let colored = format_groups(&groups, now(), &utc_offset(), 200, true);

        assert!(!plain.contains('\u{1b}'), "plain output must stay plain");
        assert!(colored.contains('\u{1b}'), "colored output must be colored");
        assert_eq!(strip_ansi(&colored), plain);
    }

    #[test]
    fn a_colored_session_line_still_fits_the_width() {
        let s = with_turns(
            session("sid", "/p", Some("main"), now() - Duration::hours(1)),
            &[(&"x".repeat(300), Role::User)],
        );

        colored::control::set_override(true);
        let out = format_groups(&group_sessions(vec![s]), now(), &utc_offset(), 60, true);
        let line = strip_ansi(out.lines().last().unwrap());

        assert_eq!(line.chars().count(), 60);
    }

    #[test]
    fn session_lines_are_cut_to_the_given_width_while_headers_stay_intact() {
        let s = with_turns(
            session("sid", "/p", Some("main"), now() - Duration::hours(1)),
            &[(&"x".repeat(300), Role::User)],
        );

        let out = format_groups(&group_sessions(vec![s]), now(), &utc_offset(), 60, false);
        let lines: Vec<&str> = out.lines().collect();
        let session_line = lines.last().unwrap();

        assert_eq!(session_line.chars().count(), 60);
        assert!(session_line.ends_with('…'));
        assert_eq!(lines[0], "/p");
    }

    #[test]
    fn summary_line_counts_sessions_projects_and_the_window() {
        let groups = group_sessions(vec![
            session("a", "/p", Some("main"), now() - Duration::hours(1)),
            session("b", "/q", None, now() - Duration::days(2)),
        ]);

        let summary = format_summary(&groups, 831, std::time::Duration::from_millis(1200), 7);

        assert_eq!(
            summary,
            "2 sessions in 2 projects (last 7 days, scanned 831 jsonl files in 1.2s)\n"
        );
    }

    #[test]
    fn json_output_is_a_flat_array_carrying_path_branch_id_and_phrases() {
        let s = with_turns(
            session("sid-1", "/p", Some("main"), now() - Duration::hours(1)),
            &[("start phrase", Role::User), ("end phrase", Role::User)],
        );

        let json = format_json(&group_sessions(vec![s]));
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &parsed.as_array().unwrap()[0];

        assert_eq!(entry["session_id"], "sid-1");
        assert_eq!(entry["cwd"], "/p");
        assert_eq!(entry["git_branch"], "main");
        assert_eq!(entry["turns"], 2);
        assert_eq!(entry["first_user_text"], "start phrase");
        assert_eq!(entry["last_user_text"], "end phrase");
        assert_eq!(entry["file_path"], "/tmp/sid-1.jsonl");
        assert!(entry["started_at"].is_string());
        assert!(entry["ended_at"].is_string());
    }

    #[test]
    fn group_sessions_nests_sessions_under_project_and_branch_newest_first() {
        let sessions = vec![
            session("old-main", "/a", Some("main"), now() - Duration::days(3)),
            session("new-feat", "/a", Some("feat"), now() - Duration::hours(2)),
            session("mid-feat", "/a", Some("feat"), now() - Duration::days(1)),
            session("other", "/b", None, now() - Duration::days(2)),
        ];

        let groups = group_sessions(sessions);

        let shape: Vec<String> = groups
            .iter()
            .flat_map(|p| {
                p.branches.iter().map(|b| {
                    let ids: Vec<&str> = b.sessions.iter().map(|s| s.session_id.as_str()).collect();
                    format!(
                        "{}|{}|{}",
                        p.cwd.display(),
                        b.branch.as_deref().unwrap_or("-"),
                        ids.join(","),
                    )
                })
            })
            .collect();

        assert_eq!(
            shape,
            vec![
                "/a|feat|new-feat,mid-feat",
                "/a|main|old-main",
                "/b|-|other",
            ]
        );
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;
    use crate::history::bookmark::Bookmark;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn tz() -> FixedOffset {
        FixedOffset::east_opt(9 * 3600).unwrap()
    }

    fn bookmarked(name: &str, tags: &[&str]) -> SessionRecord {
        let ts = Utc.with_ymd_and_hms(2026, 9, 18, 3, 0, 0).unwrap();
        SessionRecord {
            session_id: "abc-1".into(),
            cwd: PathBuf::from("/tmp/proj"),
            git_branch: None,
            file_path: PathBuf::from("/tmp/abc-1.jsonl"),
            started_at: ts,
            ended_at: ts,
            turns: vec![Turn {
                role: Role::User,
                timestamp: ts,
                text: "hello".into(),
            }],
            bookmark: Some(Bookmark {
                name: name.into(),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                cwd: None,
                session_started_at: None,
                bookmarked_at: ts,
                updated_at: ts,
            }),
        }
    }

    #[test]
    fn a_labelled_session_leads_its_phrase_column_with_the_name_and_tags() {
        let line = session_line(&bookmarked("auth refactor", &["infra"]), &tz(), 200, false);
        let phrases = line.split("abc-1").nth(1).unwrap();
        assert!(
            phrases.trim_start().starts_with("★auth refactor #infra"),
            "got: {line}"
        );
        assert!(phrases.contains("hello"), "got: {line}");
    }

    #[test]
    fn an_unlabelled_session_line_is_unchanged() {
        let mut session = bookmarked("x", &[]);
        session.bookmark = None;
        let line = session_line(&session, &tz(), 200, false);
        let phrases = line.split("abc-1").nth(1).unwrap();
        assert_eq!(phrases.trim(), "hello");
    }

    #[test]
    fn the_coloured_line_paints_the_label_apart_from_the_phrases() {
        colored::control::set_override(true);
        let line = session_line(&bookmarked("auth refactor", &["infra"]), &tz(), 200, true);
        assert!(line.contains("auth refactor"), "got: {line}");
        // The bookmark carries its own colour sequence, distinct from the uncoloured phrase.
        let painted = "★auth refactor #infra".magenta().to_string();
        assert!(line.contains(&painted), "got: {line}");
    }

    #[test]
    fn json_carries_the_label() {
        let groups = group_sessions(vec![bookmarked("auth refactor", &["infra"])]);
        let parsed: serde_json::Value = serde_json::from_str(&format_json(&groups)).unwrap();
        assert_eq!(parsed[0]["bookmark"]["name"], "auth refactor");
        assert_eq!(parsed[0]["bookmark"]["tags"][0], "infra");
    }
}
