// Session bookmarks: names and tags a user attaches to a session so it can be found again.
//
// Bookmarks live in their own file rather than on the `Session` records in sessions.json:
// those are wiped by `sync_sessions` as soon as the TTY or process is gone, which is
// exactly when a bookmark starts being useful. The store keys on the session id, which is
// the same value in the hook payload, in the transcript's `sessionId` field, and in the
// transcript's file name.

use chrono::{DateTime, Offset, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const BOOKMARKS_FILE: &str = "bookmarks.json";
const LOCK_FILE: &str = "bookmarks.lock";

/// A name and tags attached to one session.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    #[serde(default)]
    pub tags: Vec<String>,
    /// Working directory of the bookmarked session, copied here so listing bookmarks needs
    /// no transcript scan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_started_at: Option<DateTime<Utc>>,
    pub bookmarked_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Kept on disk but left out of listings, lookups and session annotations until
    /// `unhide`: a reversible alternative to `rm`. Omitted from the file while false, so
    /// stores written before this field existed read unchanged.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
}

impl Bookmark {
    /// `#tag #tag` rendering used by every text surface.
    pub fn tag_display(&self) -> String {
        self.tags
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    pub fn has_tag(&self, tag: &str) -> bool {
        self.tags.iter().any(|t| t.eq_ignore_ascii_case(tag))
    }
}

/// What a caller wants written; the store supplies the timestamps.
#[derive(Debug, Clone, Default)]
pub struct BookmarkDraft {
    pub name: String,
    pub tags: Vec<String>,
    pub cwd: Option<String>,
    pub session_started_at: Option<DateTime<Utc>>,
}

/// Every bookmark, as loaded from disk.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BookmarkBook {
    #[serde(default)]
    bookmarks: BTreeMap<String, Bookmark>,
}

impl BookmarkBook {
    pub fn get(&self, session_id: &str) -> Option<&Bookmark> {
        self.bookmarks.get(session_id)
    }

    pub fn is_empty(&self) -> bool {
        self.bookmarks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.bookmarks.len()
    }

    /// Most recently edited first, which is the order every listing wants.
    pub fn entries_newest_first(&self) -> Vec<(&str, &Bookmark)> {
        let mut entries: Vec<(&str, &Bookmark)> = self
            .bookmarks
            .iter()
            .map(|(id, bookmark)| (id.as_str(), bookmark))
            .collect();
        entries.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at).then(a.0.cmp(b.0)));
        entries
    }

    /// Keep only the bookmarks carrying `tag` (case-insensitive).
    pub fn with_tag(&self, tag: &str) -> Vec<(&str, &Bookmark)> {
        self.entries_newest_first()
            .into_iter()
            .filter(|(_, bookmark)| bookmark.has_tag(tag))
            .collect()
    }
}

/// What a write did: the bookmark now stored, and the one it displaced.
///
/// `replaced` exists so the caller can say what was destroyed. A mistyped `--current`
/// lands on whatever session the terminal happens to be in, and an output that reads the
/// same for a new bookmark and for an overwrite gives no chance to notice.
#[derive(Debug)]
pub struct UpsertOutcome {
    pub bookmark: Bookmark,
    pub replaced: Option<Bookmark>,
}

/// What a tag edit did.
///
/// `noop` carries the tags that were already there (when adding) or were not there at
/// all (when removing). Saying so beats a silent success: a misspelled tag removal that
/// reports nothing looks exactly like one that worked.
#[derive(Debug)]
pub struct TagEdit {
    pub bookmark: Bookmark,
    pub changed: Vec<String>,
    pub noop: Vec<String>,
}

/// Why a draft could not be stored.
#[derive(Debug, PartialEq)]
pub enum BookmarkError {
    /// Neither a name nor a tag was given, so there would be nothing to remember.
    Empty,
}

impl std::fmt::Display for BookmarkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BookmarkError::Empty => write!(f, "a bookmark needs a name or at least one tag"),
        }
    }
}

/// Trim, drop the blanks, and drop repeats while keeping the order the user typed.
pub fn normalize_tags(tags: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in tags {
        for part in raw.split(',') {
            let tag = part.trim().trim_start_matches('#').trim();
            if tag.is_empty() {
                continue;
            }
            if out.iter().any(|t: &String| t.eq_ignore_ascii_case(tag)) {
                continue;
            }
            out.push(tag.to_string());
        }
    }
    out
}

/// Reading and writing bookmarks.json.
pub struct BookmarkStore {
    path: PathBuf,
    lock_path: PathBuf,
}

impl BookmarkStore {
    pub fn new() -> Self {
        Self::at(&crate::monitor::paths::data_dir())
    }

    pub fn at(dir: &Path) -> Self {
        Self {
            path: dir.join(BOOKMARKS_FILE),
            lock_path: dir.join(LOCK_FILE),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load every bookmark. A missing file is an empty book; a corrupt one is an error,
    /// because overwriting it would throw away bookmarks the user cannot recreate.
    pub fn load(&self) -> io::Result<BookmarkBook> {
        if !self.path.exists() {
            return Ok(BookmarkBook::default());
        }
        let content = fs::read_to_string(&self.path)?;
        serde_json::from_str(&content).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Failed to parse {}: {}", self.path.display(), e),
            )
        })
    }

    /// Load for a read-only surface that must not fail because of a broken file:
    /// warn once on stderr and carry on without bookmarks.
    pub fn load_or_warn(&self) -> BookmarkBook {
        match self.load() {
            Ok(book) => book,
            Err(e) => {
                eprintln!("cckit: ignoring bookmarks: {e}");
                BookmarkBook::default()
            }
        }
    }

    /// Write `draft` under `session_id`, creating or replacing it. An existing bookmark
    /// keeps its original `bookmarked_at` so the listing can show when it was first named.
    pub fn upsert(
        &self,
        session_id: &str,
        draft: BookmarkDraft,
        now: DateTime<Utc>,
    ) -> io::Result<UpsertOutcome> {
        let name = draft.name.trim().to_string();
        let tags = normalize_tags(&draft.tags);
        if name.is_empty() && tags.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                BookmarkError::Empty.to_string(),
            ));
        }

        self.with_lock(|book| {
            let existing = book.bookmarks.get(session_id);
            let bookmarked_at = existing.map(|e| e.bookmarked_at).unwrap_or(now);
            let bookmark = Bookmark {
                name,
                tags,
                cwd: draft.cwd,
                session_started_at: draft.session_started_at,
                bookmarked_at,
                updated_at: now,
                // `set` replaces the whole bookmark, so naming a hidden one brings it back.
                hidden: false,
            };
            // Only a change in what the bookmark says counts as a replacement: rerunning
            // the same command is not a loss and must not be announced as one.
            let replaced = existing
                .filter(|e| e.name != bookmark.name || e.tags != bookmark.tags)
                .cloned();
            book.bookmarks
                .insert(session_id.to_string(), bookmark.clone());
            UpsertOutcome { bookmark, replaced }
        })
    }

    /// Add or remove tags on an existing bookmark, leaving its name alone.
    ///
    /// This exists because `upsert` replaces everything: adding one tag through `set`
    /// means retyping the name and every tag already there, and forgetting one silently
    /// drops it.
    pub fn edit_tags(
        &self,
        session_id: &str,
        tags: &[String],
        remove: bool,
        now: DateTime<Utc>,
    ) -> io::Result<TagEdit> {
        let wanted = normalize_tags(tags);
        self.with_lock(|book| -> io::Result<TagEdit> {
            let Some(existing) = book.bookmarks.get(session_id) else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no bookmark on session {session_id}"),
                ));
            };

            let mut kept = existing.tags.clone();
            let mut changed = Vec::new();
            let mut noop = Vec::new();

            for tag in &wanted {
                let present = kept.iter().any(|t| t.eq_ignore_ascii_case(tag));
                match (remove, present) {
                    (false, false) => {
                        kept.push(tag.clone());
                        changed.push(tag.clone());
                    }
                    (true, true) => {
                        kept.retain(|t| !t.eq_ignore_ascii_case(tag));
                        changed.push(tag.clone());
                    }
                    _ => noop.push(tag.clone()),
                }
            }

            if existing.name.trim().is_empty() && kept.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "that would leave the bookmark with nothing to remember; use `rm` to delete it",
                ));
            }

            let bookmark = Bookmark {
                tags: kept,
                updated_at: if changed.is_empty() {
                    existing.updated_at
                } else {
                    now
                },
                ..existing.clone()
            };
            book.bookmarks
                .insert(session_id.to_string(), bookmark.clone());
            Ok(TagEdit {
                bookmark,
                changed,
                noop,
            })
        })?
    }

    /// Hide or unhide the bookmark for `session_id`. Returns whether anything changed;
    /// `NotFound` when the session carries no bookmark.
    pub fn set_hidden(
        &self,
        session_id: &str,
        hidden: bool,
        now: DateTime<Utc>,
    ) -> io::Result<bool> {
        self.with_lock(|book| {
            let Some(bookmark) = book.bookmarks.get_mut(session_id) else {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("no bookmark on session {session_id}"),
                ));
            };
            if bookmark.hidden == hidden {
                return Ok(false);
            }
            bookmark.hidden = hidden;
            bookmark.updated_at = now;
            Ok(true)
        })?
    }

    /// Drop the bookmark for `session_id`. False when there was nothing to drop.
    pub fn remove(&self, session_id: &str) -> io::Result<bool> {
        self.with_lock(|book| book.bookmarks.remove(session_id).is_some())
    }

    fn ensure_dir(&self) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// Read, modify and write back under an exclusive lock, so two terminals bookmarking
    /// at the same moment cannot lose each other's edit.
    fn with_lock<F, T>(&self, f: F) -> io::Result<T>
    where
        F: FnOnce(&mut BookmarkBook) -> T,
    {
        use fs2::FileExt;

        self.ensure_dir()?;
        let lock_file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&self.lock_path)?;
        lock_file.lock_exclusive()?;

        let result = (|| {
            let mut book = self.load()?;
            let value = f(&mut book);
            self.save(&book)?;
            Ok(value)
        })();

        let _ = fs2::FileExt::unlock(&lock_file);
        result
    }

    fn save(&self, book: &BookmarkBook) -> io::Result<()> {
        self.ensure_dir()?;
        let dir = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("Invalid bookmarks file path"))?;
        let tmp_path = dir.join(format!("{}.tmp.{}", BOOKMARKS_FILE, std::process::id()));
        let content = serde_json::to_string_pretty(book)?;

        {
            let mut file = OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
        }
        fs::rename(&tmp_path, &self.path)?;
        Ok(())
    }
}

impl Default for BookmarkStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Copy the stored bookmarks onto the freshly parsed session records.
pub fn attach_bookmarks(sessions: &mut [crate::history::SessionRecord], book: &BookmarkBook) {
    if book.is_empty() {
        return;
    }
    for session in sessions.iter_mut() {
        session.bookmark = book.get(&session.session_id).filter(|b| !b.hidden).cloned();
    }
}

/// Where `session bookmark set` should write.
pub enum Target {
    /// The session running in this terminal right now.
    Current,
    /// A session id, or a leading fragment of one.
    Id(String),
}

/// `cckit session bookmark set`. Returns the process exit code.
pub fn run_set(target: Target, name: Option<String>, tags: Vec<String>) -> io::Result<i32> {
    let (session_id, cwd, started_at) = match target {
        Target::Current => {
            let store = crate::monitor::storage::Storage::new().load();
            let codex_thread = std::env::var(CODEX_THREAD_ENV).ok();
            let tty = current_tty();
            match resolve_current(&store, codex_thread.as_deref(), &tty) {
                CurrentTarget::Live(session) => (
                    session.session_id.clone(),
                    Some(session.cwd.clone()),
                    Some(session.created_at),
                ),
                // No hook has recorded the thread yet; the id is still exact, and the
                // command runs in the session's working directory.
                CurrentTarget::CodexThread(id) => (
                    id,
                    std::env::current_dir()
                        .ok()
                        .map(|d| d.display().to_string()),
                    None,
                ),
                CurrentTarget::NotFound => {
                    eprintln!("error: no live session found on this terminal ({tty}).");
                    eprintln!(
                        "hint: run it from inside the session (`!cckit ...`), or give the id: \
                         `cckit session bookmark set <ID> <NAME>`."
                    );
                    return Ok(1);
                }
            }
        }
        Target::Id(prefix) => {
            let paths = transcript_paths();
            let ids: Vec<String> = paths
                .iter()
                .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
                .collect();
            match resolve_prefix(&prefix, &ids) {
                Resolution::None => {
                    eprintln!("error: no session matches \"{prefix}\".");
                    return Ok(1);
                }
                Resolution::Ambiguous(candidates) => {
                    eprintln!("error: \"{prefix}\" matches {} sessions:", candidates.len());
                    for id in candidates.iter().take(10) {
                        eprintln!("  {id}");
                    }
                    if candidates.len() > 10 {
                        eprintln!("  ... and {} more", candidates.len() - 10);
                    }
                    return Ok(2);
                }
                Resolution::Unique(id) => {
                    // Only the one transcript is parsed, so naming a session stays instant
                    // even though the history as a whole is large.
                    let record = paths
                        .iter()
                        .find(|p| p.file_stem().is_some_and(|s| s == id.as_str()))
                        .and_then(|p| crate::history::loader::parse_session_file(p));
                    let cwd = record.as_ref().map(|r| r.cwd.display().to_string());
                    let started = record.as_ref().map(|r| r.started_at);
                    (id, cwd, started)
                }
            }
        }
    };

    let draft = BookmarkDraft {
        name: name.unwrap_or_default(),
        tags,
        cwd,
        session_started_at: started_at,
    };
    let store = BookmarkStore::new();
    match store.upsert(&session_id, draft, Utc::now()) {
        Ok(outcome) => {
            let oneline = crate::history::format::bookmark_oneline;
            match outcome.replaced {
                // Name what was destroyed. An overwrite that reads like a new bookmark
                // hides a `--current` that landed on the wrong terminal.
                Some(old) => println!(
                    "renamed {} from \"{}\" to \"{}\"",
                    session_id,
                    oneline(&old),
                    oneline(&outcome.bookmark)
                ),
                None => println!(
                    "bookmarked {} as {}",
                    session_id,
                    oneline(&outcome.bookmark)
                ),
            }
            Ok(0)
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
            eprintln!("error: {e}");
            Ok(2)
        }
        Err(e) => Err(e),
    }
}

/// `cckit session bookmark id`. Prints the session id and nothing else, so the output
/// can be substituted straight into another command.
pub fn run_id(query: &str) -> io::Result<i32> {
    let book = BookmarkStore::new().load()?;
    match lookup(&book, query) {
        Resolution::Unique(id) => {
            println!("{id}");
            Ok(0)
        }
        Resolution::None => {
            eprintln!("error: no bookmark matches \"{query}\".");
            Ok(1)
        }
        Resolution::Ambiguous(candidates) => {
            // Stay off stdout: a caller substituting this must get nothing rather than
            // one of several sessions picked on its behalf.
            eprintln!("error: \"{query}\" matches {} bookmarks:", candidates.len());
            for id in &candidates {
                let name = book
                    .get(id)
                    .map(crate::history::format::bookmark_oneline)
                    .unwrap_or_default();
                eprintln!("  {id}  {name}");
            }
            Ok(2)
        }
    }
}

/// `cckit session bookmark tag`.
pub fn run_tag(prefix: &str, tags: Vec<String>, remove: bool) -> io::Result<i32> {
    if tags.is_empty() {
        eprintln!("error: name at least one tag.");
        return Ok(2);
    }
    let store = BookmarkStore::new();
    let book = store.load()?;
    let ids: Vec<String> = book
        .entries_newest_first()
        .into_iter()
        .map(|(id, _)| id.to_string())
        .collect();

    let id = match resolve_prefix(prefix, &ids) {
        Resolution::Unique(id) => id,
        Resolution::None => {
            eprintln!("error: no bookmarked session matches \"{prefix}\".");
            return Ok(1);
        }
        Resolution::Ambiguous(candidates) => {
            eprintln!(
                "error: \"{prefix}\" matches {} bookmarks:",
                candidates.len()
            );
            for id in candidates.iter().take(10) {
                eprintln!("  {id}");
            }
            return Ok(2);
        }
    };

    match store.edit_tags(&id, &tags, remove, Utc::now()) {
        Ok(edit) => {
            let verb = if remove { "removed" } else { "added" };
            if !edit.changed.is_empty() {
                println!("{verb} {}", normalize_tags(&edit.changed).join(" "));
            }
            for tag in &edit.noop {
                let why = if remove { "not set" } else { "already set" };
                eprintln!("note: {tag} was {why}");
            }
            println!(
                "{} is now {}",
                id,
                crate::history::format::bookmark_oneline(&edit.bookmark)
            );
            Ok(0)
        }
        Err(e) if e.kind() == io::ErrorKind::InvalidInput => {
            eprintln!("error: {e}");
            Ok(2)
        }
        Err(e) => Err(e),
    }
}

/// `cckit session bookmark ls`.
pub fn run_ls(
    tag: Option<String>,
    here: bool,
    all: bool,
    group_by: Option<GroupBy>,
    json: bool,
) -> io::Result<i32> {
    let store = BookmarkStore::new();
    let book = store.load()?;
    let mut entries = match tag.as_deref() {
        Some(tag) => book.with_tag(tag),
        None => book.entries_newest_first(),
    };
    if here {
        let root = std::env::current_dir()?;
        entries.retain(|(_, bookmark)| is_under(bookmark, &root));
    }
    // Counted after the other filters, so the note speaks for the same scope as the list.
    let hidden = entries.iter().filter(|(_, b)| b.hidden).count();
    if !all {
        entries.retain(|(_, bookmark)| !bookmark.hidden);
    }

    if json {
        // The JSON stays flat whatever the grouping: a `jq` consumer can bucket it however
        // it likes, and a shape that changes with a display flag is harder to depend on.
        println!("{}", format_list_json(&entries));
        return Ok(0);
    }

    let tz = chrono::Local::now().offset().fix();
    let color = colored::control::SHOULD_COLORIZE.should_colorize();
    if !all && hidden > 0 && entries.is_empty() {
        // The usual empty message says nothing has been bookmarked yet, which is false here.
        let where_kept = shorten_home(
            &store.path().display().to_string(),
            dirs::home_dir().as_deref(),
        );
        let unit = if hidden == 1 { "bookmark" } else { "bookmarks" };
        println!(
            "No visible bookmarks; {hidden} {unit} hidden. `cckit session bookmark ls --all` shows them."
        );
        println!("\nBookmarks are kept in {where_kept}");
        return Ok(0);
    }
    match group_by {
        Some(GroupBy::Path) => print!(
            "{}",
            format_groups(&group_by_path(&entries), &tz, color, store.path())
        ),
        Some(GroupBy::Tag) => print!(
            "{}",
            format_groups(&group_by_tag(&entries), &tz, color, store.path())
        ),
        None => print!("{}", format_list(&entries, &tz, color, store.path())),
    }
    if !all && hidden > 0 {
        print!("{}", hidden_note(hidden));
    }
    Ok(0)
}

/// The line that keeps hidden bookmarks from looking deleted.
fn hidden_note(hidden: usize) -> String {
    let verb = if hidden == 1 { "is" } else { "are" };
    format!("{hidden} more {verb} hidden; `cckit session bookmark ls --all` shows them\n")
}

/// `cckit session bookmark rm`. The prefix is resolved against the bookmarked sessions only,
/// which is both the shorter list and the one the user is looking at.
pub fn run_rm(prefix: &str) -> io::Result<i32> {
    let store = BookmarkStore::new();
    let book = store.load()?;
    let ids: Vec<String> = book
        .entries_newest_first()
        .into_iter()
        .map(|(id, _)| id.to_string())
        .collect();
    match resolve_prefix(prefix, &ids) {
        Resolution::None => {
            eprintln!("error: no bookmarked session matches \"{prefix}\".");
            Ok(1)
        }
        Resolution::Ambiguous(candidates) => {
            eprintln!(
                "error: \"{prefix}\" matches {} bookmarks:",
                candidates.len()
            );
            for id in candidates.iter().take(10) {
                eprintln!("  {id}");
            }
            Ok(2)
        }
        Resolution::Unique(id) => {
            store.remove(&id)?;
            println!("removed the bookmark on {id}");
            Ok(0)
        }
    }
}

/// `cckit session bookmark hide` and `unhide`. The prefix is resolved against every
/// bookmark, hidden or not, so `unhide` reaches what `ls` no longer shows.
pub fn run_hide(prefix: &str, hidden: bool) -> io::Result<i32> {
    let store = BookmarkStore::new();
    let book = store.load()?;
    let ids: Vec<String> = book
        .entries_newest_first()
        .into_iter()
        .map(|(id, _)| id.to_string())
        .collect();
    let id = match resolve_prefix(prefix, &ids) {
        Resolution::Unique(id) => id,
        Resolution::None => {
            eprintln!("error: no bookmarked session matches \"{prefix}\".");
            return Ok(1);
        }
        Resolution::Ambiguous(candidates) => {
            eprintln!(
                "error: \"{prefix}\" matches {} bookmarks:",
                candidates.len()
            );
            for id in candidates.iter().take(10) {
                eprintln!("  {id}");
            }
            return Ok(2);
        }
    };

    let name = book
        .get(&id)
        .map(crate::history::format::bookmark_oneline)
        .unwrap_or_default();
    if store.set_hidden(&id, hidden, Utc::now())? {
        let verb = if hidden { "hid" } else { "unhid" };
        println!("{verb} the bookmark on {id} ({name})");
    } else {
        let state = if hidden { "hidden" } else { "visible" };
        eprintln!("note: the bookmark on {id} ({name}) was already {state}");
    }
    Ok(0)
}

/// Every transcript file, newest first is not needed here, only the paths.
fn transcript_paths() -> Vec<PathBuf> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    crate::history::loader::collect_session_paths(&home.join(".claude/projects"))
}

/// Whether a bookmark's session ran in `root` or anywhere below it.
///
/// Compared segment by segment rather than as text, so a sibling that merely shares a
/// prefix (`/proj-wt` next to `/proj`, which is what a worktree looks like) is not taken
/// for a child. A bookmark written before cckit recorded the cwd matches nothing.
pub fn is_under(bookmark: &Bookmark, root: &Path) -> bool {
    bookmark
        .cwd
        .as_deref()
        .is_some_and(|cwd| Path::new(cwd).starts_with(root))
}

/// One heading and the bookmarks under it.
pub struct Group<'a> {
    pub heading: String,
    pub entries: Vec<(&'a str, &'a Bookmark)>,
    /// False when the heading already names the directory, so the rows need not repeat it.
    pub rows_show_cwd: bool,
}

/// Sort newest first so grouping does not depend on how the caller happened to order
/// its input: the first group, and the first row inside it, are then the most recent.
fn newest_first<'a>(entries: &[(&'a str, &'a Bookmark)]) -> Vec<(&'a str, &'a Bookmark)> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|a, b| b.1.updated_at.cmp(&a.1.updated_at).then(a.0.cmp(b.0)));
    sorted
}

/// Bucket by the directory the session ran in, newest group first.
pub fn group_by_path<'a>(entries: &[(&'a str, &'a Bookmark)]) -> Vec<Group<'a>> {
    let heading_of = |b: &Bookmark| b.cwd.clone().unwrap_or_else(|| UNKNOWN_HEADING.to_string());
    let mut groups: Vec<Group<'a>> = Vec::new();
    for (id, bookmark) in &newest_first(entries) {
        let heading = heading_of(bookmark);
        match groups.iter_mut().find(|g| g.heading == heading) {
            Some(group) => group.entries.push((id, bookmark)),
            None => groups.push(Group {
                heading,
                entries: vec![(id, bookmark)],
                rows_show_cwd: false,
            }),
        }
    }
    groups
}

/// Bucket by tag, newest group first, with the untagged ones last.
///
/// A bookmark appears under every tag it carries. Collapsing it to one would mean
/// searching `#design` and not finding something that is tagged `#design`.
pub fn group_by_tag<'a>(entries: &[(&'a str, &'a Bookmark)]) -> Vec<Group<'a>> {
    let mut groups: Vec<Group<'a>> = Vec::new();
    let mut untagged: Vec<(&'a str, &'a Bookmark)> = Vec::new();

    for (id, bookmark) in &newest_first(entries) {
        if bookmark.tags.is_empty() {
            untagged.push((id, bookmark));
            continue;
        }
        for tag in &bookmark.tags {
            let heading = format!("#{tag}");
            match groups.iter_mut().find(|g| g.heading == heading) {
                Some(group) => group.entries.push((id, bookmark)),
                None => groups.push(Group {
                    heading,
                    entries: vec![(id, bookmark)],
                    rows_show_cwd: true,
                }),
            }
        }
    }
    if !untagged.is_empty() {
        groups.push(Group {
            heading: NO_TAG_HEADING.to_string(),
            entries: untagged,
            rows_show_cwd: true,
        });
    }
    groups
}

/// One bookmark as a row: when it was last edited, what it says, its session id, and the
/// directory it ran in. Shared so the flat and grouped listings cannot drift apart.
fn bookmark_row(
    session_id: &str,
    bookmark: &Bookmark,
    tz: &chrono::FixedOffset,
    color: bool,
    show_cwd: bool,
) -> String {
    use colored::Colorize;

    let home = dirs::home_dir();
    let when = bookmark
        .updated_at
        .with_timezone(tz)
        .format("%Y-%m-%d %H:%M");
    let text = crate::history::format::bookmark_oneline(bookmark);
    let cwd = bookmark
        .cwd
        .as_deref()
        .filter(|_| show_cwd)
        .map(|c| shorten_home(c, home.as_deref()))
        .unwrap_or_default();
    // Only `ls --all` lists a hidden bookmark, and there it must not pass for a live one:
    // the mark says so in plain text, and with color the whole row is dimmed.
    let mark = if bookmark.hidden { " (hidden)" } else { "" };
    let plain = format!("{when}  {text}{mark}  {session_id}");
    // Under `--group-by path` the heading already names the directory, so the row ends
    // at the session id rather than trailing an empty column.
    let plain = if cwd.is_empty() {
        plain
    } else {
        format!("{plain}  {cwd}")
    };
    if !color {
        return plain;
    }
    if bookmark.hidden {
        return plain.dimmed().to_string();
    }
    let head = format!(
        "{}  {}  {}",
        when.to_string().dimmed(),
        text.magenta(),
        session_id.blue(),
    );
    if cwd.is_empty() {
        head
    } else {
        format!("{head}  {}", cwd.dimmed())
    }
}

/// The grouping `bookmark ls` was asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Path,
    Tag,
}

/// Headings with their bookmarks indented underneath, laid out like `session recent`
/// so both listings read the same way.
pub fn format_groups(
    groups: &[Group<'_>],
    tz: &chrono::FixedOffset,
    color: bool,
    store_path: &Path,
) -> String {
    use colored::Colorize;

    let home = dirs::home_dir();
    let where_kept = shorten_home(&store_path.display().to_string(), home.as_deref());
    if groups.is_empty() {
        return format!("No bookmarks matched.\n\nBookmarks are kept in {where_kept}\n");
    }

    let mut out = String::new();
    let mut shown = 0usize;
    for (i, group) in groups.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let heading = shorten_home(&group.heading, home.as_deref());
        out.push_str(&if color {
            format!("{}\n", heading.cyan().bold())
        } else {
            format!("{heading}\n")
        });
        for (session_id, bookmark) in &group.entries {
            out.push_str(&format!(
                "  {}\n",
                bookmark_row(session_id, bookmark, tz, color, group.rows_show_cwd)
            ));
            shown += 1;
        }
    }
    let unit = if shown == 1 { "row" } else { "rows" };
    // A bookmark with two tags is two rows under `--group-by tag`, so this counts rows
    // rather than claiming a bookmark count the listing does not show.
    let footer = format!("\n{shown} {unit} in {where_kept}\n");
    out.push_str(&if color {
        footer.dimmed().to_string()
    } else {
        footer
    });
    out
}

/// Heading for bookmarks written before cckit recorded the working directory.
const UNKNOWN_HEADING: &str = "(unknown)";
/// Heading for the bookmarks carrying no tag at all.
const NO_TAG_HEADING: &str = "(no tag)";

/// One line per bookmark: when it was last edited, what it says, which session it names,
/// and where that session ran. The project comes last so a narrow terminal cuts it
/// rather than the id.
pub fn format_list(
    entries: &[(&str, &Bookmark)],
    tz: &chrono::FixedOffset,
    color: bool,
    store_path: &Path,
) -> String {
    use colored::Colorize;

    let home = dirs::home_dir();
    let where_kept = shorten_home(&store_path.display().to_string(), home.as_deref());

    if entries.is_empty() {
        // Name the file even when there is nothing to show: "nothing bookmarked yet" and
        // "cckit is reading a different file than you think" look the same otherwise.
        return format!(
            "No bookmarked sessions yet. Find one with `cckit session search`, then\n\
             `cckit session bookmark set <ID> <NAME>`.\n\n\
             Bookmarks are kept in {where_kept}\n"
        );
    }
    let mut out = String::new();
    for (session_id, bookmark) in entries {
        out.push_str(&format!(
            "{}\n",
            bookmark_row(session_id, bookmark, tz, color, true)
        ));
    }
    let count = entries.len();
    let unit = if count == 1 { "bookmark" } else { "bookmarks" };
    let footer = format!("\n{count} {unit} in {where_kept}\n");
    out.push_str(&if color {
        footer.dimmed().to_string()
    } else {
        footer
    });
    out
}

/// Flat array so `jq` consumers do not have to parse the columns.
pub fn format_list_json(entries: &[(&str, &Bookmark)]) -> String {
    let values: Vec<serde_json::Value> = entries
        .iter()
        .map(|(session_id, bookmark)| {
            serde_json::json!({
                "session_id": session_id,
                "name": bookmark.name,
                "tags": bookmark.tags,
                "cwd": bookmark.cwd,
                "session_started_at": bookmark.session_started_at.map(|t| t.to_rfc3339()),
                "bookmarked_at": bookmark.bookmarked_at.to_rfc3339(),
                "updated_at": bookmark.updated_at.to_rfc3339(),
                "hidden": bookmark.hidden,
            })
        })
        .collect();
    serde_json::to_string_pretty(&values).unwrap_or_else(|_| "[]".into())
}

/// Replace the home prefix with `~` so the project column stays short.
pub fn shorten_home(path: &str, home: Option<&Path>) -> String {
    let Some(home) = home.map(|h| h.to_string_lossy().to_string()) else {
        return path.to_string();
    };
    match path.strip_prefix(&home) {
        Some(rest) => format!("~{rest}"),
        None => path.to_string(),
    }
}

/// Turn one `ps -o tty=` field into a device path, or `None` when the process has no
/// controlling terminal (`??`) and so names no session.
fn normalize_tty(raw: &str) -> Option<String> {
    let name = raw.trim();
    if name.is_empty() || name.starts_with('?') || name == "not a tty" {
        return None;
    }
    if name.starts_with("/dev/") {
        return Some(name.to_string());
    }
    Some(format!("/dev/{name}"))
}

/// The terminal this command is attached to, looking up the process tree until one is
/// found. The walk matters because a command run from a Claude Code tool call sits in a
/// shell with no controlling terminal of its own, while its parent still has one.
pub fn current_tty() -> String {
    use std::process::Command;

    if let Ok(out) = Command::new("tty").output()
        && let Some(tty) = normalize_tty(&String::from_utf8_lossy(&out.stdout))
    {
        return tty;
    }

    let mut pid = std::process::id().to_string();
    // Five levels clear the tool-call shell, cargo and a wrapper or two without ever
    // walking far enough to reach an unrelated terminal.
    for _ in 0..5 {
        let Some(parent) = ps_field(&pid, "ppid=") else {
            break;
        };
        if parent.is_empty() || parent == "0" || parent == "1" {
            break;
        }
        if let Some(tty) = ps_field(&parent, "tty=").as_deref().and_then(normalize_tty) {
            return tty;
        }
        pid = parent;
    }
    "unknown".to_string()
}

fn ps_field(pid: &str, field: &str) -> Option<String> {
    let out = std::process::Command::new("ps")
        .args(["-o", field, "-p", pid])
        .output()
        .ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The live session running on `tty`, as `--current` means it: the newest one, skipping
/// subagents, which share their parent's terminal but are not what the user is in.
/// Set by Codex in the environment of every command it runs, `!cmd` included. Those run
/// under its app-server daemon, which has no terminal, so this is the only way such a
/// command can tell which session it belongs to.
const CODEX_THREAD_ENV: &str = "CODEX_THREAD_ID";

/// Which session `--current` means.
#[derive(Debug)]
pub enum CurrentTarget<'a> {
    Live(&'a crate::monitor::session::Session),
    /// A Codex thread id from the environment that no hook has recorded.
    CodexThread(String),
    NotFound,
}

/// The Codex thread id wins when present: it names the session exactly, whereas the
/// terminal is `unknown` for a command Codex runs.
pub fn resolve_current<'a>(
    store: &'a crate::monitor::session::SessionStore,
    codex_thread: Option<&str>,
    tty: &str,
) -> CurrentTarget<'a> {
    if let Some(id) = codex_thread.map(str::trim).filter(|id| !id.is_empty()) {
        return store
            .sessions
            .values()
            .filter(|s| s.session_id == id)
            .max_by_key(|s| s.updated_at)
            .map(CurrentTarget::Live)
            .unwrap_or_else(|| CurrentTarget::CodexThread(id.to_string()));
    }
    current_session(store, tty).map_or(CurrentTarget::NotFound, CurrentTarget::Live)
}

pub fn current_session<'a>(
    store: &'a crate::monitor::session::SessionStore,
    tty: &str,
) -> Option<&'a crate::monitor::session::Session> {
    // "unknown" is what both an unidentifiable terminal and a desktop session record,
    // so matching on it would bookmark whichever desktop session happened to be stored.
    if tty == "unknown" {
        return None;
    }
    store
        .sessions
        .values()
        .filter(|s| s.tty == tty)
        .filter(|s| !s.is_subagent())
        .max_by_key(|s| s.updated_at)
}

/// Every session id that still has a transcript under `projects_dir`. Read from the
/// file names, which are the session ids, so no transcript has to be parsed.
pub fn known_session_ids(projects_dir: &Path) -> Vec<String> {
    crate::history::loader::collect_session_paths(projects_dir)
        .iter()
        .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
        .collect()
}

/// How a session-id prefix matched the known ids.
#[derive(Debug, PartialEq)]
pub enum Resolution {
    Unique(String),
    None,
    Ambiguous(Vec<String>),
}

/// Find the session a query names, among the bookmarked ones.
///
/// Three nets are cast in order, and the first that catches anything decides: a substring
/// of the name, then a tag, then a leading fragment of the session id. Name beats tag
/// because a tag is deliberately the broader net; letting it widen an already-good name
/// match would turn a unique answer into an ambiguous one.
///
/// Several matches are never narrowed to one. The result of this goes inside `$(...)`,
/// where quietly resuming the wrong session is worse than failing.
pub fn lookup(book: &BookmarkBook, query: &str) -> Resolution {
    if query.trim().is_empty() {
        return Resolution::None;
    }
    let needle = query.trim().to_lowercase();

    let by_name: Vec<String> = book
        .entries_newest_first()
        .into_iter()
        .filter(|(_, b)| !b.hidden && b.name.to_lowercase().contains(&needle))
        .map(|(id, _)| id.to_string())
        .collect();
    if let Some(found) = decide(by_name) {
        return found;
    }

    let by_tag: Vec<String> = book
        .with_tag(&needle)
        .into_iter()
        .filter(|(_, b)| !b.hidden)
        .map(|(id, _)| id.to_string())
        .collect();
    if let Some(found) = decide(by_tag) {
        return found;
    }

    let ids: Vec<String> = book
        .entries_newest_first()
        .into_iter()
        .filter(|(_, b)| !b.hidden)
        .map(|(id, _)| id.to_string())
        .collect();
    resolve_prefix(query.trim(), &ids)
}

/// Turn a set of matches into a verdict, or `None` when nothing matched and the next
/// net should be tried.
fn decide(mut matches: Vec<String>) -> Option<Resolution> {
    match matches.len() {
        0 => None,
        1 => Some(Resolution::Unique(matches.remove(0))),
        _ => {
            matches.sort();
            Some(Resolution::Ambiguous(matches))
        }
    }
}

/// Resolve a full id or a leading fragment of one against `known`.
pub fn resolve_prefix(prefix: &str, known: &[String]) -> Resolution {
    if prefix.is_empty() {
        return Resolution::None;
    }
    if let Some(exact) = known.iter().find(|id| id.as_str() == prefix) {
        return Resolution::Unique(exact.clone());
    }
    let lower = prefix.to_ascii_lowercase();
    let mut matches: Vec<String> = known
        .iter()
        .filter(|id| id.to_ascii_lowercase().starts_with(&lower))
        .cloned()
        .collect();
    match matches.len() {
        0 => Resolution::None,
        1 => Resolution::Unique(matches.remove(0)),
        _ => {
            matches.sort();
            Resolution::Ambiguous(matches)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn draft(name: &str, tags: &[&str]) -> BookmarkDraft {
        BookmarkDraft {
            name: name.to_string(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn load_returns_an_empty_book_when_no_file_exists() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn upsert_then_load_round_trips_name_and_tags() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store
            .upsert("abc", draft("auth work", &["auth"]), at(0))
            .unwrap();

        let book = store.load().unwrap();
        let bookmark = book.get("abc").unwrap();
        assert_eq!(bookmark.name, "auth work");
        assert_eq!(bookmark.tags, vec!["auth"]);
        assert_eq!(bookmark.bookmarked_at, at(0));
    }

    #[test]
    fn upsert_keeps_labeled_at_and_moves_updated_at() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("abc", draft("first", &[]), at(0)).unwrap();
        store.upsert("abc", draft("renamed", &[]), at(60)).unwrap();

        let book = store.load().unwrap();
        let bookmark = book.get("abc").unwrap();
        assert_eq!(bookmark.name, "renamed");
        assert_eq!(bookmark.bookmarked_at, at(0));
        assert_eq!(bookmark.updated_at, at(60));
    }

    #[test]
    fn upsert_stores_the_session_cwd_so_listing_needs_no_transcript() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store
            .upsert(
                "abc",
                BookmarkDraft {
                    name: "work".to_string(),
                    cwd: Some("/tmp/proj".to_string()),
                    session_started_at: Some(at(-500)),
                    ..Default::default()
                },
                at(0),
            )
            .unwrap();

        let book = store.load().unwrap();
        let bookmark = book.get("abc").unwrap();
        assert_eq!(bookmark.cwd.as_deref(), Some("/tmp/proj"));
        assert_eq!(bookmark.session_started_at, Some(at(-500)));
    }

    #[test]
    fn upsert_rejects_a_draft_with_neither_name_nor_tag() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        let err = store
            .upsert("abc", draft("   ", &[" "]), at(0))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn upsert_accepts_tags_without_a_name() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("abc", draft("", &["auth"]), at(0)).unwrap();
        assert_eq!(store.load().unwrap().get("abc").unwrap().tags, vec!["auth"]);
    }

    #[test]
    fn remove_reports_whether_anything_was_dropped() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("abc", draft("work", &[]), at(0)).unwrap();

        assert!(store.remove("abc").unwrap());
        assert!(!store.remove("abc").unwrap());
        assert!(store.load().unwrap().is_empty());
    }

    #[test]
    fn load_fails_on_a_corrupt_file_instead_of_discarding_labels() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(BOOKMARKS_FILE), "{ not json").unwrap();
        let store = BookmarkStore::at(dir.path());
        assert_eq!(store.load().unwrap_err().kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn entries_are_listed_most_recently_edited_first() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("old", draft("old", &[]), at(0)).unwrap();
        store.upsert("new", draft("new", &[]), at(60)).unwrap();
        store.upsert("mid", draft("mid", &[]), at(30)).unwrap();

        let book = store.load().unwrap();
        let ids: Vec<&str> = book
            .entries_newest_first()
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(ids, vec!["new", "mid", "old"]);
    }

    #[test]
    fn with_tag_matches_regardless_of_case() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("a", draft("a", &["Auth"]), at(0)).unwrap();
        store.upsert("b", draft("b", &["infra"]), at(10)).unwrap();

        let book = store.load().unwrap();
        let ids: Vec<&str> = book.with_tag("auth").iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec!["a"]);
    }

    #[test]
    fn tags_are_trimmed_split_on_commas_and_deduped_in_order() {
        let input = vec![
            " auth ".to_string(),
            "#design,auth".to_string(),
            "".to_string(),
            "AUTH".to_string(),
        ];
        assert_eq!(normalize_tags(&input), vec!["auth", "design"]);
    }

    #[test]
    fn a_full_id_resolves_to_itself() {
        let known = vec!["aaaa-1".to_string(), "bbbb-2".to_string()];
        assert_eq!(
            resolve_prefix("aaaa-1", &known),
            Resolution::Unique("aaaa-1".to_string())
        );
    }

    #[test]
    fn a_unique_fragment_resolves_to_the_only_match() {
        let known = vec!["aaaa-1".to_string(), "bbbb-2".to_string()];
        assert_eq!(
            resolve_prefix("aa", &known),
            Resolution::Unique("aaaa-1".to_string())
        );
    }

    #[test]
    fn a_shared_fragment_reports_every_candidate() {
        let known = vec!["ab-2".to_string(), "aa-1".to_string(), "b-3".to_string()];
        assert_eq!(
            resolve_prefix("a", &known),
            Resolution::Ambiguous(vec!["aa-1".to_string(), "ab-2".to_string()])
        );
    }

    #[test]
    fn an_unknown_fragment_resolves_to_nothing() {
        let known = vec!["aaaa-1".to_string()];
        assert_eq!(resolve_prefix("zz", &known), Resolution::None);
        assert_eq!(resolve_prefix("", &known), Resolution::None);
    }
}

#[cfg(test)]
mod attach_tests {
    use super::*;
    use crate::history::{Role, SessionRecord, Turn};
    use chrono::TimeZone;
    use std::path::PathBuf;

    fn session(id: &str) -> SessionRecord {
        let ts = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
        SessionRecord {
            session_id: id.into(),
            cwd: PathBuf::from("/tmp"),
            git_branch: None,
            file_path: PathBuf::from(format!("/tmp/{id}.jsonl")),
            started_at: ts,
            ended_at: ts,
            turns: vec![Turn {
                role: Role::User,
                timestamp: ts,
                text: "hello".into(),
            }],
            bookmark: None,
        }
    }

    fn book_with(session_id: &str, name: &str) -> BookmarkBook {
        let dir = tempfile::TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store
            .upsert(
                session_id,
                BookmarkDraft {
                    name: name.into(),
                    ..Default::default()
                },
                Utc.with_ymd_and_hms(2026, 9, 2, 0, 0, 0).unwrap(),
            )
            .unwrap();
        store.load().unwrap()
    }

    #[test]
    fn attach_labels_puts_the_stored_label_on_the_matching_session() {
        let mut sessions = vec![session("abc"), session("def")];
        attach_bookmarks(&mut sessions, &book_with("abc", "auth work"));

        assert_eq!(
            sessions[0].bookmark.as_ref().map(|l| l.name.as_str()),
            Some("auth work")
        );
        assert!(sessions[1].bookmark.is_none());
    }
}

#[cfg(test)]
mod listing_tests {
    use super::*;
    use chrono::TimeZone;

    fn bookmark(name: &str, tags: &[&str], cwd: Option<&str>, updated: i64) -> Bookmark {
        let ts = Utc.timestamp_opt(1_700_000_000 + updated, 0).unwrap();
        Bookmark {
            name: name.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            cwd: cwd.map(|c| c.to_string()),
            session_started_at: None,
            bookmarked_at: ts,
            updated_at: ts,
            hidden: false,
        }
    }

    fn tz() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(9 * 3600).unwrap()
    }

    #[test]
    fn the_listing_shows_the_date_name_tags_id_and_project() {
        let l = bookmark("auth refactor", &["infra"], Some("/tmp/proj"), 0);
        let out = format_list(
            &[("abcdef-1", &l)],
            &tz(),
            false,
            Path::new("/d/bookmarks.json"),
        );
        assert!(out.contains("auth refactor"), "got: {out}");
        assert!(out.contains("#infra"), "got: {out}");
        assert!(out.contains("abcdef-1"), "got: {out}");
        assert!(out.contains("/tmp/proj"), "got: {out}");
    }

    #[test]
    fn the_listing_says_so_when_there_is_nothing_to_show() {
        let out = format_list(&[], &tz(), false, Path::new("/d/bookmarks.json"));
        assert!(out.contains("No bookmarked sessions"), "got: {out}");
    }

    #[test]
    fn the_json_listing_is_a_flat_array_carrying_the_session_id() {
        let l = bookmark("auth refactor", &["infra"], Some("/tmp/proj"), 0);
        let json = format_list_json(&[("abcdef-1", &l)]);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0]["session_id"], "abcdef-1");
        assert_eq!(parsed[0]["name"], "auth refactor");
        assert_eq!(parsed[0]["tags"][0], "infra");
        assert_eq!(parsed[0]["cwd"], "/tmp/proj");
    }

    #[test]
    fn a_home_relative_project_path_is_shortened() {
        let home = std::path::Path::new("/Users/tomo");
        assert_eq!(shorten_home("/Users/tomo/src/app", Some(home)), "~/src/app");
        assert_eq!(shorten_home("/opt/app", Some(home)), "/opt/app");
    }

    #[test]
    fn known_session_ids_come_from_the_transcript_file_names() {
        let dir = tempfile::TempDir::new().unwrap();
        let project = dir.path().join("-Users-tomo-proj");
        fs::create_dir_all(&project).unwrap();
        fs::write(project.join("aaaa-1.jsonl"), "").unwrap();
        fs::write(project.join("bbbb-2.jsonl"), "").unwrap();
        fs::write(project.join("notes.txt"), "").unwrap();

        let mut ids = known_session_ids(dir.path());
        ids.sort();
        assert_eq!(ids, vec!["aaaa-1".to_string(), "bbbb-2".to_string()]);
    }
}

#[cfg(test)]
mod current_tests {
    use super::*;
    use crate::monitor::session::{Session, SessionStatus, SessionStore};
    use chrono::TimeZone;

    fn session(id: &str, tty: &str, updated: i64, tool_count: u32, prompts: u32) -> Session {
        let ts = Utc.timestamp_opt(1_700_000_000 + updated, 0).unwrap();
        Session {
            session_id: id.into(),
            cwd: "/tmp/proj".into(),
            tty: tty.into(),
            status: SessionStatus::Running,
            created_at: ts,
            updated_at: ts,
            last_tool: None,
            last_tool_input: None,
            pid: None,
            prompt_count: prompts,
            compact_count: 0,
            transcript_path: None,
            tool_started_at: None,
            last_tool_duration_ms: None,
            tool_count,
            context_used_tokens: None,
            context_max_tokens: None,
            model: Some("claude-opus-5".into()),
            subagent_name: None,
        }
    }

    fn store(sessions: Vec<Session>) -> SessionStore {
        let mut store = SessionStore::default();
        for s in sessions {
            store.sessions.insert(s.key(), s);
        }
        store
    }

    #[test]
    fn the_current_session_is_the_one_on_this_tty() {
        let store = store(vec![
            session("here", "/dev/ttys001", 0, 0, 1),
            session("elsewhere", "/dev/ttys002", 10, 0, 1),
        ]);
        assert_eq!(
            current_session(&store, "/dev/ttys001").map(|s| s.session_id.clone()),
            Some("here".to_string())
        );
    }

    fn resolved(target: CurrentTarget<'_>) -> String {
        match target {
            CurrentTarget::Live(s) => format!("live:{}", s.session_id),
            CurrentTarget::CodexThread(id) => format!("thread:{id}"),
            CurrentTarget::NotFound => "none".to_string(),
        }
    }

    #[test]
    fn a_codex_thread_id_finds_its_session_without_a_terminal() {
        let store = store(vec![
            session("01a0f528", "unknown", 0, 0, 1),
            session("other", "/dev/ttys001", 10, 0, 1),
        ]);
        assert_eq!(
            resolved(resolve_current(&store, Some("01a0f528"), "unknown")),
            "live:01a0f528"
        );
    }

    #[test]
    fn a_codex_thread_id_beats_the_terminal() {
        let store = store(vec![
            session("codex", "/dev/ttys001", 0, 0, 1),
            session("claude", "/dev/ttys001", 10, 0, 1),
        ]);
        assert_eq!(
            resolved(resolve_current(&store, Some("codex"), "/dev/ttys001")),
            "live:codex"
        );
    }

    #[test]
    fn an_unrecorded_codex_thread_is_still_named_by_its_id() {
        let store = store(vec![]);
        assert_eq!(
            resolved(resolve_current(&store, Some(" 01a0f528 "), "unknown")),
            "thread:01a0f528"
        );
    }

    #[test]
    fn without_a_codex_thread_the_terminal_decides() {
        let store = store(vec![session("here", "/dev/ttys001", 0, 0, 1)]);
        assert_eq!(
            resolved(resolve_current(&store, None, "/dev/ttys001")),
            "live:here"
        );
        assert_eq!(
            resolved(resolve_current(&store, Some(""), "unknown")),
            "none"
        );
    }

    #[test]
    fn a_subagent_on_the_same_tty_is_not_the_current_session() {
        let store = store(vec![
            session("main", "/dev/ttys001", 0, 3, 2),
            session("subagent", "/dev/ttys001", 10, 5, 0),
        ]);
        assert_eq!(
            current_session(&store, "/dev/ttys001").map(|s| s.session_id.clone()),
            Some("main".to_string())
        );
    }

    #[test]
    fn the_newest_session_wins_when_the_tty_was_reused() {
        let store = store(vec![
            session("older", "/dev/ttys001", 0, 0, 1),
            session("newer", "/dev/ttys001", 10, 0, 1),
        ]);
        assert_eq!(
            current_session(&store, "/dev/ttys001").map(|s| s.session_id.clone()),
            Some("newer".to_string())
        );
    }

    #[test]
    fn an_unknown_tty_never_matches_a_session_that_also_has_no_tty() {
        // Codex Desktop sessions are stored with tty "unknown". When this terminal
        // cannot be identified either, the two must not be taken for each other.
        let store = store(vec![session("desktop", "unknown", 0, 0, 1)]);
        assert!(current_session(&store, "unknown").is_none());
    }

    #[test]
    fn an_unknown_tty_has_no_current_session() {
        let store = store(vec![session("here", "/dev/ttys001", 0, 0, 1)]);
        assert!(current_session(&store, "unknown").is_none());
    }
}

#[cfg(test)]
mod tty_tests {
    use super::*;

    #[test]
    fn a_real_terminal_name_becomes_a_device_path() {
        assert_eq!(normalize_tty("ttys054 "), Some("/dev/ttys054".to_string()));
        assert_eq!(
            normalize_tty("/dev/ttys054"),
            Some("/dev/ttys054".to_string())
        );
    }

    #[test]
    fn a_process_without_a_controlling_terminal_reports_nothing() {
        // `ps -o tty=` prints "??" for a process that has no controlling terminal,
        // which is what the shell a hook or tool call runs in looks like.
        assert_eq!(normalize_tty("?? "), None);
        assert_eq!(normalize_tty("?"), None);
        assert_eq!(normalize_tty("   "), None);
        assert_eq!(normalize_tty("not a tty"), None);
    }
}

#[cfg(test)]
mod overwrite_tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn draft(name: &str) -> BookmarkDraft {
        BookmarkDraft {
            name: name.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn a_first_bookmark_replaces_nothing() {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        let outcome = store.upsert("abc", draft("first"), at(0)).unwrap();
        assert!(outcome.replaced.is_none());
        assert_eq!(outcome.bookmark.name, "first");
    }

    #[test]
    fn overwriting_reports_what_was_there_before() {
        // Without this, a mistyped --current silently destroys a name and the output
        // looks exactly like a fresh bookmark.
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("abc", draft("original"), at(0)).unwrap();

        let outcome = store.upsert("abc", draft("replacement"), at(60)).unwrap();
        let replaced = outcome.replaced.expect("the old bookmark is reported");
        assert_eq!(replaced.name, "original");
        assert_eq!(outcome.bookmark.name, "replacement");
    }

    #[test]
    fn rewriting_the_same_text_is_not_reported_as_a_replacement() {
        // Re-running the same command is not a loss, so it should not look like one.
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store.upsert("abc", draft("same"), at(0)).unwrap();

        let outcome = store.upsert("abc", draft("same"), at(60)).unwrap();
        assert!(outcome.replaced.is_none());
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn book(entries: &[(&str, &str, &[&str])]) -> BookmarkBook {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        for (i, (id, name, tags)) in entries.iter().enumerate() {
            store
                .upsert(
                    id,
                    BookmarkDraft {
                        name: (*name).to_string(),
                        tags: tags.iter().map(|t| t.to_string()).collect(),
                        ..Default::default()
                    },
                    Utc.timestamp_opt(1_700_000_000 + i as i64, 0).unwrap(),
                )
                .unwrap();
        }
        store.load().unwrap()
    }

    #[test]
    fn a_word_from_the_name_finds_the_session() {
        let b = book(&[
            ("aaaa-1", "auth refactor", &[]),
            ("bbbb-2", "infra work", &[]),
        ]);
        assert_eq!(lookup(&b, "auth"), Resolution::Unique("aaaa-1".to_string()));
    }

    #[test]
    fn the_name_match_ignores_case() {
        let b = book(&[("aaaa-1", "Auth Refactor", &[])]);
        assert_eq!(
            lookup(&b, "auth refactor"),
            Resolution::Unique("aaaa-1".to_string())
        );
    }

    #[test]
    fn a_tag_finds_the_session_when_no_name_matches() {
        let b = book(&[("aaaa-1", "some work", &["auth"])]);
        assert_eq!(lookup(&b, "auth"), Resolution::Unique("aaaa-1".to_string()));
    }

    #[test]
    fn a_session_id_prefix_still_works() {
        let b = book(&[("aaaa-1", "unrelated", &[])]);
        assert_eq!(lookup(&b, "aaaa"), Resolution::Unique("aaaa-1".to_string()));
    }

    #[test]
    fn an_ambiguous_query_names_every_candidate_instead_of_guessing() {
        // Silently picking one would put the wrong session inside a `$(...)`.
        let b = book(&[
            ("aaaa-1", "auth refactor", &[]),
            ("bbbb-2", "auth cleanup", &[]),
        ]);
        assert_eq!(
            lookup(&b, "auth"),
            Resolution::Ambiguous(vec!["aaaa-1".to_string(), "bbbb-2".to_string()])
        );
    }

    #[test]
    fn a_name_match_wins_over_a_tag_match() {
        // The tag is the broader net, so it only applies when no name matched.
        let b = book(&[
            ("aaaa-1", "auth refactor", &[]),
            ("bbbb-2", "something else", &["auth"]),
        ]);
        assert_eq!(lookup(&b, "auth"), Resolution::Unique("aaaa-1".to_string()));
    }

    #[test]
    fn a_query_that_matches_nothing_resolves_to_nothing() {
        let b = book(&[("aaaa-1", "auth refactor", &[])]);
        assert_eq!(lookup(&b, "deploy"), Resolution::None);
        assert_eq!(lookup(&b, ""), Resolution::None);
    }
}

#[cfg(test)]
mod tag_edit_tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn store_with(name: &str, tags: &[&str]) -> (TempDir, BookmarkStore) {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        store
            .upsert(
                "abc",
                BookmarkDraft {
                    name: name.to_string(),
                    tags: tags.iter().map(|t| t.to_string()).collect(),
                    ..Default::default()
                },
                at(0),
            )
            .unwrap();
        (dir, store)
    }

    fn tags(items: &[&str]) -> Vec<String> {
        items.iter().map(|t| t.to_string()).collect()
    }

    #[test]
    fn adding_a_tag_keeps_the_name_and_the_tags_already_there() {
        let (_dir, store) = store_with("auth refactor", &["cckit", "design"]);
        let edit = store
            .edit_tags("abc", &tags(&["urgent"]), false, at(60))
            .unwrap();

        assert_eq!(edit.bookmark.name, "auth refactor");
        assert_eq!(edit.bookmark.tags, vec!["cckit", "design", "urgent"]);
        assert_eq!(edit.changed, vec!["urgent"]);
    }

    #[test]
    fn adding_a_tag_that_is_already_there_changes_nothing_and_says_so() {
        let (_dir, store) = store_with("work", &["cckit"]);
        let edit = store
            .edit_tags("abc", &tags(&["CCKIT"]), false, at(60))
            .unwrap();

        assert_eq!(edit.bookmark.tags, vec!["cckit"]);
        assert!(edit.changed.is_empty());
        assert_eq!(edit.noop, vec!["CCKIT"]);
    }

    #[test]
    fn removing_takes_only_the_named_tags() {
        let (_dir, store) = store_with("work", &["cckit", "design", "urgent"]);
        let edit = store
            .edit_tags("abc", &tags(&["design"]), true, at(60))
            .unwrap();

        assert_eq!(edit.bookmark.tags, vec!["cckit", "urgent"]);
        assert_eq!(edit.changed, vec!["design"]);
    }

    #[test]
    fn removing_a_tag_that_was_never_there_is_reported_not_silent() {
        let (_dir, store) = store_with("work", &["cckit"]);
        let edit = store
            .edit_tags("abc", &tags(&["nope"]), true, at(60))
            .unwrap();

        assert_eq!(edit.bookmark.tags, vec!["cckit"]);
        assert!(edit.changed.is_empty());
        assert_eq!(edit.noop, vec!["nope"]);
    }

    #[test]
    fn editing_a_session_with_no_bookmark_is_an_error() {
        let (_dir, store) = store_with("work", &["cckit"]);
        let err = store
            .edit_tags("nosuch", &tags(&["x"]), false, at(60))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn stripping_the_last_tag_of_a_nameless_bookmark_is_refused() {
        // A bookmark with neither a name nor a tag remembers nothing; `rm` is the way to
        // delete one, not a tag edit that empties it.
        let (_dir, store) = store_with("", &["cckit"]);
        let err = store
            .edit_tags("abc", &tags(&["cckit"]), true, at(60))
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn a_named_bookmark_may_lose_all_its_tags() {
        let (_dir, store) = store_with("auth refactor", &["cckit"]);
        let edit = store
            .edit_tags("abc", &tags(&["cckit"]), true, at(60))
            .unwrap();
        assert!(edit.bookmark.tags.is_empty());
        assert_eq!(edit.bookmark.name, "auth refactor");
    }
}

#[cfg(test)]
mod listing_footer_tests {
    use super::*;
    use chrono::TimeZone;

    fn bookmark(name: &str) -> Bookmark {
        let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        Bookmark {
            name: name.into(),
            tags: Vec::new(),
            cwd: None,
            session_started_at: None,
            bookmarked_at: ts,
            updated_at: ts,
            hidden: false,
        }
    }

    fn tz() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(9 * 3600).unwrap()
    }

    #[test]
    fn the_listing_says_where_the_bookmarks_are_kept() {
        let b = bookmark("auth refactor");
        let out = format_list(
            &[("abc", &b)],
            &tz(),
            false,
            Path::new("/data/cckit/bookmarks.json"),
        );
        assert!(out.contains("/data/cckit/bookmarks.json"), "got: {out}");
    }

    #[test]
    fn the_footer_counts_what_it_listed() {
        let a = bookmark("one");
        let b = bookmark("two");
        let out = format_list(
            &[("a", &a), ("b", &b)],
            &tz(),
            false,
            Path::new("/data/bookmarks.json"),
        );
        assert!(out.contains("2 bookmarks"), "got: {out}");
    }

    #[test]
    fn an_empty_listing_still_says_where_the_file_would_be() {
        // "Nothing here" and "I looked in the wrong place" are different problems, and
        // the path is what tells them apart.
        let out = format_list(&[], &tz(), false, Path::new("/data/bookmarks.json"));
        assert!(out.contains("/data/bookmarks.json"), "got: {out}");
    }
}

#[cfg(test)]
mod ls_filter_tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn bookmark(name: &str, tags: &[&str], cwd: Option<&str>) -> Bookmark {
        Bookmark {
            name: name.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            cwd: cwd.map(|c| c.to_string()),
            session_started_at: None,
            bookmarked_at: at(0),
            updated_at: at(0),
            hidden: false,
        }
    }

    #[test]
    fn under_keeps_the_directory_itself() {
        let b = bookmark("x", &[], Some("/home/me/proj"));
        assert!(is_under(&b, Path::new("/home/me/proj")));
    }

    #[test]
    fn under_keeps_a_subdirectory() {
        // A repo root should find work started deep inside it.
        let b = bookmark("x", &[], Some("/home/me/proj/a/b"));
        assert!(is_under(&b, Path::new("/home/me/proj")));
    }

    #[test]
    fn under_drops_a_parent_directory() {
        // Standing in a subdirectory does not make the parent's sessions yours.
        let b = bookmark("x", &[], Some("/home/me"));
        assert!(!is_under(&b, Path::new("/home/me/proj")));
    }

    #[test]
    fn under_does_not_match_a_sibling_that_merely_shares_a_prefix() {
        // A worktree is a sibling path, not a child: /proj must not catch /proj-wt.
        let b = bookmark("x", &[], Some("/home/me/proj-wt"));
        assert!(!is_under(&b, Path::new("/home/me/proj")));
    }

    #[test]
    fn a_bookmark_with_no_recorded_cwd_is_not_under_anything() {
        let b = bookmark("x", &[], None);
        assert!(!is_under(&b, Path::new("/home/me/proj")));
    }
}

#[cfg(test)]
mod ls_grouping_tests {
    use super::*;
    use chrono::TimeZone;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn bookmark(name: &str, tags: &[&str], cwd: Option<&str>, updated: i64) -> Bookmark {
        Bookmark {
            name: name.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            cwd: cwd.map(|c| c.to_string()),
            session_started_at: None,
            bookmarked_at: at(0),
            updated_at: at(updated),
            hidden: false,
        }
    }

    #[test]
    fn grouping_by_path_puts_each_directory_under_its_own_heading() {
        let a = bookmark("a", &[], Some("/p/one"), 10);
        let b = bookmark("b", &[], Some("/p/two"), 20);
        let groups = group_by_path(&[("a", &a), ("b", &b)]);

        let headings: Vec<&str> = groups.iter().map(|g| g.heading.as_str()).collect();
        // Newest group first, like every other listing in cckit.
        assert_eq!(headings, vec!["/p/two", "/p/one"]);
    }

    #[test]
    fn a_bookmark_without_a_cwd_lands_in_a_named_bucket() {
        let a = bookmark("a", &[], None, 10);
        let groups = group_by_path(&[("a", &a)]);
        assert_eq!(groups[0].heading, "(unknown)");
    }

    #[test]
    fn grouping_by_tag_lists_a_bookmark_under_each_of_its_tags() {
        // Dropping the duplicate would make "I searched #design and it wasn't there".
        let a = bookmark("a", &["cckit", "design"], None, 10);
        let groups = group_by_tag(&[("a", &a)]);

        let headings: Vec<&str> = groups.iter().map(|g| g.heading.as_str()).collect();
        assert_eq!(headings, vec!["#cckit", "#design"]);
        assert_eq!(groups[0].entries.len(), 1);
        assert_eq!(groups[1].entries.len(), 1);
    }

    #[test]
    fn untagged_bookmarks_collect_at_the_end() {
        let a = bookmark("a", &["cckit"], None, 10);
        let b = bookmark("b", &[], None, 20);
        let groups = group_by_tag(&[("b", &b), ("a", &a)]);

        let headings: Vec<&str> = groups.iter().map(|g| g.heading.as_str()).collect();
        assert_eq!(headings, vec!["#cckit", "(no tag)"]);
    }

    #[test]
    fn tag_groups_are_ordered_by_their_newest_member() {
        let old = bookmark("old", &["alpha"], None, 10);
        let new = bookmark("new", &["zulu"], None, 20);
        let groups = group_by_tag(&[("old", &old), ("new", &new)]);

        let headings: Vec<&str> = groups.iter().map(|g| g.heading.as_str()).collect();
        assert_eq!(headings, vec!["#zulu", "#alpha"]);
    }
}

#[cfg(test)]
mod group_render_tests {
    use super::*;
    use chrono::TimeZone;

    fn bookmark(name: &str, tags: &[&str], cwd: Option<&str>) -> Bookmark {
        let ts = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        Bookmark {
            name: name.into(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            cwd: cwd.map(|c| c.to_string()),
            session_started_at: None,
            bookmarked_at: ts,
            updated_at: ts,
            hidden: false,
        }
    }

    fn tz() -> chrono::FixedOffset {
        chrono::FixedOffset::east_opt(9 * 3600).unwrap()
    }

    #[test]
    fn grouping_by_path_does_not_repeat_the_directory_on_every_row() {
        // The heading already says where it ran; saying it again doubles the line length.
        let b = bookmark("work", &[], Some("/p/one"));
        let out = format_groups(
            &group_by_path(&[("abc", &b)]),
            &tz(),
            false,
            Path::new("/d/bookmarks.json"),
        );
        assert_eq!(out.matches("/p/one").count(), 1, "got:\n{out}");
    }

    #[test]
    fn grouping_by_tag_keeps_the_directory_on_each_row() {
        // Here the heading is a tag, so the row is the only place the path can appear.
        let b = bookmark("work", &["cckit"], Some("/p/one"));
        let out = format_groups(
            &group_by_tag(&[("abc", &b)]),
            &tz(),
            false,
            Path::new("/d/bookmarks.json"),
        );
        assert!(out.contains("/p/one"), "got:\n{out}");
    }

    #[test]
    fn the_footer_counts_rows_not_bookmarks_when_a_tag_repeats_one() {
        let b = bookmark("work", &["cckit", "design"], None);
        let out = format_groups(
            &group_by_tag(&[("abc", &b)]),
            &tz(),
            false,
            Path::new("/d/bookmarks.json"),
        );
        assert!(out.contains("2 rows"), "got:\n{out}");
    }
}

#[cfg(test)]
mod hide_tests {
    use super::*;
    use chrono::TimeZone;
    use tempfile::TempDir;

    fn at(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn store_with(entries: &[(&str, &str, &[&str])]) -> (TempDir, BookmarkStore) {
        let dir = TempDir::new().unwrap();
        let store = BookmarkStore::at(dir.path());
        for (i, (id, name, tags)) in entries.iter().enumerate() {
            store
                .upsert(
                    id,
                    BookmarkDraft {
                        name: (*name).to_string(),
                        tags: tags.iter().map(|t| t.to_string()).collect(),
                        ..Default::default()
                    },
                    at(i as i64),
                )
                .unwrap();
        }
        (dir, store)
    }

    #[test]
    fn hiding_keeps_the_bookmark_and_unhiding_restores_it() {
        let (_dir, store) = store_with(&[("aaaa-1", "auth refactor", &["auth"])]);
        assert!(store.set_hidden("aaaa-1", true, at(10)).unwrap());
        let hidden = store.load().unwrap().get("aaaa-1").cloned().unwrap();
        assert!(hidden.hidden);
        assert_eq!(hidden.name, "auth refactor");
        assert_eq!(hidden.tags, vec!["auth".to_string()]);

        assert!(store.set_hidden("aaaa-1", false, at(20)).unwrap());
        assert!(!store.load().unwrap().get("aaaa-1").unwrap().hidden);
    }

    #[test]
    fn hiding_twice_reports_no_change() {
        let (_dir, store) = store_with(&[("aaaa-1", "auth", &[])]);
        assert!(store.set_hidden("aaaa-1", true, at(10)).unwrap());
        assert!(!store.set_hidden("aaaa-1", true, at(20)).unwrap());
        assert_eq!(
            store.load().unwrap().get("aaaa-1").unwrap().updated_at,
            at(10)
        );
    }

    #[test]
    fn hiding_an_unbookmarked_session_is_not_found() {
        let (_dir, store) = store_with(&[]);
        let err = store.set_hidden("nope", true, at(0)).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn setting_a_hidden_bookmark_again_makes_it_visible() {
        let (_dir, store) = store_with(&[("aaaa-1", "auth", &[])]);
        store.set_hidden("aaaa-1", true, at(10)).unwrap();
        store
            .upsert(
                "aaaa-1",
                BookmarkDraft {
                    name: "auth".into(),
                    ..Default::default()
                },
                at(20),
            )
            .unwrap();
        assert!(!store.load().unwrap().get("aaaa-1").unwrap().hidden);
    }

    #[test]
    fn lookup_skips_hidden_bookmarks_in_every_net() {
        let (_dir, store) = store_with(&[
            ("aaaa-1", "auth refactor", &["infra"]),
            ("bbbb-2", "auth review", &[]),
        ]);
        store.set_hidden("aaaa-1", true, at(10)).unwrap();
        let book = store.load().unwrap();
        // Without the hidden one the name match is no longer ambiguous.
        assert_eq!(lookup(&book, "auth"), Resolution::Unique("bbbb-2".into()));
        assert_eq!(lookup(&book, "infra"), Resolution::None);
        assert_eq!(lookup(&book, "aaaa"), Resolution::None);
    }

    #[test]
    fn hidden_bookmarks_are_not_attached_to_sessions() {
        use crate::history::SessionRecord;
        use std::path::PathBuf;

        let (_dir, store) = store_with(&[("aaaa-1", "auth", &[])]);
        store.set_hidden("aaaa-1", true, at(10)).unwrap();
        let mut sessions = vec![SessionRecord {
            session_id: "aaaa-1".into(),
            cwd: PathBuf::from("/tmp"),
            git_branch: None,
            file_path: PathBuf::from("/tmp/aaaa-1.jsonl"),
            started_at: at(0),
            ended_at: at(0),
            turns: Vec::new(),
            bookmark: None,
        }];
        attach_bookmarks(&mut sessions, &store.load().unwrap());
        assert!(sessions[0].bookmark.is_none());
    }

    #[test]
    fn the_flag_is_left_out_of_the_file_until_set_and_old_files_read_as_visible() {
        let (dir, store) = store_with(&[("aaaa-1", "auth", &[])]);
        let raw = fs::read_to_string(dir.path().join(BOOKMARKS_FILE)).unwrap();
        assert!(!raw.contains("hidden"), "got: {raw}");
        assert!(!store.load().unwrap().get("aaaa-1").unwrap().hidden);

        store.set_hidden("aaaa-1", true, at(10)).unwrap();
        let raw = fs::read_to_string(dir.path().join(BOOKMARKS_FILE)).unwrap();
        assert!(raw.contains("\"hidden\": true"), "got: {raw}");
    }

    #[test]
    fn a_listed_hidden_bookmark_is_marked() {
        let (_dir, store) = store_with(&[("aaaa-1", "auth", &[])]);
        store.set_hidden("aaaa-1", true, at(10)).unwrap();
        let book = store.load().unwrap();
        let entries = book.entries_newest_first();
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        let out = format_list(&entries, &tz, false, Path::new("/d/bookmarks.json"));
        assert!(out.contains("auth (hidden)  aaaa-1"), "got: {out}");
        let json: serde_json::Value = serde_json::from_str(&format_list_json(&entries)).unwrap();
        assert_eq!(json[0]["hidden"], true);
    }

    #[test]
    fn a_hidden_row_is_dimmed_whole_under_color() {
        colored::control::set_override(true);
        let (_dir, store) = store_with(&[("aaaa-1", "auth", &[]), ("bbbb-2", "infra", &[])]);
        store.set_hidden("aaaa-1", true, at(10)).unwrap();
        let book = store.load().unwrap();
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        let hidden = bookmark_row("aaaa-1", book.get("aaaa-1").unwrap(), &tz, true, false);
        let visible = bookmark_row("bbbb-2", book.get("bbbb-2").unwrap(), &tz, true, false);
        // One dim span around the whole row, no magenta name or blue id inside it.
        assert!(hidden.starts_with("\u{1b}[2m"), "got: {hidden:?}");
        assert!(!hidden.contains("\u{1b}[35m") && !hidden.contains("\u{1b}[34m"));
        assert!(hidden.contains("auth (hidden)  aaaa-1"), "got: {hidden:?}");
        assert!(visible.contains("\u{1b}[35m"), "got: {visible:?}");
    }

    #[test]
    fn the_hidden_note_names_the_count_and_the_way_back() {
        assert_eq!(
            hidden_note(1),
            "1 more is hidden; `cckit session bookmark ls --all` shows them\n"
        );
        assert!(hidden_note(3).starts_with("3 more are hidden"));
    }
}
