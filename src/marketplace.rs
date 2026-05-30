use colored::Colorize;
use serde::Deserialize;
use std::path::Path;

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

pub struct Marketplace {
    pub name: String,
    pub plugins: Vec<MarketplacePlugin>,
}

pub struct MarketplacePlugin {
    pub dir_name: String,
    pub manifest: Option<PluginManifest>,
    pub skills: Vec<PluginSkill>,
    pub hooks: Option<HooksInfo>,
    pub mcp_servers: Vec<McpServer>,
    pub mcp_valid_json: Option<bool>,
}

#[derive(Deserialize, Debug)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
}

pub struct PluginSkill {
    pub dir_name: String,
    pub name: String,
    pub description: Option<String>,
    pub has_frontmatter: bool,
}

pub struct HooksInfo {
    pub valid_json: bool,
    pub hook_count: usize,
}

pub struct McpServer {
    pub name: String,
    pub server_type: String,
}

// ---------------------------------------------------------------------------
// Scan helpers
// ---------------------------------------------------------------------------

fn read_manifest(plugin_path: &Path) -> Option<PluginManifest> {
    let p = plugin_path.join(".claude-plugin").join("plugin.json");
    let content = std::fs::read_to_string(&p).ok()?;
    serde_json::from_str(&content).ok()
}

fn scan_plugin_skills(plugin_path: &Path) -> Vec<PluginSkill> {
    let skills_dir = plugin_path.join("skills");
    let mut skills = Vec::new();

    let entries = match std::fs::read_dir(&skills_dir) {
        Ok(e) => e,
        Err(_) => return skills,
    };

    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        let skill_md = entry.path().join("SKILL.md");

        let content = std::fs::read_to_string(&skill_md).unwrap_or_default();
        let (fm_name, description) = crate::cli::parse_frontmatter(&content);
        let has_frontmatter = fm_name.is_some() || description.is_some();
        let name = fm_name.unwrap_or_else(|| dir_name.clone());

        skills.push(PluginSkill {
            dir_name,
            name,
            description,
            has_frontmatter,
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

fn read_hooks(plugin_path: &Path) -> Option<HooksInfo> {
    let p = plugin_path.join("hooks").join("hooks.json");
    let content = std::fs::read_to_string(&p).ok()?;

    match serde_json::from_str::<serde_json::Value>(&content) {
        Ok(v) => {
            let hook_count = v
                .get("hooks")
                .and_then(|h| h.as_object())
                .map(|o| o.len())
                .unwrap_or(0);
            Some(HooksInfo {
                valid_json: true,
                hook_count,
            })
        }
        Err(_) => Some(HooksInfo {
            valid_json: false,
            hook_count: 0,
        }),
    }
}

/// Returns `(Vec<McpServer>, Option<bool>)`.
/// `None` means the file does not exist.
/// `Some(false)` means invalid JSON.
/// `Some(true)` means valid JSON.
fn read_mcp_servers(plugin_path: &Path) -> (Vec<McpServer>, Option<bool>) {
    let p = plugin_path.join("mcp-servers").join("config.json");
    let content = match std::fs::read_to_string(&p) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), None),
    };

    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), Some(false)),
    };

    let mut servers = Vec::new();
    let obj = value.get("mcpServers").and_then(|v| v.as_object());
    if let Some(obj) = obj {
        for (key, val) in obj {
            let server_type = val
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string();
            servers.push(McpServer {
                name: key.clone(),
                server_type,
            });
        }
    }
    servers.sort_by(|a, b| a.name.cmp(&b.name));
    (servers, Some(true))
}

// ---------------------------------------------------------------------------
// Main scan
// ---------------------------------------------------------------------------

pub fn scan_marketplace(path: &Path) -> Marketplace {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string());

    let plugins_dir = path.join("plugins");
    let mut plugins = Vec::new();

    let entries = match std::fs::read_dir(&plugins_dir) {
        Ok(e) => e,
        Err(_) => return Marketplace { name, plugins },
    };

    for entry in entries.flatten() {
        let meta = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !meta.is_dir() {
            continue;
        }

        let dir_name = entry.file_name().to_string_lossy().to_string();
        let plugin_path = entry.path();

        let manifest = read_manifest(&plugin_path);
        let skills = scan_plugin_skills(&plugin_path);
        let hooks = read_hooks(&plugin_path);
        let (mcp_servers, mcp_valid_json) = read_mcp_servers(&plugin_path);

        plugins.push(MarketplacePlugin {
            dir_name,
            manifest,
            skills,
            hooks,
            mcp_servers,
            mcp_valid_json,
        });
    }

    plugins.sort_by(|a, b| a.dir_name.cmp(&b.dir_name));
    Marketplace { name, plugins }
}

// ---------------------------------------------------------------------------
// Task 3: summary_command
// ---------------------------------------------------------------------------

pub fn summary_command(path: &Path) {
    let marketplace = scan_marketplace(path);
    let plugin_count = marketplace.plugins.len();
    println!("{} ({} plugins)", marketplace.name.bold(), plugin_count);

    for plugin in &marketplace.plugins {
        println!();
        match &plugin.manifest {
            Some(m) => {
                print!("  {}", plugin.dir_name.cyan());
                print!(" {}", format!("v{}", m.version).dimmed());
                println!(" — {}", m.description);
            }
            None => {
                print!("  {}", plugin.dir_name.cyan());
                println!(" {}", "(no plugin.json)".red());
            }
        }

        // Skills
        if plugin.skills.is_empty() {
            println!("    Skills: {}", "(none)".dimmed());
        } else {
            println!("    Skills:");
            for skill in &plugin.skills {
                match &skill.description {
                    Some(desc) => {
                        let first_line = desc.lines().next().unwrap_or(desc);
                        println!("      {} — {}", skill.name.green(), first_line.dimmed());
                    }
                    None => println!("      {}", skill.name.green()),
                }
            }
        }

        // Hooks
        match &plugin.hooks {
            Some(h) if h.valid_json => {
                println!("    Hooks: {} hook(s)", h.hook_count);
            }
            Some(_) => {
                println!("    Hooks: {}", "invalid JSON".red());
            }
            None => {
                println!("    Hooks: {}", "(none)".dimmed());
            }
        }

        // MCP Servers
        if plugin.mcp_servers.is_empty() {
            println!("    MCP Servers: {}", "(none)".dimmed());
        } else {
            println!("    MCP Servers:");
            for s in &plugin.mcp_servers {
                println!("      {} ({})", s.name.yellow(), s.server_type);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task 4: doctor_command + validate_marketplace
// ---------------------------------------------------------------------------

pub struct ValidationResult {
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub checks_passed: usize,
    pub checks_total: usize,
}

pub fn validate_marketplace(marketplace: &Marketplace) -> ValidationResult {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    let mut checks_passed = 0usize;
    let mut checks_total = 0usize;

    for plugin in &marketplace.plugins {
        let dir = &plugin.dir_name;

        // Check 1: plugin.json existence
        checks_total += 1;
        if plugin.manifest.is_none() {
            issues.push(format!("[{}] missing plugin.json", dir));
        } else {
            checks_passed += 1;
        }

        // Check 2: name vs dir_name consistency (only if manifest exists)
        if let Some(m) = &plugin.manifest {
            checks_total += 1;
            if m.name != *dir {
                warnings.push(format!(
                    "[{}] plugin.json name {:?} does not match directory name",
                    dir, m.name
                ));
            } else {
                checks_passed += 1;
            }
        }

        // Check 3: SKILL.md frontmatter
        for skill in &plugin.skills {
            checks_total += 1;
            if !skill.has_frontmatter {
                warnings.push(format!(
                    "[{}] skill {:?} is missing frontmatter in SKILL.md",
                    dir, skill.dir_name
                ));
            } else {
                checks_passed += 1;
            }
        }

        // Check 4: hooks.json validity
        if let Some(h) = &plugin.hooks {
            checks_total += 1;
            if !h.valid_json {
                issues.push(format!("[{}] hooks/hooks.json is invalid JSON", dir));
            } else {
                checks_passed += 1;
            }
        }

        // Check 5: mcp-servers/config.json validity
        if let Some(valid) = plugin.mcp_valid_json {
            checks_total += 1;
            if !valid {
                issues.push(format!("[{}] mcp-servers/config.json is invalid JSON", dir));
            } else {
                checks_passed += 1;
            }
        }

        // Check 6: empty plugin (no skills, hooks, or MCP servers)
        checks_total += 1;
        let is_empty =
            plugin.skills.is_empty() && plugin.hooks.is_none() && plugin.mcp_servers.is_empty();
        if is_empty {
            warnings.push(format!(
                "[{}] plugin has no skills, hooks, or MCP servers",
                dir
            ));
        } else {
            checks_passed += 1;
        }
    }

    ValidationResult {
        issues,
        warnings,
        checks_passed,
        checks_total,
    }
}

pub fn doctor_command(path: &Path) {
    let marketplace = scan_marketplace(path);
    let result = validate_marketplace(&marketplace);

    println!("{}: {}", "cckit Marketplace Doctor".bold(), path.display());
    println!();

    if marketplace.plugins.is_empty() {
        println!("  {}", "No plugins found in plugins/ directory".yellow());
        return;
    }

    for plugin in &marketplace.plugins {
        let dir = &plugin.dir_name;

        // plugin.json
        if plugin.manifest.is_some() {
            println!("{} [{}] plugin.json found", "✓".green(), dir);
        } else {
            println!("{} [{}] plugin.json missing", "✗".red(), dir);
        }

        // name consistency
        if let Some(m) = &plugin.manifest {
            if m.name == *dir {
                println!("{} [{}] name matches directory", "✓".green(), dir);
            } else {
                println!(
                    "{} [{}] name {:?} does not match directory",
                    "!".yellow(),
                    dir,
                    m.name
                );
            }
        }

        // skills frontmatter
        for skill in &plugin.skills {
            if skill.has_frontmatter {
                println!(
                    "{} [{}] skill {:?} has frontmatter",
                    "✓".green(),
                    dir,
                    skill.dir_name
                );
            } else {
                println!(
                    "{} [{}] skill {:?} missing frontmatter",
                    "!".yellow(),
                    dir,
                    skill.dir_name
                );
            }
        }

        // hooks
        if let Some(h) = &plugin.hooks {
            if h.valid_json {
                println!("{} [{}] hooks.json valid", "✓".green(), dir);
            } else {
                println!("{} [{}] hooks.json invalid JSON", "✗".red(), dir);
            }
        }

        // mcp
        if let Some(valid) = plugin.mcp_valid_json {
            if valid {
                println!("{} [{}] mcp-servers/config.json valid", "✓".green(), dir);
            } else {
                println!(
                    "{} [{}] mcp-servers/config.json invalid JSON",
                    "✗".red(),
                    dir
                );
            }
        }

        // empty
        let is_empty =
            plugin.skills.is_empty() && plugin.hooks.is_none() && plugin.mcp_servers.is_empty();
        if is_empty {
            println!("{} [{}] plugin has no content", "!".yellow(), dir);
        }
    }

    println!();
    println!(
        "Checks passed: {}/{}",
        result.checks_passed, result.checks_total
    );

    if !result.warnings.is_empty() {
        println!();
        println!("Warnings:");
        for w in &result.warnings {
            println!("  {} {}", "!".yellow(), w);
        }
    }

    if !result.issues.is_empty() {
        println!();
        println!("Errors:");
        for e in &result.issues {
            println!("  {} {}", "✗".red(), e);
        }
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Creates a full plugin fixture under `<root>/plugins/<name>/`.
    fn create_plugin_fixture(
        root: &Path,
        name: &str,
        manifest_name: Option<&str>, // None = no plugin.json
        skills: &[(&str, Option<&str>, Option<&str>)], // (dir, fm_name, fm_desc)
        hooks_json: Option<&str>,    // None = no file
        mcp_json: Option<&str>,      // None = no file
    ) {
        let plugin_dir = root.join("plugins").join(name);
        fs::create_dir_all(&plugin_dir).unwrap();

        // manifest
        if let Some(mname) = manifest_name {
            let mp = plugin_dir.join(".claude-plugin");
            fs::create_dir_all(&mp).unwrap();
            let json = serde_json::json!({
                "name": mname,
                "version": "1.0.0",
                "description": "Test plugin"
            });
            fs::write(mp.join("plugin.json"), json.to_string()).unwrap();
        }

        // skills
        for (skill_dir, fm_name, fm_desc) in skills {
            let sd = plugin_dir.join("skills").join(skill_dir);
            fs::create_dir_all(&sd).unwrap();
            let mut content = String::new();
            if fm_name.is_some() || fm_desc.is_some() {
                content.push_str("---\n");
                if let Some(n) = fm_name {
                    content.push_str(&format!("name: {}\n", n));
                }
                if let Some(d) = fm_desc {
                    content.push_str(&format!("description: {}\n", d));
                }
                content.push_str("---\n");
            }
            fs::write(sd.join("SKILL.md"), content).unwrap();
        }

        // hooks
        if let Some(h) = hooks_json {
            let hd = plugin_dir.join("hooks");
            fs::create_dir_all(&hd).unwrap();
            fs::write(hd.join("hooks.json"), h).unwrap();
        }

        // mcp
        if let Some(m) = mcp_json {
            let md = plugin_dir.join("mcp-servers");
            fs::create_dir_all(&md).unwrap();
            fs::write(md.join("config.json"), m).unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // Scan tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_scan_marketplace_basic() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(
            tmp.path(),
            "my-plugin",
            Some("my-plugin"),
            &[("sample", Some("sample"), Some("A sample skill"))],
            Some(r#"{"hooks": {"PreToolUse": []}}"#),
            Some(r#"{"mcpServers": {"my-server": {"type": "http"}}}"#),
        );

        let m = scan_marketplace(tmp.path());
        assert_eq!(m.plugins.len(), 1);
        let p = &m.plugins[0];
        assert_eq!(p.dir_name, "my-plugin");
        assert!(p.manifest.is_some());
        assert_eq!(p.skills.len(), 1);
        assert_eq!(p.skills[0].name, "sample");
        assert_eq!(p.skills[0].description.as_deref(), Some("A sample skill"));
        assert!(p.skills[0].has_frontmatter);
        assert!(p.hooks.is_some());
        let hooks = p.hooks.as_ref().unwrap();
        assert!(hooks.valid_json);
        assert_eq!(hooks.hook_count, 1);
        assert_eq!(p.mcp_servers.len(), 1);
        assert_eq!(p.mcp_servers[0].name, "my-server");
        assert_eq!(p.mcp_valid_json, Some(true));
    }

    #[test]
    fn test_scan_marketplace_no_manifest() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(tmp.path(), "no-manifest", None, &[], None, None);

        let m = scan_marketplace(tmp.path());
        assert_eq!(m.plugins.len(), 1);
        assert!(m.plugins[0].manifest.is_none());
    }

    #[test]
    fn test_scan_marketplace_empty() {
        let tmp = TempDir::new().unwrap();
        // Create empty plugins/ dir
        fs::create_dir_all(tmp.path().join("plugins")).unwrap();

        let m = scan_marketplace(tmp.path());
        assert_eq!(m.plugins.len(), 0);
    }

    #[test]
    fn test_scan_marketplace_no_plugins_dir() {
        let tmp = TempDir::new().unwrap();
        // No plugins/ dir at all
        let m = scan_marketplace(tmp.path());
        assert_eq!(m.plugins.len(), 0);
    }

    #[test]
    fn test_scan_marketplace_invalid_mcp_json() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(
            tmp.path(),
            "bad-mcp",
            Some("bad-mcp"),
            &[],
            None,
            Some("this is not json {{{"),
        );

        let m = scan_marketplace(tmp.path());
        assert_eq!(m.plugins.len(), 1);
        assert_eq!(m.plugins[0].mcp_valid_json, Some(false));
        assert!(m.plugins[0].mcp_servers.is_empty());
    }

    // -----------------------------------------------------------------------
    // Validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_marketplace_all_ok() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(
            tmp.path(),
            "ok-plugin",
            Some("ok-plugin"),
            &[("skill-a", Some("skill-a"), Some("does something"))],
            Some(r#"{"hooks": {}}"#),
            Some(r#"{"mcpServers": {"srv": {"type": "stdio"}}}"#),
        );

        let m = scan_marketplace(tmp.path());
        let r = validate_marketplace(&m);
        assert!(r.issues.is_empty(), "expected no issues: {:?}", r.issues);
        assert!(
            r.warnings.is_empty(),
            "expected no warnings: {:?}",
            r.warnings
        );
        assert_eq!(r.checks_passed, r.checks_total);
    }

    #[test]
    fn test_validate_marketplace_missing_manifest() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(
            tmp.path(),
            "no-json",
            None,
            &[("skill-x", Some("skill-x"), None)],
            None,
            None,
        );

        let m = scan_marketplace(tmp.path());
        let r = validate_marketplace(&m);
        assert!(
            r.issues.iter().any(|i| i.contains("missing plugin.json")),
            "issues: {:?}",
            r.issues
        );
    }

    #[test]
    fn test_validate_marketplace_name_mismatch() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(
            tmp.path(),
            "dir-name",
            Some("different-name"),
            &[("s", Some("s"), Some("desc"))],
            None,
            None,
        );

        let m = scan_marketplace(tmp.path());
        let r = validate_marketplace(&m);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("does not match directory name")),
            "warnings: {:?}",
            r.warnings
        );
    }

    #[test]
    fn test_validate_marketplace_empty_plugin() {
        let tmp = TempDir::new().unwrap();
        create_plugin_fixture(
            tmp.path(),
            "empty-plugin",
            Some("empty-plugin"),
            &[],  // no skills
            None, // no hooks
            None, // no mcp
        );

        let m = scan_marketplace(tmp.path());
        let r = validate_marketplace(&m);
        assert!(
            r.warnings
                .iter()
                .any(|w| w.contains("no skills, hooks, or MCP servers")),
            "warnings: {:?}",
            r.warnings
        );
    }
}
