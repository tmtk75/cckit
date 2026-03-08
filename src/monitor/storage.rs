use super::session::{SessionStore, TuiState};
use chrono::Utc;
use fs2::FileExt;
use std::fs::{self, OpenOptions};
use std::io;
use std::io::Write;
use std::path::PathBuf;

const SESSIONS_FILE: &str = "sessions.json";
const LOCK_FILE: &str = "sessions.lock";
const TUI_STATE_FILE: &str = "tui_state.json";
const AF_CONFIG_FILE: &str = "af_disabled.json";

fn get_data_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| {
            dirs::home_dir()
                .expect("Could not find home directory")
                .join(".local/share")
        })
        .join("cckit")
}

pub struct Storage {
    path: PathBuf,
    lock_path: PathBuf,
}

impl Storage {
    pub fn new() -> Self {
        let dir = get_data_dir();
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
            .ok_or_else(|| io::Error::new(io::ErrorKind::Other, "Invalid sessions file path"))?;
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

    /// Check if a process is alive
    fn pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    /// Check if a claude process is running under the given parent PID's TTY
    fn has_claude_on_tty(tty: &str) -> bool {
        // Strip /dev/ prefix for ps TTY matching
        let tty_short = tty.strip_prefix("/dev/").unwrap_or(tty);
        if let Ok(output) = std::process::Command::new("ps")
            .args(["-t", tty_short, "-o", "comm="])
            .output()
        {
            let stdout = String::from_utf8_lossy(&output.stdout);
            stdout.lines().any(|line| {
                let cmd = line.trim();
                cmd.contains("claude") || cmd.contains("Claude")
            })
        } else {
            true // If ps fails, assume alive to be safe
        }
    }

    /// Check if a session is stale (TTY gone, process dead, or no claude on TTY)
    fn is_stale(session: &super::session::Session) -> bool {
        if !Self::tty_exists(&session.tty) {
            return true;
        }
        if let Some(pid) = session.pid {
            if !Self::pid_alive(pid) {
                return true;
            }
        }
        !Self::has_claude_on_tty(&session.tty)
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

    /// Save TUI state (for menubar integration)
    pub fn save_tui_state(&self, state: &TuiState) -> io::Result<()> {
        self.ensure_dir()?;
        let tui_path = self.path.parent().unwrap().join(TUI_STATE_FILE);
        let content = serde_json::to_string_pretty(state)?;
        fs::write(&tui_path, content)?;
        Ok(())
    }

    /// Load TUI state (returns None if not exists or invalid)
    pub fn load_tui_state(&self) -> Option<TuiState> {
        let tui_path = self.path.parent()?.join(TUI_STATE_FILE);
        let content = fs::read_to_string(&tui_path).ok()?;
        let state: TuiState = serde_json::from_str(&content).ok()?;

        // Check if TTY still exists (TUI is still running)
        if Self::tty_exists(&state.tty) {
            Some(state)
        } else {
            // Clean up stale state file
            let _ = fs::remove_file(&tui_path);
            None
        }
    }

    /// Clear TUI state (called when TUI exits)
    pub fn clear_tui_state(&self) -> io::Result<()> {
        let tui_path = self.path.parent().unwrap().join(TUI_STATE_FILE);
        if tui_path.exists() {
            fs::remove_file(&tui_path)?;
        }
        Ok(())
    }

    /// Load AF disabled projects set
    pub fn load_af_disabled(&self) -> std::collections::HashSet<String> {
        let af_path = self.path.parent().unwrap().join(AF_CONFIG_FILE);
        let content = fs::read_to_string(&af_path).unwrap_or_default();
        serde_json::from_str(&content).unwrap_or_default()
    }

    /// Save AF disabled projects set
    pub fn save_af_disabled(&self, disabled: &std::collections::HashSet<String>) -> io::Result<()> {
        self.ensure_dir()?;
        let af_path = self.path.parent().unwrap().join(AF_CONFIG_FILE);
        let content = serde_json::to_string_pretty(disabled)?;
        fs::write(&af_path, content)?;
        Ok(())
    }
}

impl Default for Storage {
    fn default() -> Self {
        Self::new()
    }
}
