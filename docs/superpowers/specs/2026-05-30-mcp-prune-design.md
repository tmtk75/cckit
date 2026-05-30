# `cckit mcp prune` Design

Remove stale MCP residue left behind in Claude Code config files after an MCP server has been removed from its real definition.

## Table of Contents

- [Why](#why)
- [Command](#command)
- [Definition of "stale"](#definition-of-stale)
- [Prune targets](#prune-targets)
- [Behavior](#behavior)
- [Output](#output)
- [Architecture](#architecture)
- [Testing](#testing)
- [Edge cases](#edge-cases)
- [Out of scope](#out-of-scope)

## Why

When an MCP server is removed from its real definition (a project `.mcp.json`, user
scope, or local scope), Claude Code leaves residue behind:

- The per-project `.claude/settings.local.json` keeps the server name in its
  `enabledMcpjsonServers` (or `disabledMcpjsonServers`) approval array.
- A `.mcp.json` whose last server was removed is left as an empty
  `{"mcpServers": {}}` file.

`cckit mcp ls` reads `enabledMcpjsonServers` (via `scan_local_mcp_servers`) and
surfaces these names as phantom `(local)` entries that no longer correspond to any
real server. There is no way to clean them in bulk: `cckit prune` only removes dead
project paths, and `cckit tidy-up --mcp-only` is advisory (it reports duplicates and
missing binaries but does not fix anything).

`cckit mcp prune` fills that gap: it detects and removes MCP residue across all
projects.

## Command

```
cckit mcp prune [--execute] [--no-backup]
```

Mirrors the existing `cckit prune` ergonomics:

- **Dry-run by default** — prints what would be removed and exits without writing.
- `--execute` — actually apply the changes.
- `--no-backup` — skip the per-file `.bak` backup (backups are created by default
  when `--execute` is used).

## Definition of "stale"

A server name is **stale for a project** when it is absent from that project's
**live set**. The live set is the union of every place an MCP server can be really
defined:

- User scope: `~/.claude.json` top-level `mcpServers`
- Global: `~/.claude/.mcp.json` `mcpServers`
- Installed plugin MCP definitions (`load_plugin_mcp_definitions`)
- The project's `.mcp.json` `mcpServers`
- The project's local scope: `~/.claude.json` `projects[path].mcpServers`

A name present in any of these is a real server and is never pruned. Only names that
appear *exclusively* in approval arrays or in an emptied `.mcp.json` are residue.

## Prune targets

| # | Target | Action |
|---|--------|--------|
| A | Stale names in `.claude/settings.local.json` → `enabledMcpjsonServers` | Remove the name from the array. Keep the (possibly empty) array and the file — it holds unrelated settings such as `permissions`. |
| B | Stale names in `.claude/settings.local.json` → `disabledMcpjsonServers` | Same as A. |
| C | A `.mcp.json` whose only meaningful content is an empty `mcpServers` | Delete the file. "Only meaningful content" = top-level keys are exactly `{"mcpServers"}` and `mcpServers` is `{}`. |
| D | Non-existent project paths in `~/.claude.json` `projects` | **Report only.** Print the count and hint to run `cckit prune`. Do not delete here — that is the existing command's responsibility. |

## Behavior

1. Enumerate projects from `~/.claude.json` `projects` keys (same source as
   `collect_all_mcp_servers`).
2. Build the global part of the live set once (user scope ∪ global `.mcp.json` ∪
   plugins).
3. For each project, extend the live set with that project's `.mcp.json` names and
   local-scope names, then detect stale entries (A/B) and empty `.mcp.json` (C).
4. Collect dead project paths (D).
5. **Dry-run:** print all findings grouped by category and exit.
6. **`--execute`:**
   - For each file to modify (A/B) or delete (C), create `<file>.bak` first unless
     `--no-backup`.
   - Rewrite `settings.local.json` with the pruned arrays (pretty-printed, preserving
     other keys and key order via `serde_json` `preserve_order`).
   - Delete empty `.mcp.json` files.
   - Print a summary of what was changed.

When nothing is stale, print a green "Nothing to prune." line and exit 0.

## Output

Dry-run example:

```
Scanning 41 projects for stale MCP residue...

Stale enabled approvals (3)
  - chrome-devtools   ~/.ghq/github.com/tmtk75/moomoo-openapi-demo/.claude/settings.local.json
  - chrome-devtools   ~/.ghq/github.com/kiicorp/tank-infra/.claude/settings.local.json
  - chrome-devtools   ~/.ghq/github.com/tmtk75/blog/.claude/settings.local.json

Empty .mcp.json files (1)
  - ~/.ghq/github.com/tmtk75/pet-care-depot/.mcp.json

Dead project paths (1)  -> run `cckit prune` to remove

Run with --execute to apply.
```

Colors follow the existing convention (`yellow` counts, `red`/`dimmed` removed
items, `cyan` for the `--execute` hint, `green` for success).

## Architecture

New code in `src/cli.rs`, reusing existing helpers
(`parse_user_mcp_servers`, `parse_mcp_json`, `load_plugin_mcp_definitions`,
`load_claude_config`, `shorten_path`).

- **`McpPrunePlan`** (struct): the result of analysis — the pure data the command
  acts on. Holds:
  - `enabled_removals: Vec<StaleApproval>` (file path + server name)
  - `disabled_removals: Vec<StaleApproval>`
  - `empty_mcp_json: Vec<PathBuf>`
  - `dead_paths: Vec<String>`
- **`compute_mcp_prune_plan(config, home) -> McpPrunePlan`** (pure function): takes
  the parsed config and home dir, returns the plan. No filesystem writes, no
  printing — this is the unit-tested core. (Reads are done via small injectable
  helpers or by passing pre-read JSON so tests can drive it with fixtures.)
- **`mcp_prune_command(execute, no_backup)`**: orchestration — builds the plan,
  prints it, and on `--execute` performs the backups, writes, and deletes.

Keeping `compute_mcp_prune_plan` free of IO is the key boundary: detection logic is
tested in isolation, and the command layer only handles presentation and file
mutation.

Wire-up:
- Add a `Prune { #[arg(long)] execute: bool, #[arg(long)] no_backup: bool }` variant
  to the existing `mcp` subcommand enum.
- Dispatch to `mcp_prune_command` in the command match.

## Testing

Unit tests target `compute_mcp_prune_plan` (pure, fixture-driven):

- **A:** an `enabledMcpjsonServers` name with no live definition is flagged; a name
  present in `.mcp.json` / user / local / global / plugin is **not** flagged.
- **B:** same for `disabledMcpjsonServers`.
- **C:** `{"mcpServers": {}}` is flagged for deletion; a `.mcp.json` with a remaining
  server, or with other top-level keys, is not.
- **D:** a non-existent project path is collected; an existing one is not.
- **Live-set precedence:** a name defined only at user scope keeps every project's
  approval entry intact (not stale).

Follow TDD: write each test red first, then implement until green. `cargo test`,
`cargo clippy -- -D warnings`, and `cargo fmt --check` must pass (CI gates).

## Edge cases

- `settings.local.json` with no `enabledMcpjsonServers`/`disabledMcpjsonServers` key
  → nothing to do for that file.
- Pruning empties an approval array → leave `[]` in place (do not delete the key or
  file).
- `.mcp.json` with an empty `mcpServers` **and** other top-level keys → not deleted
  (treated as intentional config); left untouched.
- Malformed / unreadable JSON → skip that file silently (consistent with existing
  scan functions that return early on parse errors).
- Current working directory is excluded by `collect_all_mcp_servers` for `ls`, but
  `prune` operates over all projects regardless of cwd (it is a maintenance command).

## Out of scope

- Removing real (non-empty) `.mcp.json` entries or moving servers between scopes —
  that is manual / a separate `promote` feature.
- Deleting dead project paths from `~/.claude.json` — owned by `cckit prune`.
- De-duplication advice — owned by `cckit tidy-up`.
