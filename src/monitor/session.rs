use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SessionStatus {
    Running,
    AwaitingApproval,
    WaitingInput,
    Stopped,
}

impl std::fmt::Display for SessionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SessionStatus::Running => write!(f, "running"),
            SessionStatus::AwaitingApproval => write!(f, "tooling"),
            SessionStatus::WaitingInput => write!(f, "waiting"),
            SessionStatus::Stopped => write!(f, "stopped"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub session_id: String,
    pub cwd: String,
    pub tty: String,
    pub status: SessionStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool_input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(default)]
    pub prompt_count: u32,
    #[serde(default)]
    pub compact_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// Timestamp when the current tool started (PreToolUse)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_started_at: Option<DateTime<Utc>>,
    /// Duration of the last completed tool execution in milliseconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_tool_duration_ms: Option<i64>,
    /// Total number of tool invocations in this session
    #[serde(default)]
    pub tool_count: u32,
}

impl Session {
    /// Generate the storage key for this session
    pub fn key(&self) -> String {
        format!("{}:{}", self.session_id, self.tty)
    }

    #[allow(dead_code)]
    pub fn project_name(&self) -> &str {
        self.cwd.rsplit('/').next().unwrap_or(&self.cwd)
    }

    pub fn short_cwd(&self) -> String {
        if let Some(home) = dirs::home_dir() {
            let home_str = home.to_string_lossy();
            if self.cwd.starts_with(home_str.as_ref()) {
                return self.cwd.replacen(home_str.as_ref(), "~", 1);
            }
        }
        self.cwd.clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStore {
    pub sessions: HashMap<String, Session>,
    pub updated_at: DateTime<Utc>,
}

/// TUI instance state for menubar integration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiState {
    pub tty: String,
    pub pid: u32,
    pub started_at: DateTime<Utc>,
}

impl Default for SessionStore {
    fn default() -> Self {
        Self {
            sessions: HashMap::new(),
            updated_at: Utc::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_session(cwd: &str) -> Session {
        Session {
            session_id: "test-id".to_string(),
            cwd: cwd.to_string(),
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
    fn test_session_status_display() {
        assert_eq!(format!("{}", SessionStatus::Running), "running");
        assert_eq!(format!("{}", SessionStatus::AwaitingApproval), "tooling");
        assert_eq!(format!("{}", SessionStatus::WaitingInput), "waiting");
        assert_eq!(format!("{}", SessionStatus::Stopped), "stopped");
    }

    #[test]
    fn test_session_project_name() {
        let session = create_test_session("/home/user/projects/my-project");
        assert_eq!(session.project_name(), "my-project");

        let session = create_test_session("/Users/foo/bar/baz");
        assert_eq!(session.project_name(), "baz");

        let session = create_test_session("single");
        assert_eq!(session.project_name(), "single");
    }

    #[test]
    fn test_session_serialize_deserialize() {
        let session = create_test_session("/home/user/project");
        let json = serde_json::to_string(&session).unwrap();
        let deserialized: Session = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.session_id, session.session_id);
        assert_eq!(deserialized.cwd, session.cwd);
        assert_eq!(deserialized.status, SessionStatus::Running);
    }

    #[test]
    fn test_session_status_serde() {
        let json = r#""running""#;
        let status: SessionStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, SessionStatus::Running);

        let json = r#""awaiting_approval""#;
        let status: SessionStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, SessionStatus::AwaitingApproval);

        let json = r#""waiting_input""#;
        let status: SessionStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, SessionStatus::WaitingInput);

        let json = r#""stopped""#;
        let status: SessionStatus = serde_json::from_str(json).unwrap();
        assert_eq!(status, SessionStatus::Stopped);
    }

    #[test]
    fn test_session_store_default() {
        let store = SessionStore::default();
        assert!(store.sessions.is_empty());
    }
}
