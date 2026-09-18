// Monitor launch history: mine Monitor tool invocations from ~/.claude/projects/**/*.jsonl
// to answer "which projects run background monitors, how often, and when last".
//
// Only the launch side is recovered here. Whether a monitor is still running is a
// property of the process table, not of the transcript: a launch with `persistent: false`
// expires after its timeout, and a monitor killed by TaskStop leaves the transcript
// looking exactly like one that is still streaming.

use chrono::{DateTime, Duration, FixedOffset, Local, Offset, Utc};
use colored::{ColoredString, Colorize};
use rayon::prelude::*;
use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

/// Label for launches whose tool input carried no description.
pub const NO_DESCRIPTION: &str = "(no description)";

/// One Monitor tool launch recovered from a transcript line.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorLaunch {
    pub started_at: DateTime<Utc>,
    pub cwd: String,
    pub session_id: String,
    pub description: String,
    pub command: String,
}

/// Every launch of one monitor (identified by its description) inside one project.
#[derive(Debug, Clone)]
pub struct MonitorGroup {
    pub description: String,
    /// Command of the most recent launch; earlier ones may differ (session id arguments).
    pub command: String,
    pub count: u32,
    pub last_started: DateTime<Utc>,
    /// Session that launched it last, so the row can be followed up with `session search`.
    pub last_session_id: String,
    /// Every distinct command this monitor was launched with. A relaunch in a new session
    /// carries a new session id inside the command, so recognizing a running process
    /// takes the whole set, not just the newest one.
    pub commands: Vec<String>,
    /// How many processes are running this monitor right now; `None` when nobody looked.
    pub alive: Option<u32>,
}

/// One working directory and the monitors launched inside it.
#[derive(Debug, Clone)]
pub struct ProjectMonitors {
    pub cwd: String,
    pub monitors: Vec<MonitorGroup>,
    /// Whether the directory is still on disk. Transcripts outlive the directories they
    /// were recorded in, and a removed git worktree leaves a path nothing can `cd` into.
    pub exists: bool,
}

impl ProjectMonitors {
    pub fn launch_count(&self) -> u32 {
        self.monitors.iter().map(|m| m.count).sum()
    }

    fn last_started(&self) -> DateTime<Utc> {
        self.monitors
            .iter()
            .map(|m| m.last_started)
            .max()
            .unwrap_or_else(Utc::now)
    }
}

/// Extract every Monitor launch recorded on one transcript JSONL line.
///
/// A single assistant line can carry several `tool_use` blocks, so this returns a Vec.
/// Lines that merely mention the tool (the `deferred_tools_record` attachment listing
/// available tools, for instance) carry no `type: "tool_use"` and are ignored.
pub fn parse_monitor_launches_line(line: &str) -> Vec<MonitorLaunch> {
    // Cheap pre-filter before the JSON parse.
    if !line.contains("\"name\":\"Monitor\"") {
        return Vec::new();
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
        return Vec::new();
    };
    let Some(started_at) = v
        .get("timestamp")
        .and_then(|t| t.as_str())
        .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&Utc))
    else {
        return Vec::new();
    };
    let cwd = v.get("cwd").and_then(|c| c.as_str()).unwrap_or_default();
    let session_id = v
        .get("sessionId")
        .and_then(|s| s.as_str())
        .unwrap_or_default();

    let mut inputs = Vec::new();
    collect_monitor_inputs(&v, &mut inputs);
    inputs
        .into_iter()
        .map(|input| MonitorLaunch {
            started_at,
            cwd: cwd.to_string(),
            session_id: session_id.to_string(),
            description: input
                .get("description")
                .and_then(|d| d.as_str())
                .filter(|d| !d.trim().is_empty())
                .unwrap_or(NO_DESCRIPTION)
                .to_string(),
            command: input
                .get("command")
                .and_then(|c| c.as_str())
                .unwrap_or_default()
                .to_string(),
        })
        .collect()
}

/// Collect the `input` of every `tool_use` block naming the Monitor tool. The search is
/// defensive about where the blocks sit rather than assuming `message.content[]`.
fn collect_monitor_inputs<'a>(v: &'a serde_json::Value, out: &mut Vec<&'a serde_json::Value>) {
    match v {
        serde_json::Value::Object(map) => {
            let is_monitor_call = map.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && map.get("name").and_then(|n| n.as_str()) == Some("Monitor");
            if is_monitor_call && let Some(input) = map.get("input").filter(|i| i.is_object()) {
                out.push(input);
            }
            for val in map.values() {
                collect_monitor_inputs(val, out);
            }
        }
        serde_json::Value::Array(arr) => {
            for val in arr {
                collect_monitor_inputs(val, out);
            }
        }
        _ => {}
    }
}

/// Keep the launches that happened within the last `days` days of `now`. `days <= 0`
/// keeps everything, which is how the caller asks for the full history.
pub fn filter_recent(
    launches: Vec<MonitorLaunch>,
    now: DateTime<Utc>,
    days: i64,
) -> Vec<MonitorLaunch> {
    if days <= 0 {
        return launches;
    }
    let cutoff = now - Duration::days(days);
    launches
        .into_iter()
        .filter(|l| l.started_at >= cutoff)
        .collect()
}

/// Keep the launches whose description or command contains `pattern`, ignoring case.
pub fn filter_pattern(launches: Vec<MonitorLaunch>, pattern: &str) -> Vec<MonitorLaunch> {
    let needle = pattern.to_lowercase();
    launches
        .into_iter()
        .filter(|l| {
            l.description.to_lowercase().contains(&needle)
                || l.command.to_lowercase().contains(&needle)
        })
        .collect()
}

/// Fold launches into per-project, per-description groups. Projects are ordered by their
/// most recent launch first, and so are the monitors inside each one.
pub fn group_launches(launches: Vec<MonitorLaunch>) -> Vec<ProjectMonitors> {
    let mut projects: Vec<ProjectMonitors> = Vec::new();
    for launch in launches {
        let project = match projects.iter_mut().find(|p| p.cwd == launch.cwd) {
            Some(p) => p,
            None => {
                projects.push(ProjectMonitors {
                    cwd: launch.cwd.clone(),
                    monitors: Vec::new(),
                    // Nothing has looked at the filesystem yet; `mark_missing` decides.
                    exists: true,
                });
                projects.last_mut().expect("just pushed")
            }
        };
        match project
            .monitors
            .iter_mut()
            .find(|m| m.description == launch.description)
        {
            Some(m) => {
                m.count += 1;
                if !m.commands.contains(&launch.command) {
                    m.commands.push(launch.command.clone());
                }
                if launch.started_at > m.last_started {
                    m.last_started = launch.started_at;
                    m.command = launch.command;
                    m.last_session_id = launch.session_id;
                }
            }
            None => project.monitors.push(MonitorGroup {
                description: launch.description,
                commands: vec![launch.command.clone()],
                command: launch.command,
                count: 1,
                last_started: launch.started_at,
                last_session_id: launch.session_id,
                alive: None,
            }),
        }
    }

    for project in &mut projects {
        project
            .monitors
            .sort_by_key(|m| std::cmp::Reverse(m.last_started));
    }
    projects.sort_by_key(|p| std::cmp::Reverse(p.last_started()));
    projects
}

/// Record which project directories are still on disk. `exists` is taken as an argument
/// so the decision stays testable without touching the filesystem.
pub fn mark_missing(projects: &mut [ProjectMonitors], exists: impl Fn(&str) -> bool) {
    for project in projects.iter_mut() {
        project.exists = exists(&project.cwd);
    }
}

/// Drop the projects whose directory is gone, returning how many were dropped so the
/// summary can say so instead of quietly showing smaller numbers.
pub fn drop_missing(projects: &mut Vec<ProjectMonitors>) -> usize {
    let before = projects.len();
    projects.retain(|p| p.exists);
    before - projects.len()
}

/// One row of `ps -eo pid=,ppid=,command=`.
#[derive(Debug, Clone, PartialEq)]
pub struct PsRow {
    pub pid: u32,
    pub ppid: u32,
    pub command: String,
}

/// A background task process still running under a Claude Code session.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveTask {
    pub pid: u32,
    /// The process that launched it, which is the `claude` running the session.
    pub session_pid: u32,
    /// The command Claude Code was asked to run, recovered from the shell wrapper.
    pub command: String,
}

/// Marker that Claude Code's shell wrapper leaves in every background task it spawns.
/// A task is launched as `zsh -c source <snapshot> … && eval '<command>' < /dev/null && …`,
/// so both parts have to be present before the payload can be recovered.
const SNAPSHOT_MARKER: &str = "/shell-snapshots/snapshot-";
const EVAL_OPEN: &str = " eval '";
const EVAL_CLOSE: &str = "' < /dev/null";

pub fn parse_ps_table(output: &str) -> Vec<PsRow> {
    output
        .lines()
        .filter_map(|line| {
            let mut fields = line.trim_start().splitn(3, char::is_whitespace);
            let pid = fields.next()?.parse().ok()?;
            let ppid = fields.next()?.trim_start().parse().ok()?;
            let command = fields.next()?.trim_start().to_string();
            Some(PsRow { pid, ppid, command })
        })
        .collect()
}

/// Recover the command Claude Code was given from the shell wrapper `ps` reports.
/// Returns None for any process that is not one of those wrappers.
pub fn task_command(wrapper: &str) -> Option<String> {
    if !wrapper.contains(SNAPSHOT_MARKER) {
        return None;
    }
    let start = wrapper.find(EVAL_OPEN)? + EVAL_OPEN.len();
    let rest = &wrapper[start..];
    // The payload itself may hold quotes, so the closing marker is matched from the right.
    let end = rest.rfind(EVAL_CLOSE)?;
    // `'\''` is how the shell embeds a single quote inside a single-quoted string.
    Some(rest[..end].replace("'\\''", "'"))
}

/// Pick the Claude Code background tasks out of a process table.
pub fn live_tasks(rows: &[PsRow]) -> Vec<LiveTask> {
    rows.iter()
        .filter_map(|row| {
            Some(LiveTask {
                pid: row.pid,
                session_pid: row.ppid,
                command: task_command(&row.command)?,
            })
        })
        .collect()
}

/// Read the process table. An unusable `ps` is reported as an empty table rather than as
/// an error: the launch history is still worth printing without the liveness column.
pub fn scan_live_tasks() -> Vec<LiveTask> {
    let Ok(output) = std::process::Command::new("ps")
        .args(["-eo", "pid=,ppid=,command="])
        .output()
    else {
        return Vec::new();
    };
    live_tasks(&parse_ps_table(&String::from_utf8_lossy(&output.stdout)))
}

/// Attribute each running task to the monitor that launched it, setting `alive` on every
/// group (to `Some(0)` when nothing matched, so the output can say the check ran).
/// Returns the tasks no group claimed.
pub fn mark_alive(projects: &mut [ProjectMonitors], live: &[LiveTask]) -> Vec<LiveTask> {
    let mut claimed = vec![false; live.len()];
    for project in projects.iter_mut() {
        for monitor in &mut project.monitors {
            let mut count = 0;
            for (i, task) in live.iter().enumerate() {
                if monitor.commands.contains(&task.command) {
                    claimed[i] = true;
                    count += 1;
                }
            }
            monitor.alive = Some(count);
        }
    }
    live.iter()
        .zip(claimed)
        .filter(|(_, claimed)| !claimed)
        .map(|(task, _)| task.clone())
        .collect()
}

/// How many processes are running the listed monitors right now.
pub fn running_count(projects: &[ProjectMonitors]) -> u32 {
    projects
        .iter()
        .flat_map(|p| p.monitors.iter())
        .filter_map(|m| m.alive)
        .sum()
}

/// Trailing block for monitors that are running but whose launch the window or the filter
/// left out. Without it, narrowing `--days` would quietly hide a monitor still streaming.
pub fn format_unclaimed(tasks: &[LiveTask], width: usize, color: bool) -> String {
    if tasks.is_empty() {
        return String::new();
    }
    let mut out = format!(
        "{}\n",
        paint("running but not listed above:", color, |s| s.yellow())
    );
    for task in tasks {
        let head = format!("    {} ", task.pid);
        let budget = width.saturating_sub(head.chars().count());
        let command = truncate_to_width(&flatten(&task.command), budget);
        out.push_str(&format!(
            "{}{}\n",
            head,
            paint(&command, color, |s| s.dimmed())
        ));
    }
    out
}

/// One line per monitor under a project header: when it last ran, how long ago that was,
/// how many times it has run, and the description it was launched with. The description
/// comes last so the line can be cut to the terminal width without losing the columns.
pub fn format_groups(
    projects: &[ProjectMonitors],
    now: DateTime<Utc>,
    tz: &FixedOffset,
    width: usize,
    color: bool,
) -> String {
    let mut out = String::new();
    for (i, project) in projects.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let gone = if project.exists { "" } else { "  (gone)" };
        out.push_str(&format!(
            "{}{}\n",
            paint(&project.cwd, color, |s| if project.exists {
                s.cyan().bold()
            } else {
                s.dimmed()
            }),
            paint(gone, color, |s| s.dimmed()),
        ));
        for monitor in &project.monitors {
            out.push_str(&monitor_line(monitor, now, tz, width, color));
            out.push('\n');
        }
    }
    out
}

fn monitor_line(
    monitor: &MonitorGroup,
    now: DateTime<Utc>,
    tz: &FixedOffset,
    width: usize,
    color: bool,
) -> String {
    let last = monitor.last_started.with_timezone(tz).format("%m-%d %H:%M");
    let ago = relative_since(now, monitor.last_started);
    let count = monitor.count.to_string();
    // The marker sits inside the existing indent, so a run with liveness checked lines up
    // column for column with one without it.
    let running = monitor.alive.unwrap_or(0) > 0;
    let (lead, marker) = if running {
        ("  ", "● ")
    } else {
        ("    ", "")
    };

    // The columns left of the description identify the monitor, so they are measured and
    // never cut; the description takes whatever width is left.
    let head = format!("{lead}{marker}{last}  {ago:>8}  {count:>5}  ");
    let budget = width.saturating_sub(head.chars().count());
    if budget == 0 {
        return truncate_to_width(&head, width);
    }
    let description = truncate_to_width(&flatten(&monitor.description), budget);

    if !color {
        return format!("{head}{description}");
    }
    format!(
        "{lead}{}{}  {}  {}  {}",
        paint(marker, color, |s| s.green()),
        last.to_string().yellow(),
        format!("{ago:>8}").dimmed(),
        format!("{count:>5}").dimmed(),
        description,
    )
}

/// Closing statistics line: how much was found and how much was read to find it.
pub fn format_summary(
    projects: &[ProjectMonitors],
    scanned_files: usize,
    elapsed: std::time::Duration,
    days: i64,
    running: Option<u32>,
    hidden: usize,
) -> String {
    let launches: u32 = projects.iter().map(|p| p.launch_count()).sum();
    let monitors: usize = projects.iter().map(|p| p.monitors.len()).sum();
    let window = if days <= 0 {
        "all time".to_string()
    } else {
        format!("last {} day{}", days, plural(days as usize))
    };
    let running = match running {
        Some(n) => format!(", {n} running now"),
        None => String::new(),
    };
    let hidden = match hidden {
        0 => String::new(),
        n => format!("{} gone project{} hidden, ", n, plural(n)),
    };
    format!(
        "{} launch{} of {} monitor{} in {} project{}{running} ({}, {hidden}scanned {} jsonl files in {:.1}s)\n",
        launches,
        if launches == 1 { "" } else { "es" },
        monitors,
        plural(monitors),
        projects.len(),
        plural(projects.len()),
        window,
        scanned_files,
        elapsed.as_secs_f64(),
    )
}

/// Flat array, one entry per monitor, so `jq` consumers do not have to walk the groups.
/// Text fields are NOT truncated.
pub fn format_json(projects: &[ProjectMonitors]) -> String {
    let entries: Vec<serde_json::Value> = projects
        .iter()
        .flat_map(|p| {
            p.monitors.iter().map(move |m| {
                serde_json::json!({
                    "cwd": p.cwd,
                    "cwd_exists": p.exists,
                    "description": m.description,
                    "command": m.command,
                    "count": m.count,
                    "last_started": m.last_started.to_rfc3339(),
                    "last_session_id": m.last_session_id,
                    // null when liveness was not checked, so a 0 always means "checked".
                    "alive": m.alive,
                })
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".into())
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

/// Collect the Monitor launches in every path, calling `on_file` once per file. Parsing
/// runs on rayon's pool; the per-file vectors concatenate in an arbitrary order, which
/// `group_launches` does not depend on.
pub fn collect_paths(paths: &[PathBuf], on_file: impl Fn() + Sync + Send) -> Vec<MonitorLaunch> {
    paths
        .par_iter()
        .map(|path| {
            let content = fs::read_to_string(path);
            on_file();
            let Ok(content) = content else {
                return Vec::new();
            };
            content
                .lines()
                .flat_map(parse_monitor_launches_line)
                .collect::<Vec<_>>()
        })
        .reduce(Vec::new, |mut acc, mut other| {
            acc.append(&mut other);
            acc
        })
}

/// Walk every `*.jsonl` under `root`, collecting the Monitor launches recorded in them.
pub fn collect_launches(root: &Path) -> Vec<MonitorLaunch> {
    collect_paths(&super::skill_usage::collect_jsonl_paths(root), || {})
}

#[derive(Debug, Clone)]
pub struct MonitorLsOpts {
    /// Size of the lookback window in days; 0 lists the full history.
    pub days: i64,
    /// Keep only the monitors whose description or command contains this text.
    pub filter: Option<String>,
    /// Cross-check the process table and mark the monitors running right now.
    pub alive: bool,
    /// Keep the projects whose directory is gone instead of dropping them.
    pub include_missing: bool,
    pub json: bool,
}

/// Width used when there is no terminal to measure, or when what it reports is too narrow
/// to hold even the identifying columns of a monitor line.
const FALLBACK_WIDTH: usize = 200;

/// Narrowest terminal still worth honouring: below this the description would be cut to
/// an ellipsis, leaving the line unreadable.
const MIN_WIDTH: usize = 50;

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

pub fn run_monitor_ls(opts: MonitorLsOpts) -> io::Result<i32> {
    let start = std::time::Instant::now();
    let (launches, scanned) = super::scan_monitor_launches_with_progress();

    let now = Utc::now();
    // Kept before the window is applied: a process still running from a launch that
    // `--days` excludes is exactly what the trailing block has to be able to name.
    let known_commands: std::collections::HashSet<String> = if opts.alive {
        launches.iter().map(|l| l.command.clone()).collect()
    } else {
        std::collections::HashSet::new()
    };

    let mut kept = filter_recent(launches, now, opts.days);
    if let Some(pattern) = &opts.filter {
        kept = filter_pattern(kept, pattern);
    }
    let mut projects = group_launches(kept);
    mark_missing(&mut projects, |cwd| Path::new(cwd).is_dir());
    // Dropped before the liveness check, so a monitor still running inside a directory
    // that has been removed surfaces in the trailing block rather than vanishing.
    let hidden = if opts.include_missing {
        0
    } else {
        drop_missing(&mut projects)
    };

    let mut unclaimed = Vec::new();
    if opts.alive {
        unclaimed = mark_alive(&mut projects, &scan_live_tasks())
            .into_iter()
            // Anything the transcripts never recorded as a monitor is another kind of
            // background task (a `run_in_background` Bash call, say), not ours to report.
            .filter(|task| known_commands.contains(&task.command))
            .collect();
    }
    let running = opts
        .alive
        .then(|| running_count(&projects) + unclaimed.len() as u32);

    if opts.json {
        println!("{}", format_json(&projects));
        // Same split as `session search`: the notice goes to stderr so stdout stays a
        // bare JSON array, but a running monitor still never disappears silently.
        for task in &unclaimed {
            eprintln!(
                "note: running but not listed (pid {}): {}",
                task.pid, task.command
            );
        }
    } else {
        // `colored` already folds in the tty check and NO_COLOR / CLICOLOR_FORCE.
        let color = colored::control::SHOULD_COLORIZE.should_colorize();
        let tz = Local::now().offset().fix();
        let width = output_width();
        print!("{}", format_groups(&projects, now, &tz, width, color));
        if !projects.is_empty() {
            println!();
        }
        let unclaimed = format_unclaimed(&unclaimed, width, color);
        if !unclaimed.is_empty() {
            print!("{unclaimed}");
            println!();
        }
        print!(
            "{}",
            format_summary(
                &projects,
                scanned,
                start.elapsed(),
                opts.days,
                running,
                hidden
            )
        );
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-09-20T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn utc_offset() -> FixedOffset {
        FixedOffset::east_opt(0).unwrap()
    }

    fn launch(cwd: &str, description: &str, at: DateTime<Utc>) -> MonitorLaunch {
        MonitorLaunch {
            started_at: at,
            cwd: cwd.into(),
            session_id: "sid".into(),
            description: description.into(),
            command: "watch.sh".into(),
        }
    }

    #[test]
    fn parses_a_monitor_launch_line() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-19T23:13:33.111Z","sessionId":"e41433b8","cwd":"/Users/t/workspace","message":{"content":[{"type":"tool_use","id":"toolu_1","name":"Monitor","input":{"command":"watch.sh e41433b8","description":"agmsg inbox stream","timeout_ms":1800000}}]}}"#;

        let launches = parse_monitor_launches_line(line);

        assert_eq!(launches.len(), 1);
        assert_eq!(launches[0].cwd, "/Users/t/workspace");
        assert_eq!(launches[0].session_id, "e41433b8");
        assert_eq!(launches[0].description, "agmsg inbox stream");
        assert_eq!(launches[0].command, "watch.sh e41433b8");
        assert_eq!(
            launches[0].started_at.to_rfc3339(),
            "2026-09-19T23:13:33.111+00:00"
        );
    }

    #[test]
    fn a_line_listing_monitor_as_an_available_tool_is_not_a_launch() {
        // The deferred_tools_record attachment names every tool the session may call.
        // It carries a name and a description but no tool_use block, so counting it
        // would inflate every session that merely had the tool available.
        let line = r#"{"type":"user","timestamp":"2026-09-19T23:13:34.000Z","cwd":"/p","attachment":{"type":"deferred_tools_record","entries":[{"name":"Monitor","description":"Start a background monitor that streams events."}]}}"#;

        assert!(parse_monitor_launches_line(line).is_empty());
    }

    #[test]
    fn parses_every_tool_use_on_a_line_that_launched_two_monitors() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-19T23:13:33Z","sessionId":"s","cwd":"/p","message":{"content":[{"type":"tool_use","name":"Monitor","input":{"command":"a","description":"first"}},{"type":"tool_use","name":"Monitor","input":{"command":"b","description":"second"}}]}}"#;

        let descriptions: Vec<String> = parse_monitor_launches_line(line)
            .into_iter()
            .map(|l| l.description)
            .collect();

        assert_eq!(descriptions, vec!["first", "second"]);
    }

    #[test]
    fn a_launch_without_a_description_gets_a_placeholder() {
        let line = r#"{"type":"assistant","timestamp":"2026-09-19T23:13:33Z","sessionId":"s","cwd":"/p","message":{"content":[{"type":"tool_use","name":"Monitor","input":{"command":"a"}}]}}"#;

        assert_eq!(
            parse_monitor_launches_line(line)[0].description,
            NO_DESCRIPTION
        );
    }

    #[test]
    fn lines_without_a_timestamp_or_without_the_tool_are_skipped() {
        let no_ts = r#"{"type":"assistant","cwd":"/p","message":{"content":[{"type":"tool_use","name":"Monitor","input":{"command":"a","description":"d"}}]}}"#;
        let other_tool = r#"{"type":"assistant","timestamp":"2026-09-19T23:13:33Z","cwd":"/p","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"ls"}}]}}"#;

        assert!(parse_monitor_launches_line(no_ts).is_empty());
        assert!(parse_monitor_launches_line(other_tool).is_empty());
        assert!(parse_monitor_launches_line("not json at all").is_empty());
    }

    #[test]
    fn grouping_counts_launches_per_project_and_description_newest_first() {
        let launches = vec![
            launch("/a", "agmsg inbox stream", now() - Duration::days(3)),
            launch("/a", "agmsg inbox stream", now() - Duration::hours(2)),
            launch("/a", "CI run status", now() - Duration::days(1)),
            launch("/b", "agmsg inbox stream", now() - Duration::days(5)),
        ];

        let projects = group_launches(launches);

        let shape: Vec<String> = projects
            .iter()
            .flat_map(|p| {
                p.monitors
                    .iter()
                    .map(move |m| format!("{}|{}|{}", p.cwd, m.description, m.count))
            })
            .collect();

        assert_eq!(
            shape,
            vec![
                "/a|agmsg inbox stream|2",
                "/a|CI run status|1",
                "/b|agmsg inbox stream|1",
            ]
        );
    }

    #[test]
    fn a_group_keeps_the_command_and_session_of_its_newest_launch() {
        let mut old = launch("/a", "agmsg inbox stream", now() - Duration::days(3));
        old.command = "watch.sh old-session".into();
        old.session_id = "old-session".into();
        let mut new = launch("/a", "agmsg inbox stream", now() - Duration::hours(1));
        new.command = "watch.sh new-session".into();
        new.session_id = "new-session".into();

        // Fed oldest-last so the newest is not simply the one that arrived last.
        let projects = group_launches(vec![new, old]);
        let group = &projects[0].monitors[0];

        assert_eq!(group.command, "watch.sh new-session");
        assert_eq!(group.last_session_id, "new-session");
        assert_eq!(group.count, 2);
    }

    #[test]
    fn filters_drop_launches_outside_the_window_and_off_the_pattern() {
        let launches = vec![
            launch("/a", "agmsg inbox stream", now() - Duration::days(2)),
            launch("/a", "CI run status", now() - Duration::days(100)),
            launch("/b", "CI run status", now() - Duration::days(1)),
        ];

        let recent = filter_recent(launches.clone(), now(), 90);
        assert_eq!(recent.len(), 2);

        // days <= 0 is the "no window" request, not an empty window.
        assert_eq!(filter_recent(launches.clone(), now(), 0).len(), 3);

        let matched = filter_pattern(launches, "AGMSG");
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].description, "agmsg inbox stream");
    }

    #[test]
    fn a_monitor_line_holds_the_last_run_its_age_the_count_and_the_description() {
        let projects = group_launches(vec![
            launch(
                "/Users/t/proj",
                "agmsg inbox stream",
                now() - Duration::hours(2),
            ),
            launch(
                "/Users/t/proj",
                "agmsg inbox stream",
                now() - Duration::days(4),
            ),
        ]);

        let out = format_groups(&projects, now(), &utc_offset(), 200, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(lines[0], "/Users/t/proj");
        assert_eq!(
            lines[1],
            "    09-19 22:00    2h ago      2  agmsg inbox stream"
        );
    }

    #[test]
    fn monitor_lines_are_cut_to_the_given_width_while_headers_stay_intact() {
        let projects = group_launches(vec![launch(
            "/p",
            &"x".repeat(300),
            now() - Duration::hours(1),
        )]);

        let out = format_groups(&projects, now(), &utc_offset(), 60, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(lines[0], "/p");
        assert_eq!(lines[1].chars().count(), 60);
        assert!(lines[1].ends_with('…'));
    }

    #[test]
    fn colored_output_carries_escapes_but_the_same_visible_text() {
        let projects = group_launches(vec![launch("/p", "agmsg inbox stream", now())]);

        // `colored` suppresses styling when stdout is not a tty, which it never is under
        // `cargo test`; the override lets the colored path actually emit escapes.
        colored::control::set_override(true);
        let plain = format_groups(&projects, now(), &utc_offset(), 200, false);
        let colored = format_groups(&projects, now(), &utc_offset(), 200, true);

        assert!(!plain.contains('\u{1b}'), "plain output must stay plain");
        assert!(colored.contains('\u{1b}'), "colored output must be colored");
        assert_eq!(strip_ansi(&colored), plain);
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
    fn summary_line_counts_launches_monitors_projects_and_the_window() {
        let projects = group_launches(vec![
            launch("/a", "agmsg inbox stream", now() - Duration::hours(1)),
            launch("/a", "agmsg inbox stream", now() - Duration::hours(2)),
            launch("/b", "CI run status", now() - Duration::days(2)),
        ]);

        let summary = format_summary(
            &projects,
            831,
            std::time::Duration::from_millis(1200),
            90,
            None,
            0,
        );
        assert_eq!(
            summary,
            "3 launches of 2 monitors in 2 projects (last 90 days, scanned 831 jsonl files in 1.2s)\n"
        );

        let all = format_summary(
            &projects,
            831,
            std::time::Duration::from_millis(1200),
            0,
            None,
            0,
        );
        assert!(all.contains("(all time, scanned 831 jsonl files"), "{all}");
    }

    #[test]
    fn the_summary_says_how_many_gone_projects_it_hid() {
        let projects = group_launches(vec![launch("/a", "agmsg inbox stream", now())]);

        let summary = format_summary(
            &projects,
            1,
            std::time::Duration::from_millis(0),
            90,
            None,
            2,
        );

        assert!(
            summary.contains("(last 90 days, 2 gone projects hidden, scanned 1 jsonl"),
            "{summary}"
        );
    }

    #[test]
    fn projects_whose_directory_is_gone_are_flagged_and_dropped() {
        let mut projects = group_launches(vec![
            launch("/live", "agmsg inbox stream", now()),
            launch("/gone", "agmsg inbox stream", now() - Duration::hours(1)),
        ]);

        mark_missing(&mut projects, |cwd| cwd == "/live");
        assert!(!projects[1].exists);

        // While kept, a gone project is labelled rather than looking current.
        let out = format_groups(&projects, now(), &utc_offset(), 200, false);
        assert!(out.contains("/gone  (gone)"), "{out}");

        let hidden = drop_missing(&mut projects);
        assert_eq!(hidden, 1);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].cwd, "/live");
    }

    #[test]
    fn a_checked_summary_reports_the_running_count_even_when_it_is_zero() {
        let projects = group_launches(vec![launch("/a", "agmsg inbox stream", now())]);

        let checked = format_summary(
            &projects,
            1,
            std::time::Duration::from_millis(0),
            90,
            Some(0),
            0,
        );

        assert!(
            checked.contains("in 1 project, 0 running now ("),
            "{checked}"
        );
    }

    #[test]
    fn json_output_is_a_flat_array_carrying_project_description_count_and_last_run() {
        let projects = group_launches(vec![launch(
            "/p",
            "agmsg inbox stream",
            now() - Duration::hours(1),
        )]);

        let json = format_json(&projects);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let entry = &parsed.as_array().unwrap()[0];

        assert_eq!(entry["cwd"], "/p");
        assert_eq!(entry["description"], "agmsg inbox stream");
        assert_eq!(entry["command"], "watch.sh");
        assert_eq!(entry["count"], 1);
        assert_eq!(entry["last_session_id"], "sid");
        assert_eq!(entry["last_started"], "2026-09-19T23:00:00+00:00");
        // Nobody checked, so the field must not read as "not running".
        assert!(entry["alive"].is_null());
    }

    /// The wrapper `ps` reports for a background task, with `<command>` substituted in.
    fn wrapper(command: &str) -> String {
        format!(
            "/bin/zsh -c source /Users/t/.claude/shell-snapshots/snapshot-zsh-178985.sh \
             2>/dev/null || true && setopt NO_EXTENDED_GLOB 2>/dev/null || true && \
             eval '{command}' < /dev/null && pwd -P >| /tmp/claude-501/cwd-dd0b"
        )
    }

    #[test]
    fn ps_rows_split_into_pid_parent_and_the_whole_command() {
        let rows = parse_ps_table(
            "  17025 15891 /bin/zsh -c source a.sh && eval 'x'\n\
             15920 15891 npm exec chrome-devtools-mcp@latest\n\
             garbage line\n",
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].pid, 17025);
        assert_eq!(rows[0].ppid, 15891);
        assert_eq!(rows[0].command, "/bin/zsh -c source a.sh && eval 'x'");
        assert_eq!(rows[1].command, "npm exec chrome-devtools-mcp@latest");
    }

    #[test]
    fn the_launched_command_is_recovered_from_the_shell_wrapper() {
        let recovered = task_command(&wrapper("watch.sh e41433b8 /p claude-code"));
        assert_eq!(
            recovered.as_deref(),
            Some("watch.sh e41433b8 /p claude-code")
        );

        // A payload holding a quote closes on the rightmost marker, not the first one.
        let quoted = task_command(&wrapper(r#"sh -c 'echo hi' < /dev/null; true"#));
        assert_eq!(quoted.as_deref(), Some("sh -c 'echo hi' < /dev/null; true"));

        // The shell embeds a literal single quote as '\'' and it has to come back out.
        let escaped = task_command(&wrapper(r#"echo '\''x'\''"#));
        assert_eq!(escaped.as_deref(), Some("echo 'x'"));
    }

    #[test]
    fn processes_that_are_not_claude_code_background_tasks_are_not_live_tasks() {
        // An MCP server is a direct child of the same claude process, but it is started
        // without the shell wrapper, so it must never be taken for a monitor.
        assert_eq!(task_command("npm exec chrome-devtools-mcp@latest"), None);
        assert_eq!(task_command("/bin/zsh -c ls -la"), None);
        // A wrapper cut short by the ps column width yields nothing rather than a guess.
        assert_eq!(
            task_command("/bin/zsh -c source /Users/t/.claude/shell-snapshots/snapshot-zsh.sh &&"),
            None
        );
    }

    #[test]
    fn live_tasks_keep_the_pid_and_the_session_that_owns_them() {
        let rows = parse_ps_table(&format!(
            "17025 15891 {}\n15920 15891 npm exec chrome-devtools-mcp\n",
            wrapper("watch.sh s1 /p claude-code")
        ));

        let live = live_tasks(&rows);

        assert_eq!(live.len(), 1);
        assert_eq!(live[0].pid, 17025);
        assert_eq!(live[0].session_pid, 15891);
        assert_eq!(live[0].command, "watch.sh s1 /p claude-code");
    }

    #[test]
    fn marking_matches_a_running_task_against_every_command_the_monitor_ran_with() {
        // The monitor was relaunched in a new session, so its newest command carries a
        // different session id than the process that is actually still running.
        let mut old = launch("/p", "agmsg inbox stream", now() - Duration::days(2));
        old.command = "watch.sh old-session /p".into();
        let mut new = launch("/p", "agmsg inbox stream", now() - Duration::hours(1));
        new.command = "watch.sh new-session /p".into();
        let mut projects = group_launches(vec![old, new]);

        let unclaimed = mark_alive(
            &mut projects,
            &[LiveTask {
                pid: 1,
                session_pid: 2,
                command: "watch.sh old-session /p".into(),
            }],
        );

        assert_eq!(projects[0].monitors[0].alive, Some(1));
        assert_eq!(running_count(&projects), 1);
        assert!(unclaimed.is_empty());
    }

    #[test]
    fn a_running_task_no_listed_monitor_claims_is_returned_for_the_trailing_block() {
        let mut projects = group_launches(vec![launch("/p", "agmsg inbox stream", now())]);
        let stray = LiveTask {
            pid: 99,
            session_pid: 2,
            command: "watch.sh from-a-launch-outside-the-window /q".into(),
        };

        let unclaimed = mark_alive(&mut projects, std::slice::from_ref(&stray));

        assert_eq!(unclaimed, vec![stray]);
        // The check ran and found nothing for this monitor, which is not the same as
        // never having looked.
        assert_eq!(projects[0].monitors[0].alive, Some(0));
    }

    #[test]
    fn a_running_monitor_is_marked_without_shifting_the_other_columns() {
        let mut projects = group_launches(vec![launch(
            "/Users/t/proj",
            "agmsg inbox stream",
            now() - Duration::hours(2),
        )]);
        let idle = format_groups(&projects, now(), &utc_offset(), 200, false);

        mark_alive(
            &mut projects,
            &[LiveTask {
                pid: 1,
                session_pid: 2,
                command: "watch.sh".into(),
            }],
        );
        let running = format_groups(&projects, now(), &utc_offset(), 200, false);

        assert_eq!(
            idle.lines().nth(1).unwrap(),
            "    09-19 22:00    2h ago      1  agmsg inbox stream"
        );
        assert_eq!(
            running.lines().nth(1).unwrap(),
            "  ● 09-19 22:00    2h ago      1  agmsg inbox stream"
        );
    }

    #[test]
    fn the_trailing_block_names_each_unlisted_running_monitor_by_pid() {
        let tasks = vec![LiveTask {
            pid: 17157,
            session_pid: 15891,
            command: "watch.sh e41433b8 /p claude-code".into(),
        }];

        let out = format_unclaimed(&tasks, 200, false);
        let lines: Vec<&str> = out.lines().collect();

        assert_eq!(lines[0], "running but not listed above:");
        assert_eq!(lines[1], "    17157 watch.sh e41433b8 /p claude-code");
        assert_eq!(format_unclaimed(&[], 200, false), "");
    }

    #[test]
    fn an_unusable_terminal_width_falls_back_to_the_wide_default() {
        assert_eq!(usable_width(Some(120)), 120);
        assert_eq!(usable_width(Some(0)), FALLBACK_WIDTH);
        assert_eq!(usable_width(Some(20)), FALLBACK_WIDTH);
        assert_eq!(usable_width(None), FALLBACK_WIDTH);
    }
}
