use super::session::{Session, SessionStatus};
use super::storage::Storage;
use chrono::Utc;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::process::Command;

/// Context usage extracted from transcript
#[derive(Debug, Default)]
pub struct ContextUsage {
    pub used_tokens: Option<u64>,
    pub max_tokens: Option<u64>,
    pub model: Option<String>,
}

/// Models whose transcript name implies the 1M context window.
/// Claude Code enables 1M by default for the frontier 4.x and 5 generations.
const ONE_M_CONTEXT_FAMILIES: [&str; 5] = ["opus-4", "sonnet-4", "opus-5", "sonnet-5", "fable-5"];

/// Derive max context window size from model name.
///
/// Model IDs carry an explicit `[1m]` suffix when the 1M window is enabled
/// (e.g. `claude-opus-5[1m]`), but transcripts strip it before writing
/// `message.model`, so fall back to a generation heuristic.
pub fn model_max_tokens(model: &str) -> Option<u64> {
    if model.contains("[1m]") {
        return Some(1_000_000);
    }
    if ONE_M_CONTEXT_FAMILIES.iter().any(|f| model.contains(f)) {
        return Some(1_000_000);
    }
    if model.contains("haiku") || model.contains("opus") || model.contains("sonnet") {
        return Some(200_000);
    }
    None
}

/// Resolve the context window for a session from the two model IDs we can see.
///
/// `hook_model` comes from the hook payload and is authoritative: both Claude
/// Code and Codex send the full ID there, including the `[1m]` suffix that
/// transcripts strip. `transcript_model` is the fallback for sessions that
/// never delivered a model in a hook payload.
pub fn resolve_max_tokens(hook_model: Option<&str>, transcript_model: Option<&str>) -> Option<u64> {
    if hook_model.is_some_and(|m| m.contains("[1m]")) {
        return Some(1_000_000);
    }
    transcript_model.or(hook_model).and_then(model_max_tokens)
}

/// Read the last assistant message from a transcript JSONL to extract usage info.
/// Only reads the last 64KB to avoid loading entire (potentially multi-MB) files.
pub fn read_context_usage(transcript_path: &str) -> ContextUsage {
    use std::io::{Read as _, Seek, SeekFrom};

    let mut usage = ContextUsage::default();

    let mut file = match std::fs::File::open(transcript_path) {
        Ok(f) => f,
        Err(_) => return usage,
    };

    let metadata = match file.metadata() {
        Ok(m) => m,
        Err(_) => return usage,
    };

    let file_len = metadata.len();
    // Read last 64KB (enough to contain the most recent assistant messages with usage)
    let tail_size: u64 = 64 * 1024;
    let start = file_len.saturating_sub(tail_size);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return usage;
    }

    let mut content = Vec::with_capacity(tail_size as usize);
    if file.read_to_end(&mut content).is_err() {
        return usage;
    }

    // Scan from the end to find the last line with "usage"
    for line in content.rsplit(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        // Quick check before parsing JSON
        if !line.windows(7).any(|w| w == b"\"usage\"") {
            continue;
        }
        if let Ok(val) = serde_json::from_slice::<serde_json::Value>(line)
            && let Some(msg) = val.get("message")
            && let Some(u) = msg.get("usage")
        {
            let input = u.get("input_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            let cache_read = u
                .get("cache_read_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let cache_creation = u
                .get("cache_creation_input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = u.get("output_tokens").and_then(|v| v.as_u64()).unwrap_or(0);
            usage.used_tokens = Some(input + cache_read + cache_creation + output);

            if let Some(model) = msg.get("model").and_then(|v| v.as_str()) {
                usage.model = Some(model.to_string());
                usage.max_tokens = model_max_tokens(model);
            }
            return usage;
        }
    }

    usage
}

/// The tool call Antigravity nests instead of sending flat `tool_name` /
/// `tool_input` fields.
#[derive(Debug, Deserialize)]
pub struct ToolCall {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub args: Option<serde_json::Value>,
}

/// One hook payload, in either of the two shapes cckit is sent.
///
/// Claude Code and Codex send snake_case with the event name inside the payload.
/// Antigravity (`agy`) sends camelCase (protojson), nests the tool call, carries the
/// working directory as a list, and leaves the event name out entirely, so it is passed
/// on the command line instead. `normalize` folds the second shape onto the first.
#[derive(Debug, Deserialize)]
pub struct HookInput {
    /// grok sends this key too, so only Antigravity's spelling needs an alias.
    #[serde(alias = "conversationId")]
    pub session_id: String,
    /// Absent in Antigravity payloads; taken from `workspace_paths` instead.
    #[serde(default)]
    pub cwd: String,
    /// Absent in Antigravity payloads; supplied by `cckit session hook --event`.
    /// grok sends this key with Claude's PascalCase value, so it needs no translation.
    #[serde(default)]
    pub hook_event_name: String,
    /// grok's own event name, in its own spelling (`"pre_tool_use"`). Only grok sends
    /// it, which is how a grok session is told apart from a Claude Code one: both
    /// arrive through the hooks in `~/.claude/settings.json`.
    #[serde(default, rename = "hookEventName")]
    pub grok_event_name: Option<String>,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Antigravity's spellings of the three fields above. They cannot be serde aliases:
    /// grok sends both the camelCase and the snake_case key in the same payload, and
    /// serde rejects a field that arrives twice ("duplicate field").
    #[serde(default, rename = "toolName")]
    tool_name_camel: Option<String>,
    #[serde(default, rename = "toolInput")]
    tool_input_camel: Option<serde_json::Value>,
    #[serde(default, rename = "transcriptPath")]
    transcript_path_camel: Option<String>,
    /// Full model ID, e.g. "claude-opus-5[1m]". Always present in Codex hooks
    /// and sent by Claude Code too. Unlike the transcript's `message.model`,
    /// this keeps the "[1m]" suffix, so it is the source of truth for the
    /// context window size.
    #[serde(default, alias = "modelName")]
    pub model: Option<String>,
    /// Antigravity's working directories. Empty in print mode (`agy -p`), which is
    /// why an empty list must not overwrite anything.
    #[serde(default, alias = "workspacePaths")]
    pub workspace_paths: Option<Vec<String>>,
    #[serde(default, alias = "toolCall")]
    pub tool_call: Option<ToolCall>,
}

impl HookInput {
    /// Fold an Antigravity payload onto the fields the event handling reads.
    /// `event` is the name passed on the command line, absent for the other tools.
    pub fn normalize(&mut self, event: Option<&str>) {
        if let Some(event) = event {
            self.hook_event_name = event.to_string();
        }
        if self.cwd.is_empty()
            && let Some(first) = self
                .workspace_paths
                .as_ref()
                .and_then(|paths| paths.first())
                .filter(|p| !p.is_empty())
        {
            self.cwd = first.clone();
        }
        if self.tool_name.is_none() {
            self.tool_name = self.tool_name_camel.take();
        }
        if self.tool_input.is_none() {
            self.tool_input = self.tool_input_camel.take();
        }
        if self.transcript_path.is_none() {
            self.transcript_path = self.transcript_path_camel.take();
        }
        if let Some(call) = self.tool_call.take() {
            if self.tool_name.is_none() {
                self.tool_name = call.name;
            }
            if self.tool_input.is_none() {
                self.tool_input = call.args;
            }
        }
        // grok sends no model id at all. Record the product instead, or the session
        // would show as an unknown agent: it reaches cckit through the same hooks as
        // Claude Code, so nothing else in the payload would name it.
        if self.grok_event_name.is_some() && self.model.is_none() {
            self.model = Some("grok".to_string());
        }
    }
}

/// Read the model out of a grok `summary.json`.
///
/// grok's hook payload carries no model id, but it keeps one per session on disk and
/// rewrites it as the session goes, so this is also how a mid-session model switch is
/// picked up. A missing or broken file is not an error: the session simply keeps the
/// bare product name.
fn grok_model_from_summary(content: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(content).ok()?;
    let model = value.get("current_model_id")?.as_str()?;
    (!model.is_empty()).then(|| model.to_string())
}

/// Where grok keeps the summary for the session whose transcript is `transcript_path`.
///
/// grok points the transcript at
/// `~/.grok/sessions/<url-encoded cwd>/<session id>/updates.jsonl`, and `summary.json`
/// sits beside it, so the session's own path is all that is needed. Deriving it this way
/// avoids reproducing grok's url-encoding of the working directory.
fn grok_summary_path(transcript_path: &str) -> Option<std::path::PathBuf> {
    std::path::Path::new(transcript_path)
        .parent()
        .map(|dir| dir.join("summary.json"))
}

/// Read the model grok recorded next to `transcript_path`.
fn find_grok_model(transcript_path: &str) -> Option<String> {
    let summary = grok_summary_path(transcript_path)?;
    let content = std::fs::read_to_string(summary).ok()?;
    grok_model_from_summary(&content)
}

/// Read one hook payload from stdin and apply it.
///
/// `event` is the name given on the command line. Antigravity leaves the event out of
/// the payload, so its hooks pass it here; Claude Code and Codex pass `None` and the
/// name is read from the payload. Its presence also marks the Antigravity path, which
/// must answer on stdout: a hook that prints nothing is treated as a refusal.
pub fn handle_hook(event: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let mut hook_input: HookInput = match serde_json::from_str(&input) {
        Ok(v) => v,
        Err(e) => {
            log_error("hook", &format!("{} input={}", e, input));
            return Err(e.into());
        }
    };
    hook_input.normalize(event);
    // grok names its model only on disk, so look it up once the session id is known.
    // Re-read on every event: the model can be switched mid-session.
    if hook_input.grok_event_name.is_some()
        && let Some(transcript) = hook_input.transcript_path.clone()
        && let Some(model) = find_grok_model(&transcript)
    {
        hook_input.model = Some(model);
    }

    let storage = Storage::new();
    let mut tty = get_current_tty();
    let mut pid = get_parent_pid();
    if tty == "unknown"
        && let Some(transcript) = hook_input.transcript_path.as_deref()
        && let Some((tui_tty, tui_pid)) = find_codex_tui_tty(transcript, &hook_input.cwd)
    {
        tty = tui_tty;
        pid = Some(tui_pid);
    }
    let key = format!("{}:{}", hook_input.session_id, tty);

    storage.with_lock(|store| apply_event(store, key.clone(), &hook_input, tty.clone(), pid))?;

    // Antigravity reads the hook's stdout as its verdict. An empty object is the
    // "nothing to say" answer: it neither grants nor denies a tool, and it lets a Stop
    // proceed, which is exactly what a monitor should do.
    if event.is_some() {
        println!("{{}}");
    }

    Ok(())
}

/// Apply one hook event to the session store.
///
/// Split out of `handle_hook` so the state transitions can be tested without a
/// terminal, a process tree or a real store on disk.
pub fn apply_event(
    store: &mut super::session::SessionStore,
    key: String,
    hook_input: &HookInput,
    tty: String,
    pid: Option<u32>,
) {
    let event = hook_input.hook_event_name.as_str();
    // Remove old sessions with the same TTY but different session_id, and the `unknown`
    // entry of this same session once a real TTY has been found for it.
    store.sessions.retain(|k, s| {
        k == &key
            || (s.tty != tty && !(s.tty == "unknown" && s.session_id == hook_input.session_id))
    });

    // Update transcript_path on every event if provided
    let transcript = hook_input.transcript_path.clone();

    let now = Utc::now();

    match event {
        "SessionStart" => {
            store.sessions.insert(
                key.clone(),
                Session {
                    session_id: hook_input.session_id.clone(),
                    cwd: hook_input.cwd.clone(),
                    tty: tty.clone(),
                    status: SessionStatus::WaitingInput,
                    created_at: now,
                    updated_at: now,
                    last_tool: None,
                    last_tool_input: None,
                    pid,
                    prompt_count: 0,
                    compact_count: 0,
                    transcript_path: transcript.clone(),
                    tool_started_at: None,
                    last_tool_duration_ms: None,
                    tool_count: 0,
                    context_used_tokens: None,
                    context_max_tokens: None,
                    model: None,
                    subagent_name: None,
                },
            );
        }

        "UserPromptSubmit" => {
            let session = store
                .sessions
                .entry(key.clone())
                .or_insert_with(|| Session {
                    session_id: hook_input.session_id.clone(),
                    cwd: hook_input.cwd.clone(),
                    tty: tty.clone(),
                    status: SessionStatus::Running,
                    created_at: now,
                    updated_at: now,
                    last_tool: None,
                    last_tool_input: None,
                    pid,
                    prompt_count: 0,
                    compact_count: 0,
                    transcript_path: transcript.clone(),
                    tool_started_at: None,
                    last_tool_duration_ms: None,
                    tool_count: 0,
                    context_used_tokens: None,
                    context_max_tokens: None,
                    model: None,
                    subagent_name: None,
                });
            session.status = SessionStatus::Running;
            session.updated_at = now;
            session.cwd = hook_input.cwd.clone();
            session.prompt_count += 1;
            if session.pid.is_none() {
                session.pid = pid;
            }
        }

        "PreToolUse" => {
            if let Some(session) = store.sessions.get_mut(&key) {
                session.status = SessionStatus::AwaitingApproval;
                session.last_tool = hook_input.tool_name.clone();
                session.last_tool_input = extract_tool_summary(hook_input);
                session.updated_at = now;
                session.tool_started_at = Some(now);
                session.tool_count += 1;
                if session.pid.is_none() {
                    session.pid = pid;
                }
            } else {
                store.sessions.insert(
                    key.clone(),
                    Session {
                        session_id: hook_input.session_id.clone(),
                        cwd: hook_input.cwd.clone(),
                        tty: tty.clone(),
                        status: SessionStatus::AwaitingApproval,
                        created_at: now,
                        updated_at: now,
                        last_tool: hook_input.tool_name.clone(),
                        last_tool_input: extract_tool_summary(hook_input),
                        pid,
                        prompt_count: 0,
                        compact_count: 0,
                        transcript_path: transcript.clone(),
                        tool_started_at: Some(now),
                        last_tool_duration_ms: None,
                        tool_count: 1,
                        context_used_tokens: None,
                        context_max_tokens: None,
                        model: None,
                        subagent_name: None,
                    },
                );
            }
        }

        // Antigravity calls the model repeatedly within one turn, so only the
        // invocation that follows a Stop is a prompt the user actually typed.
        // cckit installs no PreToolUse hook there: that event's contract forces a
        // permission decision, and a monitor must not answer it.
        "PreInvocation" => {
            let session = store
                .sessions
                .entry(key.clone())
                .or_insert_with(|| Session {
                    session_id: hook_input.session_id.clone(),
                    cwd: hook_input.cwd.clone(),
                    tty: tty.clone(),
                    status: SessionStatus::WaitingInput,
                    created_at: now,
                    updated_at: now,
                    last_tool: None,
                    last_tool_input: None,
                    pid,
                    prompt_count: 0,
                    compact_count: 0,
                    transcript_path: transcript.clone(),
                    tool_started_at: None,
                    last_tool_duration_ms: None,
                    tool_count: 0,
                    context_used_tokens: None,
                    context_max_tokens: None,
                    model: None,
                    subagent_name: None,
                });
            if session.status == SessionStatus::WaitingInput {
                session.prompt_count += 1;
            }
            session.status = SessionStatus::Running;
            session.updated_at = now;
            if !hook_input.cwd.is_empty() {
                session.cwd = hook_input.cwd.clone();
            }
            if session.pid.is_none() {
                session.pid = pid;
            }
        }

        "PostInvocation" => {
            if let Some(session) = store.sessions.get_mut(&key) {
                session.status = SessionStatus::Running;
                session.updated_at = now;
            }
        }

        "PostToolUse" => {
            if let Some(session) = store.sessions.get_mut(&key) {
                session.status = SessionStatus::Running;
                session.updated_at = now;
                match session.tool_started_at.take() {
                    Some(started) => {
                        session.last_tool_duration_ms =
                            Some(now.signed_duration_since(started).num_milliseconds());
                    }
                    // No PreToolUse ran, so this hook is the only sighting of the
                    // call and has to both name it and count it.
                    None => {
                        if hook_input.tool_name.is_some() {
                            session.last_tool = hook_input.tool_name.clone();
                            session.last_tool_input = extract_tool_summary(hook_input);
                            session.tool_count += 1;
                        }
                    }
                }
            }
        }

        "Stop" | "SubagentStop" => {
            if let Some(session) = store.sessions.get_mut(&key) {
                session.status = SessionStatus::WaitingInput;
                session.updated_at = now;
                // Update context usage from transcript
                if let Some(ref tp) = session.transcript_path {
                    let ctx = read_context_usage(tp);
                    if ctx.used_tokens.is_some() {
                        let hook_model = hook_input.model.clone().or_else(|| session.model.clone());
                        let max = resolve_max_tokens(hook_model.as_deref(), ctx.model.as_deref());
                        session.context_used_tokens = ctx.used_tokens;
                        session.context_max_tokens = max;
                        // Never downgrade a hook-provided ID: it carries "[1m]".
                        if !session.model.as_deref().is_some_and(|m| m.contains("[1m]")) {
                            session.model = ctx.model;
                        }
                    }
                }
            }
        }

        "Notification" => {
            if let Some(session) = store.sessions.get_mut(&key) {
                session.updated_at = now;
            }
        }

        "PreCompact" => {
            if let Some(session) = store.sessions.get_mut(&key) {
                session.compact_count += 1;
                session.updated_at = now;
            }
        }

        "SessionEnd" => {
            store.sessions.remove(&key);
        }

        _ => {
            // Ignore unknown events
        }
    }

    // Update transcript_path if provided (may arrive on any event)
    if let Some(ref tp) = transcript
        && let Some(session) = store.sessions.get_mut(&key)
        && session.transcript_path.is_none()
    {
        session.transcript_path = Some(tp.clone());
    }

    // Set model from hook input whenever provided. This is the authoritative
    // ID (it keeps the "[1m]" suffix), so it overrides any transcript value.
    if let Some(ref model) = hook_input.model
        && let Some(session) = store.sessions.get_mut(&key)
    {
        session.model = Some(model.clone());
    }

    // Extract subagent name if this looks like a subagent and name is not yet set
    if let Some(session) = store.sessions.get_mut(&key)
        && session.subagent_name.is_none()
        && session.is_subagent()
        && let Some(ref tp) = session.transcript_path
    {
        session.subagent_name = extract_subagent_name(tp);
    }

    store.updated_at = Utc::now();
}

pub fn log_error(event: &str, error: &str) {
    let data_dir =
        dirs::data_local_dir().or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
    let log_path = match data_dir {
        Some(d) => d.join("cckit").join("error.log"),
        None => return,
    };

    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&log_path) {
        let timestamp = Utc::now().format("%Y-%m-%d %H:%M:%S");
        let _ = writeln!(file, "[{}] event={} error={}", timestamp, event, error);
    }
}

fn get_current_tty() -> String {
    // Try tty command first
    if let Ok(output) = Command::new("tty").output() {
        let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !tty.is_empty() && tty != "not a tty" {
            return tty;
        }
    }

    // Fallback: get parent process TTY via ps command
    // This works when stdin is piped (e.g., in Claude Code / Codex hooks)
    if let Ok(ppid) = std::env::var("PPID").or_else(|_| {
        // If PPID env var not available, try to get it from ps
        Command::new("ps")
            .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_string())
            .ok_or(std::env::VarError::NotPresent)
    }) && let Ok(output) = Command::new("ps")
        .args(["-o", "tty=", "-p", &ppid])
        .output()
    {
        let tty = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !tty.is_empty() && tty != "??" {
            return format!("/dev/{}", tty);
        }
    }

    "unknown".to_string()
}

/// The terminal of the Codex TUI that owns a session whose hook ran without one.
///
/// Since 0.159 the Codex TUI hands its turns to a shared `codex app-server
/// --managed-daemon`, so hooks are children of a process with no terminal. Nothing in
/// the payload or the daemon names the client, so the TUI is found by working
/// directory: a `codex` process on a terminal whose cwd is the session's. Only a unique
/// match counts; two TUIs in one directory stay `unknown` rather than risk focusing the
/// wrong one. Codex Desktop threads are skipped by their transcript's `originator`.
fn find_codex_tui_tty(transcript: &str, cwd: &str) -> Option<(String, u32)> {
    if !is_codex_tui_transcript(transcript) {
        return None;
    }
    let ps = Command::new("ps")
        .args(["-axo", "pid=,tty=,comm="])
        .output()
        .ok()?;
    let candidates = codex_tui_processes(&String::from_utf8_lossy(&ps.stdout));
    if candidates.is_empty() {
        return None;
    }
    let pids: Vec<String> = candidates.iter().map(|(pid, _)| pid.to_string()).collect();
    let lsof = Command::new("lsof")
        .args(["-a", "-d", "cwd", "-Fn", "-p", &pids.join(",")])
        .output()
        .ok()?;
    let cwds = parse_lsof_cwds(&String::from_utf8_lossy(&lsof.stdout));
    unique_tty_in_cwd(&candidates, &cwds, cwd)
}

fn is_codex_tui_transcript(transcript: &str) -> bool {
    let Ok(file) = std::fs::File::open(transcript) else {
        return false;
    };
    let mut first = String::new();
    if io::BufRead::read_line(&mut io::BufReader::new(file), &mut first).is_err() {
        return false;
    }
    serde_json::from_str::<serde_json::Value>(&first)
        .ok()
        .and_then(|v| {
            v.pointer("/payload/originator")
                .and_then(|o| o.as_str())
                .map(|o| o == "codex-tui")
        })
        .unwrap_or(false)
}

/// `(pid, tty)` of every `codex` program running on a terminal, from `ps -o pid=,tty=,comm=`.
fn codex_tui_processes(ps_output: &str) -> Vec<(u32, String)> {
    ps_output
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let pid = fields.next()?.parse().ok()?;
            let tty = fields.next()?;
            // comm is a path and may contain spaces, so rejoin the rest.
            let comm = fields.collect::<Vec<_>>().join(" ");
            let program = comm.rsplit('/').next().unwrap_or(&comm);
            (program == "codex" && tty != "??" && tty != "-").then(|| (pid, tty.to_string()))
        })
        .collect()
}

/// `(pid, cwd)` pairs from `lsof -d cwd -Fn`, which prints a `p<pid>` line before each `n<path>`.
fn parse_lsof_cwds(output: &str) -> Vec<(u32, String)> {
    let mut pid = None;
    let mut cwds = Vec::new();
    for line in output.lines() {
        if let Some(p) = line.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let Some(path) = line.strip_prefix('n')
            && let Some(p) = pid
        {
            cwds.push((p, path.to_string()));
        }
    }
    cwds
}

fn unique_tty_in_cwd(
    candidates: &[(u32, String)],
    cwds: &[(u32, String)],
    cwd: &str,
) -> Option<(String, u32)> {
    let mut matched = candidates
        .iter()
        .filter(|(pid, _)| cwds.iter().any(|(p, c)| p == pid && c == cwd));
    let (pid, tty) = matched.next()?;
    if matched.next().is_some() {
        return None;
    }
    Some((format!("/dev/{tty}"), *pid))
}

fn get_parent_pid() -> Option<u32> {
    // Try PPID env var first
    if let Ok(ppid) = std::env::var("PPID")
        && let Ok(pid) = ppid.parse::<u32>()
    {
        return Some(pid);
    }

    // Fallback: get PPID via ps command
    if let Ok(output) = Command::new("ps")
        .args(["-o", "ppid=", "-p", &std::process::id().to_string()])
        .output()
        && let Ok(ppid_str) = String::from_utf8(output.stdout)
        && let Ok(pid) = ppid_str.trim().parse::<u32>()
    {
        return Some(pid);
    }

    None
}

fn extract_tool_summary(hook_input: &HookInput) -> Option<String> {
    let tool_input = hook_input.tool_input.as_ref()?;

    match hook_input.tool_name.as_deref() {
        Some("Bash") => tool_input
            .get("command")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60)),
        Some("Read") => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60)),
        Some("Write") => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| format!("-> {}", truncate(s, 57))),
        Some("Edit") => tool_input
            .get("file_path")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60)),
        Some("Glob") => tool_input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60)),
        Some("Grep") => tool_input
            .get("pattern")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60)),
        Some("Task") => tool_input
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| truncate(s, 60)),
        _ => None,
    }
}

/// Extract a subagent name from the transcript's first line.
/// Looks for `agentName` field (set by Claude Code for team/agent sessions).
pub fn extract_subagent_name(transcript_path: &str) -> Option<String> {
    use std::io::BufRead;

    let file = std::fs::File::open(transcript_path).ok()?;
    let reader = std::io::BufReader::new(file);
    let first_line = reader.lines().next()?.ok()?;

    let val: serde_json::Value = serde_json::from_str(&first_line).ok()?;

    // Try agentName field first (team agents)
    if let Some(name) = val.get("agentName").and_then(|v| v.as_str())
        && !name.is_empty()
    {
        return Some(name.to_string());
    }

    // Fallback: extract from first user message content
    if val.get("type")?.as_str()? == "user" {
        let content = val.get("message")?.get("content")?.as_str()?;
        let first = content.lines().next().unwrap_or(content);
        return Some(truncate(first, 24));
    }

    None
}

fn truncate(s: &str, max: usize) -> String {
    // Take first line only
    let first_line = s.lines().next().unwrap_or(s);
    let chars: Vec<char> = first_line.chars().collect();
    if chars.len() > max {
        format!("{}...", chars[..max].iter().collect::<String>())
    } else {
        first_line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_truncate_short_string() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("test", 4), "test");
    }

    #[test]
    fn test_truncate_long_string() {
        assert_eq!(truncate("hello world", 5), "hello...");
        assert_eq!(truncate("abcdefghij", 3), "abc...");
    }

    #[test]
    fn test_truncate_multiline() {
        assert_eq!(truncate("line1\nline2\nline3", 10), "line1");
        assert_eq!(truncate("first\nsecond", 3), "fir...");
    }

    #[test]
    fn test_truncate_empty() {
        assert_eq!(truncate("", 10), "");
    }

    #[test]
    fn test_extract_tool_summary_bash() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("Bash".to_string()),
            tool_input: Some(json!({"command": "ls -la"})),
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(
            extract_tool_summary(&hook_input),
            Some("ls -la".to_string())
        );
    }

    #[test]
    fn test_extract_tool_summary_read() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("Read".to_string()),
            tool_input: Some(json!({"file_path": "/path/to/file.rs"})),
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(
            extract_tool_summary(&hook_input),
            Some("/path/to/file.rs".to_string())
        );
    }

    #[test]
    fn test_extract_tool_summary_write() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("Write".to_string()),
            tool_input: Some(json!({"file_path": "/path/to/file.rs"})),
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(
            extract_tool_summary(&hook_input),
            Some("-> /path/to/file.rs".to_string())
        );
    }

    #[test]
    fn test_extract_tool_summary_glob() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("Glob".to_string()),
            tool_input: Some(json!({"pattern": "**/*.rs"})),
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(
            extract_tool_summary(&hook_input),
            Some("**/*.rs".to_string())
        );
    }

    #[test]
    fn test_extract_tool_summary_task() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("Task".to_string()),
            tool_input: Some(json!({"description": "Explore codebase"})),
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(
            extract_tool_summary(&hook_input),
            Some("Explore codebase".to_string())
        );
    }

    #[test]
    fn test_extract_tool_summary_unknown_tool() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("UnknownTool".to_string()),
            tool_input: Some(json!({"foo": "bar"})),
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(extract_tool_summary(&hook_input), None);
    }

    #[test]
    fn test_extract_tool_summary_no_tool_input() {
        let hook_input = HookInput {
            session_id: "test".to_string(),
            cwd: "/test".to_string(),
            hook_event_name: "PreToolUse".to_string(),
            grok_event_name: None,
            tool_name_camel: None,
            tool_input_camel: None,
            transcript_path_camel: None,
            tool_name: Some("Bash".to_string()),
            tool_input: None,
            transcript_path: None,
            model: None,
            workspace_paths: None,
            tool_call: None,
        };
        assert_eq!(extract_tool_summary(&hook_input), None);
    }

    #[test]
    fn test_hook_input_deserialize() {
        let json = r#"{
            "session_id": "abc123",
            "cwd": "/home/user/project",
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": {"command": "echo hello"}
        }"#;
        let hook_input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(hook_input.session_id, "abc123");
        assert_eq!(hook_input.cwd, "/home/user/project");
        assert_eq!(hook_input.hook_event_name, "PreToolUse");
        assert_eq!(hook_input.tool_name, Some("Bash".to_string()));
    }

    #[test]
    fn test_hook_input_deserialize_minimal() {
        let json = r#"{
            "session_id": "abc123",
            "cwd": "/home/user",
            "hook_event_name": "SessionStart"
        }"#;
        let hook_input: HookInput = serde_json::from_str(json).unwrap();
        assert_eq!(hook_input.session_id, "abc123");
        assert_eq!(hook_input.tool_name, None);
        assert_eq!(hook_input.tool_input, None);
    }

    #[test]
    fn test_model_max_tokens_explicit_1m_suffix() {
        // An explicit "[1m]" suffix wins over any family heuristic.
        assert_eq!(model_max_tokens("claude-opus-5[1m]"), Some(1_000_000));
        assert_eq!(model_max_tokens("claude-opus-4-8[1m]"), Some(1_000_000));
        assert_eq!(model_max_tokens("claude-haiku-4-5[1m]"), Some(1_000_000));
    }

    #[test]
    fn test_model_max_tokens_claude_5_family() {
        // Transcripts strip the "[1m]" suffix, so the 5 generation falls back
        // to the family heuristic and must not land on the 200k default.
        assert_eq!(model_max_tokens("claude-opus-5"), Some(1_000_000));
        assert_eq!(model_max_tokens("claude-sonnet-5"), Some(1_000_000));
        assert_eq!(model_max_tokens("claude-fable-5"), Some(1_000_000));
    }

    #[test]
    fn test_model_max_tokens_claude_4_family() {
        assert_eq!(model_max_tokens("claude-opus-4-7"), Some(1_000_000));
        assert_eq!(model_max_tokens("claude-sonnet-4-6"), Some(1_000_000));
        assert_eq!(
            model_max_tokens("claude-opus-4-5-20251101"),
            Some(1_000_000)
        );
    }

    #[test]
    fn test_model_max_tokens_200k_models() {
        assert_eq!(model_max_tokens("claude-haiku-4-5-20251001"), Some(200_000));
        assert_eq!(model_max_tokens("haiku"), Some(200_000));
        assert_eq!(model_max_tokens("opus"), Some(200_000));
        assert_eq!(model_max_tokens("sonnet"), Some(200_000));
    }

    #[test]
    fn test_model_max_tokens_unknown() {
        assert_eq!(model_max_tokens("<synthetic>"), None);
        assert_eq!(model_max_tokens("gpt-5-codex"), None);
    }

    #[test]
    fn test_resolve_max_tokens_hook_suffix_wins() {
        // Hook input keeps "[1m]"; the transcript strips it. Hook wins.
        assert_eq!(
            resolve_max_tokens(Some("claude-opus-5[1m]"), Some("claude-opus-5")),
            Some(1_000_000)
        );
        // Even when the family heuristic would say 200k.
        assert_eq!(
            resolve_max_tokens(
                Some("claude-haiku-4-5[1m]"),
                Some("claude-haiku-4-5-20251001")
            ),
            Some(1_000_000)
        );
    }

    #[test]
    fn test_resolve_max_tokens_falls_back_to_transcript() {
        assert_eq!(
            resolve_max_tokens(None, Some("claude-opus-5")),
            Some(1_000_000)
        );
        assert_eq!(
            resolve_max_tokens(None, Some("claude-haiku-4-5-20251001")),
            Some(200_000)
        );
        // A hook model without the suffix is no better than the transcript one.
        assert_eq!(
            resolve_max_tokens(Some("claude-opus-5"), Some("claude-opus-5")),
            Some(1_000_000)
        );
    }

    #[test]
    fn test_resolve_max_tokens_uses_hook_when_no_transcript() {
        assert_eq!(
            resolve_max_tokens(Some("claude-haiku-4-5-20251001"), None),
            Some(200_000)
        );
        assert_eq!(resolve_max_tokens(Some("gpt-5.6-sol"), None), None);
        assert_eq!(resolve_max_tokens(None, None), None);
    }
}

#[cfg(test)]
mod agy_tests {
    use super::*;

    /// A real `PreInvocation` payload, captured from agy 1.2.7.
    const PRE_INVOCATION: &str = r#"{
      "artifactDirectoryPath": "/Users/t/.gemini/antigravity-cli/brain/9a0dc49d",
      "conversationId": "9a0dc49d-2c3b-4b3c-8776-1bb1b4dee7f8",
      "initialNumSteps": 1,
      "invocationNum": 3,
      "modelName": "gemini-3.8-flash-high",
      "transcriptPath": "/Users/t/.gemini/antigravity-cli/brain/9a0dc49d/logs/transcript_full.jsonl",
      "workspacePaths": ["/tmp/agyprobe"]
    }"#;

    /// A real `PreToolUse` payload, captured from agy 1.2.7. cckit does not install a
    /// PreToolUse hook, but PostToolUse carries the same nested `toolCall`.
    const TOOL_USE: &str = r#"{
      "conversationId": "9a0dc49d-2c3b-4b3c-8776-1bb1b4dee7f8",
      "modelName": "gemini-3.8-flash-high",
      "stepIdx": 6,
      "toolCall": {
        "args": {"AbsolutePath": "/tmp/agyprobe/hello.txt", "toolSummary": "Check file"},
        "name": "view_file"
      },
      "transcriptPath": "/Users/t/.gemini/antigravity-cli/brain/9a0dc49d/logs/transcript_full.jsonl",
      "workspacePaths": ["/tmp/agyprobe"]
    }"#;

    /// Print mode (`agy -p`) sends the same payload with no workspace at all.
    const NO_WORKSPACE: &str = r#"{
      "conversationId": "10001348-f8db-47ea-b214-5bba48b31ca7",
      "modelName": "gemini-3.8-flash-high",
      "workspacePaths": []
    }"#;

    fn parse(json: &str, event: Option<&str>) -> HookInput {
        let mut input: HookInput = serde_json::from_str(json).expect("payload parses");
        input.normalize(event);
        input
    }

    #[test]
    fn the_conversation_id_is_read_as_the_session_id() {
        let input = parse(PRE_INVOCATION, Some("PreInvocation"));
        assert_eq!(input.session_id, "9a0dc49d-2c3b-4b3c-8776-1bb1b4dee7f8");
    }

    #[test]
    fn the_event_name_comes_from_the_command_line() {
        let input = parse(PRE_INVOCATION, Some("PreInvocation"));
        assert_eq!(input.hook_event_name, "PreInvocation");
    }

    #[test]
    fn the_first_workspace_path_becomes_the_cwd() {
        let input = parse(PRE_INVOCATION, Some("PreInvocation"));
        assert_eq!(input.cwd, "/tmp/agyprobe");
    }

    #[test]
    fn an_empty_workspace_list_leaves_the_cwd_empty_rather_than_guessing() {
        let input = parse(NO_WORKSPACE, Some("Stop"));
        assert_eq!(input.cwd, "");
    }

    #[test]
    fn the_model_name_is_read_as_the_model() {
        let input = parse(PRE_INVOCATION, Some("PreInvocation"));
        assert_eq!(input.model.as_deref(), Some("gemini-3.8-flash-high"));
    }

    #[test]
    fn the_transcript_path_is_read_from_its_camel_case_name() {
        let input = parse(PRE_INVOCATION, Some("PreInvocation"));
        assert!(
            input
                .transcript_path
                .as_deref()
                .is_some_and(|p| p.ends_with("transcript_full.jsonl")),
            "got: {:?}",
            input.transcript_path
        );
    }

    #[test]
    fn the_nested_tool_call_is_flattened_onto_tool_name_and_input() {
        let input = parse(TOOL_USE, Some("PostToolUse"));
        assert_eq!(input.tool_name.as_deref(), Some("view_file"));
        assert_eq!(
            input
                .tool_input
                .as_ref()
                .and_then(|v| v["AbsolutePath"].as_str()),
            Some("/tmp/agyprobe/hello.txt")
        );
    }

    #[test]
    fn a_claude_code_payload_is_left_alone() {
        let json = r#"{
          "session_id": "abc",
          "cwd": "/home/me/proj",
          "hook_event_name": "PreToolUse",
          "tool_name": "Bash",
          "model": "claude-opus-5[1m]"
        }"#;
        // No --event is passed for Claude Code, so the payload's own name must stand.
        let input = parse(json, None);
        assert_eq!(input.hook_event_name, "PreToolUse");
        assert_eq!(input.cwd, "/home/me/proj");
        assert_eq!(input.tool_name.as_deref(), Some("Bash"));
        assert_eq!(input.model.as_deref(), Some("claude-opus-5[1m]"));
    }
}

#[cfg(test)]
mod agy_event_tests {
    use super::*;
    use crate::monitor::session::{SessionStatus, SessionStore};

    const TTY: &str = "/dev/ttys001";
    const CONV: &str = "9a0dc49d-2c3b-4b3c-8776-1bb1b4dee7f8";

    fn key() -> String {
        format!("{CONV}:{TTY}")
    }

    fn input(event: &str, json: &str) -> HookInput {
        let mut input: HookInput = serde_json::from_str(json).expect("payload parses");
        input.normalize(Some(event));
        input
    }

    fn agy(event: &str) -> HookInput {
        input(
            event,
            &format!(
                r#"{{"conversationId": "{CONV}", "modelName": "gemini-3.8-flash-high",
                    "workspacePaths": ["/tmp/proj"],
                    "transcriptPath": "/tmp/brain/transcript_full.jsonl"}}"#
            ),
        )
    }

    fn apply(store: &mut SessionStore, input: &HookInput) {
        apply_event(store, key(), input, TTY.to_string(), Some(42));
    }

    #[test]
    fn pre_invocation_starts_a_session_that_was_not_there() {
        let mut store = SessionStore::default();
        apply(&mut store, &agy("PreInvocation"));

        let session = store.sessions.get(&key()).expect("session created");
        assert_eq!(session.status, SessionStatus::Running);
        assert_eq!(session.cwd, "/tmp/proj");
        assert_eq!(session.model.as_deref(), Some("gemini-3.8-flash-high"));
        assert_eq!(session.prompt_count, 1);
    }

    #[test]
    fn a_pre_invocation_after_a_stop_counts_as_a_new_prompt() {
        let mut store = SessionStore::default();
        apply(&mut store, &agy("PreInvocation"));
        apply(&mut store, &agy("Stop"));
        apply(&mut store, &agy("PreInvocation"));

        assert_eq!(store.sessions[&key()].prompt_count, 2);
    }

    #[test]
    fn a_second_invocation_within_one_turn_is_not_a_new_prompt() {
        // agy calls the model repeatedly inside a single turn, so only the invocation
        // that follows a Stop is one the user actually typed.
        let mut store = SessionStore::default();
        apply(&mut store, &agy("PreInvocation"));
        apply(&mut store, &agy("PostInvocation"));
        apply(&mut store, &agy("PreInvocation"));

        assert_eq!(store.sessions[&key()].prompt_count, 1);
        assert_eq!(store.sessions[&key()].status, SessionStatus::Running);
    }

    #[test]
    fn stop_puts_the_session_back_to_waiting_for_input() {
        let mut store = SessionStore::default();
        apply(&mut store, &agy("PreInvocation"));
        apply(&mut store, &agy("Stop"));

        assert_eq!(store.sessions[&key()].status, SessionStatus::WaitingInput);
    }

    #[test]
    fn post_tool_use_records_the_tool_when_no_pre_tool_hook_ran() {
        let mut store = SessionStore::default();
        apply(&mut store, &agy("PreInvocation"));
        let tool = input(
            "PostToolUse",
            &format!(
                r#"{{"conversationId": "{CONV}", "workspacePaths": ["/tmp/proj"],
                    "toolCall": {{"name": "view_file",
                                  "args": {{"AbsolutePath": "/tmp/proj/hello.txt"}}}}}}"#
            ),
        );
        apply(&mut store, &tool);

        let session = &store.sessions[&key()];
        assert_eq!(session.last_tool.as_deref(), Some("view_file"));
        assert_eq!(session.tool_count, 1);
        assert_eq!(session.status, SessionStatus::Running);
    }

    #[test]
    fn a_claude_code_tool_is_not_counted_twice_across_its_two_hooks() {
        // Claude Code fires PreToolUse and PostToolUse for one call; only the pre hook
        // may count it, or every tool would be counted twice.
        let mut store = SessionStore::default();
        let start = input(
            "SessionStart",
            r#"{"session_id": "abc", "cwd": "/tmp/proj", "hook_event_name": "SessionStart"}"#,
        );
        apply_event(&mut store, "abc:tty".into(), &start, "tty".into(), None);

        for event in ["PreToolUse", "PostToolUse"] {
            let hook = input(
                event,
                &format!(
                    r#"{{"session_id": "abc", "cwd": "/tmp/proj",
                        "hook_event_name": "{event}", "tool_name": "Bash"}}"#
                ),
            );
            apply_event(&mut store, "abc:tty".into(), &hook, "tty".into(), None);
        }

        assert_eq!(store.sessions["abc:tty"].tool_count, 1);
    }
}

#[cfg(test)]
mod grok_tests {
    use super::*;

    /// A grok payload, in the shape grok 1.0.40 actually sends: every field appears
    /// under BOTH its camelCase and its snake_case key. Reading either one as a serde
    /// alias would make the whole payload fail with "duplicate field".
    const GROK_TOOL: &str = r#"{
      "hookEventName": "pre_tool_use",
      "hook_event_name": "PreToolUse",
      "sessionId": "abc-123",
      "session_id": "abc-123",
      "cwd": "/Users/you/project",
      "workspaceRoot": "/Users/you/project",
      "permissionMode": "default",
      "permission_mode": "default",
      "toolName": "run_terminal_command",
      "tool_name": "run_terminal_command",
      "toolInput": { "command": "npm test" },
      "tool_input": { "command": "npm test" },
      "transcriptPath": "/Users/t/.grok/sessions/%2Fp/abc-123/updates.jsonl",
      "transcript_path": "/Users/t/.grok/sessions/%2Fp/abc-123/updates.jsonl",
      "timestamp": "2026-04-14T12:00:00Z"
    }"#;

    fn parse(json: &str) -> HookInput {
        let mut input: HookInput = serde_json::from_str(json).expect("payload parses");
        // grok arrives through the Claude Code hooks, so no --event is passed.
        input.normalize(None);
        input
    }

    #[test]
    fn a_payload_carrying_both_spellings_of_every_field_still_parses() {
        // The duplicate-field trap: grok sends camelCase AND snake_case together.
        let input = parse(GROK_TOOL);
        assert_eq!(input.session_id, "abc-123");
        assert!(
            input
                .transcript_path
                .as_deref()
                .is_some_and(|p| p.ends_with("abc-123/updates.jsonl"))
        );
    }

    #[test]
    fn the_summary_sits_beside_the_transcript() {
        let summary =
            grok_summary_path("/Users/t/.grok/sessions/%2Fp/abc-123/updates.jsonl").unwrap();
        assert!(
            summary.ends_with("abc-123/summary.json"),
            "got: {summary:?}"
        );
    }

    #[test]
    fn the_event_name_comes_from_the_claude_compatible_key() {
        // grok sends both spellings; the snake_case key carries the PascalCase value
        // cckit already dispatches on.
        assert_eq!(parse(GROK_TOOL).hook_event_name, "PreToolUse");
    }

    #[test]
    fn the_camel_case_tool_fields_are_read() {
        let input = parse(GROK_TOOL);
        assert_eq!(input.tool_name.as_deref(), Some("run_terminal_command"));
        assert_eq!(
            input
                .tool_input
                .as_ref()
                .and_then(|v| v["command"].as_str()),
            Some("npm test")
        );
    }

    #[test]
    fn a_payload_without_a_model_is_marked_as_grok() {
        // grok sends no model id. Without this the session would show as an unknown
        // agent, since it arrives through the same hooks as Claude Code.
        assert_eq!(parse(GROK_TOOL).model.as_deref(), Some("grok"));
    }

    #[test]
    fn a_claude_code_payload_is_not_mistaken_for_grok() {
        let json = r#"{
          "session_id": "abc",
          "cwd": "/tmp",
          "hook_event_name": "PreToolUse",
          "tool_name": "Bash",
          "model": "claude-opus-5[1m]"
        }"#;
        let input = parse(json);
        assert_eq!(input.model.as_deref(), Some("claude-opus-5[1m]"));
        assert_eq!(input.tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn a_grok_payload_that_does_carry_a_model_keeps_it() {
        let json = r#"{
          "hookEventName": "stop",
          "hook_event_name": "Stop",
          "sessionId": "abc-123",
          "session_id": "abc-123",
          "cwd": "/tmp",
          "model": "grok-4.6"
        }"#;
        assert_eq!(parse(json).model.as_deref(), Some("grok-4.6"));
    }
}

#[cfg(test)]
mod grok_model_tests {
    use super::*;

    /// A real summary.json, trimmed to the fields that matter here.
    const SUMMARY: &str = r#"{
      "info": { "id": "01a0d3d5", "cwd": "/Users/t/proj" },
      "created_at": "2026-09-24T14:32:37.599232Z",
      "current_model_id": "grok-4.7",
      "agent_name": "grok-build-plan",
      "reasoning_effort": "high"
    }"#;

    #[test]
    fn the_current_model_is_read_from_the_session_summary() {
        assert_eq!(
            grok_model_from_summary(SUMMARY).as_deref(),
            Some("grok-4.7")
        );
    }

    #[test]
    fn a_summary_without_a_model_yields_nothing() {
        let json = r#"{"info": {"id": "abc"}, "num_messages": 3}"#;
        assert_eq!(grok_model_from_summary(json), None);
    }

    #[test]
    fn a_broken_summary_is_ignored_rather_than_failing_the_hook() {
        assert_eq!(grok_model_from_summary("{ not json"), None);
        assert_eq!(grok_model_from_summary(""), None);
    }

    #[test]
    fn an_empty_model_id_is_treated_as_absent() {
        let json = r#"{"current_model_id": ""}"#;
        assert_eq!(grok_model_from_summary(json), None);
    }
}

#[cfg(test)]
mod codex_tui_tests {
    use super::*;
    use crate::monitor::session::SessionStore;

    const PS: &str = "\
69063 ??       /Users/me/.codex/packages/app-server-daemon/bin/codex
73342 ttys102  codex
92203 ttys054  /opt/homebrew/bin/codex
88001 ttys010  /Applications/Codex Helper.app/Contents/MacOS/codex-code-mode-host
88002 ttys011  zsh
";

    #[test]
    fn test_codex_tui_processes_keeps_codex_on_a_terminal() {
        assert_eq!(
            codex_tui_processes(PS),
            vec![(73342, "ttys102".into()), (92203, "ttys054".into())]
        );
    }

    #[test]
    fn test_parse_lsof_cwds() {
        let out = "p73342\nfcwd\nn/work/a\np92203\nfcwd\nn/work/b\n";
        assert_eq!(
            parse_lsof_cwds(out),
            vec![(73342, "/work/a".into()), (92203, "/work/b".into())]
        );
    }

    #[test]
    fn test_unique_tty_in_cwd() {
        let candidates = codex_tui_processes(PS);
        let cwds = vec![(73342, "/work/a".into()), (92203, "/work/b".into())];
        assert_eq!(
            unique_tty_in_cwd(&candidates, &cwds, "/work/a"),
            Some(("/dev/ttys102".into(), 73342))
        );
        assert_eq!(unique_tty_in_cwd(&candidates, &cwds, "/work/c"), None);
    }

    #[test]
    fn test_unique_tty_in_cwd_refuses_two_tuis_in_one_directory() {
        let candidates = codex_tui_processes(PS);
        let cwds = vec![(73342, "/work/a".into()), (92203, "/work/a".into())];
        assert_eq!(unique_tty_in_cwd(&candidates, &cwds, "/work/a"), None);
    }

    #[test]
    fn test_is_codex_tui_transcript() {
        let dir = std::env::temp_dir().join(format!("cckit-codex-tui-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let tui = dir.join("tui.jsonl");
        let desktop = dir.join("desktop.jsonl");
        std::fs::write(
            &tui,
            "{\"type\":\"session_meta\",\"payload\":{\"originator\":\"codex-tui\"}}\n{}\n",
        )
        .unwrap();
        std::fs::write(
            &desktop,
            "{\"type\":\"session_meta\",\"payload\":{\"originator\":\"Codex Desktop\"}}\n",
        )
        .unwrap();
        assert!(is_codex_tui_transcript(tui.to_str().unwrap()));
        assert!(!is_codex_tui_transcript(desktop.to_str().unwrap()));
        assert!(!is_codex_tui_transcript(
            dir.join("missing").to_str().unwrap()
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_real_tty_replaces_unknown_entry_of_same_session() {
        let payload = |sid: &str| -> HookInput {
            let mut input: HookInput = serde_json::from_str(&format!(
                r#"{{"session_id":"{sid}","cwd":"/work/a","hook_event_name":"UserPromptSubmit"}}"#
            ))
            .unwrap();
            input.normalize(None);
            input
        };
        let mut store = SessionStore::default();
        apply_event(
            &mut store,
            "s1:unknown".into(),
            &payload("s1"),
            "unknown".into(),
            None,
        );
        apply_event(
            &mut store,
            "s2:unknown".into(),
            &payload("s2"),
            "unknown".into(),
            None,
        );
        apply_event(
            &mut store,
            "s2:/dev/ttys102".into(),
            &payload("s2"),
            "/dev/ttys102".into(),
            None,
        );
        assert!(store.sessions.contains_key("s2:/dev/ttys102"));
        assert!(!store.sessions.contains_key("s2:unknown"));
    }
}
