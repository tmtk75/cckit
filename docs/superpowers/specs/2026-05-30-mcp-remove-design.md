# `cckit mcp remove` Design

Replace the advisory `cckit mcp how-to-remove` with `cckit mcp remove`, which actually
deletes MCP server definitions across scopes instead of printing manual edit steps.

## Table of Contents

- [Why](#why)
- [Command](#command)
- [Scopes and removal mechanics](#scopes-and-removal-mechanics)
- [Selection semantics](#selection-semantics)
- [Behavior](#behavior)
- [Output](#output)
- [Architecture](#architecture)
- [Testing](#testing)
- [Edge cases](#edge-cases)
- [Out of scope](#out-of-scope)

## Why

`cckit mcp how-to-remove` only prints manual instructions ("Edit `<path>/.mcp.json` and
remove `notion` from mcpServers"). When a server like `notion` is configured in 7
projects, the user must hand-edit 7 files — tedious and error-prone.

`cckit mcp remove` does the deletion directly: it finds every matching server across
scopes and removes it, with the same dry-run/`--execute`/backup safety model as
`cckit mcp prune`. `how-to-remove` is removed entirely; `remove`'s dry-run output
serves the same "show me what would change" purpose.

## Command

```
cckit mcp remove [-f <pat>] [--scope <global|user|project>] [--execute] [--no-backup]
```

- `-f, --filter <pat>` — substring match on server name (same semantics as the old
  `how-to-remove --filter`).
- `--scope <global|user|project>` — restrict to one scope. Omitted = all scopes.
  Guards against accidentally nuking the wrong layer.
- **Dry-run by default** — list what would be removed and exit without writing.
- `--execute` — actually apply the removals.
- `--no-backup` — skip the per-file `.bak` backup (backups are created by default
  under `--execute`).

The `HowToRemove` variant and `mcp_how_to_remove_command` are deleted.

## Scopes and removal mechanics

Mirrors the four scopes `how-to-remove` enumerated:

| Scope | Source of truth | Removal action |
|-------|-----------------|----------------|
| project | `<project>/.mcp.json` `mcpServers` | Remove the key. If the file becomes empty (`is_empty_mcp_json`), delete the file. |
| user | `~/.claude.json` top-level `mcpServers` | Remove the key only. Never delete the file. |
| global | `~/.claude/.mcp.json` `mcpServers` | Remove the key. If the file becomes empty, delete the file. |
| plugin | plugin-bundled `.mcp.json` | **Not auto-removable.** Print the `claude plugin uninstall <name>` hint and skip. |

Local scope (`~/.claude.json` `projects[path].mcpServers`) is **not** a target — it was
not enumerated by `how-to-remove` either. The dry-run output notes that local-scope
servers are handled by `claude mcp remove -s local`.

## Selection semantics

`--filter` matches a substring of the server name and targets **every matching entry
across all in-scope locations** (e.g. `--filter notion` targets all 7 project files at
once). When the same server lives in multiple `.mcp.json` files, each file is rewritten
exactly once (entries grouped by file, like `prune`).

With no `--filter`, every removable server in scope is targeted — the dry-run default
makes this safe to inspect first.

## Behavior

1. Enumerate entries across global / user / project (and plugin, for reporting) — reuse
   the enumeration logic from the current `how-to-remove`, producing
   `(name, server_type, scope, file_path, removable)` records.
2. Apply `--filter` (substring on name) and `--scope` (exact scope class) filters.
3. **Dry-run:** print the matching entries grouped by scope section; plugin entries are
   shown as skipped with their uninstall hint; exit.
4. **`--execute`:**
   - Group removable entries by target file.
   - For each `.mcp.json` (project/global): back up (`<file>.bak` unless `--no-backup`),
     remove the key(s); if now empty, delete the file, else write it back
     pretty-printed.
   - For `~/.claude.json` (user): back up, remove the key(s) from top-level
     `mcpServers`, write back.
   - Plugin entries are skipped (counted, hint already shown).
   - Print a summary: entries removed, files changed, empty `.mcp.json` deleted, plugin
     entries skipped.

When nothing matches, print a dimmed "No matching MCP servers." and exit 0.

## Output

Dry-run example (`cckit mcp remove -f notion`):

```
7 entries match "notion" (will remove)

Project (7)
  - notion (http)   ~/.ghq/github.com/kiicorp/vrp-hub/.mcp.json
  - notion (http)   ~/.ghq/github.com/kiicorp/isms/.mcp.json
  ...

Run with --execute to remove.
```

If a plugin entry matches:

```
Plugin (1)  [skipped]
  - foo (stdio)   -> run `claude plugin uninstall <plugin>`
```

Execute summary:

```
Done: removed 7 entr(ies) from 7 file(s), 4 empty .mcp.json deleted, 0 plugin skipped.
```

Colors follow existing conventions (`cyan` counts, `red`/`dimmed` removed items,
`green` success, `yellow` notes).

## Architecture

All in `src/cli.rs`. Reuse `is_empty_mcp_json` and `backup_file` from the prune feature,
and the existing enumeration helpers (`parse_mcp_json`, `parse_user_mcp_servers`,
`load_plugin_mcp_definitions`, `load_claude_config`, `shorten_path`).

- **`McpRemoveTarget`** (struct): one removable/reportable entry —
  `{ name, server_type, scope_label, scope_class, file: Option<PathBuf>, plugin: Option<String> }`.
  `scope_class` is one of `global | user | project | plugin` for filtering/sectioning.
- **`collect_mcp_remove_targets() -> Vec<McpRemoveTarget>`**: enumeration across scopes
  (extracted from the current `how-to-remove` body, but producing structured targets
  with concrete file paths instead of instruction strings).
- **`remove_mcp_server_key(json: &mut serde_json::Value, name: &str) -> bool`** (pure):
  remove `name` from the top-level `mcpServers` object; returns true if a key was
  removed. Unit-tested.
- **`mcp_remove_command(filter, scope, execute, no_backup)`**: filter, print (dry-run),
  and on `--execute` perform grouped file mutation using `remove_mcp_server_key`,
  `is_empty_mcp_json`, and `backup_file`.

Wire-up:
- Replace the `HowToRemove { filter }` variant in `enum McpCommands` with
  `Remove { filter, scope, execute, no_backup }`.
- Replace the `McpCommands::HowToRemove` dispatch arm with `McpCommands::Remove`.
- Delete `mcp_how_to_remove_command`.

## Testing

Unit tests target the pure helper `remove_mcp_server_key`:

- Removing an existing key returns `true` and drops it from `mcpServers`.
- Removing a missing key returns `false` and leaves `mcpServers` unchanged.
- After removing the last key, `is_empty_mcp_json` reports the object as empty (verifies
  the empty-file-deletion path integrates correctly).
- A JSON value with no `mcpServers` object returns `false`.

Follow TDD (red → green). `cargo test`, `cargo clippy -- -D warnings`, and
`cargo fmt --check` must pass.

## Edge cases

- Same server in N project files → grouped, each file rewritten once.
- A `.mcp.json` that becomes empty after removal → deleted (file, after backup).
- `~/.claude.json` → only the `mcpServers` key is touched; the file is never deleted.
- Plugin-scope match → never written; reported with `claude plugin uninstall` hint.
- `--scope` value outside `global|user|project` → no matches (filtered out); the
  command still runs safely (prints "No matching MCP servers.").
- Malformed / unreadable JSON for a target file → skip that file silently on execute
  (consistent with existing scan/prune behavior).
- Backup failure for a file → skip mutating that file (do not lose data), continue with
  the rest.

## Out of scope

- Removing local-scope servers (`~/.claude.json` `projects[path].mcpServers`) — use
  `claude mcp remove -s local`.
- Uninstalling plugins — `remove` only prints the hint.
- Moving/promoting servers between scopes — separate concern.
