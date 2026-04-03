# Marketplace Subcommand Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `cckit marketplace summary <path>` and `cckit marketplace doctor <path>` to validate and inspect custom plugin marketplaces.

**Architecture:** New `src/marketplace.rs` module containing all scan/validate logic. CLI subcommand group defined in `src/cli.rs`. Reuses existing `parse_frontmatter()` from cli.rs (made `pub(crate)`).

**Tech Stack:** Rust, clap (derive), serde_json, colored

---

### Task 1: Make `parse_frontmatter` accessible from other modules

**Files:**
- Modify: `src/cli.rs:473` (change visibility)

- [ ] **Step 1: Change `parse_frontmatter` to `pub(crate)`**

In `src/cli.rs:473`, change:
```rust
fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
```
to:
```rust
pub(crate) fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
```

- [ ] **Step 2: Run tests to verify no breakage**

Run: `cargo test`
Expected: All existing tests pass.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "refactor: make parse_frontmatter pub(crate) for reuse"
```

---

### Task 2: Create `src/marketplace.rs` with data types and scan logic

**Files:**
- Create: `src/marketplace.rs`
- Modify: `src/lib.rs` (add module)

- [ ] **Step 1: Write tests for marketplace scanning**

Create `src/marketplace.rs` with test module at the bottom. Tests use a temp directory with fixture data:

```rust
use crate::cli::parse_frontmatter;
use colored::Colorize;
use serde::Deserialize;
use std::fs;
use std::path::Path;

// -- Data types --

#[derive(Debug)]
pub struct Marketplace {
    pub name: String,
    pub plugins: Vec<MarketplacePlugin>,
}

#[derive(Debug)]
pub struct MarketplacePlugin {
    pub dir_name: String,
    pub manifest: Option<PluginManifest>,
    pub skills: Vec<PluginSkill>,
    pub hooks: Option<HooksInfo>,
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Debug, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    pub description: String,
}

#[derive(Debug)]
pub struct PluginSkill {
    pub dir_name: String,
    pub name: String,
    pub description: Option<String>,
    pub has_frontmatter: bool,
}

#[derive(Debug)]
pub struct HooksInfo {
    pub valid_json: bool,
    pub hook_count: usize,
}

#[derive(Debug, Deserialize)]
pub struct McpServer {
    pub name: String,
    pub server_type: String,
}

// -- Scan functions (stubs for now) --

pub fn scan_marketplace(path: &Path) -> Marketplace {
    todo!()
}

pub fn summary_command(path: &Path) {
    todo!()
}

pub fn doctor_command(path: &Path) {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_plugin_fixture(base: &Path, name: &str, manifest: Option<&str>, skill: Option<(&str, &str)>, hooks: Option<&str>, mcp: Option<&str>) {
        let plugin_dir = base.join("plugins").join(name);
        if let Some(m) = manifest {
            let manifest_dir = plugin_dir.join(".claude-plugin");
            fs::create_dir_all(&manifest_dir).unwrap();
            fs::write(manifest_dir.join("plugin.json"), m).unwrap();
        }
        if let Some((skill_name, content)) = skill {
            let skill_dir = plugin_dir.join("skills").join(skill_name);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(skill_dir.join("SKILL.md"), content).unwrap();
        }
        if let Some(h) = hooks {
            let hooks_dir = plugin_dir.join("hooks");
            fs::create_dir_all(&hooks_dir).unwrap();
            fs::write(hooks_dir.join("hooks.json"), h).unwrap();
        }
        if let Some(m) = mcp {
            let mcp_dir = plugin_dir.join("mcp-servers");
            fs::create_dir_all(&mcp_dir).unwrap();
            fs::write(mcp_dir.join("config.json"), m).unwrap();
        }
    }

    #[test]
    fn test_scan_marketplace_basic() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        create_plugin_fixture(
            base,
            "my-plugin",
            Some(r#"{"name": "my-plugin", "version": "1.0.0", "description": "Test plugin"}"#),
            Some(("my-skill", "---\nname: my-skill\ndescription: A test skill\n---\n# Content")),
            Some(r#"{"hooks": {}}"#),
            Some(r#"{"mcpServers": {"ctx7": {"type": "http", "url": "https://example.com"}}}"#),
        );

        let marketplace = scan_marketplace(base);
        assert_eq!(marketplace.plugins.len(), 1);

        let plugin = &marketplace.plugins[0];
        assert_eq!(plugin.dir_name, "my-plugin");
        assert!(plugin.manifest.is_some());

        let manifest = plugin.manifest.as_ref().unwrap();
        assert_eq!(manifest.name, "my-plugin");
        assert_eq!(manifest.version, "1.0.0");

        assert_eq!(plugin.skills.len(), 1);
        assert_eq!(plugin.skills[0].name, "my-skill");
        assert_eq!(plugin.skills[0].description, Some("A test skill".to_string()));
        assert!(plugin.skills[0].has_frontmatter);

        assert!(plugin.hooks.is_some());
        let hooks = plugin.hooks.as_ref().unwrap();
        assert!(hooks.valid_json);
        assert_eq!(hooks.hook_count, 0);

        assert_eq!(plugin.mcp_servers.len(), 1);
        assert_eq!(plugin.mcp_servers[0].name, "ctx7");
        assert_eq!(plugin.mcp_servers[0].server_type, "http");
    }

    #[test]
    fn test_scan_marketplace_no_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        // Plugin dir exists but no plugin.json
        fs::create_dir_all(base.join("plugins/broken-plugin")).unwrap();

        let marketplace = scan_marketplace(base);
        assert_eq!(marketplace.plugins.len(), 1);
        assert!(marketplace.plugins[0].manifest.is_none());
    }

    #[test]
    fn test_scan_marketplace_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();
        fs::create_dir_all(base.join("plugins")).unwrap();

        let marketplace = scan_marketplace(base);
        assert!(marketplace.plugins.is_empty());
    }

    #[test]
    fn test_scan_marketplace_no_plugins_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let marketplace = scan_marketplace(tmp.path());
        assert!(marketplace.plugins.is_empty());
    }
}
```

- [ ] **Step 2: Add `tempfile` dev-dependency to Cargo.toml**

Add under `[dev-dependencies]`:
```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Register the module in lib.rs**

In `src/lib.rs`, add:
```rust
pub mod marketplace;
```

- [ ] **Step 4: Run tests — expect failures (todo!)**

Run: `cargo test marketplace`
Expected: FAIL — `todo!()` panics. This confirms tests are wired up.

- [ ] **Step 5: Implement `scan_marketplace`**

Replace the `scan_marketplace` stub in `src/marketplace.rs`:

```rust
pub fn scan_marketplace(path: &Path) -> Marketplace {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let plugins_dir = path.join("plugins");
    let mut plugins = Vec::new();

    if !plugins_dir.exists() {
        return Marketplace { name, plugins };
    }

    let mut entries: Vec<_> = fs::read_dir(&plugins_dir)
        .unwrap_or_else(|_| panic!("Cannot read {}", plugins_dir.display()))
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let dir_path = entry.path();
        let dir_name = entry.file_name().to_string_lossy().to_string();

        let manifest = read_manifest(&dir_path);
        let skills = scan_plugin_skills(&dir_path);
        let hooks = read_hooks(&dir_path);
        let mcp_servers = read_mcp_servers(&dir_path);

        plugins.push(MarketplacePlugin {
            dir_name,
            manifest,
            skills,
            hooks,
            mcp_servers,
        });
    }

    Marketplace { name, plugins }
}

fn read_manifest(plugin_dir: &Path) -> Option<PluginManifest> {
    let path = plugin_dir.join(".claude-plugin/plugin.json");
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

fn scan_plugin_skills(plugin_dir: &Path) -> Vec<PluginSkill> {
    let skills_dir = plugin_dir.join("skills");
    let mut results = Vec::new();

    if !skills_dir.exists() {
        return results;
    }

    let mut entries: Vec<_> = fs::read_dir(&skills_dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    entries.sort_by_key(|e| e.file_name());

    for entry in entries {
        let skill_dir = entry.path();
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let skill_md = skill_dir.join("SKILL.md");

        if skill_md.exists() {
            if let Ok(content) = fs::read_to_string(&skill_md) {
                let (name, description) = parse_frontmatter(&content);
                let has_frontmatter = name.is_some();
                results.push(PluginSkill {
                    dir_name: dir_name.clone(),
                    name: name.unwrap_or_else(|| dir_name),
                    description,
                    has_frontmatter,
                });
            }
        } else {
            results.push(PluginSkill {
                dir_name: dir_name.clone(),
                name: dir_name,
                description: None,
                has_frontmatter: false,
            });
        }
    }
    results
}

fn read_hooks(plugin_dir: &Path) -> Option<HooksInfo> {
    let path = plugin_dir.join("hooks/hooks.json");
    let content = fs::read_to_string(&path).ok()?;
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Some(HooksInfo { valid_json: false, hook_count: 0 }),
    };
    let hook_count = json
        .get("hooks")
        .and_then(|h| h.as_object())
        .map(|obj| obj.len())
        .unwrap_or(0);
    Some(HooksInfo { valid_json: true, hook_count })
}

fn read_mcp_servers(plugin_dir: &Path) -> Vec<McpServer> {
    let path = plugin_dir.join("mcp-servers/config.json");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let servers = match json.get("mcpServers").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return Vec::new(),
    };
    servers
        .iter()
        .map(|(name, val)| McpServer {
            name: name.clone(),
            server_type: val
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string(),
        })
        .collect()
}
```

- [ ] **Step 6: Run tests — expect all pass**

Run: `cargo test marketplace`
Expected: All 4 tests pass.

- [ ] **Step 7: Commit**

```bash
git add src/marketplace.rs src/lib.rs Cargo.toml
git commit -m "feat(marketplace): add marketplace scan logic with tests"
```

---

### Task 3: Implement `summary_command`

**Files:**
- Modify: `src/marketplace.rs`

- [ ] **Step 1: Implement `summary_command`**

Replace the `summary_command` stub:

```rust
pub fn summary_command(path: &Path) {
    let marketplace = scan_marketplace(path);

    println!(
        "{} ({} plugins)",
        marketplace.name.bold(),
        marketplace.plugins.len()
    );

    if marketplace.plugins.is_empty() {
        println!();
        println!("  {}", "(no plugins found)".dimmed());
        return;
    }

    for plugin in &marketplace.plugins {
        println!();
        match &plugin.manifest {
            Some(m) => {
                println!(
                    "  {} {} {} {}",
                    m.name.cyan(),
                    format!("v{}", m.version).dimmed(),
                    "—".dimmed(),
                    m.description
                );
            }
            None => {
                println!(
                    "  {} {}",
                    plugin.dir_name.cyan(),
                    "(no plugin.json)".red()
                );
            }
        }

        // Skills
        if plugin.skills.is_empty() {
            println!("    {}: {}", "Skills".bold(), "(none)".dimmed());
        } else {
            println!("    {}:", "Skills".bold());
            for skill in &plugin.skills {
                match &skill.description {
                    Some(desc) => println!("      {} {} {}", skill.name.green(), "—".dimmed(), desc.dimmed()),
                    None => println!("      {}", skill.name.green()),
                }
            }
        }

        // Hooks
        match &plugin.hooks {
            Some(h) if h.hook_count > 0 => {
                println!("    {}: {} configured", "Hooks".bold(), h.hook_count);
            }
            _ => {
                println!("    {}: {}", "Hooks".bold(), "(none)".dimmed());
            }
        }

        // MCP Servers
        if plugin.mcp_servers.is_empty() {
            println!("    {}: {}", "MCP Servers".bold(), "(none)".dimmed());
        } else {
            println!("    {}:", "MCP Servers".bold());
            for server in &plugin.mcp_servers {
                println!(
                    "      {} {}",
                    server.name.yellow(),
                    format!("({})", server.server_type).dimmed()
                );
            }
        }
    }
}
```

- [ ] **Step 2: Verify build**

Run: `cargo build`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/marketplace.rs
git commit -m "feat(marketplace): implement summary_command"
```

---

### Task 4: Implement `doctor_command`

**Files:**
- Modify: `src/marketplace.rs`

- [ ] **Step 1: Write test for doctor validation logic**

Add to the `tests` module in `src/marketplace.rs`:

```rust
#[test]
fn test_validate_marketplace_all_ok() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    create_plugin_fixture(
        base,
        "good-plugin",
        Some(r#"{"name": "good-plugin", "version": "1.0.0", "description": "A good plugin"}"#),
        Some(("my-skill", "---\nname: my-skill\ndescription: A skill\n---\n# Content")),
        Some(r#"{"hooks": {}}"#),
        Some(r#"{"mcpServers": {"s1": {"type": "http", "url": "https://example.com"}}}"#),
    );

    let marketplace = scan_marketplace(base);
    let result = validate_marketplace(&marketplace);
    assert!(result.issues.is_empty(), "Expected no issues, got: {:?}", result.issues);
}

#[test]
fn test_validate_marketplace_missing_manifest() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    fs::create_dir_all(base.join("plugins/broken")).unwrap();

    let marketplace = scan_marketplace(base);
    let result = validate_marketplace(&marketplace);
    assert!(!result.issues.is_empty());
    assert!(result.issues.iter().any(|i| i.contains("plugin.json")));
}

#[test]
fn test_validate_marketplace_name_mismatch() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    create_plugin_fixture(
        base,
        "dir-name",
        Some(r#"{"name": "different-name", "version": "1.0.0", "description": "Mismatch"}"#),
        None,
        None,
        None,
    );

    let marketplace = scan_marketplace(base);
    let result = validate_marketplace(&marketplace);
    assert!(result.warnings.iter().any(|w| w.contains("name mismatch")));
}

#[test]
fn test_validate_marketplace_empty_plugin() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    create_plugin_fixture(
        base,
        "empty-plugin",
        Some(r#"{"name": "empty-plugin", "version": "1.0.0", "description": "Empty"}"#),
        None,
        None,
        None,
    );

    let marketplace = scan_marketplace(base);
    let result = validate_marketplace(&marketplace);
    assert!(result.warnings.iter().any(|w| w.contains("no skills, hooks, or MCP servers")));
}
```

- [ ] **Step 2: Run tests — expect failures (validate_marketplace not defined)**

Run: `cargo test marketplace`
Expected: FAIL — `validate_marketplace` not found.

- [ ] **Step 3: Implement `validate_marketplace` and `doctor_command`**

Add to `src/marketplace.rs`:

```rust
#[derive(Debug)]
pub struct ValidationResult {
    pub issues: Vec<String>,
    pub warnings: Vec<String>,
    pub checks_passed: usize,
    pub checks_total: usize,
}

pub fn validate_marketplace(marketplace: &Marketplace) -> ValidationResult {
    let mut issues = Vec::new();
    let mut warnings = Vec::new();
    let mut passed = 0;
    let mut total = 0;

    for plugin in &marketplace.plugins {
        // Check 1: plugin.json existence
        total += 1;
        if plugin.manifest.is_none() {
            issues.push(format!(
                "{}/.claude-plugin/plugin.json not found or invalid",
                plugin.dir_name
            ));
        } else {
            passed += 1;
        }

        // Check 2: plugin.json name consistency
        if let Some(manifest) = &plugin.manifest {
            total += 1;
            if manifest.name != plugin.dir_name {
                warnings.push(format!(
                    "{}: name mismatch — directory '{}' vs plugin.json name '{}'",
                    plugin.dir_name, plugin.dir_name, manifest.name
                ));
            } else {
                passed += 1;
            }
        }

        // Check 3: SKILL.md frontmatter
        for skill in &plugin.skills {
            total += 1;
            if !skill.has_frontmatter {
                warnings.push(format!(
                    "{}/skills/{}/SKILL.md: missing frontmatter (name, description)",
                    plugin.dir_name, skill.dir_name
                ));
            } else {
                passed += 1;
            }
        }

        // Check 4: hooks.json validity
        if let Some(hooks) = &plugin.hooks {
            total += 1;
            if !hooks.valid_json {
                issues.push(format!(
                    "{}/hooks/hooks.json: invalid JSON",
                    plugin.dir_name
                ));
            } else {
                passed += 1;
            }
        }

        // Check 5: empty plugin warning
        if plugin.manifest.is_some()
            && plugin.skills.is_empty()
            && plugin.hooks.as_ref().map_or(true, |h| h.hook_count == 0)
            && plugin.mcp_servers.is_empty()
        {
            warnings.push(format!(
                "{} has no skills, hooks, or MCP servers",
                plugin.dir_name
            ));
        }
    }

    ValidationResult {
        issues,
        warnings,
        checks_passed: passed,
        checks_total: total,
    }
}

pub fn doctor_command(path: &Path) {
    let marketplace = scan_marketplace(path);

    println!(
        "{}: {}",
        "cckit Marketplace Doctor".bold(),
        path.display()
    );
    println!();

    if marketplace.plugins.is_empty() {
        println!("  {}", "No plugins found in plugins/ directory".yellow());
        return;
    }

    // Run checks with inline output
    for plugin in &marketplace.plugins {
        // plugin.json
        print!("Checking {}/plugin.json ... ", plugin.dir_name);
        match &plugin.manifest {
            Some(m) => {
                println!("{}", "ok".green());
                // Name consistency
                print!("Checking {}/plugin.json name consistency ... ", plugin.dir_name);
                if m.name == plugin.dir_name {
                    println!("{}", "ok".green());
                } else {
                    println!("{}", "mismatch".yellow());
                }
            }
            None => {
                println!("{}", "not found".red());
            }
        }

        // Skills
        for skill in &plugin.skills {
            print!(
                "Checking {}/skills/{}/SKILL.md frontmatter ... ",
                plugin.dir_name, skill.dir_name
            );
            if skill.has_frontmatter {
                println!("{}", "ok".green());
            } else {
                println!("{}", "missing".yellow());
            }
        }

        // Hooks
        if let Some(hooks) = &plugin.hooks {
            print!("Checking {}/hooks/hooks.json ... ", plugin.dir_name);
            if hooks.valid_json {
                println!("{}", "ok".green());
            } else {
                println!("{}", "invalid JSON".red());
            }
        }

        // MCP servers
        if !plugin.mcp_servers.is_empty() {
            print!("Checking {}/mcp-servers/config.json ... ", plugin.dir_name);
            println!("{}", "ok".green());
        }
    }

    // Validation summary
    let result = validate_marketplace(&marketplace);

    println!();

    if result.issues.is_empty() && result.warnings.is_empty() {
        println!(
            "{} All {} plugins passed all checks!",
            "✓".green(),
            marketplace.plugins.len()
        );
    } else {
        if !result.issues.is_empty() {
            println!("{}", "Issues:".red().bold());
            for issue in &result.issues {
                println!("  {} {}", "✗".red(), issue);
            }
            println!();
        }

        if !result.warnings.is_empty() {
            println!("{}", "Warnings:".yellow().bold());
            for warning in &result.warnings {
                println!("  {} {}", "!".yellow(), warning);
            }
            println!();
        }

        let ok_count = marketplace.plugins.len()
            - marketplace
                .plugins
                .iter()
                .filter(|p| p.manifest.is_none())
                .count();
        println!(
            "{} {} of {} plugins passed all checks",
            "✓".green(),
            ok_count,
            marketplace.plugins.len()
        );
    }

    if !result.issues.is_empty() {
        std::process::exit(1);
    }
}
```

- [ ] **Step 4: Run tests — all should pass**

Run: `cargo test marketplace`
Expected: All 8 tests pass (4 from Task 2 + 4 new).

- [ ] **Step 5: Commit**

```bash
git add src/marketplace.rs
git commit -m "feat(marketplace): implement doctor_command with validation"
```

---

### Task 5: Wire up CLI subcommands in `src/cli.rs`

**Files:**
- Modify: `src/cli.rs` (add Marketplace variant to Commands enum + dispatch)

- [ ] **Step 1: Add MarketplaceCommands enum and Commands variant**

After the existing `AgentCommands` subcommand enum in `src/cli.rs`, add:

```rust
#[derive(Subcommand)]
enum MarketplaceCommands {
    /// Show all plugins, skills, hooks, and MCP servers in a marketplace
    Summary {
        /// Path to the marketplace root directory
        path: String,
    },

    /// Validate marketplace plugin structure and consistency
    Doctor {
        /// Path to the marketplace root directory
        path: String,
    },
}
```

Add to the `Commands` enum (after the `Agent` variant):

```rust
    /// Inspect and validate a custom plugin marketplace
    Marketplace {
        #[command(subcommand)]
        command: MarketplaceCommands,
    },
```

- [ ] **Step 2: Add dispatch in the `run()` function**

In the match block in `run()` (after the `Agent` match arm around line 6078), add:

```rust
        Some(Commands::Marketplace { command }) => match command {
            MarketplaceCommands::Summary { path } => {
                crate::marketplace::summary_command(Path::new(&path));
            }
            MarketplaceCommands::Doctor { path } => {
                crate::marketplace::doctor_command(Path::new(&path));
            }
        },
```

- [ ] **Step 3: Build and verify**

Run: `cargo build`
Expected: Build succeeds.

- [ ] **Step 4: Manual smoke test with vrp-hub**

Run: `cargo run -- marketplace summary /Users/tomotaka/.ghq/github.com/kiicorp/vrp-hub`
Expected: Output showing 3 plugins with their skills, hooks, MCP servers.

Run: `cargo run -- marketplace doctor /Users/tomotaka/.ghq/github.com/kiicorp/vrp-hub`
Expected: Doctor output with checks and any warnings.

- [ ] **Step 5: Run full test suite**

Run: `cargo test`
Expected: All tests pass.

Run: `cargo clippy -- -D warnings`
Expected: No warnings.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs
git commit -m "feat(marketplace): wire up marketplace summary and doctor subcommands"
```

---

### Task 6: Handle invalid JSON in mcp-servers/config.json

The current `read_mcp_servers` silently ignores invalid JSON. Doctor should detect this.

**Files:**
- Modify: `src/marketplace.rs`

- [ ] **Step 1: Add test for invalid mcp-servers JSON**

Add to test module:

```rust
#[test]
fn test_validate_marketplace_invalid_mcp_json() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();

    create_plugin_fixture(
        base,
        "bad-mcp",
        Some(r#"{"name": "bad-mcp", "version": "1.0.0", "description": "Bad MCP"}"#),
        None,
        None,
        Some("not valid json {{{"),
    );

    let marketplace = scan_marketplace(base);
    let plugin = &marketplace.plugins[0];
    assert!(plugin.mcp_valid_json.is_some());
    assert!(!plugin.mcp_valid_json.unwrap());
}
```

- [ ] **Step 2: Add `mcp_valid_json` field to `MarketplacePlugin`**

```rust
#[derive(Debug)]
pub struct MarketplacePlugin {
    pub dir_name: String,
    pub manifest: Option<PluginManifest>,
    pub skills: Vec<PluginSkill>,
    pub hooks: Option<HooksInfo>,
    pub mcp_servers: Vec<McpServer>,
    pub mcp_valid_json: Option<bool>,  // None = no config.json, Some(true) = valid, Some(false) = invalid
}
```

Update `read_mcp_servers` to return `(Vec<McpServer>, Option<bool>)`:

```rust
fn read_mcp_servers(plugin_dir: &Path) -> (Vec<McpServer>, Option<bool>) {
    let path = plugin_dir.join("mcp-servers/config.json");
    let content = match fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return (Vec::new(), None),
    };
    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return (Vec::new(), Some(false)),
    };
    let servers = match json.get("mcpServers").and_then(|s| s.as_object()) {
        Some(s) => s,
        None => return (Vec::new(), Some(true)),
    };
    let list = servers
        .iter()
        .map(|(name, val)| McpServer {
            name: name.clone(),
            server_type: val
                .get("type")
                .and_then(|t| t.as_str())
                .unwrap_or("unknown")
                .to_string(),
        })
        .collect();
    (list, Some(true))
}
```

Update `scan_marketplace` to use the new return type:

```rust
let (mcp_servers, mcp_valid_json) = read_mcp_servers(&dir_path);

plugins.push(MarketplacePlugin {
    dir_name,
    manifest,
    skills,
    hooks,
    mcp_servers,
    mcp_valid_json,
});
```

Add to `validate_marketplace`:

```rust
        // Check 6: mcp-servers/config.json validity
        if let Some(valid) = plugin.mcp_valid_json {
            total += 1;
            if !valid {
                issues.push(format!(
                    "{}/mcp-servers/config.json: invalid JSON",
                    plugin.dir_name
                ));
            } else {
                passed += 1;
            }
        }
```

Update `doctor_command` MCP section:

```rust
        // MCP servers
        if let Some(valid) = plugin.mcp_valid_json {
            print!("Checking {}/mcp-servers/config.json ... ", plugin.dir_name);
            if valid {
                println!("{}", "ok".green());
            } else {
                println!("{}", "invalid JSON".red());
            }
        }
```

- [ ] **Step 3: Run tests — all should pass**

Run: `cargo test marketplace`
Expected: All 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add src/marketplace.rs
git commit -m "feat(marketplace): detect invalid JSON in mcp-servers/config.json"
```
