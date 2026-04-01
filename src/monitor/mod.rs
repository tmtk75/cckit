pub mod display;
pub mod focus;
pub mod hook;
#[cfg(target_os = "macos")]
pub mod menubar;
#[cfg(target_os = "macos")]
pub mod notification;
pub mod paths;
pub mod session;
pub mod setup;
pub mod storage;
pub mod theme;
pub mod tui;
#[cfg(target_os = "macos")]
pub mod window;

use colored::Colorize;
use session::{Session, SessionStatus};
use storage::Storage;

pub fn print_sessions_list() {
    let storage = Storage::new();
    let store = storage.load();

    if store.sessions.is_empty() {
        println!("No active sessions");
        return;
    }

    let mut sessions: Vec<&Session> = store.sessions.values().collect();
    sessions.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let active_count = sessions
        .iter()
        .filter(|s| s.status == SessionStatus::Running)
        .count();

    println!(
        "{} sessions ({} active)\n",
        sessions.len().to_string().cyan(),
        active_count.to_string().green()
    );

    for session in sessions {
        let status = match session.status {
            SessionStatus::Running => "●".green(),
            SessionStatus::AwaitingApproval => "◐".truecolor(255, 165, 0),
            SessionStatus::WaitingInput => "○".yellow(),
            SessionStatus::Stopped => "×".dimmed(),
        };

        let tool = session.last_tool.as_deref().unwrap_or("-");
        let updated = display::format_relative_time(session.updated_at);

        println!(
            "{} {} {} [{}] {}",
            status,
            session.project_name().bold(),
            session.tty.dimmed(),
            tool.cyan(),
            updated.dimmed()
        );

        if let Some(ref input) = session.last_tool_input {
            println!("    {}", input.dimmed());
        }
    }
}
