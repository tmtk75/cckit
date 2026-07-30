# Window App: Show Last-Used Model per Session

## Background

The macOS window app (`cckit app`) already stores the last-used model name for each session in `Session.model` (populated by `src/monitor/hook.rs` from Claude Code / Codex hook payloads). The value is used to derive `AgentType` (Claude/Codex/Gemini) for accent color, but it is not shown in the UI. Users cannot tell at a glance whether a running session is on Opus 4.7, Sonnet 4.5, GPT-5, etc.

## Goal

Display a short, human-readable label of the last-used model on each session card in the window app.

Out of scope: TUI (`session ls`), menubar, hover popover, session store schema changes, hook wiring.

## UI change

In `src/monitor/window.rs`, each session card's Row 1 is currently:

```
●  Claude • project_name                                     10s  50%
```

Change the agent-family label (`Claude`/`Codex`/`Gemini`/`Agent`) to the short model label when one is available:

```
●  Opus 4.7 • project_name                                   10s  50%
```

Fallback when `session.model` is `None` or unrecognized: keep the current family label (`Claude` / `Codex` / `Gemini` / `Agent`). No layout changes; column widths stay the same. Truncation on Row 1 is already handled by `cg_draw_text_truncated`.

Accent color continues to be driven by `AgentType::from_model` — unchanged.

## Short-label mapping

New pure function in `src/monitor/theme.rs`:

```rust
/// Return a short, human-readable label for a model id (e.g. "Opus 4.7").
/// Returns None if the id is unknown; caller falls back to the AgentType family label.
pub fn model_short_label(model: &str) -> Option<String>;
```

Rules (case-insensitive match on the raw id):

| Pattern                                       | Label        |
|-----------------------------------------------|--------------|
| contains `opus-4-8`                           | `Opus 4.8`   |
| contains `opus-4-7`                           | `Opus 4.7`   |
| contains `opus-4`                             | `Opus 4`     |
| contains `opus-3` or standalone `opus`        | `Opus`       |
| contains `sonnet-4-5`                         | `Sonnet 4.5` |
| contains `sonnet-4`                           | `Sonnet 4`   |
| contains `sonnet-3-5` or `3-5-sonnet`         | `Sonnet 3.5` |
| contains `sonnet`                             | `Sonnet`     |
| contains `haiku-4-5`                          | `Haiku 4.5`  |
| contains `haiku`                              | `Haiku`      |
| starts with `gpt-5`                           | `GPT-5`      |
| starts with `gpt-4o`                          | `GPT-4o`     |
| starts with `gpt-`                            | `GPT`        |
| contains `codex-mini`                         | `Codex mini` |
| starts with `codex-`                          | `Codex`      |
| contains `gemini-2.5`                         | `Gemini 2.5` |
| contains `gemini-2.0`                         | `Gemini 2.0` |
| contains `gemini-1.5`                         | `Gemini 1.5` |
| contains `gemini`                             | `Gemini`     |
| otherwise                                     | `None`       |

Version tokens are matched with digits joined by `-` (as in the raw model id, e.g. `claude-opus-4-7-20241022`) rather than with dots — dot-form ids (`gemini-2.0-flash`) match by literal substring above. No `[1m]`/context suffix handling; keep the label short.

Ordering matters: longer/more specific patterns come first (`opus-4-7` before `opus-4` before `opus`).

## Call site

In `src/monitor/window.rs` near line 1382 where `agent_label_text` is currently computed:

```rust
let agent_label_text: String = session
    .model
    .as_deref()
    .and_then(theme::model_short_label)
    .unwrap_or_else(|| {
        match agent {
            AgentType::Claude => "Claude",
            AgentType::Codex => "Codex",
            AgentType::Gemini => "Gemini",
            AgentType::Unknown => "Agent",
        }
        .to_string()
    });
let row1_text = format!("{} \u{2022} {}", agent_label_text, project);
```

## Tests

Unit tests in `theme.rs` for `model_short_label`:

- `claude-opus-4-7-20241022` → `Some("Opus 4.7")`
- `claude-opus-4-8` → `Some("Opus 4.8")`
- `claude-sonnet-4-5-20250101` → `Some("Sonnet 4.5")`
- `claude-3-5-sonnet-20241022` → `Some("Sonnet 3.5")`
- `claude-haiku-4-5-20251001` → `Some("Haiku 4.5")`
- `gpt-5-turbo` → `Some("GPT-5")`
- `gpt-4o` → `Some("GPT-4o")`
- `codex-mini-latest` → `Some("Codex mini")`
- `gemini-2.0-flash` → `Some("Gemini 2.0")`
- `some-future-model-x` → `None`
- empty string → `None`

No integration test for the window drawing (existing UI code has no rendering tests).

## Verification

- `cargo test` passes (new unit tests + existing).
- `cargo clippy -- -D warnings` clean.
- `cargo fmt --check` clean.
- Manual: `mise run build-app`, open a session on a known model, confirm short label renders on Row 1 and matches the model in use. Confirm fallback when a stopped session has no `model`.

## Non-goals / follow-ups

- No changes to menubar, TUI, hover popover, sessions.json schema, or hook parsing.
- If short-label coverage misses a future model, add a rule to `model_short_label`. The fallback preserves current behavior — no crash, just family name.
