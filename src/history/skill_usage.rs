// Skill firing history: mine Skill tool invocations from ~/.claude/projects/**/*.jsonl
// to compute, per skill, the last time it fired and how many times.

use chrono::{DateTime, Utc};
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// Aggregated firing info for a single skill (by invocation name).
#[derive(Debug, Clone)]
pub struct SkillFiring {
    pub last_fired: DateTime<Utc>,
    pub count: u32,
}

/// Extract `(skill_name, timestamp)` from one transcript JSONL line that records a
/// skill firing. Two firing forms are recognized:
///   1. Skill tool_use (model-invoked): `"name":"Skill","input":{"skill":"<name>",...}`
///   2. Slash command (user-invoked):   a message containing
///      `<command-name><name></command-name>`
///
/// Returns None when the line is neither, or lacks a parseable `timestamp`. The
/// returned name is matched against installed skills later, so non-skill slash
/// commands (e.g. `/exit`) are harmlessly ignored downstream.
pub fn parse_skill_firings_line(line: &str) -> Option<(String, DateTime<Utc>)> {
    // Cheap pre-filter before JSON parse.
    let has_skill_tool = line.contains("\"name\":\"Skill\"");
    let has_command = line.contains("<command-name>");
    if !has_skill_tool && !has_command {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(line).ok()?;
    let name = find_skill_invocation_name(&v).or_else(|| find_command_name(&v))?;
    let ts_str = v.get("timestamp").and_then(|t| t.as_str())?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .ok()?
        .with_timezone(&Utc);
    Some((name, ts))
}

/// Extract the name from a `<command-name>...</command-name>` slash-command marker
/// found anywhere in the JSON value's string fields.
fn find_command_name(v: &serde_json::Value) -> Option<String> {
    fn walk(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::String(s) => command_name_in(s),
            serde_json::Value::Object(map) => map.values().find_map(walk),
            serde_json::Value::Array(arr) => arr.iter().find_map(walk),
            _ => None,
        }
    }
    walk(v)
}

/// Pull `X` out of the first `<command-name>X</command-name>` in `s`.
fn command_name_in(s: &str) -> Option<String> {
    let open = "<command-name>";
    let close = "</command-name>";
    let start = s.find(open)? + open.len();
    let rest = &s[start..];
    let end = rest.find(close)?;
    let name = rest[..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

/// Find `input.skill` of a `name == "Skill"` tool_use anywhere in the message content.
fn find_skill_invocation_name(v: &serde_json::Value) -> Option<String> {
    // The tool_use block is usually under message.content[]; search defensively.
    fn walk(v: &serde_json::Value) -> Option<String> {
        match v {
            serde_json::Value::Object(map) => {
                if map.get("name").and_then(|n| n.as_str()) == Some("Skill")
                    && let Some(skill) = map
                        .get("input")
                        .and_then(|i| i.get("skill"))
                        .and_then(|s| s.as_str())
                {
                    return Some(skill.to_string());
                }
                for val in map.values() {
                    if let Some(found) = walk(val) {
                        return Some(found);
                    }
                }
                None
            }
            serde_json::Value::Array(arr) => arr.iter().find_map(walk),
            _ => None,
        }
    }
    walk(v)
}

/// Fold one firing into an aggregate: bump the count, keep the latest timestamp.
fn record_firing(map: &mut HashMap<String, SkillFiring>, name: String, ts: DateTime<Utc>) {
    map.entry(name)
        .and_modify(|f| {
            f.count += 1;
            if ts > f.last_fired {
                f.last_fired = ts;
            }
        })
        .or_insert(SkillFiring {
            last_fired: ts,
            count: 1,
        });
}

/// Combine two partial aggregates. Counts add and the later firing wins, both of which
/// are associative and commutative, so any split of the corpus yields the same result.
fn merge_firings(
    mut acc: HashMap<String, SkillFiring>,
    other: HashMap<String, SkillFiring>,
) -> HashMap<String, SkillFiring> {
    for (name, firing) in other {
        acc.entry(name)
            .and_modify(|f| {
                f.count += firing.count;
                if firing.last_fired > f.last_fired {
                    f.last_fired = firing.last_fired;
                }
            })
            .or_insert(firing);
    }
    acc
}

/// Collect every `*.jsonl` path under `root` (recursively, ignoring unreadable dirs).
/// Exposed separately from aggregation so a caller can learn the total up front and
/// report progress against it.
pub fn collect_jsonl_paths(root: &Path) -> Vec<std::path::PathBuf> {
    let mut paths = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                paths.push(path);
            }
        }
    }
    paths
}

/// Fold the Skill firings in every path into a per-name aggregate, calling `on_file` once
/// per file. Parsing runs on rayon's pool and the partial maps merge through
/// `merge_firings`.
pub fn aggregate_paths(
    paths: &[std::path::PathBuf],
    on_file: impl Fn() + Sync + Send,
) -> HashMap<String, SkillFiring> {
    paths
        .par_iter()
        .map(|path| {
            let mut map: HashMap<String, SkillFiring> = HashMap::new();
            let content = fs::read_to_string(path);
            on_file();
            let Ok(content) = content else {
                return map;
            };
            for line in content.lines() {
                if let Some((name, ts)) = parse_skill_firings_line(line) {
                    record_firing(&mut map, name, ts);
                }
            }
            map
        })
        .reduce(HashMap::new, merge_firings)
}

/// Walk every `*.jsonl` under `root`, folding Skill firings into a per-name aggregate.
pub fn aggregate_firings(root: &Path) -> HashMap<String, SkillFiring> {
    aggregate_paths(&collect_jsonl_paths(root), || {})
}

/// Aggregate firings from the default transcript root (`~/.claude/projects`).
pub fn scan_skill_firings() -> HashMap<String, SkillFiring> {
    let Some(home) = dirs::home_dir() else {
        return HashMap::new();
    };
    aggregate_firings(&home.join(".claude/projects"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_skill_invocation_line() {
        let line = r#"{"message":{"content":[{"type":"tool_use","name":"Skill","input":{"skill":"daily-report","args":"x"}}]},"timestamp":"2026-05-12T06:55:19.111Z"}"#;
        let (name, ts) = parse_skill_firings_line(line).unwrap();
        assert_eq!(name, "daily-report");
        assert_eq!(ts.to_rfc3339(), "2026-05-12T06:55:19.111+00:00");
    }

    #[test]
    fn keeps_namespaced_name_verbatim() {
        let line = r#"{"content":[{"name":"Skill","input":{"skill":"superpowers:brainstorming"}}],"timestamp":"2026-01-01T00:00:00Z"}"#;
        let (name, _) = parse_skill_firings_line(line).unwrap();
        assert_eq!(name, "superpowers:brainstorming");
    }

    #[test]
    fn parses_a_slash_command_invocation() {
        let line = r#"{"message":{"role":"user","content":[{"type":"text","text":"<command-name>claude-md-improver</command-name>"}]},"timestamp":"2026-05-31T09:00:00Z"}"#;
        let (name, ts) = parse_skill_firings_line(line).unwrap();
        assert_eq!(name, "claude-md-improver");
        assert_eq!(ts.to_rfc3339(), "2026-05-31T09:00:00+00:00");
    }

    #[test]
    fn command_name_in_extracts_first() {
        assert_eq!(
            command_name_in("x <command-name>foo</command-name> y").as_deref(),
            Some("foo")
        );
        assert_eq!(command_name_in("no marker"), None);
    }

    #[test]
    fn non_skill_line_returns_none() {
        let line = r#"{"message":{"content":[{"name":"Bash","input":{"command":"ls"}}]},"timestamp":"2026-05-12T06:55:19.111Z"}"#;
        assert!(parse_skill_firings_line(line).is_none());
    }

    #[test]
    fn missing_timestamp_returns_none() {
        let line = r#"{"content":[{"name":"Skill","input":{"skill":"x"}}]}"#;
        assert!(parse_skill_firings_line(line).is_none());
    }

    #[test]
    fn aggregate_takes_max_timestamp_and_counts() {
        // two firings of the same skill across lines; last_fired is the max.
        let dir = std::env::temp_dir().join(format!("cckit_skillusage_{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let file = dir.join("t.jsonl");
        let body = [
            r#"{"content":[{"name":"Skill","input":{"skill":"foo"}}],"timestamp":"2026-01-01T00:00:00Z"}"#,
            r#"{"content":[{"name":"Skill","input":{"skill":"foo"}}],"timestamp":"2026-03-01T00:00:00Z"}"#,
        ]
        .join("\n");
        fs::write(&file, body).unwrap();
        let agg = aggregate_firings(&dir);
        let f = agg.get("foo").unwrap();
        assert_eq!(f.count, 2);
        assert_eq!(f.last_fired.to_rfc3339(), "2026-03-01T00:00:00+00:00");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn merging_partial_maps_sums_counts_and_keeps_the_latest_firing() {
        let ts = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        let early = ts("2026-01-01T00:00:00Z");
        let late = ts("2026-03-01T00:00:00Z");

        // Two workers each saw part of the corpus.
        let mut a = HashMap::new();
        record_firing(&mut a, "foo".to_string(), late);
        let mut b = HashMap::new();
        record_firing(&mut b, "foo".to_string(), early);
        record_firing(&mut b, "bar".to_string(), early);

        let merged = merge_firings(a, b);

        // Counts add up, and the later timestamp wins regardless of merge order.
        let foo = merged.get("foo").unwrap();
        assert_eq!(foo.count, 2);
        assert_eq!(foo.last_fired, late);
        assert_eq!(merged.get("bar").unwrap().count, 1);
        assert_eq!(merged.len(), 2);
    }

    #[test]
    fn merging_is_order_independent() {
        let ts = |s: &str| s.parse::<DateTime<Utc>>().unwrap();
        let early = ts("2026-01-01T00:00:00Z");
        let late = ts("2026-03-01T00:00:00Z");

        let build = |t| {
            let mut m = HashMap::new();
            record_firing(&mut m, "foo".to_string(), t);
            m
        };

        let forward = merge_firings(build(early), build(late));
        let backward = merge_firings(build(late), build(early));

        assert_eq!(forward["foo"].count, backward["foo"].count);
        assert_eq!(forward["foo"].last_fired, backward["foo"].last_fired);
        assert_eq!(forward["foo"].last_fired, late);
    }
}
