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
    #[serde(alias = "conversationId")]
    pub session_id: String,
    /// Absent in Antigravity payloads; taken from `workspace_paths` instead.
    #[serde(default)]
    pub cwd: String,
    /// Absent in Antigravity payloads; supplied by `cckit session hook --event`.
    #[serde(default)]
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default, alias = "transcriptPath")]
    pub transcript_path: Option<String>,
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
        if let Some(call) = self.tool_call.take() {
            if self.tool_name.is_none() {
                self.tool_name = call.name;
            }
            if self.tool_input.is_none() {
                self.tool_input = call.args;
            }
        }
    }
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

    let storage = Storage::new();
    let tty = get_current_tty();
    let pid = get_parent_pid();
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
    // Remove old sessions with the same TTY but different session_id
    store.sessions.retain(|k, s| s.tty != tty || k == &key);

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
