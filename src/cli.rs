use crate::monitor;
use clap::{Parser, Subcommand};
use colored::Colorize;
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::process::Command;

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), " (", env!("GIT_DESCRIBE"), ")");

#[derive(Parser)]
#[command(name = "cckit")]
#[command(version = VERSION)]
#[command(about = "Claude Code Kit - A toolkit for Claude Code")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// List Claude Code projects with their skills, agents, and MCP servers
    Ls {
        #[arg(
            short,
            long,
            help = "Show all projects (including those without skills/agents)"
        )]
        all: bool,

        #[arg(long, help = "Filter projects by path pattern")]
        path_filter: Option<String>,

        #[arg(short, long, help = "Show duplicate projects (same git remote)")]
        duplicates: bool,

        #[arg(long, help = "Hide skills")]
        no_skills: bool,

        #[arg(long, help = "Hide agents")]
        no_agents: bool,

        #[arg(long, help = "Hide MCP servers")]
        no_mcp: bool,

        #[arg(long, help = "Hide commands")]
        no_commands: bool,

        #[arg(long, help = "Filter projects by MCP server name pattern")]
        mcp_filter: Option<String>,

        #[arg(long, help = "Filter projects by skill name pattern")]
        skill_filter: Option<String>,
    },

    /// Remove non-existent project paths from ~/.claude.json
    Prune {
        #[arg(long, help = "Actually remove paths (default is dry-run)")]
        execute: bool,

        #[arg(long, help = "Skip creating backup file")]
        no_backup: bool,
    },

    /// Manage Claude Code sessions
    Session {
        #[command(subcommand)]
        command: Option<SessionCommands>,
    },

    /// Show ~/.claude.json contents in a readable format
    Config {
        /// Key path to inspect (e.g., "projects", "tipsHistory")
        key: Option<String>,

        /// Show raw JSON output (pretty-printed)
        #[arg(long)]
        raw: bool,
    },

    /// Manage skills across projects
    Skill {
        #[command(subcommand)]
        command: SkillCommands,
    },

    /// Manage MCP servers across projects
    Mcp {
        #[command(subcommand)]
        command: McpCommands,
    },

    /// Show cckit status and file paths
    Status,

    /// Check cckit configuration health
    Doctor,

    /// List permissions (allow/deny) from settings files
    Permissions {
        #[arg(short, long, help = "Filter entries by pattern (substring match)")]
        filter: Option<String>,

        #[arg(long, help = "Show only risky allow patterns with warnings")]
        audit: bool,

        #[arg(
            long,
            help = "Remove risky allow patterns (requires --audit, dry-run by default)"
        )]
        clean: bool,

        #[arg(long, help = "Actually remove patterns (use with --clean)")]
        execute: bool,
    },

    /// Run as macOS app with window and/or menubar (macOS only)
    App {
        /// Show only menubar (no window)
        #[arg(long, help = "Show only menubar")]
        menubar_only: bool,

        /// Show only window (no menubar)
        #[arg(long, help = "Show only window")]
        window_only: bool,
    },

    /// Send a macOS notification (macOS only)
    Notify {
        /// Notification title
        #[arg(short, long, default_value = "cckit")]
        title: String,

        /// Notification subtitle
        #[arg(short, long)]
        subtitle: Option<String>,

        /// Notification message/body (reads from stdin if not provided)
        #[arg(short, long)]
        message: Option<String>,

        /// Sound name (e.g., "Purr", "default", "Ping")
        #[arg(long)]
        sound: Option<String>,

        /// Display duration in milliseconds
        #[arg(short, long, default_value = "3000")]
        duration: u64,

        /// Window width in pixels
        #[arg(short, long, default_value = "320")]
        width: f64,

        /// Window height in pixels
        #[arg(long)]
        height: Option<f64>,

        /// Window position: left-top, center-top, right-top, left-center, center-center, right-center, left-bottom, center-bottom, right-bottom, menubar (or: lt, ct, rt, lc, cc, rc, lb, cb, rb, mb)
        #[arg(short, long, default_value = "mb")]
        position: String,

        /// Margin from screen edge in pixels (default: 8)
        #[arg(long)]
        margin: Option<f64>,

        /// Window opacity 0.0-1.0 (default: 0.87)
        #[arg(long)]
        opacity: Option<f64>,

        /// Background color as hex (default: #333)
        #[arg(long, default_value = "#333")]
        bgcolor: String,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List active sessions (TUI, default)
    Ls {
        #[arg(short, long, help = "Show as text instead of TUI")]
        text: bool,

        #[arg(short, long, help = "Show menubar icon (macOS only)")]
        menubar: bool,

        #[arg(long, help = "Menubar only, no TUI (macOS only)")]
        no_tui: bool,

        #[arg(long, default_value = "24", help = "Icon size in menubar (px)")]
        icon_size: u32,

        #[arg(long, default_value = "2000", help = "Session check interval in ms")]
        check_interval: u64,

        #[arg(long, default_value = "500", help = "Menubar poll interval in ms")]
        poll_interval: u64,

        #[arg(long, default_value = "2000", help = "Menu update interval in ms")]
        menu_update_interval: u64,

        #[arg(long, default_value = "500", help = "Event poll timeout in ms")]
        event_timeout: u64,
    },

    /// Handle hook events from Claude Code (internal use)
    Hook,

    /// Configure Claude Code hooks in ~/.claude/settings.json
    Install {
        #[arg(long, help = "Force reconfigure all hooks")]
        force: bool,
    },

    /// Show hook configuration status
    Status,

    /// Remove cckit hooks from ~/.claude/settings.json
    Uninstall,

    /// Sync sessions.json with actual state (remove stale sessions)
    Sync {
        #[arg(long, help = "Actually remove stale sessions (default is dry-run)")]
        execute: bool,
    },

    /// Focus a Ghostty tab by project name (macOS only)
    Focus {
        /// Project name to search for in tab titles
        project: String,
    },

    /// Dump Ghostty UI tree for debugging (macOS only)
    DumpUi,

    /// Run menubar app (macOS only)
    Menubar,
}

#[derive(Subcommand)]
enum SkillCommands {
    /// Copy a skill from another project to the current project
    Copy {
        #[arg(short, long, help = "Filter skills by name pattern")]
        filter: Option<String>,

        #[arg(long, help = "Copy from a specific project path")]
        from: Option<String>,

        #[arg(short, long, help = "Skill name (skip interactive selection)")]
        name: Option<String>,

        #[arg(long, help = "Overwrite existing skill without confirmation")]
        force: bool,
    },
}

#[derive(Subcommand)]
enum McpCommands {
    /// Copy an MCP server config from another project
    Copy {
        #[arg(short, long, help = "Filter MCP servers by name pattern")]
        filter: Option<String>,

        #[arg(long, help = "Copy from a specific project path")]
        from: Option<String>,

        #[arg(short, long, help = "MCP server name (skip interactive selection)")]
        name: Option<String>,

        #[arg(long, help = "Overwrite existing MCP server without confirmation")]
        force: bool,
    },
}

#[derive(Deserialize)]
struct ClaudeConfig {
    projects: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Deserialize, Default)]
struct ClamonConfig {
    #[serde(default)]
    disable_paths: Vec<String>,
}

#[derive(Deserialize)]
struct InstalledPlugins {
    plugins: HashMap<String, Vec<PluginInstall>>,
}

#[derive(Deserialize)]
struct PluginInstall {
    #[serde(rename = "installPath")]
    install_path: String,
    version: String,
}

#[derive(Debug)]
struct ProjectInfo {
    path: String,
    skills: Vec<SkillInfo>,
    agents: Vec<AgentInfo>,
    commands: Vec<CommandInfo>,
    plugins: Vec<PluginInfo>,
    mcp_servers: Vec<McpServerInfo>,
    exists: bool,
}

#[derive(Debug)]
struct McpServerInfo {
    name: String,
    server_type: String,
    command: Option<String>,
    source: String,
}

#[derive(Debug)]
struct SkillInfo {
    name: String,
    description: Option<String>,
}

#[derive(Debug)]
struct AgentInfo {
    name: String,
    description: Option<String>,
}

#[derive(Debug)]
struct CommandInfo {
    name: String,
    description: Option<String>,
}

#[derive(Debug)]
struct PluginInfo {
    name: String,
    version: String,
    skills: Vec<SkillInfo>,
    agents: Vec<AgentInfo>,
}

fn parse_frontmatter(content: &str) -> (Option<String>, Option<String>) {
    let content = content.trim();
    if !content.starts_with("---") {
        return (None, None);
    }

    let parts: Vec<&str> = content.splitn(3, "---").collect();
    if parts.len() < 3 {
        return (None, None);
    }

    let frontmatter = parts[1].trim();
    let mut name = None;
    let mut description = None;

    for line in frontmatter.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("name:") {
            name = Some(value.trim().to_string());
        } else if let Some(value) = line.strip_prefix("description:") {
            description = Some(value.trim().to_string());
        }
    }

    (name, description)
}

fn scan_skills(dir: &Path) -> Vec<SkillInfo> {
    let skills_dir = dir.join("skills");
    let mut skills = Vec::new();

    if !skills_dir.exists() {
        return skills;
    }

    if let Ok(entries) = fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    if let Ok(content) = fs::read_to_string(&skill_file) {
                        let (name, description) = parse_frontmatter(&content);
                        let name = name.unwrap_or_else(|| {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        });
                        skills.push(SkillInfo { name, description });
                    }
                }
            }
        }
    }

    skills
}

#[derive(Debug)]
struct SkillSource {
    project_display: String,
    skill_dir: std::path::PathBuf,
    dir_name: String,
    info: SkillInfo,
}

fn scan_skills_with_paths(dir: &Path) -> Vec<SkillSource> {
    let skills_dir = dir.join("skills");
    let mut results = Vec::new();

    if !skills_dir.exists() {
        return results;
    }

    if let Ok(entries) = fs::read_dir(&skills_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    if let Ok(content) = fs::read_to_string(&skill_file) {
                        let (name, description) = parse_frontmatter(&content);
                        let dir_name = path
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        let name = name.unwrap_or_else(|| dir_name.clone());
                        results.push(SkillSource {
                            project_display: String::new(),
                            skill_dir: path.clone(),
                            dir_name,
                            info: SkillInfo { name, description },
                        });
                    }
                }
            }
        }
    }
    results
}

fn collect_all_skills(from: Option<&str>) -> Vec<SkillSource> {
    let mut all_skills = Vec::new();
    let cwd = std::env::current_dir().ok();

    if let Some(project_path) = from {
        let claude_dir = Path::new(project_path).join(".claude");
        let display = shorten_path(project_path);
        for mut skill in scan_skills_with_paths(&claude_dir) {
            skill.project_display = display.clone();
            all_skills.push(skill);
        }
    } else {
        // Global ~/.claude
        let home = dirs::home_dir().expect("Could not find home directory");
        let global_claude = home.join(".claude");
        for mut skill in scan_skills_with_paths(&global_claude) {
            skill.project_display = "~/.claude (global)".to_string();
            all_skills.push(skill);
        }

        // All projects from ~/.claude.json
        if let Ok(config) = load_claude_config() {
            if let Some(projects) = config.projects {
                for project_path in projects.keys() {
                    let claude_dir = Path::new(project_path).join(".claude");
                    let display = shorten_path(project_path);
                    for mut skill in scan_skills_with_paths(&claude_dir) {
                        skill.project_display = display.clone();
                        all_skills.push(skill);
                    }
                }
            }
        }
    }

    // Exclude skills from the current project
    if let Some(ref cwd_path) = cwd {
        all_skills.retain(|s| !s.skill_dir.starts_with(cwd_path));
    }

    all_skills.sort_by(|a, b| a.info.name.cmp(&b.info.name));
    all_skills
}

fn select_skill_fzf(skills: &[SkillSource], filter: Option<&str>) -> Option<usize> {
    use std::io::Write;

    // Build input lines for fzf
    let lines: Vec<String> = skills
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let desc = s
                .info
                .description
                .as_ref()
                .map(|d| truncate_str(d, 60))
                .unwrap_or_default();
            format!("{}\t{}\t[{}]\t{}", i, s.info.name, s.project_display, desc)
        })
        .collect();
    let input = lines.join("\n");

    let mut cmd = Command::new("fzf");
    cmd.args([
        "--header",
        "Select a skill to copy (TAB to preview)",
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "--no-multi",
        "--ansi",
    ]);
    if let Some(q) = filter {
        cmd.args(["--query", q]);
    }

    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return None,
    };

    if let Some(ref mut stdin) = child.stdin {
        let _ = stdin.write_all(input.as_bytes());
    }
    drop(child.stdin.take());

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return None,
    };

    if !output.status.success() {
        return None;
    }

    let selected = String::from_utf8_lossy(&output.stdout);
    let selected = selected.trim();
    if selected.is_empty() {
        return None;
    }

    // Extract the index from the first tab-delimited field
    selected
        .split('\t')
        .next()
        .and_then(|idx| idx.parse::<usize>().ok())
}

fn select_skill_numbered(skills: &[SkillSource]) -> Option<usize> {
    use std::io::{self, BufRead, Write};

    if skills.is_empty() {
        println!("{}", "No skills found.".yellow());
        return None;
    }

    let max_name_len = skills.iter().map(|s| s.info.name.len()).max().unwrap_or(0);
    println!("{} skills found:\n", skills.len().to_string().cyan());
    for (i, skill) in skills.iter().enumerate() {
        let desc = skill
            .info
            .description
            .as_ref()
            .map(|d| format!(" - {}", truncate_str(d, 50).dimmed()))
            .unwrap_or_default();
        println!(
            "  {:>3}) {:<width$} {}{}",
            (i + 1).to_string().cyan(),
            skill.info.name.green(),
            format!("[{}]", skill.project_display).dimmed(),
            desc,
            width = max_name_len
        );
    }

    println!();
    print!("{}", "Select skill number (or 'q' to quit): ".bold());
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let line = stdin.lock().lines().next()?.ok()?;
    let line = line.trim().to_string();

    if line == "q" || line == "Q" || line.is_empty() {
        return None;
    }

    let num: usize = match line.parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("{}: invalid number: {}", "Error".red(), line);
            return None;
        }
    };

    if num == 0 || num > skills.len() {
        eprintln!("{}: out of range: {}", "Error".red(), num);
        return None;
    }

    Some(num - 1)
}

fn select_skill(skills: &[SkillSource], filter: Option<&str>) -> Option<usize> {
    // Try fzf first
    if Command::new("fzf")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        return select_skill_fzf(skills, filter);
    }
    // Fallback to numbered list
    select_skill_numbered(skills)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<u64, Box<dyn std::error::Error>> {
    let mut copied = 0u64;
    fs::create_dir_all(dst)?;

    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if file_type.is_dir() {
            copied += copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
            copied += 1;
        }
    }
    Ok(copied)
}

fn skill_copy_command(
    filter: Option<String>,
    from: Option<String>,
    name: Option<String>,
    force: bool,
) {
    let mut skills = collect_all_skills(from.as_deref());

    // Apply filter when using --name (fzf handles its own filtering via --query)
    if name.is_some() {
        if let Some(ref pattern) = filter {
            skills.retain(|s| {
                s.info.name.contains(pattern.as_str()) || s.dir_name.contains(pattern.as_str())
            });
        }
    }

    if skills.is_empty() {
        println!("{}", "No skills found.".yellow());
        return;
    }

    let selected = if let Some(ref skill_name) = name {
        match skills
            .iter()
            .position(|s| s.info.name == *skill_name || s.dir_name == *skill_name)
        {
            Some(idx) => idx,
            None => {
                eprintln!("{}: skill '{}' not found", "Error".red(), skill_name);
                std::process::exit(1);
            }
        }
    } else {
        match select_skill(&skills, filter.as_deref()) {
            Some(idx) => idx,
            None => {
                println!("Cancelled.");
                return;
            }
        }
    };

    let skill = &skills[selected];

    // Determine destination
    let cwd = std::env::current_dir().expect("Could not determine current directory");
    let dest_base = cwd.join(".claude").join("skills");
    let dest_dir = dest_base.join(&skill.dir_name);

    // Check for conflicts
    if dest_dir.exists() && !force {
        use std::io::{self, BufRead, Write};
        eprintln!(
            "{}: skill '{}' already exists at {}",
            "Warning".yellow(),
            skill.info.name,
            shorten_path(&dest_dir.to_string_lossy())
        );
        print!("{}", "Overwrite? (y/N): ".bold());
        io::stdout().flush().ok();

        let stdin = io::stdin();
        let line = stdin
            .lock()
            .lines()
            .next()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        if line.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return;
        }
    }

    // Copy
    println!(
        "Copying skill '{}' from {} ...",
        skill.info.name.green(),
        skill.project_display.dimmed()
    );

    match copy_dir_recursive(&skill.skill_dir, &dest_dir) {
        Ok(count) => {
            println!(
                "{} Copied {} files to {}",
                "Done!".green().bold(),
                count,
                shorten_path(&dest_dir.to_string_lossy())
            );
        }
        Err(e) => {
            eprintln!("{}: {}", "Error copying skill".red(), e);
            std::process::exit(1);
        }
    }
}

fn scan_agents(dir: &Path) -> Vec<AgentInfo> {
    let agents_dir = dir.join("agents");
    let mut agents = Vec::new();

    if !agents_dir.exists() {
        return agents;
    }

    if let Ok(entries) = fs::read_dir(&agents_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().map_or(false, |e| e == "md") {
                if let Ok(content) = fs::read_to_string(&path) {
                    let (name, description) = parse_frontmatter(&content);
                    let name = name.unwrap_or_else(|| {
                        path.file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string()
                    });
                    agents.push(AgentInfo { name, description });
                }
            }
        }
    }

    agents
}

fn scan_commands(dir: &Path) -> Vec<CommandInfo> {
    let commands_dir = dir.join("commands");
    let mut commands = Vec::new();

    if !commands_dir.exists() {
        return commands;
    }

    fn scan_dir(dir: &Path, commands: &mut Vec<CommandInfo>) {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    // Recursively scan subdirectories
                    scan_dir(&path, commands);
                } else if path.is_file() && path.extension().map_or(false, |e| e == "md") {
                    if let Ok(content) = fs::read_to_string(&path) {
                        let (name, description) = parse_frontmatter(&content);
                        let name = name.unwrap_or_else(|| {
                            path.file_stem()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string()
                        });
                        commands.push(CommandInfo { name, description });
                    }
                }
            }
        }
    }

    scan_dir(&commands_dir, &mut commands);
    commands
}

fn scan_mcp_servers(project_dir: &Path) -> Vec<McpServerInfo> {
    let mcp_file = project_dir.join(".mcp.json");
    let mut servers = Vec::new();

    if !mcp_file.exists() {
        return servers;
    }

    let source = mcp_file.to_string_lossy().to_string();

    let content = match fs::read_to_string(&mcp_file) {
        Ok(c) => c,
        Err(_) => return servers,
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return servers,
    };

    if let Some(mcp_servers) = json.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, config) in mcp_servers {
            let server_type = config
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let command = config.get("command").and_then(|v| v.as_str()).map(|s| {
                let args = config
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if args.is_empty() {
                    s.to_string()
                } else {
                    format!("{} {}", s, args)
                }
            });
            servers.push(McpServerInfo {
                name: name.clone(),
                server_type,
                command,
                source: source.clone(),
            });
        }
    }

    servers.sort_by(|a, b| a.name.cmp(&b.name));
    servers
}

#[derive(Debug)]
struct McpSource {
    project_display: String,
    server_name: String,
    config: serde_json::Value,
    info: McpServerInfo,
}

fn scan_mcp_sources(project_dir: &Path) -> Vec<McpSource> {
    let mcp_file = project_dir.join(".mcp.json");
    let mut sources = Vec::new();

    if !mcp_file.exists() {
        return sources;
    }

    let source_path = mcp_file.to_string_lossy().to_string();

    let content = match fs::read_to_string(&mcp_file) {
        Ok(c) => c,
        Err(_) => return sources,
    };

    let json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return sources,
    };

    if let Some(mcp_servers) = json.get("mcpServers").and_then(|v| v.as_object()) {
        for (name, config) in mcp_servers {
            let server_type = config
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let command = config.get("command").and_then(|v| v.as_str()).map(|s| {
                let args = config
                    .get("args")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if args.is_empty() {
                    s.to_string()
                } else {
                    format!("{} {}", s, args)
                }
            });
            sources.push(McpSource {
                project_display: String::new(),
                server_name: name.clone(),
                config: config.clone(),
                info: McpServerInfo {
                    name: name.clone(),
                    server_type,
                    command,
                    source: source_path.clone(),
                },
            });
        }
    }

    sources.sort_by(|a, b| a.server_name.cmp(&b.server_name));
    sources
}

fn collect_all_mcp_servers(from: Option<&str>) -> Vec<McpSource> {
    let mut all = Vec::new();
    let cwd = std::env::current_dir().ok();

    if let Some(project_path) = from {
        let display = shorten_path(project_path);
        for mut src in scan_mcp_sources(Path::new(project_path)) {
            src.project_display = display.clone();
            all.push(src);
        }
    } else {
        if let Ok(config) = load_claude_config() {
            if let Some(projects) = config.projects {
                for project_path in projects.keys() {
                    let display = shorten_path(project_path);
                    for mut src in scan_mcp_sources(Path::new(project_path)) {
                        src.project_display = display.clone();
                        all.push(src);
                    }
                }
            }
        }
    }

    // Exclude MCP servers from the current project
    if let Some(ref cwd_path) = cwd {
        let cwd_str = cwd_path.to_string_lossy().to_string();
        all.retain(|s| !s.info.source.starts_with(&cwd_str));
    }

    all.sort_by(|a, b| a.server_name.cmp(&b.server_name));
    all
}

fn select_mcp_fzf(servers: &[McpSource], filter: Option<&str>) -> Option<usize> {
    use std::io::Write;

    let lines: Vec<String> = servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let cmd = s
                .info
                .command
                .as_ref()
                .map(|c| truncate_str(c, 40))
                .unwrap_or_default();
            format!(
                "{}\t{}\t[{}]\t({}) {}",
                i, s.server_name, s.project_display, s.info.server_type, cmd
            )
        })
        .collect();
    let input = lines.join("\n");

    let mut cmd = Command::new("fzf");
    cmd.args([
        "--header",
        "Select an MCP server to copy",
        "--delimiter",
        "\t",
        "--with-nth",
        "2..",
        "--no-multi",
        "--ansi",
    ]);
    if let Some(q) = filter {
        cmd.args(["--query", q]);
    }

    cmd.stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::inherit());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(_) => return None,
    };

    if let Some(ref mut stdin) = child.stdin {
        let _ = stdin.write_all(input.as_bytes());
    }
    drop(child.stdin.take());

    let output = match child.wait_with_output() {
        Ok(o) => o,
        Err(_) => return None,
    };

    if !output.status.success() {
        return None;
    }

    let selected = String::from_utf8_lossy(&output.stdout);
    let selected = selected.trim();
    if selected.is_empty() {
        return None;
    }

    selected
        .split('\t')
        .next()
        .and_then(|idx| idx.parse::<usize>().ok())
}

fn select_mcp_numbered(servers: &[McpSource]) -> Option<usize> {
    use std::io::{self, BufRead, Write};

    if servers.is_empty() {
        println!("{}", "No MCP servers found.".yellow());
        return None;
    }

    let max_name_len = servers
        .iter()
        .map(|s| s.server_name.len())
        .max()
        .unwrap_or(0);
    println!("{} MCP servers found:\n", servers.len().to_string().cyan());
    for (i, server) in servers.iter().enumerate() {
        let cmd = server
            .info
            .command
            .as_ref()
            .map(|c| format!(" {}", truncate_str(c, 40).dimmed()))
            .unwrap_or_default();
        println!(
            "  {:>3}) {:<width$} {} {}{}",
            (i + 1).to_string().cyan(),
            server.server_name.bright_blue(),
            format!("[{}]", server.project_display).dimmed(),
            format!("({})", server.info.server_type).dimmed(),
            cmd,
            width = max_name_len
        );
    }

    println!();
    print!("{}", "Select MCP server number (or 'q' to quit): ".bold());
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let line = stdin.lock().lines().next()?.ok()?;
    let line = line.trim().to_string();

    if line == "q" || line == "Q" || line.is_empty() {
        return None;
    }

    let num: usize = match line.parse() {
        Ok(n) => n,
        Err(_) => {
            eprintln!("{}: invalid number: {}", "Error".red(), line);
            return None;
        }
    };

    if num == 0 || num > servers.len() {
        eprintln!("{}: out of range: {}", "Error".red(), num);
        return None;
    }

    Some(num - 1)
}

fn select_mcp(servers: &[McpSource], filter: Option<&str>) -> Option<usize> {
    if Command::new("fzf")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok()
    {
        return select_mcp_fzf(servers, filter);
    }
    select_mcp_numbered(servers)
}

fn mcp_copy_command(
    filter: Option<String>,
    from: Option<String>,
    name: Option<String>,
    force: bool,
) {
    let mut servers = collect_all_mcp_servers(from.as_deref());

    if name.is_some() {
        if let Some(ref pattern) = filter {
            servers.retain(|s| s.server_name.contains(pattern.as_str()));
        }
    }

    if servers.is_empty() {
        println!("{}", "No MCP servers found.".yellow());
        return;
    }

    let selected = if let Some(ref server_name) = name {
        match servers.iter().position(|s| s.server_name == *server_name) {
            Some(idx) => idx,
            None => {
                eprintln!("{}: MCP server '{}' not found", "Error".red(), server_name);
                std::process::exit(1);
            }
        }
    } else {
        match select_mcp(&servers, filter.as_deref()) {
            Some(idx) => idx,
            None => {
                println!("Cancelled.");
                return;
            }
        }
    };

    let server = &servers[selected];

    // Read or create .mcp.json
    let cwd = std::env::current_dir().expect("Could not determine current directory");
    let mcp_path = cwd.join(".mcp.json");

    let mut mcp_json: serde_json::Value = if mcp_path.exists() {
        match fs::read_to_string(&mcp_path) {
            Ok(content) => serde_json::from_str(&content)
                .unwrap_or_else(|_| serde_json::json!({"mcpServers": {}})),
            Err(_) => serde_json::json!({"mcpServers": {}}),
        }
    } else {
        serde_json::json!({"mcpServers": {}})
    };

    // Check for conflicts
    let mcp_servers = mcp_json.as_object_mut().and_then(|o| {
        o.entry("mcpServers")
            .or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
    });

    let mcp_servers = match mcp_servers {
        Some(s) => s,
        None => {
            eprintln!("{}: invalid .mcp.json structure", "Error".red());
            std::process::exit(1);
        }
    };

    if mcp_servers.contains_key(&server.server_name) && !force {
        use std::io::{self, BufRead, Write};
        eprintln!(
            "{}: MCP server '{}' already exists in .mcp.json",
            "Warning".yellow(),
            server.server_name
        );
        print!("{}", "Overwrite? (y/N): ".bold());
        io::stdout().flush().ok();

        let stdin = io::stdin();
        let line = stdin
            .lock()
            .lines()
            .next()
            .and_then(|r| r.ok())
            .unwrap_or_default();
        if line.trim().to_lowercase() != "y" {
            println!("Cancelled.");
            return;
        }
    }

    mcp_servers.insert(server.server_name.clone(), server.config.clone());

    // Write back
    println!(
        "Adding MCP server '{}' from {} ...",
        server.server_name.bright_blue(),
        server.project_display.dimmed()
    );

    let output = serde_json::to_string_pretty(&mcp_json).expect("Failed to serialize JSON");
    match fs::write(&mcp_path, format!("{}\n", output)) {
        Ok(_) => {
            println!(
                "{} Added '{}' to .mcp.json",
                "Done!".green().bold(),
                server.server_name.bright_blue()
            );
        }
        Err(e) => {
            eprintln!("{}: {}", "Error writing .mcp.json".red(), e);
            std::process::exit(1);
        }
    }
}

fn scan_plugins(claude_dir: &Path) -> Vec<PluginInfo> {
    let plugins_file = claude_dir.join("plugins/installed_plugins.json");
    let mut plugins = Vec::new();

    if !plugins_file.exists() {
        return plugins;
    }

    let content = match fs::read_to_string(&plugins_file) {
        Ok(c) => c,
        Err(_) => return plugins,
    };

    let installed: InstalledPlugins = match serde_json::from_str(&content) {
        Ok(p) => p,
        Err(_) => return plugins,
    };

    for (plugin_id, installs) in installed.plugins {
        if let Some(install) = installs.first() {
            let plugin_path = Path::new(&install.install_path);
            let skills = scan_skills(plugin_path);
            let agents = scan_agents(plugin_path);

            plugins.push(PluginInfo {
                name: plugin_id,
                version: install.version.clone(),
                skills,
                agents,
            });
        }
    }

    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

fn get_global_info() -> ProjectInfo {
    let home = dirs::home_dir().expect("Could not find home directory");
    let claude_dir = home.join(".claude");

    let skills = scan_skills(&claude_dir);
    let agents = scan_agents(&claude_dir);
    let commands = scan_commands(&claude_dir);
    let plugins = scan_plugins(&claude_dir);
    // Global doesn't have .mcp.json in the same way
    let mcp_servers = Vec::new();

    ProjectInfo {
        path: "~/.claude (global)".to_string(),
        skills,
        agents,
        commands,
        plugins,
        mcp_servers,
        exists: claude_dir.exists(),
    }
}

fn get_project_info(project_path: &str) -> ProjectInfo {
    let path = Path::new(project_path);
    let exists = path.exists();
    let claude_dir = path.join(".claude");

    let (skills, agents, commands) = if claude_dir.exists() {
        (
            scan_skills(&claude_dir),
            scan_agents(&claude_dir),
            scan_commands(&claude_dir),
        )
    } else {
        (Vec::new(), Vec::new(), Vec::new())
    };

    let mcp_servers = scan_mcp_servers(path);

    ProjectInfo {
        path: project_path.to_string(),
        skills,
        agents,
        commands,
        plugins: Vec::new(),
        mcp_servers,
        exists,
    }
}

fn get_git_remote_url(project_path: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(project_path)
        .output()
        .ok()?;

    if output.status.success() {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Some(normalize_git_url(&url))
    } else {
        None
    }
}

fn normalize_git_url(url: &str) -> String {
    let url = url.to_lowercase();
    let url = url.strip_suffix(".git").unwrap_or(&url);
    url.to_string()
}

fn load_claude_config() -> Result<ClaudeConfig, Box<dyn std::error::Error>> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let config_path = home.join(".claude.json");
    let content = fs::read_to_string(&config_path)?;
    let config: ClaudeConfig = serde_json::from_str(&content)?;
    Ok(config)
}

fn load_cckit_config() -> ClamonConfig {
    // Priority: ./config.toml > ~/.config/cckit/config.toml
    let candidates = [
        std::env::current_dir().ok().map(|p| p.join("config.toml")),
        dirs::config_dir().map(|p| p.join("cckit/config.toml")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() {
            if let Ok(content) = fs::read_to_string(&candidate) {
                if let Ok(config) = toml::from_str(&content) {
                    return config;
                }
            }
        }
    }
    ClamonConfig::default()
}

fn shorten_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home_str = home.to_string_lossy();
        if path.starts_with(home_str.as_ref()) {
            return path.replacen(home_str.as_ref(), "~", 1);
        }
    }
    path.to_string()
}

fn is_path_disabled(path: &str, disable_paths: &[String]) -> bool {
    for pattern in disable_paths {
        if pattern.contains('*') || pattern.contains('?') {
            // Glob pattern matching
            if let Ok(glob_pattern) = glob::Pattern::new(pattern) {
                if glob_pattern.matches(path) {
                    return true;
                }
            }
        } else {
            // Prefix matching
            if path.starts_with(pattern) {
                return true;
            }
        }
    }
    false
}

fn truncate_str(s: &str, max_chars: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > max_chars {
        format!("{}...", chars[..max_chars].iter().collect::<String>())
    } else {
        s.to_string()
    }
}

fn print_skills(skills: &[SkillInfo], indent: &str) {
    if skills.is_empty() {
        return;
    }
    println!("{}{}:", indent, "Skills".yellow());
    for skill in skills {
        let desc = skill
            .description
            .as_ref()
            .map(|d| format!(" - {}", truncate_str(d, 57).dimmed()))
            .unwrap_or_default();
        println!(
            "{}  {} {}{}",
            indent,
            "-".dimmed(),
            skill.name.green(),
            desc
        );
    }
}

fn print_agents(agents: &[AgentInfo], indent: &str) {
    if agents.is_empty() {
        return;
    }
    println!("{}{}:", indent, "Agents".blue());
    for agent in agents {
        let desc = agent
            .description
            .as_ref()
            .map(|d| format!(" - {}", truncate_str(d, 57).dimmed()))
            .unwrap_or_default();
        println!("{}  {} {}{}", indent, "-".dimmed(), agent.name.cyan(), desc);
    }
}

fn print_commands(commands: &[CommandInfo], indent: &str) {
    if commands.is_empty() {
        return;
    }
    println!("{}{}:", indent, "Commands".yellow());
    for cmd in commands {
        let desc = cmd
            .description
            .as_ref()
            .map(|d| format!(" - {}", truncate_str(d, 57).dimmed()))
            .unwrap_or_default();
        println!(
            "{}  {} /{}{}",
            indent,
            "-".dimmed(),
            cmd.name.yellow(),
            desc
        );
    }
}

fn print_project(info: &ProjectInfo, opts: &LsOptions) {
    let status = if info.exists {
        "".to_string()
    } else {
        " (not found)".red().to_string()
    };

    println!("{}{}", info.path.bold(), status);

    if !opts.no_skills {
        print_skills(&info.skills, "  ");
    }
    if !opts.no_agents {
        print_agents(&info.agents, "  ");
    }
    if !opts.no_commands {
        print_commands(&info.commands, "  ");
    }

    if !info.plugins.is_empty() {
        println!("  {}:", "Plugins".magenta());
        for plugin in &info.plugins {
            println!(
                "    {} {} {}",
                "-".dimmed(),
                plugin.name.magenta(),
                format!("({})", plugin.version).dimmed()
            );
            if !opts.no_skills {
                print_skills(&plugin.skills, "      ");
            }
            if !opts.no_agents {
                print_agents(&plugin.agents, "      ");
            }
        }
    }

    if !opts.no_mcp && !info.mcp_servers.is_empty() {
        println!("  {}:", "MCP Servers".bright_blue());
        for server in &info.mcp_servers {
            let cmd = server
                .command
                .as_ref()
                .map(|c| format!(" - {}", c.dimmed()))
                .unwrap_or_default();
            let source = format!("from {}", shorten_path(&server.source)).dimmed();
            println!(
                "    {} {} {}{} {}",
                "-".dimmed(),
                server.name.bright_blue(),
                format!("({})", server.server_type).dimmed(),
                cmd,
                source
            );
        }
    }

    println!();
}

fn run_sync(execute: bool) {
    let storage = monitor::storage::Storage::new();

    let stale_sessions = match storage.find_stale_sessions() {
        Ok(sessions) => sessions,
        Err(e) => {
            eprintln!("{}: {}", "Error reading sessions".red(), e);
            std::process::exit(1);
        }
    };

    if stale_sessions.is_empty() {
        println!("{}", "All sessions are valid. Nothing to sync.".green());
        return;
    }

    println!(
        "Found {} stale sessions (TTY no longer exists):",
        stale_sessions.len().to_string().yellow()
    );

    for (key, session) in &stale_sessions {
        println!(
            "  {} {} {} ({})",
            "-".dimmed(),
            session.cwd.red(),
            session.tty.dimmed(),
            key.dimmed()
        );
    }

    if !execute {
        println!();
        println!("Run with {} to remove these sessions.", "--execute".cyan());
        return;
    }

    match storage.sync_sessions() {
        Ok(removed) => {
            println!();
            println!(
                "{} {} stale sessions from sessions.json",
                "Removed".green(),
                removed.len().to_string().yellow()
            );
        }
        Err(e) => {
            eprintln!("{}: {}", "Error syncing sessions".red(), e);
            std::process::exit(1);
        }
    }
}

fn get_file_info(path: &Path) -> String {
    if path.exists() {
        let metadata = fs::metadata(path);
        match metadata {
            Ok(m) => {
                let size = m.len();
                let modified = m
                    .modified()
                    .ok()
                    .and_then(|t| {
                        let datetime: chrono::DateTime<chrono::Local> = t.into();
                        Some(datetime.format("%Y-%m-%d %H:%M:%S").to_string())
                    })
                    .unwrap_or_else(|| "unknown".to_string());
                format!(
                    "{} ({} bytes, modified: {})",
                    "exists".green(),
                    size,
                    modified
                )
            }
            Err(_) => format!("{}", "exists (no metadata)".green()),
        }
    } else {
        format!("{}", "not found".red())
    }
}

fn value_type_label(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn value_preview(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::Null => "null".to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::String(s) => {
            if s.len() > 60 {
                format!("\"{}...\" ({} chars)", &s[..57], s.len())
            } else {
                format!("\"{}\"", s)
            }
        }
        serde_json::Value::Array(a) => format!("[{} items]", a.len()),
        serde_json::Value::Object(o) => format!("{{{} keys}}", o.len()),
    }
}

fn claude_json_key_description(key: &str) -> Option<&'static str> {
    Some(match key {
        "autoUpdaterStatus" => "Auto-updater state",
        "autoUpdates" => "Enable auto-updates",
        "cachedChromeExtensionInstalled" => "Chrome extension detected",
        "cachedDynamicConfigs" => "Statsig dynamic configs cache",
        "cachedGrowthBookFeatures" => "Growth/feature flags cache",
        "cachedStatsigGates" => "Statsig feature gates cache",
        "changelogLastFetched" => "Last changelog fetch timestamp",
        "claudeCodeFirstTokenDate" => "First API token date",
        "claudeMaxTier" => "Max subscription tier",
        "clientDataCache" => "Client data cache",
        "fallbackAvailableWarningThreshold" => "Fallback warning threshold",
        "feedbackSurveyState" => "Feedback survey state",
        "firstStartTime" => "First startup timestamp",
        "githubActionSetupCount" => "GitHub Actions setup count",
        "githubRepoPaths" => "GitHub repo path mappings",
        "groveConfigCache" => "Grove config cache",
        "hasAcknowledgedCostThreshold" => "Cost threshold acknowledged",
        "hasAvailableMaxSubscription" => "Max subscription available",
        "hasAvailableSubscription" => "Subscription available",
        "hasCompletedOnboarding" => "Onboarding completed",
        "hasIdeOnboardingBeenShown" => "IDE onboarding shown",
        "hasOpusPlanDefault" => "Opus as default plan model",
        "hasSeenStashHint" => "Stash hint seen",
        "hasSeenTasksHint" => "Tasks hint seen",
        "hasShownOpus45Notice" => "Opus 4.5 notice shown",
        "hasShownOpus46Notice" => "Opus 4.6 notice shown",
        "hasUsedBackslashReturn" => "Used backslash-return",
        "hasVisitedPasses" => "Visited passes page",
        "installMethod" => "Installation method",
        "isQualifiedForDataSharing" => "Data sharing qualification",
        "iterm2BackupPath" => "iTerm2 config backup path",
        "iterm2SetupInProgress" => "iTerm2 setup in progress",
        "lastOnboardingVersion" => "Last onboarding version",
        "lastPlanModeUse" => "Last plan mode usage timestamp",
        "lastReleaseNotesSeen" => "Last release notes seen",
        "lspRecommendationIgnoredCount" => "LSP recommendation ignored count",
        "maxSubscriptionNoticeCount" => "Max subscription notice count",
        "mcpServers" => "MCP server configurations",
        "memoryUsageCount" => "/memory command usage count",
        "numStartups" => "Total startup count",
        "oauthAccount" => "OAuth account info",
        "officialMarketplaceAutoInstallAttempted" => "Marketplace auto-install attempted",
        "officialMarketplaceAutoInstalled" => "Marketplace auto-installed",
        "opus45MigrationComplete" => "Opus 4.5 migration done",
        "opus46FeedSeenCount" => "Opus 4.6 feed seen count",
        "opusProMigrationComplete" => "Opus Pro migration done",
        "passesEligibilityCache" => "Passes eligibility cache",
        "passesLastSeenRemaining" => "Passes remaining count",
        "passesUpsellSeenCount" => "Passes upsell seen count",
        "projects" => "Per-project settings (allowedTools, etc.)",
        "promptQueueUseCount" => "Prompt queue usage count",
        "recommendedSubscription" => "Recommended subscription plan",
        "s1mAccessCache" => "S1M access cache",
        "shiftEnterKeyBindingInstalled" => "Shift+Enter keybinding installed",
        "showExpandedTodos" => "Show expanded todo list",
        "showSpinnerTree" => "Show spinner tree UI",
        "skillUsage" => "Skill usage statistics",
        "sonnet45MigrationComplete" => "Sonnet 4.5 migration done",
        "statsigModel" => "Statsig model config",
        "subscriptionNoticeCount" => "Subscription notice count",
        "subscriptionUpsellShownCount" => "Subscription upsell shown count",
        "thinkingMigrationComplete" => "Thinking mode migration done",
        "tipsHistory" => "Tip display history (startup counts)",
        "userID" => "Anonymous user identifier",
        _ => return None,
    })
}

fn print_value_detail(v: &serde_json::Value) {
    match v {
        serde_json::Value::Object(map) => {
            let max_key_len = map.keys().map(|k| k.len()).max().unwrap_or(0);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            for k in keys {
                let val = &map[k];
                let type_label = value_type_label(val);
                let preview = value_preview(val);
                println!(
                    "  {:<width$}  {:<8} {}",
                    k.cyan(),
                    type_label.dimmed(),
                    preview,
                    width = max_key_len
                );
            }
        }
        serde_json::Value::Array(arr) => {
            let show = 5;
            for (i, item) in arr.iter().enumerate().take(show) {
                let s = serde_json::to_string(item).unwrap_or_default();
                if s.len() > 100 {
                    println!("  [{}] {}...", i, &s[..97]);
                } else {
                    println!("  [{}] {}", i, s);
                }
            }
            if arr.len() > show {
                println!("  ... ({} more)", arr.len() - show);
            }
        }
        _ => {
            println!("{}", serde_json::to_string_pretty(v).unwrap_or_default());
        }
    }
}

fn config_command(key: Option<String>, raw: bool) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("{}", "Error: Could not find home directory".red());
            std::process::exit(1);
        }
    };

    let claude_json = home.join(".claude.json");
    let content = match fs::read_to_string(&claude_json) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: {}", "Error reading ~/.claude.json".red(), e);
            std::process::exit(1);
        }
    };

    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {}", "Error parsing ~/.claude.json".red(), e);
            std::process::exit(1);
        }
    };

    if raw {
        println!(
            "{}",
            serde_json::to_string_pretty(&value).unwrap_or_default()
        );
        return;
    }

    match key {
        Some(k) => {
            // Lookup the key (supports dot-separated paths)
            let mut current = &value;
            for part in k.split('.') {
                current = match current.get(part) {
                    Some(v) => v,
                    None => {
                        eprintln!("{}: key \"{}\" not found", "Error".red(), k);
                        std::process::exit(1);
                    }
                };
            }
            println!("{} {}", "~/.claude.json".dimmed(), k.bold());
            println!();
            print_value_detail(current);
        }
        None => {
            println!("{}", "~/.claude.json".bold());
            println!();
            if let serde_json::Value::Object(map) = &value {
                let max_key_len = map.keys().map(|k| k.len()).max().unwrap_or(0);
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for k in keys {
                    let v = &map[k];
                    let type_label = value_type_label(v);
                    let preview = value_preview(v);
                    let desc = claude_json_key_description(k)
                        .map(|d| format!("  # {}", d).dimmed().to_string())
                        .unwrap_or_default();
                    println!(
                        "  {:<width$}  {:<8} {}{}",
                        k.cyan(),
                        type_label.dimmed(),
                        preview,
                        desc,
                        width = max_key_len
                    );
                }
            }
        }
    }
}

struct RiskyPattern {
    pattern: &'static str,
    reason: &'static str,
}

const RISKY_PATTERNS: &[RiskyPattern] = &[
    // Arbitrary code execution
    RiskyPattern {
        pattern: "Bash(python:",
        reason: "arbitrary code execution via python",
    },
    RiskyPattern {
        pattern: "Bash(python3:",
        reason: "arbitrary code execution via python3",
    },
    RiskyPattern {
        pattern: "Bash(node:",
        reason: "arbitrary code execution via node",
    },
    RiskyPattern {
        pattern: "Bash(source:",
        reason: "arbitrary script sourcing",
    },
    // File destruction
    RiskyPattern {
        pattern: "Bash(rm:",
        reason: "file deletion",
    },
    // Destructive git operations
    RiskyPattern {
        pattern: "Bash(git push:",
        reason: "can force push and destroy remote history",
    },
    RiskyPattern {
        pattern: "Bash(git reset:",
        reason: "can discard uncommitted changes with --hard",
    },
    RiskyPattern {
        pattern: "Bash(git checkout:",
        reason: "can discard working tree changes",
    },
    // Overly broad wildcards
    RiskyPattern {
        pattern: "Bash(gh:*)",
        reason: "allows ALL gh commands including destructive ones",
    },
    RiskyPattern {
        pattern: "Bash(terraform:*)",
        reason: "allows ALL terraform commands including apply/destroy",
    },
    RiskyPattern {
        pattern: "Bash(pnpm:*)",
        reason: "allows ALL pnpm commands including pnpm exec",
    },
    RiskyPattern {
        pattern: "Bash(cat:",
        reason: "can bypass Read deny rules to read sensitive files",
    },
    // Infrastructure access
    RiskyPattern {
        pattern: "Bash(aws ",
        reason: "AWS CLI access (check scope)",
    },
    RiskyPattern {
        pattern: "Bash(AWS_PROFILE=",
        reason: "AWS CLI access with profile (check scope)",
    },
    // macOS
    RiskyPattern {
        pattern: "Bash(osascript",
        reason: "AppleScript can perform arbitrary macOS actions",
    },
    // Slack send
    RiskyPattern {
        pattern: "slack_send_message",
        reason: "can send Slack messages",
    },
];

fn check_risky(entry: &str) -> Option<&'static str> {
    RISKY_PATTERNS
        .iter()
        .find(|r| entry.starts_with(r.pattern) || entry.contains(r.pattern))
        .map(|r| r.reason)
}

fn print_permissions(
    label: &str,
    path: &std::path::Path,
    filter: Option<&str>,
    audit: bool,
    separator: bool,
) -> bool {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {} ({})", "Error parsing".red(), label, e);
            return false;
        }
    };

    let permissions = match value.get("permissions") {
        Some(p) => p,
        None => return false,
    };

    let allow = permissions.get("allow").and_then(|v| v.as_array());
    let deny = permissions.get("deny").and_then(|v| v.as_array());

    if allow.is_none() && deny.is_none() {
        return false;
    }

    let matches_filter = |s: &str| -> bool {
        match filter {
            Some(f) => s.contains(f),
            None => true,
        }
    };

    let filtered_allow: Vec<&str> = allow
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter(|s| matches_filter(s))
        .collect();
    let filtered_deny: Vec<&str> = deny
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter(|s| matches_filter(s))
        .collect();

    if audit {
        // Audit mode: only show risky allow entries
        let risky: Vec<(&str, &str)> = filtered_allow
            .iter()
            .filter_map(|s| check_risky(s).map(|reason| (*s, reason)))
            .collect();

        if risky.is_empty() {
            return false;
        }

        if separator {
            println!();
        }
        println!("{}:", label.cyan());

        let max_len = risky.iter().map(|(s, _)| s.len()).max().unwrap_or(0);
        for (entry, reason) in &risky {
            println!(
                "    {:<width$}  -- {}",
                entry,
                reason.yellow(),
                width = max_len
            );
        }

        return true;
    }

    if filtered_allow.is_empty() && filtered_deny.is_empty() {
        return false;
    }

    if separator {
        println!();
    }
    println!("{}:", label.cyan());

    if !filtered_allow.is_empty() {
        println!("  {}:", "allow".green());
        for s in &filtered_allow {
            println!("    {}", s);
        }
    }

    if !filtered_deny.is_empty() {
        println!("  {}:", "deny".red());
        for s in &filtered_deny {
            println!("    {}", s);
        }
    }

    true
}

fn collect_settings_files(home: &std::path::Path) -> Vec<(String, std::path::PathBuf)> {
    let mut files = vec![(
        "~/.claude/settings.json".to_string(),
        home.join(".claude").join("settings.json"),
    )];

    if let Ok(config) = load_claude_config() {
        if let Some(projects) = config.projects {
            let mut paths: Vec<&String> = projects.keys().collect();
            paths.sort();

            for project_path in paths {
                let base = Path::new(project_path);
                for filename in &["settings.json", "settings.local.json"] {
                    let settings_path = base.join(".claude").join(filename);
                    if settings_path.exists() {
                        let label = shorten_path(&settings_path.to_string_lossy());
                        files.push((label, settings_path));
                    }
                }
            }
        }
    }

    files
}

fn find_risky_entries(path: &std::path::Path) -> Vec<String> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let allow = value
        .get("permissions")
        .and_then(|p| p.get("allow"))
        .and_then(|v| v.as_array());

    match allow {
        Some(items) => items
            .iter()
            .filter_map(|v| v.as_str())
            .filter(|s| check_risky(s).is_some())
            .map(|s| s.to_string())
            .collect(),
        None => vec![],
    }
}

fn remove_allow_entries(
    path: &std::path::Path,
    entries_to_remove: &[String],
) -> Result<(), String> {
    let content =
        fs::read_to_string(path).map_err(|e| format!("Error reading {}: {}", path.display(), e))?;

    let mut value: serde_json::Value = serde_json::from_str(&content)
        .map_err(|e| format!("Error parsing {}: {}", path.display(), e))?;

    let allow = value
        .get_mut("permissions")
        .and_then(|p| p.get_mut("allow"))
        .and_then(|v| v.as_array_mut());

    if let Some(arr) = allow {
        arr.retain(|v| {
            v.as_str()
                .map(|s| !entries_to_remove.contains(&s.to_string()))
                .unwrap_or(true)
        });
    }

    let pretty = serde_json::to_string_pretty(&value)
        .map_err(|e| format!("Error serializing JSON: {}", e))?;

    fs::write(path, format!("{}\n", pretty))
        .map_err(|e| format!("Error writing {}: {}", path.display(), e))?;

    Ok(())
}

fn permissions_command(filter: Option<String>, audit: bool, clean: bool, execute: bool) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("{}", "Error: Could not find home directory".red());
            std::process::exit(1);
        }
    };

    if clean && !audit {
        eprintln!("{}", "--clean requires --audit".red());
        std::process::exit(1);
    }

    if execute && !clean {
        eprintln!("{}", "--execute requires --clean".red());
        std::process::exit(1);
    }

    let files = collect_settings_files(&home);
    let filter_ref = filter.as_deref();

    if clean {
        // Clean mode: collect risky entries per file, then remove them
        let mut targets: Vec<(String, std::path::PathBuf, Vec<String>)> = Vec::new();

        for (label, path) in &files {
            let mut risky = find_risky_entries(path);
            if let Some(f) = filter_ref {
                risky.retain(|s| s.contains(f));
            }
            if !risky.is_empty() {
                targets.push((label.clone(), path.clone(), risky));
            }
        }

        if targets.is_empty() {
            println!("{}", "No risky patterns found. Nothing to clean.".green());
            return;
        }

        let total: usize = targets.iter().map(|(_, _, entries)| entries.len()).sum();
        println!(
            "Found {} risky entries across {} files:\n",
            total.to_string().yellow(),
            targets.len().to_string().yellow(),
        );

        for (label, _, entries) in &targets {
            println!("{}:", label.cyan());
            for entry in entries {
                let reason = check_risky(entry).unwrap_or("");
                println!("    {}  -- {}", entry.red(), reason.yellow());
            }
            println!();
        }

        if !execute {
            println!(
                "Run with {} {} {} to remove these entries.",
                "--audit".cyan(),
                "--clean".cyan(),
                "--execute".cyan(),
            );
            return;
        }

        // Execute removal
        let mut removed_total = 0;
        for (label, path, entries) in &targets {
            match remove_allow_entries(path, entries) {
                Ok(()) => {
                    println!(
                        "{} {} entries from {}",
                        "Removed".green(),
                        entries.len().to_string().yellow(),
                        label.cyan(),
                    );
                    removed_total += entries.len();
                }
                Err(e) => {
                    eprintln!("{}", e.red());
                }
            }
        }

        println!(
            "\n{} {} risky entries removed.",
            "Done.".green(),
            removed_total.to_string().yellow(),
        );
        return;
    }

    // Normal / audit display mode
    let mut found_any = false;

    for (label, path) in &files {
        if print_permissions(label, path, filter_ref, audit, found_any) {
            found_any = true;
        }
    }

    if !found_any {
        println!("{}", "No permissions found in any settings file.".yellow());
    }
}

fn status_command() {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("{}", "Error: Could not find home directory".red());
            std::process::exit(1);
        }
    };

    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| home.join(".local/share"))
        .join("cckit");

    println!("{}", "cckit Status".bold());
    println!();

    // Claude Code config files
    println!("{}", "Claude Code Files:".cyan());
    let claude_json = home.join(".claude.json");
    println!(
        "  ~/.claude.json              {}",
        get_file_info(&claude_json)
    );

    let settings_json = home.join(".claude").join("settings.json");
    println!(
        "  ~/.claude/settings.json     {}",
        get_file_info(&settings_json)
    );

    println!();

    // cckit data files
    println!("{}", "cckit Data Files:".cyan());
    let sessions_json = data_dir.join("sessions.json");
    println!(
        "  sessions.json               {}",
        get_file_info(&sessions_json)
    );
    if cfg!(target_os = "macos") {
        println!("    Path: ~/Library/Application Support/cckit/sessions.json");
    } else {
        println!("    Path: {}", sessions_json.display());
    }

    println!();

    // cckit config files
    println!("{}", "cckit Config Files:".cyan());
    let cwd_config = std::env::current_dir().ok().map(|p| p.join("config.toml"));
    let global_config = dirs::config_dir().map(|p| p.join("cckit/config.toml"));

    if let Some(ref path) = cwd_config {
        println!("  ./config.toml               {}", get_file_info(path));
    }
    if let Some(ref path) = global_config {
        let display_path = if cfg!(target_os = "macos") {
            "~/.config/cckit/config.toml".to_string()
        } else {
            shorten_path(&path.to_string_lossy())
        };
        println!(
            "  {}  {}",
            format!("{:<28}", display_path),
            get_file_info(path)
        );
    }

    println!();

    // Environment variables
    println!("{}", "Environment Variables:".cyan());
    println!(
        "  {:<24} {}",
        "HOME",
        std::env::var("HOME").unwrap_or_default().green()
    );

    println!();

    // Session summary
    if sessions_json.exists() {
        if let Ok(content) = fs::read_to_string(&sessions_json) {
            if let Ok(store) = serde_json::from_str::<serde_json::Value>(&content) {
                if let Some(sessions) = store.get("sessions").and_then(|s| s.as_object()) {
                    let total = sessions.len();
                    let active = sessions
                        .values()
                        .filter(|s| s.get("status").and_then(|v| v.as_str()) != Some("stopped"))
                        .count();
                    println!("{}", "Session Summary:".cyan());
                    println!("  Total sessions: {}", total);
                    println!("  Active: {}", active);
                }
            }
        }
    }
}

fn doctor_command() {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("{}", "Error: Could not find home directory".red());
            std::process::exit(1);
        }
    };

    println!("{}", "cckit Doctor".bold());
    println!();

    let mut issues: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    // Check ~/.claude.json
    print!("Checking ~/.claude.json ... ");
    let claude_json = home.join(".claude.json");
    if claude_json.exists() {
        println!("{}", "ok".green());
    } else {
        println!("{}", "not found".yellow());
        warnings.push("~/.claude.json not found (no projects registered yet)".to_string());
    }

    // Check ~/.claude/settings.json
    print!("Checking ~/.claude/settings.json ... ");
    let settings_json = home.join(".claude").join("settings.json");
    if !settings_json.exists() {
        println!("{}", "not found".red());
        issues.push("~/.claude/settings.json not found".to_string());
        issues.push("  Run: cckit session install".to_string());
    } else {
        println!("{}", "ok".green());

        // Check hooks configuration
        match fs::read_to_string(&settings_json) {
            Ok(content) => match serde_json::from_str::<serde_json::Value>(&content) {
                Ok(settings) => {
                    let hooks = settings.get("hooks");
                    let hook_events = [
                        "SessionStart",
                        "SessionEnd",
                        "UserPromptSubmit",
                        "PreToolUse",
                        "PostToolUse",
                        "Stop",
                    ];

                    let mut missing_hooks = Vec::new();

                    for event in hook_events {
                        print!("Checking {} hook ... ", event);
                        let has_hook = hooks
                            .and_then(|h| h.get(event))
                            .and_then(|arr| arr.as_array())
                            .map(|arr| {
                                arr.iter().any(|item| {
                                    item.get("hooks")
                                        .and_then(|h| h.as_array())
                                        .map(|hooks| {
                                            hooks.iter().any(|hook| {
                                                hook.get("command")
                                                    .and_then(|c| c.as_str())
                                                    .map(|c| c.contains("cckit session hook"))
                                                    .unwrap_or(false)
                                            })
                                        })
                                        .unwrap_or(false)
                                })
                            })
                            .unwrap_or(false);

                        if has_hook {
                            println!("{}", "ok".green());
                        } else {
                            println!("{}", "not configured".yellow());
                            missing_hooks.push(event);
                        }
                    }

                    if !missing_hooks.is_empty() {
                        issues.push(format!("Missing hooks: {}", missing_hooks.join(", ")));
                        issues.push("  Run: cckit session install".to_string());
                    }
                }
                Err(e) => {
                    issues.push(format!("Failed to parse settings.json: {}", e));
                }
            },
            Err(e) => {
                issues.push(format!("Failed to read settings.json: {}", e));
            }
        }
    }

    // Check data directory
    print!("Checking data directory ... ");
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| home.join(".local/share"))
        .join("cckit");
    if data_dir.exists() {
        println!("{}", "ok".green());
    } else {
        println!("{} (will be created on first use)", "not found".yellow());
        warnings.push(format!(
            "Data directory not found: {} (will be created on first use)",
            data_dir.display()
        ));
    }

    println!();

    // Print summary
    if issues.is_empty() && warnings.is_empty() {
        println!("{} All checks passed!", "✓".green());
    } else {
        if !issues.is_empty() {
            println!("{}", "Issues:".red().bold());
            for issue in &issues {
                println!("  {} {}", "✗".red(), issue);
            }
            println!();
        }

        if !warnings.is_empty() {
            println!("{}", "Warnings:".yellow().bold());
            for warning in &warnings {
                println!("  {} {}", "!".yellow(), warning);
            }
            println!();
        }

        if !issues.is_empty() {
            std::process::exit(1);
        }
    }
}

fn prune_command(execute: bool, no_backup: bool) {
    let home = match dirs::home_dir() {
        Some(h) => h,
        None => {
            eprintln!("{}", "Error: Could not find home directory".red());
            std::process::exit(1);
        }
    };

    let config_path = home.join(".claude.json");

    // Read the config file as raw JSON to preserve structure
    let content = match fs::read_to_string(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}: {}", "Error reading ~/.claude.json".red(), e);
            std::process::exit(1);
        }
    };

    let mut json: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{}: {}", "Error parsing ~/.claude.json".red(), e);
            std::process::exit(1);
        }
    };

    // Get projects section
    let projects = match json.get("projects").and_then(|v| v.as_object()) {
        Some(p) => p,
        None => {
            println!("No projects found in ~/.claude.json");
            return;
        }
    };

    // Find non-existent paths
    let non_existent: Vec<String> = projects
        .keys()
        .filter(|path| !Path::new(path).exists())
        .cloned()
        .collect();

    if non_existent.is_empty() {
        println!("{}", "All project paths exist. Nothing to prune.".green());
        return;
    }

    // Display non-existent paths
    println!(
        "Found {} non-existent paths in ~/.claude.json:",
        non_existent.len().to_string().yellow()
    );
    for path in &non_existent {
        println!("  {} {}", "-".dimmed(), path.red());
    }

    if !execute {
        println!();
        println!("Run with {} to remove these paths.", "--execute".cyan());
        return;
    }

    // Create backup unless --no-backup is specified
    if !no_backup {
        let backup_path = home.join(".claude.json.bak");
        if let Err(e) = fs::copy(&config_path, &backup_path) {
            eprintln!("{}: {}", "Error creating backup".red(), e);
            std::process::exit(1);
        }
        println!();
        println!(
            "{} {}",
            "Backup created:".green(),
            backup_path.display().to_string().dimmed()
        );
    }

    // Remove non-existent paths from JSON
    if let Some(projects_obj) = json.get_mut("projects").and_then(|v| v.as_object_mut()) {
        for path in &non_existent {
            projects_obj.remove(path);
        }
    }

    // Write back to file
    let pretty = match serde_json::to_string_pretty(&json) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{}: {}", "Error serializing JSON".red(), e);
            std::process::exit(1);
        }
    };

    if let Err(e) = fs::write(&config_path, pretty) {
        eprintln!("{}: {}", "Error writing ~/.claude.json".red(), e);
        std::process::exit(1);
    }

    println!(
        "{} {} non-existent paths from ~/.claude.json",
        "Removed".green(),
        non_existent.len().to_string().yellow()
    );
}

struct LsOptions {
    all: bool,
    path_filter: Option<String>,
    duplicates: bool,
    no_skills: bool,
    no_agents: bool,
    no_mcp: bool,
    no_commands: bool,
    mcp_filter: Option<String>,
    skill_filter: Option<String>,
}

fn ls_command(opts: LsOptions) {
    let config = match load_claude_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{}: {}", "Error loading ~/.claude.json".red(), e);
            std::process::exit(1);
        }
    };

    let cckit_config = load_cckit_config();

    let projects = match config.projects {
        Some(projects) => projects,
        None => {
            println!("No projects found in ~/.claude.json");
            return;
        }
    };

    // Get global ~/.claude info
    let global_info = get_global_info();

    // Get home directory path for deduplication
    let home_path = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    // Collect projects and track seen git remotes for deduplication
    // Maps remote URL to the path of the first project with that remote
    let mut seen_remotes: HashMap<String, String> = HashMap::new();
    let mut project_infos: Vec<ProjectInfo> = Vec::new();
    let mut duplicates: Vec<(String, String)> = Vec::new(); // (skipped_path, kept_path)

    let filtered_paths: Vec<&String> = projects
        .keys()
        .filter(|path| {
            // Skip home directory (already shown as global)
            if *path == &home_path {
                return false;
            }
            // Skip disabled paths from config.toml
            if is_path_disabled(path, &cckit_config.disable_paths) {
                return false;
            }
            if let Some(ref filter) = opts.path_filter {
                path.contains(filter)
            } else {
                true
            }
        })
        .collect();

    // Build project infos first, then deduplicate
    let mut all_infos: Vec<(ProjectInfo, Option<String>)> = filtered_paths
        .iter()
        .map(|path| {
            let info = get_project_info(path);
            let remote = get_git_remote_url(path);
            (info, remote)
        })
        .collect();

    // Sort: projects with skills/agents/commands first, then mcp, then by path length
    all_infos.sort_by(|a, b| {
        // Priority: skills/agents/commands > mcp only > nothing
        fn priority(info: &ProjectInfo) -> u8 {
            if !info.skills.is_empty() || !info.agents.is_empty() || !info.commands.is_empty() {
                0 // highest priority
            } else if !info.mcp_servers.is_empty() {
                1
            } else {
                2 // lowest priority
            }
        }
        let a_priority = priority(&a.0);
        let b_priority = priority(&b.0);
        match a_priority.cmp(&b_priority) {
            std::cmp::Ordering::Equal => a.0.path.len().cmp(&b.0.path.len()),
            other => other,
        }
    });

    // Filter by MCP server name if specified
    let all_infos: Vec<(ProjectInfo, Option<String>)> =
        if let Some(ref mcp_filter) = opts.mcp_filter {
            all_infos
                .into_iter()
                .filter(|(info, _)| {
                    info.mcp_servers
                        .iter()
                        .any(|server| server.name.contains(mcp_filter.as_str()))
                })
                .collect()
        } else {
            all_infos
        };

    // Filter by skill name if specified
    let all_infos: Vec<(ProjectInfo, Option<String>)> =
        if let Some(ref skill_filter) = opts.skill_filter {
            all_infos
                .into_iter()
                .filter(|(info, _)| {
                    info.skills
                        .iter()
                        .any(|skill| skill.name.contains(skill_filter.as_str()))
                })
                .collect()
        } else {
            all_infos
        };

    // Deduplicate by git remote
    for (info, remote) in all_infos {
        if let Some(ref url) = remote {
            if let Some(kept_path) = seen_remotes.get(url) {
                duplicates.push((info.path.clone(), kept_path.clone()));
                continue;
            }
            seen_remotes.insert(url.clone(), info.path.clone());
        }
        project_infos.push(info);
    }

    project_infos.sort_by(|a, b| a.path.cmp(&b.path));

    // Check if project has visible content based on options
    let has_visible_content = |p: &ProjectInfo| {
        (!opts.no_skills && !p.skills.is_empty())
            || (!opts.no_agents && !p.agents.is_empty())
            || (!opts.no_commands && !p.commands.is_empty())
            || (!opts.no_mcp && !p.mcp_servers.is_empty())
            || !p.plugins.is_empty() // plugins always shown if present
    };

    // Check if global matches filter criteria
    let global_matches_filters = {
        let mcp_ok = opts.mcp_filter.as_ref().map_or(true, |filter| {
            global_info
                .mcp_servers
                .iter()
                .any(|s| s.name.contains(filter.as_str()))
        });
        let skill_ok = opts.skill_filter.as_ref().map_or(true, |filter| {
            global_info
                .skills
                .iter()
                .any(|s| s.name.contains(filter.as_str()))
        });
        mcp_ok && skill_ok
    };

    let has_global_content = has_visible_content(&global_info) && global_matches_filters;

    let display_infos: Vec<&ProjectInfo> = if opts.all {
        project_infos.iter().collect()
    } else {
        project_infos
            .iter()
            .filter(|p| has_visible_content(p))
            .collect()
    };

    let total_with_content = display_infos.len() + if has_global_content { 1 } else { 0 };

    if !has_global_content && display_infos.is_empty() {
        println!("No projects with visible content found.");
        println!("Use {} to show all projects.", "--all".cyan());
        return;
    }

    println!(
        "{} projects ({} with content)\n",
        (projects.len() + 1).to_string().cyan(),
        total_with_content.to_string().green()
    );

    // Print global first
    if has_global_content || opts.all {
        print_project(&global_info, &opts);
    }

    // Print project infos
    for info in display_infos {
        print_project(info, &opts);
    }

    // Print duplicates
    if opts.duplicates && !duplicates.is_empty() {
        duplicates.sort_by(|a, b| a.0.cmp(&b.0));
        println!("{}", "Duplicates (same git remote):".dimmed());
        for (skipped, kept) in &duplicates {
            println!("  {} {} {}", skipped.dimmed(), "->".dimmed(), kept.dimmed());
        }
    }
}

fn parse_hook_notification(hook_json: &serde_json::Value, default_title: &str) -> (String, String) {
    // Extract project name from cwd
    let title = hook_json
        .get("cwd")
        .and_then(|v| v.as_str())
        .and_then(|cwd| std::path::Path::new(cwd).file_name())
        .and_then(|name| name.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| default_title.to_string());

    let event = hook_json
        .get("hook_event_name")
        .and_then(|v| v.as_str())
        .unwrap_or("Unknown");

    // Build message based on event type
    let message = match event {
        "Stop" => build_stop_message(hook_json),
        "SessionStart" => "Session started".to_string(),
        "SessionEnd" => "Session ended".to_string(),
        _ => format!("Event: {}", event),
    };

    (title, message)
}

fn build_stop_message(hook_json: &serde_json::Value) -> String {
    let mut parts = Vec::new();

    // Add cwd (shortened)
    if let Some(cwd) = hook_json.get("cwd").and_then(|v| v.as_str()) {
        parts.push(format!("📁 {}", shorten_path(cwd)));
    }

    // Get session info from storage
    let session_id = hook_json.get("session_id").and_then(|v| v.as_str());
    if let Some(sid) = session_id {
        let storage = monitor::storage::Storage::new();
        let store = storage.load();

        // Find session by session_id (key format is "session_id:tty")
        if let Some((_, session)) = store.sessions.iter().find(|(k, _)| k.starts_with(sid)) {
            // Duration
            let duration = chrono::Utc::now().signed_duration_since(session.created_at);
            let duration_str = format_duration(duration);
            parts.push(format!("⏱ {}", duration_str));

            // Last tool
            if let Some(ref tool) = session.last_tool {
                let tool_info = if let Some(ref input) = session.last_tool_input {
                    format!("[{}] {}", tool, input)
                } else {
                    format!("[{}]", tool)
                };
                parts.push(tool_info);
            }

            // PID
            if let Some(pid) = session.pid {
                parts.push(format!("pid:{}", pid));
            }
        }
    }

    // Get last assistant message from transcript
    if let Some(transcript_path) = hook_json.get("transcript_path").and_then(|v| v.as_str()) {
        if let Some(msg) = get_last_assistant_message(transcript_path) {
            parts.push(msg);
        }
    }

    if parts.is_empty() {
        "Done".to_string()
    } else {
        parts.join("\n")
    }
}

fn format_duration(duration: chrono::Duration) -> String {
    let secs = duration.num_seconds();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn get_last_assistant_message(transcript_path: &str) -> Option<String> {
    let content = fs::read_to_string(transcript_path).ok()?;

    // Parse JSONL from the end and find last assistant message
    for line in content.lines().rev() {
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(json) = serde_json::from_str::<serde_json::Value>(line) {
            if json.get("type").and_then(|t| t.as_str()) == Some("assistant") {
                if let Some(content_arr) = json
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                {
                    for item in content_arr {
                        if item.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(text) = item.get("text").and_then(|t| t.as_str()) {
                                // Take first 3 lines, truncate to 200 chars
                                let summary: String =
                                    text.lines().take(3).collect::<Vec<_>>().join("\n");
                                let chars: Vec<char> = summary.chars().collect();
                                if chars.len() > 200 {
                                    return Some(format!(
                                        "{}...",
                                        chars[..200].iter().collect::<String>()
                                    ));
                                }
                                return Some(summary);
                            }
                        }
                    }
                }
            }
        }
    }

    None
}

pub fn run() {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Ls {
            all,
            path_filter,
            duplicates,
            no_skills,
            no_agents,
            no_mcp,
            no_commands,
            mcp_filter,
            skill_filter,
        }) => {
            ls_command(LsOptions {
                all,
                path_filter,
                duplicates,
                no_skills,
                no_agents,
                no_mcp,
                no_commands,
                mcp_filter,
                skill_filter,
            });
        }
        Some(Commands::Prune { execute, no_backup }) => {
            prune_command(execute, no_backup);
        }
        Some(Commands::Config { key, raw }) => {
            config_command(key, raw);
        }
        Some(Commands::Skill { command }) => match command {
            SkillCommands::Copy {
                filter,
                from,
                name,
                force,
            } => {
                skill_copy_command(filter, from, name, force);
            }
        },
        Some(Commands::Mcp { command }) => match command {
            McpCommands::Copy {
                filter,
                from,
                name,
                force,
            } => {
                mcp_copy_command(filter, from, name, force);
            }
        },
        Some(Commands::Status) => {
            status_command();
        }
        Some(Commands::Doctor) => {
            doctor_command();
        }
        Some(Commands::Permissions {
            filter,
            audit,
            clean,
            execute,
        }) => {
            permissions_command(filter, audit, clean, execute);
        }
        Some(Commands::Session { command }) => {
            match command {
                Some(SessionCommands::Ls {
                    text,
                    menubar,
                    no_tui,
                    icon_size,
                    check_interval,
                    poll_interval,
                    menu_update_interval,
                    event_timeout,
                }) => {
                    if text {
                        monitor::print_sessions_list();
                    } else if no_tui {
                        #[cfg(target_os = "macos")]
                        {
                            monitor::menubar::set_icon_size(icon_size as f64);
                            monitor::menubar::set_update_interval(menu_update_interval);
                            if let Err(e) =
                                monitor::menubar::run_menubar_with_polling(poll_interval)
                            {
                                eprintln!("{}: {}", "Error running menubar".red(), e);
                                std::process::exit(1);
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
                        {
                            eprintln!("{}", "Menubar is only supported on macOS".red());
                            std::process::exit(1);
                        }
                    } else {
                        #[cfg(target_os = "macos")]
                        if menubar {
                            monitor::menubar::set_icon_size(icon_size as f64);
                            monitor::menubar::set_update_interval(menu_update_interval);
                        }

                        #[cfg(not(target_os = "macos"))]
                        if menubar {
                            eprintln!(
                                "{}",
                                "Warning: --menubar is only supported on macOS, ignoring".yellow()
                            );
                        }

                        let tui_config = monitor::tui::TuiConfig {
                            check_interval_ms: check_interval,
                            poll_interval_ms: poll_interval,
                            menu_update_interval_ms: menu_update_interval,
                            event_timeout_ms: event_timeout,
                        };

                        let result = if menubar {
                            monitor::tui::run_tui_with_menubar(tui_config)
                        } else {
                            monitor::tui::run_tui(tui_config)
                        };
                        if let Err(e) = result {
                            eprintln!("{}: {}", "Error running TUI".red(), e);
                            std::process::exit(1);
                        }
                    }
                }
                Some(SessionCommands::Hook) => {
                    if let Err(e) = monitor::hook::handle_hook() {
                        monitor::hook::log_error("hook", &e.to_string());
                        std::process::exit(1);
                    }
                }
                Some(SessionCommands::Install { force }) => {
                    if let Err(e) = monitor::setup::run_install(force) {
                        eprintln!("{}: {}", "Error installing hooks".red(), e);
                        std::process::exit(1);
                    }
                }
                Some(SessionCommands::Status) => {
                    if let Err(e) = monitor::setup::show_status() {
                        eprintln!("{}: {}", "Error showing status".red(), e);
                        std::process::exit(1);
                    }
                }
                Some(SessionCommands::Uninstall) => {
                    if let Err(e) = monitor::setup::run_uninstall() {
                        eprintln!("{}: {}", "Error uninstalling hooks".red(), e);
                        std::process::exit(1);
                    }
                }
                Some(SessionCommands::Sync { execute }) => {
                    run_sync(execute);
                }
                Some(SessionCommands::Focus { project }) => {
                    match monitor::focus::focus_ghostty_tab(&project) {
                        Ok(true) => {
                            println!("{}", format!("Focused tab matching '{}'", project).green())
                        }
                        Ok(false) => {
                            eprintln!(
                                "{}",
                                format!("No tab found matching '{}'", project).yellow()
                            );
                            std::process::exit(1);
                        }
                        Err(e) => {
                            eprintln!("{}: {}", "Error focusing tab".red(), e);
                            std::process::exit(1);
                        }
                    }
                }
                #[cfg(target_os = "macos")]
                Some(SessionCommands::DumpUi) => match monitor::focus::ax::dump_ghostty_ui_tree() {
                    Ok(tree) => println!("{}", tree),
                    Err(e) => {
                        eprintln!("{}: {}", "Error dumping UI tree".red(), e);
                        std::process::exit(1);
                    }
                },
                #[cfg(not(target_os = "macos"))]
                Some(SessionCommands::DumpUi) => {
                    eprintln!("DumpUi is only supported on macOS");
                    std::process::exit(1);
                }
                #[cfg(target_os = "macos")]
                Some(SessionCommands::Menubar) => {
                    if let Err(e) = monitor::menubar::run_menubar() {
                        eprintln!("{}: {}", "Error running menubar".red(), e);
                        std::process::exit(1);
                    }
                }
                #[cfg(not(target_os = "macos"))]
                Some(SessionCommands::Menubar) => {
                    eprintln!("Menubar is only supported on macOS");
                    std::process::exit(1);
                }
                None => {
                    // Default: same as `session ls -m`
                    #[cfg(target_os = "macos")]
                    {
                        monitor::menubar::set_icon_size(24.0);
                        monitor::menubar::set_update_interval(2000);
                        let result =
                            monitor::tui::run_tui_with_menubar(monitor::tui::TuiConfig::default());
                        if let Err(e) = result {
                            eprintln!("{}: {}", "Error running TUI".red(), e);
                            std::process::exit(1);
                        }
                    }
                    #[cfg(not(target_os = "macos"))]
                    {
                        if let Err(e) = monitor::tui::run_tui(monitor::tui::TuiConfig::default()) {
                            eprintln!("{}: {}", "Error running TUI".red(), e);
                            std::process::exit(1);
                        }
                    }
                }
            }
        }
        #[cfg(target_os = "macos")]
        Some(Commands::App {
            menubar_only,
            window_only,
        }) => {
            if let Err(e) = monitor::window::run_app(menubar_only, window_only) {
                eprintln!("{}: {}", "Error running app".red(), e);
                std::process::exit(1);
            }
        }
        #[cfg(not(target_os = "macos"))]
        Some(Commands::App { .. }) => {
            eprintln!("App is only supported on macOS");
            std::process::exit(1);
        }
        #[cfg(target_os = "macos")]
        Some(Commands::Notify {
            title,
            subtitle,
            message,
            sound,
            duration,
            width,
            height,
            position,
            margin,
            opacity,
            bgcolor,
        }) => {
            // Determine message: use -m if provided, otherwise read from stdin
            let (final_title, final_message) = if let Some(m) = message {
                // Explicit message provided, use as-is
                (title, m)
            } else {
                // Read stdin only when -m is not provided
                use std::io::Read;
                let mut stdin_buf = String::new();
                let _ = std::io::stdin().read_to_string(&mut stdin_buf);
                let stdin_content = stdin_buf.trim();

                if !stdin_content.is_empty() {
                    // Try to parse as hook JSON
                    if let Ok(hook_json) = serde_json::from_str::<serde_json::Value>(stdin_content)
                    {
                        if hook_json.get("hook_event_name").is_some() {
                            // It's a hook JSON, parse it nicely
                            parse_hook_notification(&hook_json, &title)
                        } else {
                            // Regular JSON, just display as message
                            (title, stdin_content.to_string())
                        }
                    } else {
                        // Not JSON, use as plain message
                        (title, stdin_content.to_string())
                    }
                } else {
                    eprintln!(
                        "{}: No message provided (use -m or pipe to stdin)",
                        "Error".red()
                    );
                    std::process::exit(1);
                }
            };

            let pos = match monitor::notification::Position::parse(&position) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{}: {}", "Error".red(), e);
                    std::process::exit(1);
                }
            };
            let opts = monitor::notification::NotifyOptions {
                title: final_title,
                subtitle,
                message: final_message,
                sound,
                duration_ms: duration,
                width: Some(width),
                height,
                position: pos,
                margin,
                opacity,
                bgcolor: Some(bgcolor),
            };
            if let Err(e) = monitor::notification::send_notify(opts) {
                eprintln!("{}: {}", "Error sending notification".red(), e);
                std::process::exit(1);
            }
        }
        #[cfg(not(target_os = "macos"))]
        Some(Commands::Notify { .. }) => {
            eprintln!("Notify is only supported on macOS");
            std::process::exit(1);
        }
        None => {
            // Show help when no subcommand is provided
            use clap::CommandFactory;
            Cli::command().print_help().unwrap();
            println!();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_frontmatter_valid() {
        let content = r#"---
name: my-skill
description: This is a test skill
---

# Content here
"#;
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, Some("my-skill".to_string()));
        assert_eq!(desc, Some("This is a test skill".to_string()));
    }

    #[test]
    fn test_parse_frontmatter_no_frontmatter() {
        let content = "# Just markdown\nNo frontmatter here";
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, None);
        assert_eq!(desc, None);
    }

    #[test]
    fn test_parse_frontmatter_incomplete() {
        let content = "---\nname: test\n";
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, None);
        assert_eq!(desc, None);
    }

    #[test]
    fn test_parse_frontmatter_name_only() {
        let content = r#"---
name: agent-name
---
"#;
        let (name, desc) = parse_frontmatter(content);
        assert_eq!(name, Some("agent-name".to_string()));
        assert_eq!(desc, None);
    }

    #[test]
    fn test_get_project_info_nonexistent() {
        let info = get_project_info("/nonexistent/path");
        assert!(!info.exists);
        assert!(info.skills.is_empty());
        assert!(info.agents.is_empty());
    }

    #[test]
    fn test_normalize_git_url() {
        assert_eq!(
            normalize_git_url("https://github.com/user/repo.git"),
            "https://github.com/user/repo"
        );
        assert_eq!(
            normalize_git_url("https://github.com/user/repo"),
            "https://github.com/user/repo"
        );
        assert_eq!(
            normalize_git_url("HTTPS://GitHub.com/User/Repo.GIT"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn test_truncate_str() {
        assert_eq!(truncate_str("hello", 10), "hello");
        assert_eq!(truncate_str("hello world", 5), "hello...");
        assert_eq!(truncate_str("", 5), "");
        assert_eq!(truncate_str("abc", 3), "abc");
        assert_eq!(truncate_str("abcd", 3), "abc...");
    }

    #[test]
    fn test_truncate_str_unicode() {
        assert_eq!(truncate_str("こんにちは", 3), "こんに...");
        assert_eq!(truncate_str("日本語", 5), "日本語");
    }

    #[test]
    fn test_is_path_disabled_prefix() {
        let patterns = vec!["/tmp/".to_string(), "/var/log".to_string()];
        assert!(is_path_disabled("/tmp/foo", &patterns));
        assert!(is_path_disabled("/var/log/syslog", &patterns));
        assert!(!is_path_disabled("/home/user", &patterns));
    }

    #[test]
    fn test_is_path_disabled_glob() {
        let patterns = vec!["*.tmp".to_string(), "/home/*/Downloads/*".to_string()];
        assert!(is_path_disabled("file.tmp", &patterns));
        assert!(is_path_disabled("/home/user/Downloads/file.zip", &patterns));
        assert!(!is_path_disabled(
            "/home/user/Documents/file.txt",
            &patterns
        ));
    }

    #[test]
    fn test_format_duration_seconds() {
        assert_eq!(format_duration(chrono::Duration::seconds(0)), "0s");
        assert_eq!(format_duration(chrono::Duration::seconds(30)), "30s");
        assert_eq!(format_duration(chrono::Duration::seconds(59)), "59s");
    }

    #[test]
    fn test_format_duration_minutes() {
        assert_eq!(format_duration(chrono::Duration::seconds(60)), "1m 0s");
        assert_eq!(format_duration(chrono::Duration::seconds(90)), "1m 30s");
        assert_eq!(format_duration(chrono::Duration::seconds(3599)), "59m 59s");
    }

    #[test]
    fn test_format_duration_hours() {
        assert_eq!(format_duration(chrono::Duration::seconds(3600)), "1h 0m");
        assert_eq!(format_duration(chrono::Duration::seconds(3660)), "1h 1m");
        assert_eq!(format_duration(chrono::Duration::seconds(7200)), "2h 0m");
    }
}
