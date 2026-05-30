# Apply `disable_paths` to mcp commands

Make the existing `disable_paths` config (already honored by the main `ls` command)
also exclude projects from `cckit mcp ls`, `mcp prune`, and `mcp remove`, and let
patterns use a leading `~/`.

## Why

Nested project checkouts (e.g. `~/.ghq/.../my-biz-workspace/projects/corona/echonet-lite`)
appear as noise in `cckit mcp ls`. The config already has `disable_paths`
(glob/prefix matching via `is_path_disabled`, read by `load_cckit_config` from
`./config.toml` or `~/.config/cckit/config.toml`), but it is only applied to the main
project `ls` — not to the mcp subcommands. Extend it so an ignored path disappears from
every mcp view and is never pruned/removed.

## Changes

1. **Tilde expansion** — `is_path_disabled` expands a leading `~/` in each pattern to the
   home dir before matching, so users can write `~/.ghq/...` instead of an absolute
   `/Users/...` path. Patterns without `~` are unaffected (backward compatible).
2. **Apply in mcp enumeration** — skip projects where `is_path_disabled(path, &cfg.disable_paths)`
   in the three project-enumeration sites:
   - `mcp_ls_command` — section "4. Project MCP servers".
   - `compute_mcp_prune_plan` — the per-project loop and the dead-path collection.
   - `collect_mcp_remove_targets` — the project loop.

   Each loads config via the existing `load_cckit_config()` and reuses `is_path_disabled`.
   The Duplicates section and counts in `mcp ls` follow automatically (entries are filtered
   before aggregation).

## Config

Global `~/.config/cckit/config.toml`:

```toml
disable_paths = [
    "~/.ghq/github.com/kiicorp/my-biz-workspace/projects/corona/echonet-lite",
]
```

### Config search path fix

On macOS `dirs::config_dir()` resolves to `~/Library/Application Support`, so
`load_cckit_config` never looked at the conventional `~/.config/cckit/config.toml`.
Add `~/.config/cckit/config.toml` (XDG-style) to the candidate list so the file works
where users expect it. New priority:

1. `./config.toml` (cwd)
2. `~/.config/cckit/config.toml` (XDG)
3. platform config dir (`dirs::config_dir()/cckit/config.toml`; macOS native fallback)

## Testing

Unit-test `is_path_disabled`:
- prefix match (absolute pattern),
- glob match (`*` pattern),
- tilde-expanded pattern matches the corresponding absolute path,
- non-matching path returns false.

`cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` must pass.

## Out of scope

- Auto-ignoring nested projects by heuristic — explicit config patterns only.
- Applying `disable_paths` to non-mcp commands beyond the existing main `ls`.
