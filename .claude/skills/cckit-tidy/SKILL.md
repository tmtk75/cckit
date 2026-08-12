---
name: cckit-tidy
description: Walk the user through stale Claude Code skills and MCP residue surfaced by cckit, deleting only what they approve. Use when the user says "cckit片付け", "cckit整理", "skill整理", "mcp掃除", "skillとmcpの棚卸し", "cckit tidy", "tidy cckit skills and mcps", or "clean up unused skills and mcps".
---

Interactive cleanup of stale skills and MCP residue via cckit.

## Preflight

1. Run `cckit --version`. If cckit is missing, tell the user to install it (`cargo install --path .` from the cckit repo, or the release binary) and stop.
2. In parallel, run both:
   - `cckit skill stale --json` — read-only, returns a JSON array
   - `cckit mcp prune` — read-only by default (writes only with `--execute`)

## Parse

**Skills** (JSON array). For each entry keep: `name`, `origin`, `status` (`stale` / `never`), `last_fired`, `days_ago`, `fire_count`, `dir`.

Split by `origin`:
- **deletable**: `origin == "self"` or `origin` starts with `"external:installed"`
- **non-deletable**: `origin` contains `marketplace` or `plugin` — cckit cannot remove these; they will be shown for awareness only

**MCP residue** (human-readable text). Sections look like:

```
Stale enabled approvals (N)
  - <server>              <path>

Stale disabled approvals (N)
  - <server>              <path>

Empty .mcp.json (N)
  - <path>
```

Parse leniently — skip lines that don't match. Preserve the section name so you can echo it back to the user.

## Summary

Print totals in one message:

```
Skills: <A> stale (<self_count> self, <ext_count> external:installed, <mkt_count> marketplace/plugin — 表示のみ)
MCP residue: <B> item(s)
```

If both totals are zero, congratulate the user and stop.

## Per-item dialogue — skills

Iterate through deletable skills. For each, present:

- Name and origin
- Last fired: `<date>` or `never`, plus `days_ago` when present
- Fire count
- Directory path
- Ask: **消す / 残す / 保留**

Then list non-deletable (marketplace/plugin) entries as a single info block with the note:

> `cckit では削除不可 — marketplace/plugin 側の設定で無効化してください`

Do not ask a question for them.

Do **not** offer batch shortcuts like「全部消す」by default. If the user asks for one mid-flow, honor it for the current category only.

## Per-item dialogue — MCP residue

Iterate through each residue item (server + path or empty `.mcp.json` path). Ask **消す / 残す / 保留**.

Note the coarseness constraint below before executing.

## Execution — skills

For each skill the user chose to 消す, run sequentially:

```
cckit skill remove -f <exact-name> --execute
```

- Echo the command before running.
- Surface stdout/stderr back to the user.
- On non-zero exit, stop the batch, report which skill failed, and leave remaining decisions unexecuted so the user can inspect state.

## Execution — MCP residue

`cckit mcp prune --execute` is coarse: it acts on **all** current residue at once, not per-item.

- If the user marked **every** residue item as 消す, run `cckit mcp prune --execute` and surface the output.
- If the user kept or deferred any residue item, warn:

  > `cckit mcp prune --execute は残す項目も一緒に消してしまうため、選択削除できません。全部削除して良ければ実行しますが、そうでなければスキップします。どうしますか？`

  If they still want to proceed, run it. Otherwise skip and note it in the summary.

## Report

At the end, print one line:

```
Removed: <n_skills> skill(s), <mcp_bool> MCP residue. Kept: <k>. Deferred: <d>.
```

Then remind the user:

- Removed skills live in `<data_dir>/trash/` (restore by moving the directory back).
- `cckit mcp prune --execute` wrote `.bak` files next to changed settings.

## Safety rules

- All reads (preflight + summary) must complete before any destructive command runs.
- Every destructive command is echoed before invocation.
- Do not add a custom backup layer — trust cckit's trash and `.bak` files.
- If cckit itself is missing or the JSON parse fails, stop and show the raw output rather than guessing.

## Out of scope for this skill

- Removing MCP **server definitions** (`cckit mcp remove`) — cckit has no MCP usage signal yet, so there is no evidence-based way to ask "do you use this MCP?".
- Configurable staleness threshold — fixed at cckit's 90-day default.
- Batch "delete all" shortcut.
- Undo automation — recovery is manual via trash / `.bak`.
