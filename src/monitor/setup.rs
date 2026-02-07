use colored::Colorize;
use serde_json::{json, Map, Value};
use std::fs;
use std::io;
use std::path::PathBuf;

const HOOK_EVENTS: &[&str] = &[
    "SessionStart",
    "SessionEnd",
    "UserPromptSubmit",
    "PreToolUse",
    "PostToolUse",
    "Stop",
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
                    if let Some(cmd) = hook.get("command").and_then(|c| c.as_str()) {
                        if is_cckit_hook_command(cmd) {
                            return true;
                        }
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
                !cmd.map_or(false, is_cckit_hook_command)
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
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "settings.json is not an object"))?
            .insert("hooks".to_string(), json!({}));
    }

    settings
        .get_mut("hooks")
        .and_then(|v| v.as_object_mut())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "hooks field is not an object"))
}

fn build_default_settings(cckit_cmd: &str) -> Value {
    let mut hooks = Map::new();
    for event in HOOK_EVENTS {
        hooks.insert(event.to_string(), json!([create_hook_entry(cckit_cmd)]));
    }
    json!({ "hooks": Value::Object(hooks) })
}

pub fn run_install(force: bool) -> io::Result<()> {
    let settings_path = get_settings_path();
    let cckit_cmd = get_cckit_command();

    if !settings_path.exists() {
        println!("{}", "No settings.json found.".yellow());
        println!("Creating new settings.json with hooks...");

        let settings = build_default_settings(&cckit_cmd);

        if let Some(parent) = settings_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&settings)?;
        fs::write(&settings_path, format!("{}\n", content))?;

        println!("{} Created settings.json with all hooks", "✓".green());
        return Ok(());
    }

    // Read file content as string to preserve formatting
    let content = fs::read_to_string(&settings_path)?;
    let mut settings: Value = parse_settings(&content)?;
    let hooks_obj = ensure_hooks_object(&mut settings)?;

    let mut added: Vec<&str> = Vec::new();
    let mut already_exists: Vec<&str> = Vec::new();

    println!("{}", "cckit session install".bold());
    println!();

    if force {
        let mut empty_events: Vec<&str> = Vec::new();
        for event in HOOK_EVENTS {
            if let Some(hook_array) = hooks_obj.get_mut(*event) {
                remove_cckit_hooks_from_event(hook_array);
                if hook_array.as_array().map_or(false, |arr| arr.is_empty()) {
                    empty_events.push(event);
                }
            }
        }

        for event in &empty_events {
            hooks_obj.remove(*event);
        }
    }

    for event in HOOK_EVENTS {
        if let Some(existing_array) = hooks_obj.get(*event) {
            if has_cckit_hook(existing_array) {
                already_exists.push(event);
                continue;
            }

            // Append to existing array
            if let Some(arr) = hooks_obj.get_mut(*event).and_then(|v| v.as_array_mut()) {
                arr.push(create_hook_entry(&cckit_cmd));
                added.push(event);
            }
        } else {
            // Create new array with cckit hook
            hooks_obj.insert(event.to_string(), json!([create_hook_entry(&cckit_cmd)]));
            added.push(event);
        }
    }

    if !already_exists.is_empty() {
        println!("{}:", "Already configured".yellow());
        for event in &already_exists {
            println!("  {} {}", "✓".green(), event);
        }
        println!();
    }

    if added.is_empty() {
        println!("{}", "All cckit hooks are already configured.".green());
        println!(
            "Use {} to see active sessions.",
            "cckit session".cyan()
        );
        return Ok(());
    }

    println!("{}:", "Adding".cyan());
    for event in &added {
        println!(
            "  {} {} session hook {}",
            "+".green(),
            cckit_cmd,
            event
        );
    }
    println!();

    // Write back with pretty formatting and trailing newline
    let new_content = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_path, format!("{}\n", new_content))?;

    println!(
        "{} Added {} hook(s) to settings.json",
        "✓".green(),
        added.len()
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

pub fn show_status() -> io::Result<()> {
    let settings_path = get_settings_path();

    if !settings_path.exists() {
        println!("{}", "No settings.json found.".yellow());
        println!(
            "Run {} to configure hooks.",
            "cckit session install".cyan()
        );
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
                println!("  {} {} {}", "-".yellow(), event, "(no cckit hook)".dimmed());
            }
        } else {
            println!("  {} {}", "✗".red(), event);
        }
    }

    println!();
    println!(
        "Settings: {}",
        settings_path.display().to_string().dimmed()
    );

    Ok(())
}

pub fn run_uninstall() -> io::Result<()> {
    let settings_path = get_settings_path();

    if !settings_path.exists() {
        println!("{}", "No settings.json found. Nothing to uninstall.".yellow());
        return Ok(());
    }

    let content = fs::read_to_string(&settings_path)?;
    let mut settings: Value = parse_settings(&content)?;
    let hooks = match settings.get_mut("hooks") {
        Some(h) => h,
        None => {
            println!("{}", "No hooks found. Nothing to uninstall.".yellow());
            return Ok(());
        }
    };

    let mut removed: Vec<&str> = Vec::new();
    let mut empty_events: Vec<&str> = Vec::new();

    println!("{}", "cckit session uninstall".bold());
    println!();

    for event in HOOK_EVENTS {
        if let Some(hook_array) = hooks.get_mut(*event) {
            if remove_cckit_hooks_from_event(hook_array) {
                removed.push(event);
            }
            if hook_array.as_array().map_or(false, |arr| arr.is_empty()) {
                empty_events.push(event);
            }
        }
    }

    if let Some(hooks_obj) = hooks.as_object_mut() {
        for event in &empty_events {
            hooks_obj.remove(*event);
        }
    }

    if removed.is_empty() {
        println!("{}", "No cckit hooks found. Nothing to uninstall.".yellow());
        return Ok(());
    }

    println!("{}:", "Removing".red());
    for event in &removed {
        println!("  {} {}", "-".red(), event);
    }
    println!();

    let new_content = serde_json::to_string_pretty(&settings)?;
    fs::write(&settings_path, format!("{}\n", new_content))?;

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
        assert_eq!(entry["hooks"][0]["command"], "/usr/local/bin/cckit session hook");
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
