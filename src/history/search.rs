// AND matcher over SessionRecord user turns and bookmarks.

use crate::history::{Hit, Role, SessionRecord};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;

/// Return all sessions whose any user-turn text contains every query term (AND),
/// compared case-insensitively. Results are sorted by `ended_at` descending and
/// truncated to `limit` entries.
///
/// Sessions are borrowed rather than consumed so a caller can retry the same corpus with
/// `fuzzy_search_sessions` when this finds nothing. Only the matched sessions are cloned.
pub fn search_sessions(sessions: &[SessionRecord], terms: &[String], limit: usize) -> Vec<Hit> {
    let lower_terms: Vec<String> = terms.iter().map(|t| t.to_lowercase()).collect();
    let is_match = move |text: &str| {
        let lower = text.to_lowercase();
        lower_terms.iter().all(|t| lower.contains(t))
    };
    let empty_query = terms.is_empty();
    collect_hits(sessions, limit, move |session| {
        if empty_query {
            return Match::default();
        }
        Match {
            turns: user_turn_indices(session, &is_match),
            bookmark: label_text(session).is_some_and(|text| is_match(&text)),
        }
    })
}

/// Return all sessions with a user turn that fuzzy-matches `query`, absorbing typos and
/// reordered characters. Like the exact search, results are sorted by `ended_at`
/// descending rather than by score, so the list stays chronologically readable.
///
/// Matching is per user turn (not against the concatenated session text) so the matched
/// turn indices are real, and the hit count means the same thing as in the exact search.
pub fn fuzzy_search_sessions(sessions: &[SessionRecord], query: &str, limit: usize) -> Vec<Hit> {
    if query.is_empty() {
        return Vec::new();
    }
    let matcher = SkimMatcherV2::default().ignore_case();
    let is_match = |text: &str| matcher.fuzzy_match(text, query).is_some();
    collect_hits(sessions, limit, |session| Match {
        turns: user_turn_indices(session, is_match),
        bookmark: label_text(session).is_some_and(|text| is_match(text.as_str())),
    })
}

/// The searchable rendering of a session's bookmark: its name followed by its tags.
/// `None` when the session carries no bookmark.
fn label_text(session: &SessionRecord) -> Option<String> {
    let bookmark = session.bookmark.as_ref()?;
    let mut text = bookmark.name.clone();
    for tag in &bookmark.tags {
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(tag);
    }
    Some(text)
}

/// What a session matched on. A session with neither is not a hit.
#[derive(Debug, Default)]
struct Match {
    turns: Vec<usize>,
    bookmark: bool,
}

impl Match {
    fn is_empty(&self) -> bool {
        self.turns.is_empty() && !self.bookmark
    }
}

/// Indices of the user turns in `session` whose text satisfies `is_match`.
fn user_turn_indices(session: &SessionRecord, is_match: impl Fn(&str) -> bool) -> Vec<usize> {
    session
        .turns
        .iter()
        .enumerate()
        .filter(|(_, turn)| turn.role == Role::User)
        .filter(|(_, turn)| is_match(&turn.text))
        .map(|(idx, _)| idx)
        .collect()
}

/// Build hits from whichever sessions `matching` reports a match for, newest first,
/// truncated to `limit`. Cloning happens only for sessions that actually matched.
fn collect_hits(
    sessions: &[SessionRecord],
    limit: usize,
    matching: impl Fn(&SessionRecord) -> Match,
) -> Vec<Hit> {
    let mut hits: Vec<Hit> = sessions
        .iter()
        .filter_map(|session| {
            let matched = matching(session);
            if matched.is_empty() {
                return None;
            }
            Some(Hit {
                session: session.clone(),
                matched_turn_indices: matched.turns,
                bookmark_matched: matched.bookmark,
            })
        })
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.session.ended_at));
    hits.truncate(limit);
    hits
}

/// Concatenated text of all user turns in a session plus its bookmark, intended as the
/// haystack for interactive fuzzy matching. Precompute once per session at load time.
pub fn searchable_text(session: &SessionRecord) -> String {
    let mut out = label_text(session).unwrap_or_default();
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
            bookmark: None,
            turns,
        }
    }

    #[test]
    fn fuzzy_search_tolerates_a_typo_that_exact_search_misses() {
        let a = sample("a", (2026, 3, 12), &["add skill stale exclude"]);

        // "skll" is not a substring of "skill", so the exact matcher finds nothing.
        assert!(search_sessions(std::slice::from_ref(&a), &["skll".into()], 10).is_empty());

        // The fuzzy matcher still reaches it: s-k-l-l appears in order in "skill".
        let hits = fuzzy_search_sessions(std::slice::from_ref(&a), "skll stale", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.session_id, "a");
        assert_eq!(hits[0].matched_turn_indices, vec![0]);
    }

    #[test]
    fn fuzzy_search_records_every_matching_turn() {
        let a = sample(
            "a",
            (2026, 3, 12),
            &["progress bar", "unrelated", "progress line"],
        );
        let hits = fuzzy_search_sessions(std::slice::from_ref(&a), "progres", 10);
        assert_eq!(hits[0].matched_turn_indices, vec![0, 2]);
    }

    #[test]
    fn fuzzy_search_returns_nothing_for_an_unrelated_query() {
        let a = sample("a", (2026, 3, 12), &["osrm jni compile"]);
        assert!(fuzzy_search_sessions(std::slice::from_ref(&a), "zzzqqq", 10).is_empty());
    }

    #[test]
    fn fuzzy_search_sorts_by_ended_at_descending_and_applies_limit() {
        let old = sample("old", (2024, 1, 1), &["progress"]);
        let new = sample("new", (2026, 6, 1), &["progress"]);
        let mid = sample("mid", (2025, 1, 1), &["progress"]);
        let hits = fuzzy_search_sessions(&[old, new, mid], "progress", 2);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "mid"]);
    }

    #[test]
    fn and_match_requires_all_terms_in_same_turn() {
        let a = sample("a", (2026, 3, 12), &["osrm jni test"]);
        let b = sample("b", (2026, 3, 11), &["osrm only", "jni only"]);
        let hits = search_sessions(&[a, b], &["osrm".into(), "jni".into()], 10);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn case_insensitive_and_substring() {
        let a = sample("a", (2026, 3, 12), &["OSRM JNI compile"]);
        let hits = search_sessions(&[a], &["osrm".into(), "jni".into()], 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matched_turn_indices, vec![0]);
    }

    #[test]
    fn sorts_by_ended_at_descending_and_applies_limit() {
        let a = sample("old", (2024, 1, 1), &["foo"]);
        let b = sample("new", (2026, 6, 1), &["foo"]);
        let c = sample("mid", (2025, 1, 1), &["foo"]);
        let hits = search_sessions(&[a, b, c], &["foo".into()], 2);
        let ids: Vec<&str> = hits.iter().map(|h| h.session.session_id.as_str()).collect();
        assert_eq!(ids, vec!["new", "mid"]);
    }

    #[test]
    fn no_match_returns_empty() {
        let a = sample("a", (2026, 3, 12), &["hello world"]);
        let hits = search_sessions(&[a], &["zzz".into()], 10);
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
            bookmark: None,
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
            bookmark: None,
            turns: vec![Turn {
                role: Role::Assistant,
                timestamp: ts,
                text: "osrm jni".into(),
            }],
        };
        let hits = search_sessions(&[rec], &["osrm".into(), "jni".into()], 10);
        assert!(hits.is_empty());
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;
    use crate::history::Turn;
    use crate::history::bookmark::Bookmark;
    use chrono::{TimeZone, Utc};

    fn bookmarked(id: &str, turn_text: &str, name: &str, tags: &[&str]) -> SessionRecord {
        let ts = Utc.with_ymd_and_hms(2026, 3, 1, 0, 0, 0).unwrap();
        SessionRecord {
            session_id: id.into(),
            cwd: "/tmp".into(),
            git_branch: None,
            file_path: format!("/tmp/{id}.jsonl").into(),
            started_at: ts,
            ended_at: ts,
            turns: vec![Turn {
                role: Role::User,
                timestamp: ts,
                text: turn_text.into(),
            }],
            bookmark: Some(Bookmark {
                name: name.into(),
                tags: tags.iter().map(|t| t.to_string()).collect(),
                cwd: None,
                session_started_at: None,
                bookmarked_at: ts,
                updated_at: ts,
            }),
        }
    }

    #[test]
    fn search_finds_a_session_by_its_label_name_when_no_turn_matches() {
        let sessions = vec![bookmarked("a", "unrelated chatter", "auth refactor", &[])];
        let hits = search_sessions(&sessions, &["auth".to_string()], 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.session_id, "a");
    }

    #[test]
    fn search_finds_a_session_by_its_tag() {
        let sessions = vec![bookmarked(
            "a",
            "unrelated chatter",
            "some work",
            &["infra"],
        )];
        let hits = search_sessions(&sessions, &["infra".to_string()], 10);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn a_hit_found_only_through_its_label_is_flagged_as_such() {
        let sessions = vec![bookmarked("a", "unrelated chatter", "auth refactor", &[])];
        let hits = search_sessions(&sessions, &["auth".to_string()], 10);
        assert!(hits[0].bookmark_matched);
        assert!(hits[0].matched_turn_indices.is_empty());
    }

    #[test]
    fn a_hit_found_through_its_turns_is_not_flagged_as_a_label_match() {
        let sessions = vec![bookmarked("a", "auth in the turn", "unrelated name", &[])];
        let hits = search_sessions(&sessions, &["auth".to_string()], 10);
        assert!(!hits[0].bookmark_matched);
        assert_eq!(hits[0].matched_turn_indices, vec![0]);
    }

    #[test]
    fn fuzzy_search_finds_a_session_by_its_label_name() {
        let sessions = vec![bookmarked("a", "unrelated chatter", "auth refactor", &[])];
        let hits = fuzzy_search_sessions(&sessions, "authrefac", 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].bookmark_matched);
    }

    #[test]
    fn searchable_text_carries_the_label_so_the_interactive_filter_finds_it() {
        let session = bookmarked("a", "unrelated chatter", "auth refactor", &["infra"]);
        let text = searchable_text(&session);
        assert!(text.contains("auth refactor"), "got: {text}");
        assert!(text.contains("infra"), "got: {text}");
    }
}
