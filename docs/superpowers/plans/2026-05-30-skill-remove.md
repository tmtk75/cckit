# `cckit skill remove` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `cckit skill how-to-remove` with `cckit skill remove`, which moves matching directory-based skills to a reversible trash (dry-run/`--execute`); marketplace and plugin skills are not touched.

**Architecture:** A pure helper `trash_dest` computes collision-free trash paths (unit-tested). `collect_skill_remove_targets` enumerates removable directory skills (global + project, honoring `disable_paths`). `skill_remove_command` filters, prints the dry-run, and on `--execute` moves each target via `fs::rename`.

**Tech Stack:** Rust (edition 2024), clap derive, `colored`. All changes in `src/cli.rs`.

Spec: `docs/superpowers/specs/2026-05-30-skill-remove-design.md`

---

## File Structure

- **Modify** `src/cli.rs`:
  - `enum SkillCommands` (`:349`) — replace `HowToRemove { filter }` with `Remove { filter, scope, execute }`.
  - Dispatch (`:6907` `SkillCommands::HowToRemove`) — replace with `SkillCommands::Remove`.
  - Delete `skill_how_to_remove_command` (`:1137`).
  - Add `trash_dest`, `SkillRemoveTarget`, `collect_skill_remove_targets`, `skill_remove_command`.
  - `#[cfg(test)] mod tests` — add `test_trash_dest`.

Reuse: `scan_skills_with_paths` (`:575`), `detect_skill_origin` (`:880`), `load_claude_config`, `load_cckit_config`, `is_path_disabled`, `shorten_path`, `parse_frontmatter`.

---

### Task 1: Pure helper `trash_dest`

**Files:**
- Modify: `src/cli.rs` (add near the skill helpers, e.g. after `detect_skill_origin`)
- Test: `src/cli.rs` `mod tests`

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
#[test]
fn test_trash_dest() {
    use std::collections::HashSet;
    use std::path::Path;
    let trash = Path::new("/trash");

    // no collision
    let taken: HashSet<String> = HashSet::new();
    assert_eq!(
        trash_dest(trash, "foo", |p| taken.contains(&p.to_string_lossy().to_string())),
        Path::new("/trash/foo")
    );

    // collision on foo -> foo-2
    let taken: HashSet<String> = ["/trash/foo".to_string()].into_iter().collect();
    assert_eq!(
        trash_dest(trash, "foo", |p| taken.contains(&p.to_string_lossy().to_string())),
        Path::new("/trash/foo-2")
    );

    // collision on foo and foo-2 -> foo-3
    let taken: HashSet<String> =
        ["/trash/foo".to_string(), "/trash/foo-2".to_string()].into_iter().collect();
    assert_eq!(
        trash_dest(trash, "foo", |p| taken.contains(&p.to_string_lossy().to_string())),
        Path::new("/trash/foo-3")
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_trash_dest`
Expected: FAIL — `cannot find function 'trash_dest'`.

- [ ] **Step 3: Write minimal implementation**

Add after `detect_skill_origin`:

```rust
/// Collision-free destination under `trash_dir` for a skill named `dir_name`.
/// `exists` is injected so the logic is testable without the filesystem.
fn trash_dest<F: Fn(&Path) -> bool>(
    trash_dir: &Path,
    dir_name: &str,
    exists: F,
) -> std::path::PathBuf {
    let first = trash_dir.join(dir_name);
    if !exists(&first) {
        return first;
    }
    let mut n = 2;
    loop {
        let candidate = trash_dir.join(format!("{}-{}", dir_name, n));
        if !exists(&candidate) {
            return candidate;
        }
        n += 1;
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_trash_dest`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/cli.rs
git commit -m "feat(skill): add trash_dest helper for skill remove"
```

---

### Task 2: Enumerate removable skill targets

**Files:**
- Modify: `src/cli.rs` (add `SkillRemoveTarget` + `collect_skill_remove_targets`)

- [ ] **Step 1: Add the struct and enumerator**

Add before `skill_remove_command` (Task 3):

```rust
#[derive(Debug)]
struct SkillRemoveTarget {
    name: String,
    origin: String,
    scope_label: String,
    dir: std::path::PathBuf,
}

/// Enumerate removable directory skills (global + project), excluding symlinks
/// (marketplace) and honoring disable_paths for projects.
fn collect_skill_remove_targets(scope: Option<&str>) -> Vec<SkillRemoveTarget> {
    let mut targets = Vec::new();
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => return targets,
    };

    let want = |s: &str| scope.is_none_or(|sc| sc == s);

    // global: ~/.claude/skills/<dir>
    if want("global") {
        let global_skills = home.join(".claude/skills");
        if let Ok(entries) = fs::read_dir(&global_skills) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_symlink() || !path.join("SKILL.md").exists() {
                    continue;
                }
                let origin = detect_skill_origin(&path);
                if origin == "marketplace" || origin == "symlink" {
                    continue;
                }
                let dir_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
                let name = fs::read_to_string(path.join("SKILL.md"))
                    .ok()
                    .and_then(|c| parse_frontmatter(&c).0)
                    .unwrap_or_else(|| dir_name.clone());
                targets.push(SkillRemoveTarget {
                    name,
                    origin,
                    scope_label: "global".to_string(),
                    dir: path,
                });
            }
        }
    }

    // project: <proj>/.claude/skills/<dir>
    if want("project")
        && let Ok(config) = load_claude_config()
        && let Some(projects) = config.projects
    {
        let cckit_config = load_cckit_config();
        let mut seen: std::collections::HashSet<(String, u64)> = std::collections::HashSet::new();
        for project_path in projects.keys() {
            if is_path_disabled(project_path, &cckit_config.disable_paths) {
                continue;
            }
            let claude_dir = Path::new(project_path).join(".claude");
            if !claude_dir.join("skills").exists() {
                continue;
            }
            for src in scan_skills_with_paths(&claude_dir) {
                if src.skill_dir.is_symlink() {
                    continue;
                }
                let origin = detect_skill_origin(&src.skill_dir);
                if origin == "marketplace" || origin == "symlink" {
                    continue;
                }
                let content_hash = fs::read_to_string(src.skill_dir.join("SKILL.md"))
                    .map(|c| {
                        use std::hash::{Hash, Hasher};
                        let mut h = std::collections::hash_map::DefaultHasher::new();
                        c.hash(&mut h);
                        h.finish()
                    })
                    .unwrap_or(0);
                if !seen.insert((src.info.name.clone(), content_hash)) {
                    continue;
                }
                targets.push(SkillRemoveTarget {
                    name: src.info.name,
                    origin,
                    scope_label: format!("project:{}", shorten_path(project_path)),
                    dir: src.skill_dir,
                });
            }
        }
    }

    targets.sort_by(|a, b| a.name.cmp(&b.name).then(a.dir.cmp(&b.dir)));
    targets
}
```

- [ ] **Step 2: Verify it compiles (dead-code warning OK until Task 3)**

Run: `cargo build 2>&1 | grep -E "error" || echo "no errors"`
Expected: `no errors`.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(skill): enumerate removable skill targets"
```

---

### Task 3: The `skill_remove_command`

**Files:**
- Modify: `src/cli.rs` (add `skill_remove_command` after `collect_skill_remove_targets`)

- [ ] **Step 1: Add the command function**

```rust
fn skill_remove_command(filter: Option<String>, scope: Option<String>, execute: bool) {
    let mut targets = collect_skill_remove_targets(scope.as_deref());

    if let Some(ref f) = filter {
        let fl = f.to_lowercase();
        targets.retain(|t| t.name.to_lowercase().contains(&fl));
    }

    if targets.is_empty() {
        println!("{}", "No matching skills.".dimmed());
        return;
    }

    match &filter {
        Some(f) => println!(
            "{} skills match \"{}\" (will move to trash)\n",
            targets.len().to_string().cyan(),
            f
        ),
        None => println!(
            "{} skills (will move to trash)\n",
            targets.len().to_string().cyan()
        ),
    }

    for t in &targets {
        println!(
            "  {} {} {} {}",
            "-".dimmed(),
            t.name.bright_cyan(),
            format!("({})", t.origin).dimmed(),
            format!("[{}]", t.scope_label).dimmed()
        );
        println!("      {}", shorten_path(&t.dir.to_string_lossy()).dimmed());
    }
    println!();
    println!(
        "{}",
        "Note: marketplace (symlink) and plugin skills are not removed here.".dimmed()
    );
    println!(
        "{}",
        "      marketplace -> npx @anthropic/skills remove <name>;  plugin -> claude plugin uninstall <plugin>".dimmed()
    );

    if !execute {
        println!();
        println!("Run with {} to move them to trash.", "--execute".cyan());
        return;
    }

    let home = dirs::home_dir().unwrap_or_default();
    let trash_dir = dirs::data_local_dir()
        .unwrap_or_else(|| home.join(".local/share"))
        .join("cckit")
        .join("trash");
    if let Err(e) = fs::create_dir_all(&trash_dir) {
        eprintln!("{}: {}", "Could not create trash dir".red(), e);
        return;
    }

    let mut moved = 0usize;
    for t in &targets {
        let dir_name = t.dir.file_name().unwrap_or_default().to_string_lossy().to_string();
        let dest = trash_dest(&trash_dir, &dir_name, |p| p.exists());
        match fs::rename(&t.dir, &dest) {
            Ok(_) => moved += 1,
            Err(e) => eprintln!(
                "{} {}: {}",
                "Failed to move".red(),
                shorten_path(&t.dir.to_string_lossy()),
                e
            ),
        }
    }

    println!();
    println!(
        "{} moved {} skill(s) to {}.",
        "Done:".green(),
        moved,
        shorten_path(&trash_dir.to_string_lossy()).dimmed()
    );
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build 2>&1 | grep -E "error" || echo "no errors"`
Expected: `no errors`.

- [ ] **Step 3: Commit**

```bash
git add src/cli.rs
git commit -m "feat(skill): add skill_remove_command (dry-run + execute)"
```

---

### Task 4: Replace the `how-to-remove` wiring

**Files:**
- Modify: `src/cli.rs` `enum SkillCommands`, dispatch, and delete `skill_how_to_remove_command`.

- [ ] **Step 1: Replace the enum variant**

Replace:

```rust
    /// Show how to remove/uninstall each skill
    HowToRemove {
        #[arg(short, long, help = "Filter skills by name pattern")]
        filter: Option<String>,
    },
```

with:

```rust
    /// Remove directory skills by moving them to trash (marketplace/plugin untouched)
    Remove {
        #[arg(short, long, help = "Filter skills by name pattern")]
        filter: Option<String>,

        #[arg(long, help = "Restrict to one scope: global or project")]
        scope: Option<String>,

        #[arg(long, help = "Actually move matched skills to trash (default is dry-run)")]
        execute: bool,
    },
```

- [ ] **Step 2: Replace the dispatch arm**

Replace:

```rust
            SkillCommands::HowToRemove { filter } => {
                skill_how_to_remove_command(filter);
            }
```

with:

```rust
            SkillCommands::Remove {
                filter,
                scope,
                execute,
            } => {
                skill_remove_command(filter, scope, execute);
            }
```

- [ ] **Step 3: Delete `skill_how_to_remove_command`**

Remove the entire `fn skill_how_to_remove_command(filter: Option<String>) { ... }`.

- [ ] **Step 4: Verify clean compile (no dead code)**

Run: `cargo build 2>&1 | grep -E "error|never used" || echo "clean"`
Expected: `clean`.

- [ ] **Step 5: Verify CLI surface**

Run: `cargo run -- skill remove --help`
Expected: help with `--filter`, `--scope`, `--execute`.

Run: `cargo run -- skill how-to-remove 2>&1 | head -3`
Expected: clap error — `how-to-remove` no longer recognized.

- [ ] **Step 6: Commit**

```bash
git add src/cli.rs
git commit -m "feat(skill): replace how-to-remove with remove"
```

---

### Task 5: Final verification

**Files:** none beyond `src/cli.rs`.

- [ ] **Step 1: Full CI gate**

Run: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
Expected: tests PASS, clippy clean. Fix only `cli.rs` fmt hunks if any (pre-existing diffs
in other files are unrelated).

- [ ] **Step 2: Dry-run sanity check**

Run: `cargo run -- skill remove`
Expected: lists removable directory skills (global + project), with the marketplace/plugin
note, ending "Run with --execute to move them to trash." No filesystem change.

- [ ] **Step 3: Commit (if any fmt fixes were needed)**

```bash
git add src/cli.rs
git commit -m "style: fmt skill remove"
```

---

## Self-Review

**Spec coverage:**
- Replace how-to-remove with remove → Task 4. ✓
- Flags `--filter`/`--scope`/`--execute`, dry-run default → Task 4 enum + Task 3 logic. ✓
- Targets: global + project directory skills; exclude symlink/marketplace; plugin not enumerated → Task 2. ✓
- disable_paths on project enumeration → Task 2. ✓
- Trash move + collision suffix + rename-error skip → Task 1 (`trash_dest`) + Task 3 (`fs::rename`). ✓
- Dedup project skills by (name, content-hash) → Task 2. ✓
- Output sorted by name + marketplace/plugin note → Task 2 sort + Task 3 printing. ✓
- Testing `trash_dest` (no collision / -2 / -3) → Task 1. ✓
- Edge cases (missing SKILL.md, symlink excluded, rename failure, bad --scope) → Task 2/3. ✓

**Placeholder scan:** none. **Type consistency:** `SkillRemoveTarget { name, origin,
scope_label, dir }`, `trash_dest`, `collect_skill_remove_targets`, `skill_remove_command`
referenced consistently; `SkillCommands::Remove { filter, scope, execute }` matches the
dispatch arm and `skill_remove_command(filter, scope, execute)`.
