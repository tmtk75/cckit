# Auto-Focus Specification

## Purpose

Automatically bring the cckit app window to the front when Claude requires user input.

## Trigger Patterns (3 types)

| # | Pattern | State Transition | Default Delay | Configurable |
|---|---------|-----------------|---------------|-------------|
| 1 | **Permission request** | → `AwaitingApproval` | **3s** | Yes |
| 2 | **AskUserQuestion** | (detection method TBD) | **0s (immediate)** | Yes |
| 3 | **Task complete (waiting for input)** | `Running`/`AwaitingApproval` → `WaitingInput` | **3s** | Yes |

### Delay Rationale

- **Permission request (3s)**: Many requests are auto-approved, so wait before bringing window forward
- **AskUserQuestion (0s)**: Claude is explicitly asking a question; notify immediately
- **Task complete (3s)**: Prevent frequent window activation for short-lived tasks

## Controls

- **Granularity**: Per-project ON/OFF toggle
- **Delay settings**: Configurable per pattern (managed via config file)
- **Notification method**: Window bring-to-front only (no sound or macOS notifications)

## Not Yet Implemented (deferred)

- AskUserQuestion detection (not detectable with current Hook events)
  - Future consideration: Claude Code Hook extensions or stdout monitoring

## Current Implementation (reference)

- `bring_window_to_front()`: `window.rs` (`orderFrontRegardless` + `activateIgnoringOtherApps`)
- Timer: state change detection via `update_sessions_and_redraw()` every 2 seconds
- AF disabled storage: `~/.local/share/cckit/af_disabled.json`
- UI toggle: `f` key (per-project when a row is selected, bulk toggle when none selected)
