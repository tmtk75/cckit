# `cckit skill remove` Design

Replace the advisory `cckit skill how-to-remove` with `cckit skill remove`, which
actually removes directory-based skills by moving them to a reversible trash, instead
of printing manual `rm -rf` instructions.

## Table of Contents

- [Why](#why)
- [Command](#command)
- [Targets](#targets)
- [Trash mechanism](#trash-mechanism)
- [Selection and output](#selection-and-output)
- [Architecture](#architecture)
- [Testing](#testing)
- [Edge cases](#edge-cases)
- [Out of scope](#out-of-scope)

## Why

`cckit skill how-to-remove` only prints manual steps (`rm -rf ~/.claude/skills/<dir>`),
so deleting a skill is a hand-run, irreversible `rm -rf`. `skill remove` does it safely:
matching directory skills are **moved to a trash directory** (reversible), with the same
dry-run/`--execute` model as `mcp remove`. `how-to-remove` is removed entirely.

## Command

```
cckit skill remove [-f <pat>] [--scope global|project] [--execute]
```

- `-f, --filter <pat>` — substring match on skill name.
- `--scope <global|project>` — restrict to one scope; omitted = both.
- **Dry-run by default** — list what would be moved to trash and exit.
- `--execute` — perform the moves.

No `--no-backup`: the trash *is* the backup (every removal is reversible). There is no
hard-delete mode.

The `SkillCommands::HowToRemove` variant and `skill_how_to_remove_command` are deleted.

## Targets

Only **directory-based** skills are acted on:

| Scope | Source | Action |
|-------|--------|--------|
| global | `~/.claude/skills/<dir>` (has `SKILL.md`, **not a symlink**) | move dir to trash |
| project | `<project>/.claude/skills/<dir>` (not a symlink) | move dir to trash |
| marketplace | symlink → `~/.agents/skills/<name>` | **not touched** (dry-run prints a note: `npx @anthropic/skills remove <name>`) |
| plugin | bundled in an installed plugin | **not touched** (note: `claude plugin uninstall <plugin>`) |

A skill is **removable** when its path is a real directory (`!is_symlink()`), contains
`SKILL.md`, and `detect_skill_origin` is not `marketplace`/`symlink`. Plugin skills come
from `scan_plugins`, not from the scanned skills directories, so they are never
enumerated as targets.

Project enumeration honors `disable_paths` (consistency with `mcp`/main `ls`): skills in
an ignored project path are skipped.

## Trash mechanism

- Trash root: `<data_dir>/trash/`, where
  `data_dir = dirs::data_local_dir().unwrap_or(~/.local/share).join("cckit")`
  (macOS: `~/Library/Application Support/cckit/trash/`).
- Destination for a skill dir named `<dirname>`: `trash/<dirname>`. On name collision,
  append a numeric suffix: `<dirname>-2`, `<dirname>-3`, … (first free name wins).
- Move via `fs::rename` (home and data dir share a volume on macOS). On rename error,
  print the error and skip that skill — never silently lose it.
- The trash is not auto-pruned; restoring is a manual `mv` back. (Out of scope: a
  `skill trash` management command.)

## Selection and output

`--filter` targets **every** matching removable skill. Output is sorted by skill name.

Dry-run example (`cckit skill remove -f foo`):

```
2 skills match "foo" (will move to trash)

  - foo-helper   (personal) [global]   ~/.claude/skills/foo-helper
  - foo-utils    (no author) [project:~/.ghq/.../proj]   .../proj/.claude/skills/foo-utils

Note: marketplace skills (symlinks) and plugin skills are not removed here.
      marketplace -> npx @anthropic/skills remove <name>;  plugin -> claude plugin uninstall <plugin>

Run with --execute to move them to trash.
```

Execute summary:

```
Done: moved 2 skill(s) to ~/Library/Application Support/cckit/trash/.
```

Colors follow existing conventions (`cyan` counts, `red`/`dimmed` items, `green`
success, `yellow` notes). When nothing matches: dimmed "No matching skills.".

## Architecture

All in `src/cli.rs`. Reuse `scan_skills_with_paths`, `detect_skill_origin`,
`load_claude_config`, `load_cckit_config`, `is_path_disabled`, `shorten_path`,
`parse_frontmatter`.

- **`SkillRemoveTarget`** (struct): `{ name, origin, scope_label, dir: PathBuf }` — one
  removable directory skill.
- **`collect_skill_remove_targets(scope: Option<&str>) -> Vec<SkillRemoveTarget>`**:
  enumerate global (`~/.claude/skills`) and project (`<proj>/.claude/skills`) skills,
  keep only removable directory skills, applying `disable_paths` for projects. Dedup
  project skills by (name, content-hash) like `how-to-remove` does today.
- **`trash_dest(trash_dir: &Path, dir_name: &str, exists: F) -> PathBuf`** (pure, with an
  injected existence predicate): compute the collision-free destination. Unit-tested.
- **`skill_remove_command(filter, scope, execute)`**: filter, sort, print (dry-run), and
  on `--execute` create the trash dir and `fs::rename` each target, printing a summary.

Wire-up:
- Replace `HowToRemove { filter }` in `enum SkillCommands` with
  `Remove { filter, scope, execute }`.
- Replace the `SkillCommands::HowToRemove` dispatch arm with `SkillCommands::Remove`.
- Delete `skill_how_to_remove_command`.

## Testing

Unit-test the pure helper `trash_dest`:
- No collision → `trash/<name>`.
- Collision on `<name>` → `trash/<name>-2`.
- Collision on `<name>` and `<name>-2` → `trash/<name>-3`.

(The existence check is injected so the test needs no filesystem.)

`cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` must pass.

## Edge cases

- A skill dir whose `SKILL.md` is missing → not a skill, skipped.
- Symlink (marketplace/other) → excluded from targets; only mentioned in the note.
- Same project skill reachable from two project paths (duplicate checkout) → deduped by
  (name, content-hash).
- Trash name collision → numeric suffix, never overwrites an existing trashed skill.
- `fs::rename` failure (e.g. cross-device) → error printed, that skill skipped, others
  continue.
- `--scope` value outside `global|project` → no matches; prints "No matching skills.".

## Out of scope

- Removing marketplace skills or their shared `~/.agents/skills` targets.
- Uninstalling plugins / plugin-bundled skills.
- Trash management (listing/restoring/emptying) — manual `mv` for now.
- Hard delete (no `fs::remove_dir_all`).
