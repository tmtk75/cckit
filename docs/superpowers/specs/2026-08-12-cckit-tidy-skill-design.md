# `cckit-tidy` Skill Design

An interactive cleanup skill that walks the user through stale skills and MCP residue
surfaced by cckit, deleting only what the user explicitly approves.

## Table of Contents

- [Why](#why)
- [Trigger](#trigger)
- [Scope](#scope)
- [Flow](#flow)
- [Data sources and commands](#data-sources-and-commands)
- [Presentation](#presentation)
- [Decision UX](#decision-ux)
- [Execution](#execution)
- [Safety](#safety)
- [Location and packaging](#location-and-packaging)
- [Out of scope](#out-of-scope)

## Why

`cckit skill stale`, `cckit skill remove`, `cckit mcp prune`, and `cckit mcp remove` each
solve one piece of a cleanup workflow, but the user still has to remember which command
to run, in what order, with which flags, and has to reason case-by-case about whether a
skill or MCP is really safe to drop. That friction means people never actually run the
cleanup. A skill that stitches the same commands into a guided flow — evidence up
front, one decision per item, execution only after approval — makes the cleanup
something the user actually does, not something they file away for later.

Auditing (option 1 in brainstorming) was rejected because `cckit skill stale --json`
already covers that need. Fully automated cleanup (option 3) was rejected because the
decision "do I still want this skill" is context-dependent (rarely-fired but
irreplaceable skills exist) and mis-removals are annoying to reverse even with the
trash directory.

## Trigger

The skill's `description` frontmatter must include these phrases so Claude Code
auto-invokes it on user requests:

- Japanese: 「cckit片付け」「cckit整理」「skill整理」「mcp掃除」「skillとmcpの棚卸し」
- English: "cckit tidy", "tidy cckit skills and mcps", "clean up unused skills and mcps"

## Scope

Two categories, in this fixed order:

1. **Stale skills** — from `cckit skill stale` (default threshold: 90 days).
2. **MCP residue** — from `cckit mcp prune` (default is dry-run; orphaned
   `enabledMcpjsonServers` / `disabledMcpjsonServers` approvals and empty `.mcp.json`
   files).

Explicitly **out of scope for v1**:

- Judging whether an MCP *server definition* is unused. cckit has no transcript-based
  usage signal for MCPs today; asking "do you use this MCP?" without evidence is
  low-value and high-risk. Revisit once cckit exposes MCP invocation history.
- Cleaning up agents, commands, or plugins.

## Flow

1. **Preflight**
   - Run `cckit --version` to confirm cckit is on `PATH`. If not, print an install hint
     and stop.
   - Run `cckit skill stale --json` and `cckit mcp prune` in parallel; both are
     read-only (`mcp prune` requires `--execute` to write).
2. **Summarize**
   - Print totals: `N stale skills (X self, Y external:installed, Z marketplace/plugin)`
     and `M MCP residue items`.
   - If both totals are zero, congratulate the user and stop.
3. **Per-item dialogue** — skills first, then MCP residue (see [Decision UX](#decision-ux)).
4. **Batch execution** — after the user finishes deciding, group the "delete"
   decisions and execute them (see [Execution](#execution)).
5. **Report** — for each executed action print the cckit command that ran and its
   trash / backup path. End with a one-line summary
   (`Removed: 3 skills, 2 MCP residue entries. Kept: 5. Deferred: 1.`).

## Data sources and commands

| Purpose | Command | Notes |
|---------|---------|-------|
| Enumerate stale skills | `cckit skill stale --json` | Threshold fixed at cckit default (90 days) per user decision. |
| Enumerate MCP residue | `cckit mcp prune` | Default is dry-run; parse its text output. |
| Delete a skill | `cckit skill remove -f <name> --execute` | `--execute` required; moves to `<data_dir>/trash/` (reversible). |
| Apply MCP residue cleanup | `cckit mcp prune --execute` | Writes `.bak` backups. |
| Remove an MCP server definition | *(deferred)* | Not used in v1 — MCP definition removal is out of scope. |

If a future cckit release adds a JSON mode to `mcp prune`, prefer it. Until then,
parse the human-readable output; keep parsing tolerant (skip lines that don't match
the expected shape).

## Presentation

For each stale skill, show:

- Name
- Origin badge: `self`, `external:installed`, or `external:marketplace|plugin`
- Last-fired date (or `never`)
- Fire count
- For deletable origins (`self`, `external:installed`): where it lives on disk
- For non-deletable origins (`marketplace` / `plugin`): the note
  `cckitでは削除不可 — marketplace/plugin 側の設定で無効化してください` — no delete
  action offered, but the entry is still listed so the user is aware it's dormant.

For each MCP residue item, show what `cckit mcp prune --dry-run` reported (server name,
scope, reason: orphaned approval / empty `.mcp.json`).

## Decision UX

Per item, ask a single question with three options: **消す / 残す / 保留**.

- **消す (delete)** — queued for execution in the batch step.
- **残す (keep)** — no action; recorded so the summary can report it.
- **保留 (defer)** — no action this run; noted in the summary so the user knows to
  revisit.

Non-deletable marketplace/plugin skills are presented as **display-only** (no
question). The user acknowledged in brainstorming that they want to *see* them even
though cckit can't act on them.

Batch shortcuts must NOT be offered by default (「全部消す」/「全部残す」). They defeat
the interactive-safety intent. If the user asks for one mid-flow, honor it for the
current category only.

## Execution

Executed strictly after all decisions are collected, in this order:

1. **Skills marked delete**: one `cckit skill remove -f <name> --execute` invocation
   per skill (the filter must match a single skill — use the exact name from the JSON).
   Sequential (not parallel) so any failure output is easy to attribute. On non-zero
   exit, stop the batch, report which skill failed, and leave the remainder untouched
   so the user can inspect state before continuing.
2. **MCP residue** (only if at least one residue item was marked delete): a single
   `cckit mcp prune --execute` invocation. cckit's `mcp prune` is coarse — it acts on
   all residue at once, not per-item — so if the user chose "keep" for some residue
   items, warn before invoking and confirm the user still wants to proceed. If they
   don't, skip the MCP execution step and note it in the summary.

## Safety

- All reads before any write. The preflight and summary phases must complete before
  the first destructive command runs.
- Every destructive command echoed before it runs, and its output surfaced back.
- Rely on cckit's built-in reversibility: `skill remove` uses trash, `mcp prune` uses
  `.bak` files. The skill does not add its own backup layer.
- If cckit itself is missing or the JSON parse fails, stop and surface the raw output
  rather than guessing.

## Location and packaging

- Path: `.claude/skills/cckit-tidy/SKILL.md` in the cckit repo (project-scoped),
  matching the existing `.claude/skills/release-cckit/` pattern.
- Ships with cckit so anyone cloning the repo (or installing cckit-as-a-plugin, when
  that lands) gets it automatically.
- No scripts, references, or supporting files in v1. The SKILL.md itself carries the
  full flow.

## Out of scope

- MCP server-definition removal (see [Scope](#scope)).
- Configurable staleness threshold — v1 is fixed at cckit's 90-day default.
- Batch "delete all" shortcuts.
- Non-macOS behavior differences (cckit's data dir differs, but the commands are the
  same).
- Undo command. Recovery is via cckit's trash / `.bak` files plus manual restoration;
  document that in the SKILL.md but do not automate it.
