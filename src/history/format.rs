// Text and JSON output formatters for search hits.

use crate::history::{Hit, MatchMode};
use std::time::Duration;

const MAX_TEXT_LEN: usize = 120;
/// Matched prompts printed per session; the rest are counted, not shown.
const MAX_HIT_LINES: usize = 3;
/// Characters kept before a term that sits too deep in a prompt to show from its start.
const CONTEXT_BEFORE: usize = 40;

/// Format a single hit (no trailing newline): a heading, the session line, then the
/// first prompt, the matched prompts and the last prompt, in session order.
///
/// `terms` places each matched prompt's excerpt around the earliest term, so a match
/// deep inside a long prompt is still visible. Pass none for fuzzy hits, whose query
/// is not a substring of the text.
pub fn format_hit(hit: &Hit, terms: &[String]) -> String {
    let session = &hit.session;
    let branch = session
        .git_branch
        .as_ref()
        .map(|b| format!(" ({b})"))
        .unwrap_or_default();
    let started = session.started_at.format("%Y-%m-%d %H:%M");
    let first = session
        .first_user_text()
        .map(truncate_oneline)
        .unwrap_or_else(|| "(no user text)".into());
    let last = session
        .last_user_text()
        .map(truncate_oneline)
        .unwrap_or_else(|| "(no user text)".into());

    // A bookmark-only hit has no matched turn to count, so say what it matched instead of
    // printing a bare "matches: 0".
    let matches = if hit.matched_turn_indices.is_empty() && hit.bookmark_matched {
        "bookmark".to_string()
    } else {
        hit.matched_turn_indices.len().to_string()
    };
    let bookmark = session
        .bookmark
        .as_ref()
        .map(|l| format!("\n  bookmark: {}", bookmark_oneline(l)))
        .unwrap_or_default();
    let mut hits = String::new();
    for &idx in hit.matched_turn_indices.iter().take(MAX_HIT_LINES) {
        if let Some(turn) = session.turns.get(idx) {
            hits.push_str(&format!("\n  hit>   {}", excerpt(&turn.text, terms)));
        }
    }
    if hit.matched_turn_indices.len() > MAX_HIT_LINES {
        hits.push_str(&format!(
            "\n  hit>   (+{} more)",
            hit.matched_turn_indices.len() - MAX_HIT_LINES
        ));
    }

    format!(
        "{started}  {cwd}{branch}\n  session: {sid}  turns: {n}  matches: {m}{bookmark}\n  first> {first}{hits}\n  last>  {last}",
        started = started,
        cwd = session.cwd.display(),
        branch = branch,
        sid = session.session_id,
        n = session.turns.len(),
        m = matches,
        bookmark = bookmark,
        first = first,
        hits = hits,
        last = last,
    )
}

/// `name #tag #tag`, with either half allowed to be missing.
pub fn bookmark_oneline(bookmark: &crate::history::bookmark::Bookmark) -> String {
    let tags = bookmark.tag_display();
    match (bookmark.name.is_empty(), tags.is_empty()) {
        (true, _) => tags,
        (_, true) => bookmark.name.clone(),
        _ => format!("{} {}", bookmark.name, tags),
    }
}

/// The line shown when the exact search came up empty and the fuzzy retry found results.
pub fn fuzzy_fallback_notice(terms: &[String]) -> String {
    format!(
        "No exact match for \"{}\". Showing fuzzy matches.",
        terms.join(" ")
    )
}

/// Full one-shot output for a result set, including the trailing statistics line.
/// When `mode` is `Fuzzy` the output opens with the fallback notice.
pub fn format_oneshot(
    hits: &[Hit],
    scanned_files: usize,
    elapsed: Duration,
    mode: MatchMode,
    terms: &[String],
) -> String {
    let mut out = String::new();
    if mode == MatchMode::Fuzzy {
        out.push_str(&fuzzy_fallback_notice(terms));
        out.push_str("\n\n");
    }
    // The fuzzy query is not a substring of the text, so there is no term to centre on.
    let excerpt_terms = if mode == MatchMode::Exact { terms } else { &[] };
    for hit in hits {
        out.push_str(&format_hit(hit, excerpt_terms));
        out.push_str("\n\n");
    }
    if hits.is_empty() {
        out.push_str("No sessions matched.\n");
    }
    out.push_str(&format!(
        "Found {} sessions (scanned {} jsonl files in {:.1}s)\n",
        hits.len(),
        scanned_files,
        elapsed.as_secs_f64(),
    ));
    out
}

/// Serialize hits as pretty JSON. Text fields are NOT truncated.
pub fn format_json(hits: &[Hit]) -> String {
    let entries: Vec<serde_json::Value> = hits
        .iter()
        .map(|h| {
            let s = &h.session;
            serde_json::json!({
                "session_id": s.session_id,
                "cwd": s.cwd.display().to_string(),
                "git_branch": s.git_branch,
                "started_at": s.started_at.to_rfc3339(),
                "ended_at": s.ended_at.to_rfc3339(),
                "turns": s.turns.len(),
                "matches": h.matched_turn_indices.len(),
                "bookmark_matched": h.bookmark_matched,
                "bookmark": s.bookmark,
                "first_user_text": s.first_user_text(),
                "last_user_text": s.last_user_text(),
                "matched_user_texts": h
                    .matched_turn_indices
                    .iter()
                    .filter_map(|&i| s.turns.get(i).map(|t| t.text.as_str()))
                    .collect::<Vec<_>>(),
                "file_path": s.file_path.display().to_string(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap_or_else(|_| "[]".into())
}

/// One line of a matched prompt. When the earliest term would fall past the cut, the
/// line starts `CONTEXT_BEFORE` characters ahead of it instead of at the beginning.
fn excerpt(text: &str, terms: &[String]) -> String {
    let flat = flatten(text);
    let lower = flat.to_lowercase();
    let Some(byte) = terms
        .iter()
        .filter(|t| !t.is_empty())
        .filter_map(|t| lower.find(&t.to_lowercase()))
        .min()
    else {
        return truncate_oneline(&flat);
    };
    // Counted on the lowercased text: close enough for placing a window, and lowercasing
    // changes the length of almost no character.
    let at = lower[..byte].chars().count();
    if at + CONTEXT_BEFORE < MAX_TEXT_LEN {
        return truncate_oneline(&flat);
    }
    let rest: String = flat.chars().skip(at - CONTEXT_BEFORE).collect();
    format!("…{}", truncate_oneline(&rest))
}

fn flatten(s: &str) -> String {
    s.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

fn truncate_oneline(s: &str) -> String {
    let flat = flatten(s);
    if flat.chars().count() <= MAX_TEXT_LEN {
        flat
    } else {
        let mut truncated: String = flat.chars().take(MAX_TEXT_LEN).collect();
        truncated.push('…');
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::TimeZone;

    fn hit_with(texts: &[(&str, Role)]) -> Hit {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 6, 12, 0).unwrap();
        let turns = texts
            .iter()
            .map(|(t, r)| Turn {
                role: *r,
                timestamp: ts,
                text: (*t).to_string(),
            })
            .collect::<Vec<_>>();
        Hit {
            session: SessionRecord {
                session_id: "abc-1".into(),
                cwd: "/Users/tomo/proj".into(),
                git_branch: Some("main".into()),
                file_path: "/tmp/abc-1.jsonl".into(),
                started_at: ts,
                ended_at: ts,
                bookmark: None,
                turns,
            },
            matched_turn_indices: vec![0],
            bookmark_matched: false,
        }
    }

    #[test]
    fn format_hit_lists_first_matched_and_last_prompts_with_branch() {
        let hit = hit_with(&[("hello", Role::User), ("world", Role::User)]);
        let text = format_hit(&hit, &[]);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 5);
        assert!(lines[0].contains("/Users/tomo/proj"));
        assert!(lines[0].contains("(main)"));
        assert!(lines[1].contains("session: abc-1"));
        assert!(lines[1].contains("turns: 2"));
        assert!(lines[1].contains("matches: 1"));
        assert_eq!(lines[2], "  first> hello");
        assert_eq!(lines[3], "  hit>   hello");
        assert_eq!(lines[4], "  last>  world");
    }

    #[test]
    fn format_hit_caps_matched_prompts_and_counts_the_rest() {
        let texts: Vec<(String, Role)> = (0..5).map(|i| (format!("p{i}"), Role::User)).collect();
        let refs: Vec<(&str, Role)> = texts.iter().map(|(t, r)| (t.as_str(), *r)).collect();
        let mut hit = hit_with(&refs);
        hit.matched_turn_indices = vec![0, 1, 2, 3, 4];
        let text = format_hit(&hit, &[]);
        let hit_lines: Vec<&str> = text.lines().filter(|l| l.starts_with("  hit>")).collect();
        assert_eq!(
            hit_lines,
            vec![
                "  hit>   p0",
                "  hit>   p1",
                "  hit>   p2",
                "  hit>   (+2 more)"
            ]
        );
    }

    #[test]
    fn excerpt_starts_near_a_term_buried_deep_in_the_prompt() {
        let text = format!("{}needle tail", "x".repeat(300));
        let out = excerpt(&text, &["NEEDLE".to_string()]);
        assert!(out.starts_with('…'), "got: {out}");
        assert!(out.contains("needle tail"), "got: {out}");
        assert_eq!(
            out.chars().skip(1).take(CONTEXT_BEFORE).collect::<String>(),
            "x".repeat(CONTEXT_BEFORE)
        );
    }

    #[test]
    fn excerpt_keeps_the_start_when_the_term_is_already_visible() {
        let text = format!("needle {}", "x".repeat(300));
        assert_eq!(
            excerpt(&text, &["needle".to_string()]),
            truncate_oneline(&text)
        );
        assert_eq!(excerpt(&text, &[]), truncate_oneline(&text));
    }

    #[test]
    fn format_hit_hides_branch_when_absent() {
        let mut hit = hit_with(&[("hi", Role::User)]);
        hit.session.git_branch = None;
        let text = format_hit(&hit, &[]);
        assert!(!text.contains("()"));
        assert!(!text.contains("(main)"));
    }

    #[test]
    fn truncates_long_text_and_replaces_newlines() {
        let long: String = "a".repeat(200);
        let text = format!("{long}\nsecond line");
        let out = truncate_oneline(&text);
        assert!(out.ends_with('…'));
        assert!(!out.contains('\n'));
        assert_eq!(out.chars().count(), MAX_TEXT_LEN + 1);
    }

    #[test]
    fn format_oneshot_includes_stats_line_and_empty_hint() {
        let out = format_oneshot(&[], 42, Duration::from_millis(1500), MatchMode::Exact, &[]);
        assert!(out.contains("No sessions matched."));
        assert!(out.contains("Found 0 sessions (scanned 42 jsonl files in 1.5s)"));
    }

    #[test]
    fn format_oneshot_says_when_results_came_from_the_fuzzy_fallback() {
        let terms = vec!["skil".to_string(), "stale".to_string()];
        let out = format_oneshot(
            &[hit_with(&[("q", Role::User)])],
            42,
            Duration::from_millis(100),
            MatchMode::Fuzzy,
            &terms,
        );
        assert!(out.starts_with("No exact match for \"skil stale\". Showing fuzzy matches.\n\n"));
    }

    #[test]
    fn format_oneshot_stays_silent_about_the_matcher_when_exact_hit() {
        let out = format_oneshot(
            &[hit_with(&[("q", Role::User)])],
            42,
            Duration::from_millis(100),
            MatchMode::Exact,
            &["q".to_string()],
        );
        assert!(!out.contains("fuzzy"));
    }

    #[test]
    fn fuzzy_fallback_notice_names_the_query() {
        let terms = vec!["skil".to_string(), "stale".to_string()];
        assert_eq!(
            fuzzy_fallback_notice(&terms),
            "No exact match for \"skil stale\". Showing fuzzy matches."
        );
    }

    #[test]
    fn format_json_serializes_expected_fields() {
        let hit = hit_with(&[
            ("first user", Role::User),
            ("asst", Role::Assistant),
            ("last user", Role::User),
        ]);
        let json = format_json(&[hit]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        let arr = parsed.as_array().unwrap();
        assert_eq!(arr.len(), 1);
        let e = &arr[0];
        assert_eq!(e["session_id"], "abc-1");
        assert_eq!(e["cwd"], "/Users/tomo/proj");
        assert_eq!(e["git_branch"], "main");
        assert_eq!(e["turns"], 3);
        assert_eq!(e["matches"], 1);
        assert_eq!(e["first_user_text"], "first user");
        assert_eq!(e["last_user_text"], "last user");
        assert_eq!(e["matched_user_texts"], serde_json::json!(["first user"]));
        assert!(e["started_at"].is_string());
        assert!(e["ended_at"].is_string());
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;
    use crate::history::bookmark::Bookmark;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::TimeZone;

    fn labelled_hit(name: &str, tags: &[&str], bookmark_matched: bool) -> Hit {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 6, 12, 0).unwrap();
        Hit {
            session: SessionRecord {
                session_id: "abc-1".into(),
                cwd: "/Users/tomo/proj".into(),
                git_branch: Some("main".into()),
                file_path: "/tmp/abc-1.jsonl".into(),
                started_at: ts,
                ended_at: ts,
                turns: vec![Turn {
                    role: Role::User,
                    timestamp: ts,
                    text: "hello".into(),
                }],
                bookmark: Some(Bookmark {
                    name: name.into(),
                    tags: tags.iter().map(|t| t.to_string()).collect(),
                    cwd: None,
                    session_started_at: None,
                    bookmarked_at: ts,
                    updated_at: ts,
                    hidden: false,
                }),
            },
            matched_turn_indices: if bookmark_matched { vec![] } else { vec![0] },
            bookmark_matched,
        }
    }

    #[test]
    fn format_hit_shows_the_label_name_and_tags() {
        let text = format_hit(&labelled_hit("auth refactor", &["infra"], false), &[]);
        assert!(text.contains("auth refactor"), "got:\n{text}");
        assert!(text.contains("#infra"), "got:\n{text}");
    }

    #[test]
    fn format_hit_reports_a_label_only_match_instead_of_zero_matches() {
        let text = format_hit(&labelled_hit("auth refactor", &[], true), &[]);
        assert!(text.contains("matches: bookmark"), "got:\n{text}");
    }

    #[test]
    fn json_carries_the_label_name_and_tags() {
        let json = format_json(&[labelled_hit("auth refactor", &["infra"], false)]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["bookmark"]["name"], "auth refactor");
        assert_eq!(parsed[0]["bookmark"]["tags"][0], "infra");
    }

    #[test]
    fn json_leaves_the_label_null_for_an_unlabelled_session() {
        let mut hit = labelled_hit("x", &[], false);
        hit.session.bookmark = None;
        let json = format_json(&[hit]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed[0]["bookmark"].is_null());
    }
}
