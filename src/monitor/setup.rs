use colored::Colorize;
use serde_json::{Map, Value, json};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Core hook events required for basic session tracking
const CORE_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
];

/// Extended hook events for additional features (not required for basic operation)
const EXTENDED_HOOK_EVENTS: &[&str] = &["SubagentStop", "Notification", "PreCompact"];

/// All hook events (core + extended)
const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
    "SubagentStop",
    "Notification",
    "PreCompact",
];

fn get_settings_path() -> PathBuf {
    let home = dirs::home_dir().expect("Could not find home directory");
    home.join(".claude").join("settings.json")
}

fn get_cckit_command() -> String {
    // Always use absolute path because hooks run via /bin/sh which doesn't have user's PATH
    std::env::current_exe()
        .map(|p| {
            let path = p.to_string_lossy().to_string();
            // Replace home dir with ~ for portability
            if let Some(home) = dirs::home_dir() {
                let home_str = home.to_string_lossy().to_string();
                if path.starts_with(&home_str) {
                    return path.replacen(&home_str, "~", 1);
                }
            }
            path
        })
        .unwrap_or_else(|_| "cckit".to_string())
}

fn create_hook_entry(command: &str) -> Value {
    json!({
        "matcher": "",
        "hooks": [{
            "type": "command",
            "command": format!("{} session hook", command)
        }]
    })
}

fn is_cckit_hook_command(command: &str) -> bool {
    let mut parts = command.split_whitespace();
    let bin = match parts.next() {
        Some(p) => p,
        None => return false,
    };
    let is_cckit = bin == "cckit" || bin.ends_with("/cckit");
    if !is_cckit {
        return false;
    }

    let sub = parts.next() == Some("session");
    let action = parts.next() == Some("hook");
    sub && action
}

fn has_cckit_hook(hooks_array: &Value) -> bool {
    if let Some(arr) = hooks_array.as_array() {
        for entry in arr {
            if let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) {
                for hook in hooks {
                    if let Some(cmd) = hook.get("command").and_then(|c| c.as_str())
                        && is_cckit_hook_command(cmd)
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

fn remove_cckit_hooks_from_event(hook_array: &mut Value) -> bool {
    let Some(arr) = hook_array.as_array_mut() else {
        return false;
    };

    let mut removed_any = false;
    let mut new_entries = Vec::with_capacity(arr.len());
    for mut entry in arr.drain(..) {
        if let Some(hooks) = entry.get_mut("hooks").and_then(|h| h.as_array_mut()) {
            let before = hooks.len();
            hooks.retain(|hook| {
                let cmd = hook.get("command").and_then(|c| c.as_str());
                !cmd.is_some_and(is_cckit_hook_command)
            });
            if hooks.len() != before {
                removed_any = true;
            }
            if hooks.is_empty() {
                continue;
            }
        }
        new_entries.push(entry);
    }
    *arr = new_entries;

    removed_any
}

fn parse_settings(content: &str) -> io::Result<Value> {
    serde_json::from_str(content).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Failed to parse settings.json: {}", e),
        )
    })
}

fn ensure_hooks_object(settings: &mut Value) -> io::Result<&mut Map<String, Value>> {
    if settings.get("hooks").is_none() {
        settings
            .as_object_mut()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "settings.json is not an object")
            })?
            .insert("hooks".to_string(), json!({}));
    }

    settings
        .get_mut("hooks")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "hooks field is not an object"))
}

fn build_hooks_settings(events: &[&str], cckit_cmd: &str) -> Value {
    let mut hooks = Map::new();
    for event in events {
        hooks.insert(event.to_string(), json!([create_hook_entry(cckit_cmd)]));
    }
    json!({ "hooks": Value::Object(hooks) })
}

fn write_json_file(path: &Path, value: &Value) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(value)?;
    fs::write(path, format!("{}\n", content))
}

fn remove_cckit_hooks_for_events(
    hooks_obj: &mut Map<String, Value>,
    events: &[&'static str],
) -> Vec<&'static str> {
    let mut removed = Vec::new();
    let mut empty_events = Vec::new();

    for event in events {
        if let Some(hook_array) = hooks_obj.get_mut(*event) {
            if remove_cckit_hooks_from_event(hook_array) {
                removed.push(*event);
            }
            if hook_array.as_array().is_some_and(|arr| arr.is_empty()) {
                empty_events.push(*event);
            }
        }
    }

    for event in empty_events {
        hooks_obj.remove(event);
    }

    removed
}

fn add_cckit_hooks_to_events(
    hooks_obj: &mut Map<String, Value>,
    events: &[&'static str],
    cckit_cmd: &str,
) -> (Vec<&'static str>, Vec<&'static str>) {
    let mut added = Vec::new();
    let mut already_exists = Vec::new();

    for event in events {
        if let Some(existing_array) = hooks_obj.get(*event)
            && has_cckit_hook(existing_array)
        {
            already_exists.push(*event);
            continue;
        }

        if let Some(arr) = hooks_obj.get_mut(*event).and_then(|v| v.as_array_mut()) {
            arr.push(create_hook_entry(cckit_cmd));
        } else {
            hooks_obj.insert(event.to_string(), json!([create_hook_entry(cckit_cmd)]));
        }
        added.push(*event);
    }

    (added, already_exists)
}

struct HookInstallResult {
    created: bool,
    added: Vec<&'static str>,
    already_exists: Vec<&'static str>,
}

enum HookUninstallResult {
    MissingFile,
    MissingHooksField,
    NoHooksRemoved,
    Removed(Vec<&'static str>),
}

fn install_hooks_file(
    path: &Path,
    events: &[&'static str],
    cckit_cmd: &str,
    force: bool,
) -> io::Result<HookInstallResult> {
    if !path.exists() {
        let settings = build_hooks_settings(events, cckit_cmd);
        write_json_file(path, &settings)?;
        return Ok(HookInstallResult {
            created: true,
            added: events.to_vec(),
            already_exists: Vec::new(),
        });
    }

    let content = fs::read_to_string(path)?;
    let mut settings: Value = parse_settings(&content)?;
    let hooks_obj = ensure_hooks_object(&mut settings)?;

    if force {
        remove_cckit_hooks_for_events(hooks_obj, events);
    }

    let (added, already_exists) = add_cckit_hooks_to_events(hooks_obj, events, cckit_cmd);
    if !added.is_empty() {
        write_json_file(path, &settings)?;
    }

    Ok(HookInstallResult {
        created: false,
        added,
        already_exists,
    })
}

fn uninstall_hooks_file(path: &Path, events: &[&'static str]) -> io::Result<HookUninstallResult> {
    if !path.exists() {
        return Ok(HookUninstallResult::MissingFile);
    }

    let content = fs::read_to_string(path)?;
    let mut settings: Value = parse_settings(&content)?;
    let Some(hooks_obj) = settings
        .get_mut("hooks")
        .and_then(|hooks| hooks.as_object_mut())
    else {
        return Ok(HookUninstallResult::MissingHooksField);
    };

    let removed = remove_cckit_hooks_for_events(hooks_obj, events);
    if removed.is_empty() {
        return Ok(HookUninstallResult::NoHooksRemoved);
    }

    write_json_file(path, &settings)?;
    Ok(HookUninstallResult::Removed(removed))
}

pub fn run_install(force: bool) -> io::Result<()> {
    let settings_path = get_settings_path();
    let cckit_cmd = get_cckit_command();

    println!("{}", "cckit session install".bold());
    println!();

    let result = install_hooks_file(&settings_path, HOOK_EVENTS, &cckit_cmd, force)?;

    if result.created {
        println!("{}", "No settings.json found.".yellow());
        println!("Creating new settings.json with hooks...");
        println!("{} Created settings.json with all hooks", "✓".green());
        return Ok(());
    }

    if !result.already_exists.is_empty() {
        println!("{}:", "Already configured".yellow());
        for event in &result.already_exists {
            println!("  {} {}", "✓".green(), event);
        }
        println!();
    }

    if result.added.is_empty() {
        println!("{}", "All cckit hooks are already configured.".green());
        println!("Use {} to see active sessions.", "cckit session".cyan());
        return Ok(());
    }

    println!("{}:", "Adding".cyan());
    for event in &result.added {
        println!("  {} {} session hook {}", "+".green(), cckit_cmd, event);
    }
    println!();

    println!(
        "{} Added {} hook(s) to settings.json",
        "✓".green(),
        result.added.len()
    );
    println!();
    println!(
        "Settings file: {}",
        settings_path.display().to_string().dimmed()
    );
    println!();
    println!(
        "{}",
        "Restart Claude Code sessions for hooks to take effect.".yellow()
    );

    Ok(())
}

/// Check which hooks are missing. Returns (missing_core, missing_extended).
pub fn check_hooks_installed() -> (Vec<&'static str>, Vec<&'static str>) {
    let settings_path = get_settings_path();

    if !settings_path.exists() {
        return (CORE_HOOK_EVENTS.to_vec(), EXTENDED_HOOK_EVENTS.to_vec());
    }

    let content = match fs::read_to_string(&settings_path) {
        Ok(c) => c,
        Err(_) => return (CORE_HOOK_EVENTS.to_vec(), EXTENDED_HOOK_EVENTS.to_vec()),
    };

    let settings: Value = match parse_settings(&content) {
        Ok(s) => s,
        Err(_) => return (CORE_HOOK_EVENTS.to_vec(), EXTENDED_HOOK_EVENTS.to_vec()),
    };

    let hooks = settings.get("hooks").cloned().unwrap_or_else(|| json!({}));

    let check_events = |events: &[&'static str]| -> Vec<&'static str> {
        events
            .iter()
            .filter(|event| {
                hooks
                    .get(**event)
                    .is_none_or(|hook_array| !has_cckit_hook(hook_array))
            })
            .copied()
            .collect()
    };

    (
        check_events(CORE_HOOK_EVENTS),
        check_events(EXTENDED_HOOK_EVENTS),
    )
}

/// Returns the list of hook events that cckit requires.
pub fn hook_events() -> &'static [&'static str] {
    HOOK_EVENTS
}

pub fn show_status() -> io::Result<()> {
    let settings_path = get_settings_path();

    if !settings_path.exists() {
        println!("{}", "No settings.json found.".yellow());
        println!("Run {} to configure hooks.", "cckit session install".cyan());
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)?;
    let settings: Value = parse_settings(&content)?;

    let hooks = settings.get("hooks").cloned().unwrap_or_else(|| json!({}));

    println!("{}", "Hook status:".bold());
    println!();

    for event in HOOK_EVENTS {
        if let Some(hook_array) = hooks.get(*event) {
            if has_cckit_hook(hook_array) {
                println!("  {} {} {}", "✓".green(), event, "(cckit)".dimmed());
            } else {
                println!(
                    "  {} {} {}",
                    "-".yellow(),
                    event,
                    "(no cckit hook)".dimmed()
                );
            }
        } else {
            println!("  {} {}", "✗".red(), event);
        }
    }

    println!();
    println!("Settings: {}", settings_path.display().to_string().dimmed());

    Ok(())
}

pub fn run_uninstall() -> io::Result<()> {
    let settings_path = get_settings_path();

    println!("{}", "cckit session uninstall".bold());
    println!();

    match uninstall_hooks_file(&settings_path, HOOK_EVENTS)? {
        HookUninstallResult::MissingFile => {
            println!(
                "{}",
                "No settings.json found. Nothing to uninstall.".yellow()
            );
            Ok(())
        }
        HookUninstallResult::MissingHooksField => {
            println!("{}", "No hooks found. Nothing to uninstall.".yellow());
            Ok(())
        }
        HookUninstallResult::NoHooksRemoved => {
            println!("{}", "No cckit hooks found. Nothing to uninstall.".yellow());
            Ok(())
        }
        HookUninstallResult::Removed(removed) => {
            println!("{}:", "Removing".red());
            for event in &removed {
                println!("  {} {}", "-".red(), event);
            }
            println!();

            println!(
                "{} Removed {} hook(s) from settings.json",
                "✓".green(),
                removed.len()
            );
            println!();
            println!(
                "{}",
                "Restart Claude Code sessions for changes to take effect.".yellow()
            );

            Ok(())
        }
    }
}

// --- Codex hook support ---

/// Codex supports a subset of hook events
const CODEX_HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "PreToolUse",
    "PostToolUse",
    "UserPromptSubmit",
    "Stop",
];

fn get_codex_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".codex")
}

fn get_codex_hooks_path() -> PathBuf {
    get_codex_dir().join("hooks.json")
}

fn get_codex_config_path() -> PathBuf {
    get_codex_dir().join("config.toml")
}

/// Ensure hooks feature flag is enabled in ~/.codex/config.toml
fn ensure_codex_feature_flag() -> io::Result<bool> {
    let config_path = get_codex_config_path();
    let codex_dir = get_codex_dir();

    if !codex_dir.exists() {
        fs::create_dir_all(&codex_dir)?;
    }

    if config_path.exists() {
        let content = fs::read_to_string(&config_path)?;
        // Accept both old "codex_hooks" and new "hooks" key
        if content.contains("hooks = true") {
            return Ok(false); // already configured
        }
        if content.contains("[features]") {
            let new_content = content.replace(
                "[features]",
                "[features]\nhooks = true  # Added by cckit (https://github.com/tmtk75/cckit)",
            );
            fs::write(&config_path, new_content)?;
        } else {
            let mut file = fs::OpenOptions::new().append(true).open(&config_path)?;
            file.write_all(
                b"\n# Added by cckit (https://github.com/tmtk75/cckit)\n\
                  [features]\nhooks = true\n",
            )?;
        }
    } else {
        fs::write(
            &config_path,
            "# Added by cckit (https://github.com/tmtk75/cckit)\n\
             [features]\nhooks = true\n",
        )?;
    }

    Ok(true)
}

pub fn run_install_codex(force: bool) -> io::Result<()> {
    let hooks_path = get_codex_hooks_path();
    let cckit_cmd = get_cckit_command();

    println!("{}", "cckit session install --codex".bold());
    println!();

    // Ensure hooks feature flag is enabled
    match ensure_codex_feature_flag() {
        Ok(true) => {
            println!(
                "{} Enabled hooks in {}",
                "✓".green(),
                get_codex_config_path().display().to_string().dimmed()
            );
        }
        Ok(false) => {
            println!("{} hooks already enabled in config.toml", "✓".green());
        }
        Err(e) => {
            println!("{} Failed to update config.toml: {}", "⚠".yellow(), e);
        }
    }
    println!();

    let result = install_hooks_file(&hooks_path, CODEX_HOOK_EVENTS, &cckit_cmd, force)?;

    if result.created {
        println!("{}", "No ~/.codex/hooks.json found.".yellow());
        println!("Creating new hooks.json...");
        println!(
            "{} Created hooks.json with {} hooks",
            "✓".green(),
            CODEX_HOOK_EVENTS.len()
        );
        println!();
        println!("Hooks file: {}", hooks_path.display().to_string().dimmed());
        println!();
        println!(
            "{}",
            "Restart Codex sessions for hooks to take effect.".yellow()
        );
        return Ok(());
    }

    if !result.already_exists.is_empty() {
        println!("{}:", "Already configured".yellow());
        for event in &result.already_exists {
            println!("  {} {}", "✓".green(), event);
        }
        println!();
    }

    if result.added.is_empty() {
        println!("{}", "All cckit hooks are already configured.".green());
        return Ok(());
    }

    println!("{}:", "Adding".cyan());
    for event in &result.added {
        println!("  {} {} session hook {}", "+".green(), cckit_cmd, event);
    }
    println!();

    println!(
        "{} Added {} hook(s) to hooks.json",
        "✓".green(),
        result.added.len()
    );
    println!();
    println!("Hooks file: {}", hooks_path.display().to_string().dimmed());
    println!();
    println!(
        "{}",
        "Restart Codex sessions for hooks to take effect.".yellow()
    );

    Ok(())
}

pub fn run_uninstall_codex() -> io::Result<()> {
    let hooks_path = get_codex_hooks_path();

    println!("{}", "cckit session uninstall --codex".bold());
    println!();

    match uninstall_hooks_file(&hooks_path, CODEX_HOOK_EVENTS)? {
        HookUninstallResult::MissingFile => {
            println!(
                "{}",
                "No ~/.codex/hooks.json found. Nothing to uninstall.".yellow()
            );
            Ok(())
        }
        HookUninstallResult::MissingHooksField => {
            println!("{}", "No hooks found. Nothing to uninstall.".yellow());
            Ok(())
        }
        HookUninstallResult::NoHooksRemoved => {
            println!("{}", "No cckit hooks found. Nothing to uninstall.".yellow());
            Ok(())
        }
        HookUninstallResult::Removed(removed) => {
            println!("{}:", "Removing".red());
            for event in &removed {
                println!("  {} {}", "-".red(), event);
            }
            println!();

            println!(
                "{} Removed {} hook(s) from hooks.json",
                "✓".green(),
                removed.len()
            );
            println!();
            println!(
                "{}",
                "Restart Codex sessions for changes to take effect.".yellow()
            );

            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_hook_entry() {
        let entry = create_hook_entry("cckit");
        assert_eq!(entry["matcher"], "");
        assert!(entry["hooks"].is_array());
        let hooks = entry["hooks"].as_array().unwrap();
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["type"], "command");
        assert_eq!(hooks[0]["command"], "cckit session hook");
    }

    #[test]
    fn test_create_hook_entry_custom_command() {
        let entry = create_hook_entry("/usr/local/bin/cckit");
        assert_eq!(
            entry["hooks"][0]["command"],
            "/usr/local/bin/cckit session hook"
        );
    }

    #[test]
    fn test_has_cckit_hook_found() {
        let hooks_array = json!([
            {
                "matcher": "",
                "hooks": [
                    {"type": "command", "command": "cckit session hook"}
                ]
            }
        ]);
        assert!(has_cckit_hook(&hooks_array));
    }

    #[test]
    fn test_has_cckit_hook_not_found() {
        let hooks_array = json!([
            {
                "matcher": "",
                "hooks": [
                    {"type": "command", "command": "other-command"}
                ]
            }
        ]);
        assert!(!has_cckit_hook(&hooks_array));
    }

    #[test]
    fn test_has_cckit_hook_empty_array() {
        let hooks_array = json!([]);
        assert!(!has_cckit_hook(&hooks_array));
    }

    #[test]
    fn test_has_cckit_hook_full_path() {
        let hooks_array = json!([
            {
                "matcher": "",
                "hooks": [
                    {"type": "command", "command": "/usr/local/bin/cckit session hook"}
                ]
            }
        ]);
        assert!(has_cckit_hook(&hooks_array));
    }

    #[test]
    fn test_has_cckit_hook_mixed_hooks() {
        let hooks_array = json!([
            {
                "matcher": "",
                "hooks": [
                    {"type": "command", "command": "other-command"},
                    {"type": "command", "command": "cckit session hook"}
                ]
            }
        ]);
        assert!(has_cckit_hook(&hooks_array));
    }

    #[test]
    fn test_has_cckit_hook_multiple_entries() {
        let hooks_array = json!([
            {
                "matcher": "",
                "hooks": [{"type": "command", "command": "first"}]
            },
            {
                "matcher": "",
                "hooks": [{"type": "command", "command": "cckit session hook"}]
            }
        ]);
        assert!(has_cckit_hook(&hooks_array));
    }
}
