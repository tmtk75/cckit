# `cckit skill stale` Design

Surface skills that have not fired recently (or ever) by mining Skill tool invocations
from Claude Code transcripts, alongside an origin classification (self-made vs external)
so the user can decide what to prune.

## Table of Contents

- [Why](#why)
- [Command](#command)
- [Firing history source](#firing-history-source)
- [Skill-name matching](#skill-name-matching)
- [Origin classification](#origin-classification)
- [Output](#output)
- [Architecture](#architecture)
- [Testing](#testing)
- [Edge cases](#edge-cases)
- [Out of scope](#out-of-scope)

## Why

Skills accumulate; many never fire again. There is no way to see which skills are dead
weight. Claude Code records every Skill tool call in its transcripts
(`~/.claude/projects/**/*.jsonl`) with a skill name and timestamp, so we can compute each
skill's last-fired date and flag stale ones. Pairing that with an origin label
(self-made vs external) tells the user whether to delete (self) or uninstall (external).

## Command

```
cckit skill stale [--days N] [--all] [--json]
```

- `--days N` — staleness threshold in days (default **90**). A skill whose last firing is
  older than N days is `stale`; one with no firing record is `never`.
- `--all` — also list `active` skills (fired within N days). Default shows only
  `stale` + `never`.
- `--json` — machine-readable output.

## Firing history source

Scan `~/.claude/projects/**/*.jsonl` line by line. A Skill invocation line contains:

```json
{"name":"Skill","input":{"skill":"<name>","args":...}, ... ,"timestamp":"2026-05-12T06:55:19.111Z"}
```

For each line that contains a Skill tool_use, extract `input.skill` and the line's
`timestamp`. Aggregate per skill name: `fire_count` and `last_fired` (max timestamp).
Parsing is tolerant — lines without both fields are skipped.

This reuses the transcript root resolution from `history::loader` (`~/.claude/projects`)
but needs its own lightweight line scan (the history loader extracts conversation text,
not tool calls), implemented in a new `history::skill_usage` module so it is unit-testable
and keeps `cli.rs` focused.

## Skill-name matching

The invocation name and the installed skill identity do not always coincide:

- plain: `daily-report` == directory name.
- frontmatter name ≠ dir name: invocation `skill-best-practices-reviewer` (dir) vs ls
  name `reviewing-skill-best-practices` (frontmatter).
- namespaced (plugin/marketplace): `superpowers:brainstorming`, `my-gbrain:gbrain`,
  `lambda-zip.release-lambda-function`.

A firing record matches an installed skill when the invocation name equals any of:
its **directory name**, its **frontmatter `name`**, or — for namespaced invocations —
the segment after the last `:` or `.`. Build a lookup from each installed skill to its
set of candidate keys and join on that. Firing records that match nothing are still
counted in a "history-only" bucket (for visibility) but are not listed as skills.

## Origin classification

Per the user's choice (option c — structural markers only), each installed skill gets an
`origin` of:

- `external:marketplace` — symlink to `~/.agents/skills` (existing `detect_skill_origin`).
- `external:installed` — has `.claude-plugin/plugin.json`.
- `self` — none of the above (plain directory skill, treated as self-made).

Only structural markers count as external. A GitHub-URL heuristic was considered and
rejected: SKILL.md bodies routinely contain example/work `github.com/...` URLs (issue
links, command samples), so matching on them misclassified self-made skills wholesale.
Plugin-bundled skills are enumerated separately (via `scan_plugins`) and do not reach this
classifier.

## Output

Default (`stale` + `never`), sorted by last-fired ascending (never first, then oldest):

```
Skills not fired in 90+ days (threshold: 90d)

never    self              asc-promo-copy
never    external:plugin   loop
stale    self              configuring-mise-for-terraform   last: 2026-01-04 (146d)   3x
stale    external:github   cupertino                        last: 2026-02-10 (109d)   1x
...

Summary: 12 never, 8 stale, 50 active (hidden; use --all).
```

Columns: status (`never`/`stale`/`active`), origin, skill name, last-fired date +
days-ago (omitted for `never`), fire count. `--json` emits an array of
`{name, origin, status, last_fired, days_ago, fire_count, dir}`.

Colors: `never` red, `stale` yellow, `active` dimmed/green; origin dimmed.

## Architecture

- **`src/history/skill_usage.rs`** (new module):
  - `pub struct SkillFiring { pub last_fired: DateTime<Utc>, pub count: u32 }`
  - `pub fn parse_skill_firings_line(line: &str) -> Option<(String, DateTime<Utc>)>` —
    pure: extract `(skill_name, timestamp)` from one JSONL line, or None. Unit-tested.
  - `pub fn aggregate_firings(root: &Path) -> HashMap<String, SkillFiring>` — walk
    `*.jsonl`, fold lines via `parse_skill_firings_line`.
  - `pub fn scan_skill_firings() -> HashMap<String, SkillFiring>` — default root
    (`~/.claude/projects`).
- **`src/cli.rs`**:
  - `fn classify_skill_origin_label(skill_dir: &Path) -> String` — map to the 5 labels,
    adding the GitHub-owner heuristic over `detect_skill_origin`. Pure-ish (reads SKILL.md).
  - `fn skill_match_keys(dir_name, frontmatter_name) -> Vec<String>` — candidate keys for
    matching a firing name. Pure, unit-tested.
  - `fn skill_stale_command(days, all, json)` — enumerate installed skills (reuse the
    `collect_skill_remove_targets`-style global+project walk, but include all origins),
    join with firings, compute status, print/JSON.
  - New `SkillCommands::Stale { days, all, json }` variant + dispatch.

## Testing

- `parse_skill_firings_line`: a real Skill line → `Some((name, ts))`; a non-Skill line →
  None; a line missing `timestamp` → None; namespaced name preserved verbatim.
- `skill_match_keys`: dir==name → one key; dir≠name → both; namespaced `a:b`/`a.b` → adds
  trailing segment.
- `classify_skill_origin_label`: github.com/tmtk75 URL → `self`; github.com/other →
  `external:github`; plain dir → `self`.
- Staleness status from (last_fired, now, days) is computed by a small pure helper
  `firing_status(last_fired: Option<DateTime<Utc>>, now, days) -> &str` → tested for
  never/stale/active.

`cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` must pass.

## Edge cases

- Skill fired under a name matching nothing installed (renamed/removed) → counted in a
  history-only total, not listed.
- Same skill reachable as global and `project:~` → deduped by directory (as in
  `skill remove`).
- Transcript depth limits history: "never" means "no record in retained transcripts", not
  "never used ever" — noted in output footer.
- Timestamp parse failure on a line → that line skipped, others still counted.
- Namespaced invocation whose trailing segment collides with another skill's dir name →
  may over-credit; acceptable for a maintenance heuristic (documented).

## Out of scope

- Deleting/uninstalling stale skills — that's `skill remove` / `claude plugin uninstall`
  / `npx @anthropic/skills remove`. `stale` only reports.
- Counting skills auto-triggered without going through the Skill tool (not in transcripts).
- Per-project breakdown of where a skill fired.
