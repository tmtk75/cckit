use super::session::Session;
use chrono::{DateTime, Utc};

pub fn format_relative_time(dt: DateTime<Utc>) -> String {
    let duration = Utc::now().signed_duration_since(dt);

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

pub fn format_elapsed_short(dt: DateTime<Utc>) -> String {
    let secs = Utc::now().signed_duration_since(dt).num_seconds().max(0);

    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

pub fn session_count_parts(session: &Session) -> Vec<String> {
    let mut parts = Vec::new();

    if session.prompt_count > 0 {
        parts.push(format!("{}p", session.prompt_count));
    }
    if session.tool_count > 0 {
        parts.push(format!("{}t", session.tool_count));
    }
    if session.compact_count > 0 {
        parts.push(format!("{}c", session.compact_count));
    }

    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::monitor::session::{Session, SessionStatus};
    use chrono::Duration;

    fn test_session() -> Session {
        Session {
            session_id: "test".to_string(),
            cwd: "/tmp/test".to_string(),
            tty: "/dev/ttys001".to_string(),
            status: SessionStatus::Running,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_tool: None,
            last_tool_input: None,
            pid: None,
            prompt_count: 2,
            compact_count: 1,
            transcript_path: None,
            tool_started_at: None,
            last_tool_duration_ms: None,
            tool_count: 3,
            context_used_tokens: None,
            context_max_tokens: None,
            model: None,
            subagent_name: None,
        }
    }

    #[test]
    fn formats_relative_time_in_seconds() {
        let dt = Utc::now() - Duration::seconds(42);
        assert_eq!(format_relative_time(dt), "42s ago");
    }

    #[test]
    fn formats_elapsed_in_minutes() {
        let dt = Utc::now() - Duration::minutes(3);
        assert_eq!(format_elapsed_short(dt), "3m");
    }

    #[test]
    fn builds_session_count_parts() {
        let session = test_session();
        assert_eq!(session_count_parts(&session), vec!["2p", "3t", "1c"]);
    }
}
