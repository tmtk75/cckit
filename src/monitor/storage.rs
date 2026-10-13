use super::paths;
use super::session::SessionStore;
use chrono::Utc;
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io;
use std::io::Write;

const SESSIONS_FILE: &str = "sessions.json";
const LOCK_FILE: &str = "sessions.lock";
const AF_CONFIG_FILE: &str = "af_disabled.json";
const WINDOW_FRAME_FILE: &str = "window_frame.json";
const PROJECT_COLUMN_FILE: &str = "project_column_width.json";

pub struct Storage {
    path: std::path::PathBuf,
    lock_path: std::path::PathBuf,
}

impl Storage {
    pub fn new() -> Self {
        let dir = paths::data_dir();
        let path = dir.join(SESSIONS_FILE);
        let lock_path = dir.join(LOCK_FILE);
        Self { path, lock_path }
    }

    fn ensure_dir(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    fn load_internal(&self) -> io::Result<SessionStore> {
        if !self.path.exists() {
            return Ok(SessionStore::default());
        }

        let content = fs::read_to_string(&self.path)?;
        serde_json::from_str(&content).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse sessions.json: {}", e),
            )
        })
    }

    fn save_internal(&self, store: &SessionStore) -> io::Result<()> {
        self.ensure_dir()?;
        let content = serde_json::to_string_pretty(store)?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Invalid sessions file path"))?;
        let tmp_path = dir.join(format!("{}.tmp.{}", SESSIONS_FILE, std::process::id()));

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }

        fs::rename(&tmp_path, &self.path)?;

        if let Ok(dir_fd) = OpenOptions::new().read(true).open(dir) {
            let _ = dir_fd.sync_all();
        }

        Ok(())
    }

    /// Load without lock (for read-only access like TUI)
    pub fn load(&self) -> SessionStore {
        self.load_internal().unwrap_or_default()
    }

    /// Execute a function with exclusive lock on the sessions file
    pub fn with_lock<F, T>(&self, f: F) -> io::Result<T>
    where
        F: FnOnce(&mut SessionStore) -> T,
    {
        self.ensure_dir()?;

        let lock_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)?;

        lock_file.lock_exclusive()?;

        let mut store = self.load_internal()?;
        let result = f(&mut store);
        self.save_internal(&store)?;

        lock_file.unlock()?;

        Ok(result)
    }

    /// Check if a TTY device exists
    fn tty_exists(tty: &str) -> bool {
        std::path::Path::new(tty).exists()
    }

    /// Check if an AI coding tool (Claude Code or Codex) is running on the given TTY
    fn has_ai_tool_on_tty(tty: &str) -> bool {
        // Strip /dev/ prefix for ps TTY matching
        let tty_short = tty.strip_prefix("/dev/").unwrap_or(tty);
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-t", tty_short, "-o", "comm="])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.lines().any(Self::is_ai_tool_process)
        } else {
            true // If ps fails, assume alive to be safe
        }
    }

    /// Whether one `ps -o comm=` line names an AI coding tool cckit tracks.
    ///
    /// Antigravity and grok are matched on the program name rather than anywhere in the
    /// path: `agy` and `grok` are short enough to turn up inside unrelated names.
    fn is_ai_tool_process(line: &str) -> bool {
        let cmd = line.trim();
        if cmd.contains("claude")
            || cmd.contains("Claude")
            || cmd.contains("codex")
            || cmd.contains("Codex")
        {
            return true;
        }
        let program = cmd.rsplit('/').next().unwrap_or(cmd);
        program == "agy"
            || program == "grok"
            || program.contains("antigravity")
            || program.contains("Antigravity")
    }

    /// Check if a session is stale (TTY gone or no AI tool process on TTY)
    ///
    /// Note: We don't use PID-based checks because the stored PID is the hook's
    /// parent shell process (short-lived /bin/sh), not the claude process itself.
    /// Instead, we check if any claude process is running on the session's TTY.
    fn is_stale(session: &super::session::Session) -> bool {
        if session.tty == "unknown" {
            // No TTY (e.g. desktop app): check if pid is still alive
            return session.pid.is_none_or(|pid| !Self::pid_alive(pid));
        }
        if !Self::tty_exists(&session.tty) {
            return true;
        }
        !Self::has_ai_tool_on_tty(&session.tty)
    }

    fn pid_alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Find stale sessions (TTY gone or process dead)
    pub fn find_stale_sessions(&self) -> io::Result<Vec<(String, super::session::Session)>> {
        let store = self.load_internal()?;
        Ok(store
            .sessions
            .into_iter()
            .filter(|(_, session)| Self::is_stale(session))
            .collect())
    }

    /// Remove stale sessions (TTY gone or process dead)
    pub fn sync_sessions(&self) -> io::Result<Vec<String>> {
        self.with_lock(|store| {
            let stale_keys: Vec<String> = store
                .sessions
                .iter()
                .filter(|(_, session)| Self::is_stale(session))
                .map(|(key, _)| key.clone())
                .collect();

            for key in &stale_keys {
                store.sessions.remove(key);
            }

            if !stale_keys.is_empty() {
                store.updated_at = Utc::now();
            }

            stale_keys
        })
    }

    /// Remove a specific session by key
    pub fn remove_session(&self, key: &str) -> io::Result<bool> {
        self.with_lock(|store| {
            let removed = store.sessions.remove(key).is_some();
            if removed {
                store.updated_at = Utc::now();
            }
            removed
        })
    }

    /// Load AF disabled projects set
    pub fn load_af_disabled(&self) -> std::collections::HashSet<String> {
        let af_path = self.path.parent().unwrap().join(AF_CONFIG_FILE);
        let content = fs::read_to_string(&af_path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Save AF disabled projects set (atomic write via tmp + rename)
    pub fn save_af_disabled(&self, disabled: &std::collections::HashSet<String>) -> io::Result<()> {
        self.ensure_dir()?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Invalid data path"))?;
        let af_path = dir.join(AF_CONFIG_FILE);
        let tmp_path = dir.join(format!("{}.tmp.{}", AF_CONFIG_FILE, std::process::id()));
        let content = serde_json::to_string_pretty(disabled)?;

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }

        fs::rename(&tmp_path, &af_path)?;
        Ok(())
    }

    /// Load saved window frame (x, y, width, height)
    pub fn load_window_frame(&self) -> Option<(f64, f64, f64, f64)> {
        let path = self.path.parent()?.join(WINDOW_FRAME_FILE);
        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// The classic theme's PROJECT column width as dragged by the user; `None` when it
    /// was never dragged or was reset, which means the automatic width.
    pub fn load_project_column_width(&self) -> Option<f64> {
        let path = self.path.parent()?.join(PROJECT_COLUMN_FILE);
        let content = fs::read_to_string(&path).ok()?;
        serde_json::from_str(&content).ok()
    }

    /// Save (or with `None`, forget) the dragged PROJECT column width.
    pub fn save_project_column_width(&self, width: Option<f64>) -> io::Result<()> {
        self.ensure_dir()?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Invalid data path"))?;
        let path = dir.join(PROJECT_COLUMN_FILE);
        let Some(width) = width else {
            return match fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        };
        let tmp_path = dir.join(format!(
            "{}.tmp.{}",
            PROJECT_COLUMN_FILE,
            std::process::id()
        ));
        fs::write(&tmp_path, serde_json::to_string(&width)?)?;
        fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Save window frame (atomic write via tmp + rename)
    pub fn save_window_frame(&self, frame: (f64, f64, f64, f64)) -> io::Result<()> {
        self.ensure_dir()?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Invalid data path"))?;
        let path = dir.join(WINDOW_FRAME_FILE);
        let tmp_path = dir.join(format!("{}.tmp.{}", WINDOW_FRAME_FILE, std::process::id()));
        let content = serde_json::to_string(&frame)?;

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
        }

        fs::rename(&tmp_path, &path)?;
        Ok(())
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod agy_process_tests {
    use super::*;

    #[test]
    fn the_antigravity_cli_counts_as_a_live_agent() {
        assert!(Storage::is_ai_tool_process(
            "/Users/t/.local/share/mise/installs/antigravity-cli/1.2.7/agy"
        ));
        assert!(Storage::is_ai_tool_process("agy"));
        assert!(Storage::is_ai_tool_process(
            "/Applications/Antigravity.app/Contents/Resources/app/bin/antigravity"
        ));
    }

    #[test]
    fn a_program_that_merely_contains_those_letters_does_not() {
        // "agy" is short enough to appear inside unrelated names, so it only counts as
        // the program being run, not as a substring of the path.
        assert!(!Storage::is_ai_tool_process("/usr/local/bin/legacy-sync"));
        assert!(!Storage::is_ai_tool_process("/usr/bin/node"));
        assert!(!Storage::is_ai_tool_process(""));
    }

    #[test]
    fn the_previously_supported_tools_still_count() {
        assert!(Storage::is_ai_tool_process("claude"));
        assert!(Storage::is_ai_tool_process("/opt/homebrew/bin/codex"));
        assert!(Storage::is_ai_tool_process("Claude Code Helper"));
    }
}

#[cfg(test)]
mod grok_process_tests {
    use super::*;

    #[test]
    fn the_grok_cli_counts_as_a_live_agent() {
        assert!(Storage::is_ai_tool_process("grok"));
        assert!(Storage::is_ai_tool_process(
            "/Users/t/.local/share/mise/shims/grok"
        ));
    }

    #[test]
    fn a_program_that_merely_contains_those_letters_does_not() {
        // Same reasoning as `agy`: match the program, not any path that spells it.
        assert!(!Storage::is_ai_tool_process("/usr/local/bin/grokify-logs"));
        assert!(!Storage::is_ai_tool_process("/opt/grok/share/helper"));
    }

    #[test]
    fn project_column_width_round_trips_and_resets() {
        let dir = tempfile::TempDir::new().unwrap();
        let storage = Storage {
            path: dir.path().join(SESSIONS_FILE),
            lock_path: dir.path().join(LOCK_FILE),
        };
        assert_eq!(storage.load_project_column_width(), None);
        storage.save_project_column_width(Some(312.5)).unwrap();
        assert_eq!(storage.load_project_column_width(), Some(312.5));
        storage.save_project_column_width(None).unwrap();
        assert_eq!(storage.load_project_column_width(), None);
        // Resetting with nothing saved is not an error.
        storage.save_project_column_width(None).unwrap();
    }
}
