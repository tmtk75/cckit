# cckit Specification

**cckit** (Claude Code Kit) is a toolkit for Claude Code that provides project inspection and session monitoring capabilities.

## Overview

```
cckit [COMMAND]
```

### Commands

| Command | Description |
|---------|-------------|
| (default) | Show help |
| `ls` | List Claude Code projects with their skills, agents, commands, and MCP servers |
| `prune` | Remove non-existent project paths from `~/.claude.json` |
| `session` | Manage Claude Code sessions |

## ls Command

Lists all Claude Code projects registered in `~/.claude.json` along with their local configurations.

```
cckit ls [OPTIONS]
```

### Scanned Items

- **Skills**: `.claude/skills/*/SKILL.md`
- **Agents**: `.claude/agents/*.md`
- **Commands**: `.claude/commands/**/*.md`
- **MCP Servers**: `.mcp.json`
- **Plugins**: `~/.claude/plugins/installed_plugins.json`

### Options

| Option | Description |
|--------|-------------|
| `-a, --all` | Show all projects (including those without skills/agents) |
| `--path-filter <PATTERN>` | Filter projects by path pattern |
| `-d, --duplicates` | Show duplicate projects (same git remote) |
| `--no-skills` | Hide skills |
| `--no-agents` | Hide agents |
| `--no-mcp` | Hide MCP servers |
| `--no-commands` | Hide commands |
| `--mcp-filter <PATTERN>` | Filter projects by MCP server name pattern |
| `--skill-filter <PATTERN>` | Filter projects by skill name pattern |

### Configuration

Optional `cckit.toml` in the current directory:

```toml
disable_paths = [
  "/path/to/ignore",
  "/path/with/glob/*"
]
```

## prune Command

Removes non-existent project paths from `~/.claude.json`.

```
cckit prune [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--execute` | Actually remove paths (default is dry-run) |
| `--no-backup` | Skip creating backup file |

## session Subcommand

Manage Claude Code sessions via hooks.

```
cckit session [COMMAND]
```

### Subcommands

| Command | Description |
|---------|-------------|
| `ls` | List active sessions (TUI, default) |
| `hook <EVENT>` | Handle hook events (internal use) |
| `install` | Configure hooks in `~/.claude/settings.json` |
| `uninstall` | Remove cckit hooks from settings |
| `status` | Show hook configuration status |
| `sync` | Remove stale sessions from storage |

### session ls

Display active sessions in TUI or text mode.

```
cckit session ls [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `-t, --text` | Show as text instead of TUI |
| `-i, --interval <SECS>` | Refresh interval in seconds (default: 5) |

### TUI Interface

Interactive terminal UI showing active Claude Code sessions.

#### Display Columns

| Column | Description |
|--------|-------------|
| Status | `● run` (running), `○ wait` (waiting input), `× done` (stopped) |
| Path | Working directory (shortened with `~`) |
| Tool | Last used tool and its input summary |
| PID | Claude Code process ID |
| Created | Session creation time (relative) |
| Updated | Last activity time (relative) |

#### Key Bindings

| Key | Action |
|-----|--------|
| `↑/↓` or `j/k` | Select session |
| `Enter` or `f` | Focus terminal (macOS only) |
| `r` | Refresh |
| `1-9` | Select by index |
| `q`, `Esc`, or `Ctrl+C` | Quit |

### Hook Events

The session tracking uses Claude Code hooks:

| Event | Action |
|-------|--------|
| `SessionStart` | Create new session entry |
| `SessionEnd` | Remove session entry |
| `UserPromptSubmit` | Mark session as running |
| `PreToolUse` | Update last tool info |
| `PostToolUse` | Update timestamp |
| `Stop` | Mark session as waiting input |

### session sync

Removes stale sessions (where TTY no longer exists).

```
cckit session sync [OPTIONS]
```

| Option | Description |
|--------|-------------|
| `--execute` | Actually remove stale sessions (default is dry-run) |

## Data Storage

### Location

Data is stored in the XDG data directory:

- **macOS**: `~/Library/Application Support/cckit/`
- **Linux**: `~/.local/share/cckit/`
- **Fallback**: `~/.local/share/cckit/`

### Files

| File | Description |
|------|-------------|
| `sessions.json` | Active session data |
| `sessions.lock` | File lock for concurrent access |
| `error.log` | Hook error logs |

### Session Data Structure

```json
{
  "sessions": {
    "<session_id>:<tty>": {
      "session_id": "uuid",
      "cwd": "/path/to/project",
      "tty": "/dev/ttys001",
      "status": "running|waiting_input|stopped",
      "created_at": "2024-01-01T00:00:00Z",
      "updated_at": "2024-01-01T00:00:00Z",
      "last_tool": "Bash",
      "last_tool_input": "command summary",
      "pid": 12345
    }
  },
  "updated_at": "2024-01-01T00:00:00Z"
}
```

## Terminal Focus (macOS)

The session TUI can focus the terminal window of a selected session. Supported terminals:

- iTerm2
- Terminal.app
- Ghostty

Implementation uses AppleScript to activate the terminal application and select the tab/window matching the session's TTY.
