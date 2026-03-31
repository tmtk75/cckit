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

/// Derive max context window size from model name.
/// Claude Code uses 1M context for Opus 4.x and Sonnet 4.x by default.
pub fn model_max_tokens(model: &str) -> Option<u64> {
    if model.contains("opus-4") || model.contains("sonnet-4") {
        Some(1_000_000)
    } else if model.contains("haiku") || model.contains("opus") || model.contains("sonnet") {
        Some(200_000)
    } else {
        None
    }
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

#[derive(Debug, Deserialize)]
pub struct HookInput {
    pub session_id: String,
    pub cwd: String,
    pub hook_event_name: String,
    #[serde(default)]
    pub tool_name: Option<String>,
    #[serde(default)]
    pub tool_input: Option<serde_json::Value>,
    #[serde(default)]
    pub transcript_path: Option<String>,
    /// Model name (always present in Codex hooks, absent in Claude Code hooks)
    #[serde(default)]
    pub model: Option<String>,
}

pub fn handle_hook() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    let hook_input: HookInput = serde_json::from_str(&input)?;
    let event = hook_input.hook_event_name.as_str();

    let storage = Storage::new();
    let tty = get_current_tty();
    let pid = get_parent_pid();
    let key = format!("{}:{}", hook_input.session_id, tty);

    storage.with_lock(|store| {
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
                    session.last_tool_input = extract_tool_summary(&hook_input);
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
                            last_tool_input: extract_tool_summary(&hook_input),
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

            "PostToolUse" => {
                if let Some(session) = store.sessions.get_mut(&key) {
                    session.status = SessionStatus::Running;
                    session.updated_at = now;
                    if let Some(started) = session.tool_started_at.take() {
                        session.last_tool_duration_ms =
                            Some(now.signed_duration_since(started).num_milliseconds());
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
                            session.context_used_tokens = ctx.used_tokens;
                            session.context_max_tokens = ctx.max_tokens;
                            session.model = ctx.model;
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

        // Set model from hook input if provided (Codex always sends this)
        if let Some(ref model) = hook_input.model
            && let Some(session) = store.sessions.get_mut(&key)
            && session.model.is_none()
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
    })?;

    Ok(())
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
}
