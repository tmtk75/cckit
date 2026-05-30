# `cckit mcp remove` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `cckit mcp how-to-remove` with `cckit mcp remove`, which actually deletes matching MCP server definitions across global/user/project scopes (plugin entries are reported, not removed), with dry-run/`--execute`/backup safety.

**Architecture:** A pure helper `remove_mcp_server_key` does the key deletion and is unit-tested. `collect_mcp_remove_targets` enumerates structured removable entries (reusing existing scan helpers). `mcp_remove_command` filters, prints the dry-run, and on `--execute` mutates files grouped by path, reusing `is_empty_mcp_json` and `backup_file` from the prune feature.

**Tech Stack:** Rust (edition 2024), clap derive, serde_json (`preserve_order`), `colored`. All changes in `src/cli.rs`.

Spec: `docs/superpowers/specs/2026-05-30-mcp-remove-design.md`

---

## File Structure

- **Modify** `src/cli.rs`:
  - `enum McpCommands` (`:391` area) — replace the `HowToRemove { filter }` variant with `Remove { filter, scope, execute, no_backup }`.
  - Command dispatch (`McpCommands::HowToRemove` arm) — replace with `McpCommands::Remove`.
  - Delete `fn mcp_how_to_remove_command` (`:1860`).
  - Add `struct McpRemoveTarget`, `fn collect_mcp_remove_targets`, `fn remove_mcp_server_key`, `fn mcp_remove_command` (place near the prune functions).
  - `#[cfg(test)] mod tests` — add a unit test for `remove_mcp_server_key`.

Reuse (do not modify): `is_empty_mcp_json`, `backup_file`, `parse_mcp_json`, `parse_user_mcp_servers`, `load_plugin_mcp_definitions`, `load_claude_config`, `shorten_path`.

---

### Task 1: Pure helper `remove_mcp_server_key`

**Files:**
- Modify: `src/cli.rs` (add function next to the prune helpers, after `prune_array`)
- Test: `src/cli.rs` `mod tests`

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
#[test]
fn test_remove_mcp_server_key() {
    use serde_json::json;
    // removing an existing key returns true and drops it
    let mut v = json!({"mcpServers": {"notion": {"type": "http"}, "keep": {}}});
    assert!(remove_mcp_server_key(&mut v, "notion"));
    assert_eq!(v, json!({"mcpServers": {"keep": {}}}));

    // removing a missing key returns false, leaves it unchanged
    let mut v2 = json!({"mcpServers": {"keep": {}}});
    assert!(!remove_mcp_server_key(&mut v2, "nope"));
    assert_eq!(v2, json!({"mcpServers": {"keep": {}}}));

    // removing the last key leaves an empty object that is_empty_mcp_json flags
    let mut v3 = json!({"mcpServers": {"only": {}}});
    assert!(remove_mcp_server_key(&mut v3, "only"));
    assert!(is_empty_mcp_json(&v3));

    // no mcpServers object -> false
    let mut v4 = json!({"other": true});
    assert!(!remove_mcp_server_key(&mut v4, "x"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_remove_mcp_server_key`
Expected: FAIL — `cannot find function 'remove_mcp_server_key'`.

- [ ] **Step 3: Write minimal implementation**

Add after `prune_array`:

```rust
/// Remove `name` from the top-level `mcpServers` object. Returns true if a key was
/// actually removed.
fn remove_mcp_server_key(json: &mut serde_json::Value, name: &str) -> bool {
    json.get_mut("mcpServers")
        .and_then(|v| v.as_object_mut())
        .map(|m| m.remove(name).is_some())
        .unwrap_or(false)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_remove_mcp_server_key`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat(mcp): add remove_mcp_server_key helper"
```

---

### Task 2: Enumerate removal targets

**Files:**
- Modify: `src/cli.rs` (add `McpRemoveTarget` + `collect_mcp_remove_targets` near the prune functions)

- [ ] **Step 1: Add the struct and enumerator**

Add before `mcp_prune_command` (or after the prune helpers):

```rust
#[derive(Debug)]
struct McpRemoveTarget {
    name: String,
    server_type: String,
    /// Display label, e.g. "project:~/foo" or "user".
    scope_label: String,
    /// One of: "global", "user", "project", "plugin".
    scope_class: String,
    /// Concrete file to edit for global/user/project; None for plugin.
    file: Option<std::path::PathBuf>,
    /// Plugin name when scope_class == "plugin".
    plugin: Option<String>,
}

/// Enumerate every MCP server that `remove` can act on (global/user/project) plus
/// plugin entries (reported only).
fn collect_mcp_remove_targets() -> Vec<McpRemoveTarget> {
    let mut targets = Vec::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return targets,
    };

    // global: ~/.claude/.mcp.json
    let global_mcp = home.join(".claude/.mcp.json");
    if global_mcp.exists() {
        for s in parse_mcp_json(&global_mcp) {
            targets.push(McpRemoveTarget {
                name: s.name,
                server_type: s.server_type,
                scope_label: "global".to_string(),
                scope_class: "global".to_string(),
                file: Some(global_mcp.clone()),
                plugin: None,
            });
        }
    }

    // user: ~/.claude.json top-level mcpServers
    let user_config = home.join(".claude.json");
    if user_config.exists() {
        for s in parse_user_mcp_servers(&user_config) {
            targets.push(McpRemoveTarget {
                name: s.name,
                server_type: s.server_type,
                scope_label: "user".to_string(),
                scope_class: "user".to_string(),
                file: Some(user_config.clone()),
                plugin: None,
            });
        }
    }

    // plugin: reported only
    for (name, s) in &load_plugin_mcp_definitions() {
        let plugin_name = Path::new(&s.source)
            .parent()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        targets.push(McpRemoveTarget {
            name: name.clone(),
            server_type: s.server_type.clone(),
            scope_label: format!("plugin:{}", plugin_name),
            scope_class: "plugin".to_string(),
            file: None,
            plugin: Some(plugin_name),
        });
    }

    // project: each <dir>/.mcp.json
    if let Ok(config) = load_claude_config()
        && let Some(projects) = config.projects
    {
        for project_path in projects.keys() {
            let dir = Path::new(project_path);
            let mcp_file = dir.join(".mcp.json");
            if !dir.exists() || !mcp_file.exists() {
                continue;
            }
            let short = shorten_path(project_path);
            for s in parse_mcp_json(&mcp_file) {
                targets.push(McpRemoveTarget {
                    name: s.name,
                    server_type: s.server_type,
                    scope_label: format!("project:{}", short),
                    scope_class: "project".to_string(),
                    file: Some(mcp_file.clone()),
                    plugin: None,
                });
            }
        }
    }

    targets
}
```

- [ ] **Step 2: Verify it compiles (dead-code warning is expected until Task 3)**

Run: `cargo build 2>&1 | grep -E "error" || echo "no errors"`
Expected: `no errors` (a `never used` warning for the new items is acceptable at this
step; it goes away once `mcp_remove_command` calls them in Task 3).

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(mcp): enumerate mcp remove targets"
```

---

### Task 3: The `mcp_remove_command` (dry-run + execute)

**Files:**
- Modify: `src/cli.rs` (add `mcp_remove_command` after `collect_mcp_remove_targets`)

- [ ] **Step 1: Add the command function**

```rust
fn mcp_remove_command(
    filter: Option<String>,
    scope: Option<String>,
    execute: bool,
    no_backup: bool,
) {
    let mut targets = collect_mcp_remove_targets();

    if let Some(ref f) = filter {
        let fl = f.to_lowercase();
        targets.retain(|t| t.name.to_lowercase().contains(&fl));
    }
    if let Some(ref s) = scope {
        targets.retain(|t| &t.scope_class == s);
    }

    if targets.is_empty() {
        println!("{}", "No matching MCP servers.".dimmed());
        return;
    }

    let removable = targets.iter().filter(|t| t.file.is_some()).count();
    match &filter {
        Some(f) => println!(
            "{} entries match \"{}\" (will remove)\n",
            removable.to_string().cyan(),
            f
        ),
        None => println!("{} entries (will remove)\n", removable.to_string().cyan()),
    }

    // Sections in a stable order.
    for (label, class) in [
        ("Global", "global"),
        ("User", "user"),
        ("Project", "project"),
        ("Plugin", "plugin"),
    ] {
        let section: Vec<&McpRemoveTarget> =
            targets.iter().filter(|t| t.scope_class == class).collect();
        if section.is_empty() {
            continue;
        }
        if class == "plugin" {
            println!(
                "{} ({})  {}",
                "Plugin".bold(),
                section.len(),
                "[skipped]".yellow()
            );
            for t in &section {
                println!(
                    "  {} {:<14} -> run `claude plugin uninstall {}`",
                    "-".dimmed(),
                    t.name.bright_cyan(),
                    t.plugin.clone().unwrap_or_default()
                );
            }
            println!();
            continue;
        }
        println!("{} ({})", label.bold(), section.len());
        for t in &section {
            let loc = t
                .file
                .as_ref()
                .map(|p| shorten_path(&p.to_string_lossy()))
                .unwrap_or_default();
            println!(
                "  {} {} {}   {}",
                "-".dimmed(),
                t.name.bright_cyan(),
                format!("({})", t.server_type).dimmed(),
                loc.dimmed()
            );
        }
        println!();
    }

    println!(
        "{}",
        "Note: local-scope servers are handled by `claude mcp remove -s local`.".dimmed()
    );

    if !execute {
        println!();
        println!("Run with {} to remove.", "--execute".cyan());
        return;
    }

    // Group removable targets by file.
    use std::collections::BTreeMap;
    let mut by_file: BTreeMap<std::path::PathBuf, Vec<String>> = BTreeMap::new();
    for t in &targets {
        if let Some(file) = &t.file {
            by_file.entry(file.clone()).or_default().push(t.name.clone());
        }
    }

    let mut removed = 0usize;
    let mut files_changed = 0usize;
    let mut deleted = 0usize;
    for (file, names) in &by_file {
        let content = match fs::read_to_string(file) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let mut json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let mut local_removed = 0usize;
        for name in names {
            if remove_mcp_server_key(&mut json, name) {
                local_removed += 1;
            }
        }
        if local_removed == 0 {
            continue;
        }
        if !backup_file(file, no_backup) {
            continue;
        }
        // Delete only .mcp.json files that became empty; never delete ~/.claude.json.
        let is_dot_mcp = file.file_name().map(|n| n == "mcp.json" || n == ".mcp.json")
            .unwrap_or(false)
            || file.to_string_lossy().ends_with(".mcp.json");
        if is_dot_mcp && is_empty_mcp_json(&json) {
            if fs::remove_file(file).is_ok() {
                deleted += 1;
                removed += local_removed;
                files_changed += 1;
            }
            continue;
        }
        match serde_json::to_string_pretty(&json) {
            Ok(s) => {
                if fs::write(file, s + "\n").is_ok() {
                    removed += local_removed;
                    files_changed += 1;
                }
            }
            Err(e) => eprintln!("{}: {}", "Serialize failed".red(), e),
        }
    }

    let plugin_skipped = targets.iter().filter(|t| t.scope_class == "plugin").count();
    println!();
    println!(
        "{} removed {} entr(ies) from {} file(s), {} empty .mcp.json deleted, {} plugin skipped.",
        "Done:".green(),
        removed,
        files_changed,
        deleted,
        plugin_skipped
    );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build 2>&1 | grep -E "error" || echo "no errors"`
Expected: `no errors`.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(mcp): add mcp_remove_command (dry-run + execute)"
```

---

### Task 4: Replace the `how-to-remove` subcommand wiring

**Files:**
- Modify: `src/cli.rs` `enum McpCommands` — replace `HowToRemove` variant.
- Modify: `src/cli.rs` dispatch — replace `McpCommands::HowToRemove` arm.
- Modify: `src/cli.rs` — delete `mcp_how_to_remove_command`.

- [ ] **Step 1: Replace the enum variant**

Replace:

```rust
    /// Show how to remove/uninstall each MCP server
    HowToRemove {
        #[arg(short, long, help = "Filter MCP servers by name pattern")]
        filter: Option<String>,
    },
```

with:

```rust
    /// Remove MCP server definitions across scopes (project/user/global)
    Remove {
        #[arg(short, long, help = "Filter MCP servers by name pattern")]
        filter: Option<String>,

        #[arg(long, help = "Restrict to one scope: global, user, or project")]
        scope: Option<String>,

        #[arg(long, help = "Actually apply removals (default is dry-run)")]
        execute: bool,

        #[arg(long, help = "Skip creating .bak backups")]
        no_backup: bool,
    },
```

- [ ] **Step 2: Replace the dispatch arm**

Replace:

```rust
            McpCommands::HowToRemove { filter } => {
                mcp_how_to_remove_command(filter);
            }
```

with:

```rust
            McpCommands::Remove {
                filter,
                scope,
                execute,
                no_backup,
            } => {
                mcp_remove_command(filter, scope, execute, no_backup);
            }
```

- [ ] **Step 3: Delete `mcp_how_to_remove_command`**

Remove the entire `fn mcp_how_to_remove_command(filter: Option<String>) { ... }`
function (the one printing "Edit ... and remove ... from mcpServers").

- [ ] **Step 4: Verify it compiles cleanly (no dead code)**

Run: `cargo build 2>&1 | grep -E "error|warning: .*never used" || echo "clean"`
Expected: `clean`.

- [ ] **Step 5: Verify the CLI surface**

Run: `cargo run -- mcp remove --help`
Expected: help text with `--filter`, `--scope`, `--execute`, `--no-backup`.

Run: `cargo run -- mcp how-to-remove 2>&1 | head -3`
Expected: clap error — `how-to-remove` is no longer a recognized subcommand.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs
git commit -m "feat(mcp): replace how-to-remove with remove"
```

---

### Task 5: Final verification and docs

**Files:**
- Modify: `CLAUDE.md` (update the mcp subcommand list)

- [ ] **Step 1: Run the full CI gate**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: tests PASS, clippy clean. For `fmt`, ensure no diff is reported in `src/cli.rs`
(pre-existing diffs in other files are unrelated; fix only `cli.rs` hunks if any).

- [ ] **Step 2: Update CLAUDE.md**

In the mcp subcommand list, replace `how-to-remove` with `remove`:

> ... dedicated `skill`/`mcp`/`agent` management subcommands (`ls`, `copy`, `promote`, `prune`, `remove`, `validate`). ...

(Match exact surrounding wording; swap `how-to-remove` → `remove` in the mcp list.)

- [ ] **Step 3: Dry-run sanity check on the real machine**

Run: `cargo run -- mcp remove -f notion`
Expected: lists the 7 project `.mcp.json` files holding `notion`, ends with "Run with
--execute to remove." No file is modified.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document 'cckit mcp remove' subcommand"
```

---

## Self-Review

**Spec coverage:**
- Replace how-to-remove with remove → Task 4 (enum, dispatch, delete fn). ✓
- Command flags `--filter`/`--scope`/`--execute`/`--no-backup` → Task 4 enum, Task 3 logic. ✓
- Scope mechanics: project/global `.mcp.json` key removal + empty-file delete; user key-only; plugin skip+hint → Task 3 execute block + section printing. ✓
- Selection: filter substring, all matches, grouped by file → Task 3 (`retain`, `by_file`). ✓
- Dry-run default + execute + backups → Task 3 (`if !execute` gate, `backup_file`). ✓
- Output format → Task 3 printing. ✓
- Local scope out of scope, noted in output → Task 3 "Note:" line. ✓
- Testing `remove_mcp_server_key` (present/missing/last-key/no-object) → Task 1. ✓
- Edge cases (grouped files, empty-file delete, never delete ~/.claude.json, plugin skip, malformed JSON skip, backup failure skip) → Task 3 handles each. ✓

**Placeholder scan:** No TBD/TODO; every code step is complete.

**Type consistency:** `McpRemoveTarget` fields (`name`, `server_type`, `scope_label`,
`scope_class`, `file`, `plugin`) are referenced consistently in `collect_mcp_remove_targets`
and `mcp_remove_command`. `remove_mcp_server_key`, `is_empty_mcp_json`, `backup_file`
signatures match their definitions. `McpCommands::Remove { filter, scope, execute,
no_backup }` matches the dispatch arm and `mcp_remove_command(filter, scope, execute,
no_backup)`.
