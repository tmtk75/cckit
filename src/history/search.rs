// AND matcher over SessionRecord user turns.

use crate::history::{Hit, Role, SessionRecord};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Return all sessions whose any user-turn text contains every query term (AND),
/// compared case-insensitively. Results are sorted by `ended_at` descending and
/// truncated to `limit` entries.
pub fn search_sessions(sessions: Vec<SessionRecord>, terms: &[String], limit: usize) -> Vec<Hit> {
    let lower_terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let mut hits: Vec<Hit> = sessions
        .into_iter()
        .filter_map(|session| match_session(session, &lower_terms))
        .collect();
    hits.sort_by(|a, b| b.session.ended_at.cmp(&a.session.ended_at));
    hits.truncate(limit);
    hits
}

/// Concatenated text of all user turns in a session, intended as the haystack
/// for interactive fuzzy matching. Precompute once per session at load time.
pub fn searchable_text(session: &SessionRecord) -> String {
    let mut out = String::new();
    for turn in &session.turns {
        if turn.role == Role::User {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(&turn.text);
        }
    }
    out
}

/// Fuzzy-filter sessions against a query string. Returns indices into the input
/// slice, sorted by score descending. `haystacks[i]` is the precomputed
/// `searchable_text` for `sessions[i]`.
///
/// When `query` is empty, returns all indices sorted by `ended_at` descending.
pub fn fuzzy_filter(
    sessions: &[SessionRecord],
    haystacks: &[String],
    query: &str,
    limit: usize,
) -> Vec<usize> {
    debug_assert_eq!(sessions.len(), haystacks.len());

    if query.is_empty() {
        let mut idxs: Vec<usize> = (0..sessions.len()).collect();
        idxs.sort_by(|a, b| sessions[*b].ended_at.cmp(&sessions[*a].ended_at));
        idxs.truncate(limit);
        return idxs;
    }

    // Use the fuzzy score only to decide whether a session matches. Matching
    // sessions are then returned in `ended_at` descending order so the list
    // is always chronologically consistent, regardless of how well any
    // individual turn happened to fuzzy-score.
    let matcher = SkimMatcherV2::default().ignore_case();
    let mut matched: Vec<usize> = haystacks
        .iter()
        .enumerate()
        .filter_map(|(i, hs)| matcher.fuzzy_match(hs, query).map(|_| i))
        .collect();
    matched.sort_by(|a, b| sessions[*b].ended_at.cmp(&sessions[*a].ended_at));
    matched.truncate(limit);
    matched
}

fn match_session(session: SessionRecord, lower_terms: &[String]) -> Option<Hit> {
    if lower_terms.is_empty() {
        return None;
    }
    let mut matched: Vec<usize> = Vec::new();
    for (idx, turn) in session.turns.iter().enumerate() {
        if turn.role != Role::User {
            continue;
        }
        let lower_text = turn.text.to_lowercase();
        if lower_terms.iter().all(|t| lower_text.contains(t)) {
            matched.push(idx);
        }
    }
    if matched.is_empty() {
        None
    } else {
        Some(Hit {
            session,
            matched_turn_indices: matched,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Turn;
    use chrono::TimeZone;

    fn sample(id: &str, ended_ymd: (i32, u32, u32), user_texts: &[&str]) -> SessionRecord {
        let ts = chrono::Utc
            .with_ymd_and_hms(ended_ymd.0, ended_ymd.1, ended_ymd.2, 0, 0, 0)
            .unwrap();
        let turns = user_texts
            .iter()
            .map(|t| Turn {
                role: Role::User,
                timestamp: ts,
                text: (*t).to_string(),
            })
            .collect::<Vec<_>>();
        SessionRecord {
            session_id: id.into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: format!("/tmp/{id}.jsonl").into(),
            started_at: ts,
            ended_at: ts,
            turns,
        }
    }

    #[test]
    fn and_match_requires_all_terms_in_same_turn() {
        let a = sample("a", (2026, 3, 12), &["osrm jni test"]);
        let b = sample("b", (2026, 3, 11), &["osrm only", "jni only"]);
        let hits = search_sessions(vec![a, b], &["osrm".into(), "jni".into()], 10);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn case_insensitive_and_substring() {
        let a = sample("a", (2026, 3, 12), &["OSRM JNI compile"]);
        let hits = search_sessions(vec![a], &["osrm".into(), "jni".into()], 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched_turn_indices, vec![0]);
    }

    #[test]
    fn sorts_by_ended_at_descending_and_applies_limit() {
        let a = sample("old", (2024, 1, 1), &["foo"]);
        let b = sample("new", (2026, 6, 1), &["foo"]);
        let c = sample("mid", (2025, 1, 1), &["foo"]);
        let hits = search_sessions(vec![a, b, c], &["foo".into()], 2);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "mid"]);
    }

    #[test]
    fn no_match_returns_empty() {
        let a = sample("a", (2026, 3, 12), &["hello world"]);
        let hits = search_sessions(vec![a], &["zzz".into()], 10);
        assert!(hits.is_empty());
    }

    #[test]
    fn searchable_text_joins_only_user_turns() {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let rec = SessionRecord {
            session_id: "s".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: "/tmp/s.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            turns: vec![
                Turn {
                    role: Role::User,
                    timestamp: ts,
                    text: "first user".into(),
                },
                Turn {
                    role: Role::Assistant,
                    timestamp: ts,
                    text: "IGNORED".into(),
                },
                Turn {
                    role: Role::User,
                    timestamp: ts,
                    text: "second user".into(),
                },
            ],
        };
        let text = searchable_text(&rec);
        assert!(text.contains("first user"));
        assert!(text.contains("second user"));
        assert!(!text.contains("IGNORED"));
    }

    #[test]
    fn fuzzy_filter_empty_query_sorts_by_ended_at_desc() {
        let a = sample("old", (2024, 1, 1), &["foo"]);
        let b = sample("new", (2026, 6, 1), &["bar"]);
        let sessions = vec![a, b];
        let haystacks: Vec<String> = sessions.iter().map(searchable_text).collect();
        let idxs = fuzzy_filter(&sessions, &haystacks, "", 10);
        let ids: Vec<&str> = idxs
            .iter()
            .map(|i| sessions[*i].session_id.as_str())
            .collect();
        assert_eq!(ids, vec!["new", "old"]);
    }

    #[test]
    fn fuzzy_filter_matches_non_contiguous_characters() {
        let a = sample("hit", (2026, 3, 12), &["osrm jni testing"]);
        let b = sample("miss", (2026, 3, 12), &["totally unrelated"]);
        let sessions = vec![a, b];
        let haystacks: Vec<String> = sessions.iter().map(searchable_text).collect();
        let idxs = fuzzy_filter(&sessions, &haystacks, "osrmjni", 10);
        assert_eq!(idxs.len(), 1);
        assert_eq!(sessions[idxs[0]].session_id, "hit");
    }

    #[test]
    fn fuzzy_filter_is_case_insensitive() {
        let a = sample("a", (2026, 3, 12), &["OSRM compile"]);
        let sessions = vec![a];
        let haystacks: Vec<String> = sessions.iter().map(searchable_text).collect();
        let idxs = fuzzy_filter(&sessions, &haystacks, "osrm", 10);
        assert_eq!(idxs.len(), 1);
    }

    #[test]
    fn fuzzy_filter_matched_results_sort_by_ended_at_desc() {
        // Build three sessions where the best fuzzy match is on the oldest
        // one; the result order should still be newest-first.
        let old = sample("old", (2024, 1, 1), &["osrm perfect match"]);
        let mid = sample("mid", (2025, 6, 1), &["orsm typo something"]);
        let new = sample("new", (2026, 3, 12), &["something with osrm inside"]);
        let sessions = vec![old, mid, new];
        let haystacks: Vec<String> = sessions.iter().map(searchable_text).collect();
        let idxs = fuzzy_filter(&sessions, &haystacks, "osrm", 10);
        let ids: Vec<&str> = idxs
            .iter()
            .map(|i| sessions[*i].session_id.as_str())
            .collect();
        assert!(ids.first() == Some(&"new"));
        assert!(ids.last() == Some(&"old"));
    }

    #[test]
    fn assistant_turns_are_not_searched() {
        let ts = chrono::Utc.with_ymd_and_hms(2026, 3, 12, 0, 0, 0).unwrap();
        let rec = SessionRecord {
            session_id: "a".into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: "/tmp/a.jsonl".into(),
            started_at: ts,
            ended_at: ts,
            turns: vec![Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "osrm jni".into(),
            }],
        };
        let hits = search_sessions(vec![rec], &["osrm".into(), "jni".into()], 10);
        assert!(hits.is_empty());
    }
}
