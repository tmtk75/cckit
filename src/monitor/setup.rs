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

/// Clear [hooks.state] entries from ~/.codex/config.toml so Codex will
/// re-prompt the user to trust modified hooks.
fn clear_codex_hooks_state() -> io::Result<bool> {
    let config_path = get_codex_config_path();
    if !config_path.exists() {
        return Ok(false);
    }

    let content = fs::read_to_string(&config_path)?;
    if !content.contains("[hooks.state.") {
        return Ok(false);
    }

    let mut lines: Vec<&str> = content.lines().collect();
    let mut in_hooks_state_entry = false;
    lines.retain(|line| {
        let trimmed = line.trim();
        if trimmed.starts_with("[hooks.state.\"") {
            in_hooks_state_entry = true;
            return false;
        }
        if in_hooks_state_entry {
            if trimmed.starts_with('[') {
                in_hooks_state_entry = false;
            } else {
                return false;
            }
        }
        true
    });

    let new_content = lines.join("\n");
    // Avoid trailing blank lines from removed sections
    let new_content = new_content.trim_end().to_string() + "\n";
    fs::write(&config_path, new_content)?;
    Ok(true)
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
        if content.contains("codex_hooks") || content.contains("hooks = true") {
            return Ok(false); // already configured (legacy or new key)
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

    let hooks_changed = result.created || !result.added.is_empty();

    if result.created {
        println!("{}", "No ~/.codex/hooks.json found.".yellow());
        println!("Creating new hooks.json...");
        println!(
            "{} Created hooks.json with {} hooks",
            "✓".green(),
            CODEX_HOOK_EVENTS.len()
        );
    } else if !result.already_exists.is_empty() {
        println!("{}:", "Already configured".yellow());
        for event in &result.already_exists {
            println!("  {} {}", "✓".green(), event);
        }
        println!();
    }

    if !result.created && result.added.is_empty() {
        println!("{}", "All cckit hooks are already configured.".green());
    }

    if !result.created && !result.added.is_empty() {
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
    }

    // Clear stale trusted_hash entries so Codex will re-prompt for trust
    if hooks_changed || force {
        match clear_codex_hooks_state() {
            Ok(true) => {
                println!(
                    "{} Cleared hooks.state in config.toml (Codex will re-prompt for trust)",
                    "✓".green()
                );
            }
            Ok(false) => {}
            Err(e) => {
                println!("{} Failed to clear hooks.state: {}", "⚠".yellow(), e);
            }
        }
    }

    println!();
    println!("Hooks file: {}", hooks_path.display().to_string().dimmed());
    println!();
    println!(
        "{}",
        "Restart Codex and approve hooks via /hooks for them to take effect.".yellow()
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

/// The name cckit registers its Antigravity hooks under. Antigravity merges named hook
/// sets from every source, so a name of our own leaves other tools' hooks alone.
const AGY_HOOK_NAME: &str = "cckit";

/// The Antigravity lifecycle events cckit listens to.
///
/// `PreToolUse` is deliberately absent. Its contract requires the hook to answer with a
/// permission decision (`allow` / `deny` / `ask` / `force_ask`) and treats a missing
/// answer as a refusal, so a session monitor installed there would either rubber-stamp
/// every tool call or block them all.
const AGY_HOOK_EVENTS: &[&str] = &["PreInvocation", "PostInvocation", "PostToolUse", "Stop"];

/// Events Antigravity matches per tool name, which must wrap their handlers in a group.
const AGY_MATCHER_EVENTS: &[&str] = &["PreToolUse", "PostToolUse"];

/// Build the `hooks.json` entry cckit registers under its own name.
pub fn agy_hook_entry(command: &str) -> Value {
    let mut entry = Map::new();
    for event in AGY_HOOK_EVENTS {
        let handler = json!({
            "type": "command",
            "command": format!("{command} session hook --event {event}"),
        });
        // Tool-matched events take a group with a matcher; the rest take handlers flat.
        let handlers = if AGY_MATCHER_EVENTS.contains(event) {
            json!([{ "matcher": "", "hooks": [handler] }])
        } else {
            json!([handler])
        };
        entry.insert((*event).to_string(), handlers);
    }
    Value::Object(entry)
}

/// Add or refresh cckit's entry in a `hooks.json` document. Returns whether it changed.
///
/// The entry is replaced wholesale rather than merged: a stale one names an old binary
/// path or an event set cckit no longer listens to, and merging would keep both.
pub fn install_agy_entry(root: &mut Value, command: &str) -> bool {
    let entry = agy_hook_entry(command);
    if !root.is_object() {
        *root = json!({});
    }
    let map = root.as_object_mut().expect("just made it an object");
    if map.get(AGY_HOOK_NAME) == Some(&entry) {
        return false;
    }
    map.insert(AGY_HOOK_NAME.to_string(), entry);
    true
}

/// Drop cckit's entry from a `hooks.json` document. Returns whether anything went.
pub fn remove_agy_entry(root: &mut Value) -> bool {
    root.as_object_mut()
        .is_some_and(|map| map.remove(AGY_HOOK_NAME).is_some())
}

/// Antigravity's machine-local customization root. Workspace-level `.agents/hooks.json`
/// files are not read by the CLI, so this is the only place a global hook can live.
fn get_agy_hooks_path() -> PathBuf {
    let home = dirs::home_dir().expect("Could not find home directory");
    home.join(".gemini").join("config").join("hooks.json")
}

/// Read `hooks.json`, or start an empty document when there is none.
fn read_agy_hooks(path: &Path) -> io::Result<Value> {
    if !path.exists() {
        return Ok(json!({}));
    }
    let content = fs::read_to_string(path)?;
    if content.trim().is_empty() {
        return Ok(json!({}));
    }
    parse_settings(&content)
}

pub fn run_install_agy(force: bool) -> io::Result<()> {
    let path = get_agy_hooks_path();
    let cckit_cmd = get_cckit_command();

    println!("{}", "cckit session install --agy".bold());
    println!();

    let existed = path.exists();
    let mut root = read_agy_hooks(&path)?;
    let changed = install_agy_entry(&mut root, &cckit_cmd);

    if !changed && !force {
        println!("{}", "All cckit hooks are already configured.".green());
    } else {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        write_json_file(&path, &root)?;
        if existed {
            println!("{}:", "Adding".cyan());
        } else {
            println!("{}", "No ~/.gemini/config/hooks.json found.".yellow());
            println!("Creating new hooks.json...");
        }
        for event in AGY_HOOK_EVENTS {
            println!(
                "  {} {} session hook --event {}",
                "+".green(),
                cckit_cmd,
                event
            );
        }
        println!();
        println!(
            "{} Wrote {} hook(s) under \"{}\"",
            "✓".green(),
            AGY_HOOK_EVENTS.len(),
            AGY_HOOK_NAME
        );
    }

    println!();
    println!("Hooks file: {}", path.display().to_string().dimmed());
    println!();
    println!(
        "{}",
        "No PreToolUse hook is installed: that event requires a permission decision,".dimmed()
    );
    println!(
        "{}",
        "which a session monitor must not answer. Tool calls are recorded after they run.".dimmed()
    );

    Ok(())
}

pub fn run_uninstall_agy() -> io::Result<()> {
    let path = get_agy_hooks_path();

    println!("{}", "cckit session uninstall --agy".bold());
    println!();

    if !path.exists() {
        println!("{}", "No ~/.gemini/config/hooks.json found.".yellow());
        return Ok(());
    }

    let mut root = read_agy_hooks(&path)?;
    if !remove_agy_entry(&mut root) {
        println!("{}", "No cckit hooks found.".yellow());
        return Ok(());
    }

    write_json_file(&path, &root)?;
    println!(
        "{} Removed the \"{}\" hooks from {}",
        "✓".green(),
        AGY_HOOK_NAME,
        path.display().to_string().dimmed()
    );
    Ok(())
}

#[cfg(test)]
mod agy_hooks_tests {
    use super::*;

    fn installed() -> Value {
        let mut root = json!({});
        install_agy_entry(&mut root, "~/bin/cckit");
        root
    }

    #[test]
    fn the_entry_covers_every_event_cckit_listens_to() {
        let root = installed();
        let entry = &root["cckit"];
        for event in AGY_HOOK_EVENTS {
            assert!(entry.get(event).is_some(), "{event} missing from {entry}");
        }
    }

    #[test]
    fn no_pre_tool_use_hook_is_ever_written() {
        // Installing one would force cckit to answer Antigravity's permission prompt.
        let root = installed();
        assert!(root["cckit"].get("PreToolUse").is_none(), "got: {root}");
    }

    #[test]
    fn a_tool_matched_event_wraps_its_handlers_in_a_matcher_group() {
        let root = installed();
        let post = &root["cckit"]["PostToolUse"][0];
        assert_eq!(post["matcher"], "");
        assert_eq!(
            post["hooks"][0]["command"],
            "~/bin/cckit session hook --event PostToolUse"
        );
    }

    #[test]
    fn an_untargeted_event_lists_its_handlers_directly() {
        let root = installed();
        let pre = &root["cckit"]["PreInvocation"][0];
        assert_eq!(pre["type"], "command");
        assert_eq!(
            pre["command"],
            "~/bin/cckit session hook --event PreInvocation"
        );
    }

    #[test]
    fn installing_leaves_another_tools_hooks_untouched() {
        let mut root = json!({"lint-checker": {"PostToolUse": [{"matcher": "run_command"}]}});
        install_agy_entry(&mut root, "cckit");
        assert_eq!(
            root["lint-checker"]["PostToolUse"][0]["matcher"],
            "run_command"
        );
        assert!(root.get("cckit").is_some());
    }

    #[test]
    fn installing_twice_changes_nothing_the_second_time() {
        let mut root = json!({});
        assert!(install_agy_entry(&mut root, "cckit"));
        let after_first = root.clone();
        assert!(!install_agy_entry(&mut root, "cckit"));
        assert_eq!(root, after_first);
    }

    #[test]
    fn an_outdated_entry_is_replaced_rather_than_merged() {
        let mut root = json!({"cckit": {"Stop": [{"command": "old-path session hook"}]}});
        assert!(install_agy_entry(&mut root, "new-path"));
        assert_eq!(
            root["cckit"]["Stop"][0]["command"],
            "new-path session hook --event Stop"
        );
    }

    #[test]
    fn removing_takes_only_cckits_entry() {
        let mut root = json!({"lint-checker": {"Stop": []}});
        install_agy_entry(&mut root, "cckit");
        assert!(remove_agy_entry(&mut root));
        assert!(root.get("cckit").is_none());
        assert!(root.get("lint-checker").is_some());
    }

    #[test]
    fn removing_reports_when_there_was_nothing_to_remove() {
        let mut root = json!({"lint-checker": {}});
        assert!(!remove_agy_entry(&mut root));
    }
}
