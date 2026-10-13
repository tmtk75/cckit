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

Optional `config.toml`, read from the current directory, `~/.config/cckit/config.toml`, or the
platform config directory (first found wins):

```toml
disable_paths = [
  "/path/to/ignore",
  "/path/with/glob/*"
]

# Skills hidden from `skill stale`. Every entry is a glob, matched against the skill name
# (directory name and frontmatter `name`) and against the skill directory path. A glob with
# no wildcards matches exactly. A leading `~/` expands; a non-absolute pattern matches
# anywhere in the path.
skill_stale_exclude = [
  "isms-monthly-audit",
  "slack-*",
  "github.com/acme/**"
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
| `ls` | List active sessions as text |
| `hook <EVENT>` | Handle hook events (internal use) |
| `install` | Configure hooks in `~/.claude/settings.json` |
| `uninstall` | Remove cckit hooks from settings |
| `status` | Show hook configuration status |
| `sync` | Remove stale sessions from storage |

`cckit session` with no subcommand opens the macOS window app (same as `cckit app`); on other platforms it prints the text list.

### session ls

Print active sessions as text, one line per session.

```
cckit session ls
```

| Marker | Meaning |
|--------|---------|
| `●` | Running |
| `◐` | Awaiting approval |
| `○` | Waiting for input |
| `×` | Stopped |

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

The session window app can focus the terminal window of a selected session. Supported terminals:

- iTerm2
- Terminal.app
- Ghostty

Implementation uses AppleScript to activate the terminal application and select the tab/window matching the session's TTY.
