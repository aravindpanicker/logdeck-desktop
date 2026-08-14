//! Tailing a **Project**'s logs: one polling thread per Project, forever.
//!
//! BUILD-SPEC §5. There is no `notify` dependency: comparing `metadata().len()`
//! against the stored offset detects truncation and rotation as a direct
//! consequence — a shrinking file *is* the signal, and filesystem events do not
//! report it cleanly.
//!
//! Two modes, and the difference between them is the memory bound in ADR 0002:
//!
//! - [`Mode::Selected`] parses fully and emits `log:entry` / `log:break`.
//! - [`Mode::Background`] parses the same bytes only far enough to count
//!   **Entries** by **Level**, then **discards the text**. A background watcher
//!   that retained Entry text would turn idle memory into a function of total
//!   log volume across every registered Project.
//!
//! Selecting a Project *promotes* its watcher and *demotes* the previous one; it
//! never starts or stops watching (ADR 0002).
//!
//! Emission goes through [`EventSink`] rather than straight to `AppHandle` so
//! the poll loop is testable over a temp directory with no Tauri runtime.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::model::{Break, BreakId, BreakKind, EntryId, Level, LogEntry, ProjectId, StreamItem};
use crate::parser::Accumulator;
use crate::project::logs_dir;

/// The poll period (BUILD-SPEC §5).
pub const POLL_INTERVAL: Duration = Duration::from_millis(300);
/// The ceiling the offline backoff walks up to (D9).
pub const MAX_BACKOFF: Duration = Duration::from_secs(5);
/// The opening window stops at this many **Entries** (D6).
pub const WINDOW_ENTRIES: usize = 500;
/// …or this many bytes, whichever comes first (D6).
pub const WINDOW_MAX_BYTES: u64 = 2 * 1024 * 1024;
/// The first backward read; doubled until one of the two limits above is hit.
pub const WINDOW_CHUNK_BYTES: u64 = 64 * 1024;
/// Ceiling on how much one poll ingests, so a huge append cannot allocate
/// without bound. The remainder is picked up on the next tick.
pub const MAX_READ_PER_POLL: u64 = 8 * 1024 * 1024;

/// Event names. Written once here so a typo cannot silently detach the frontend
/// (BUILD-SPEC §3).
pub const EVENT_ENTRY: &str = "log:entry";
/// One poll's worth of closed **Entries**, in order, as a single event.
///
/// A tight-looping queue worker can close thousands of Entries inside one 300 ms
/// tick. One `emit` each would serialise thousands of payloads inside the poll
/// and make the client run a full snapshot, filter pass and re-render for every
/// one of them — the view freezes exactly during the burst the reader most needs
/// it live. The batch is the same `LogEntry` payload as [`EVENT_ENTRY`], and is
/// upserted item by item on the client, so nothing about identity or revision
/// (D2) changes.
pub const EVENT_ENTRIES: &str = "log:entries";
pub const EVENT_BREAK: &str = "log:break";
pub const EVENT_ACTIVITY: &str = "project:activity";
pub const EVENT_STATUS: &str = "project:status";

/* -------------------------------------------------------------------------- */
/* Wire types                                                                  */
/* -------------------------------------------------------------------------- */

/// One file inside a **Project**'s `storage/logs/`.
///
/// Shape is fixed by `src/lib/types.ts`, which declared it before this module
/// existed: `modified` is **epoch seconds**, not an RFC 3339 string.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogFile {
    pub name: String,
    pub bytes: u64,
    /// Last-modified time, seconds since the Unix epoch.
    pub modified: u64,
}

/// The **Target** a Project is reading (D5).
///
/// Externally tagged and camelCase, so it is `"latest"` or `{"file":"…"}` on the
/// wire — the shape `src/lib/types.ts` already assumes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Target {
    #[default]
    Latest,
    File(String),
}

/// How much a Project the user is *not* reading has written (D8).
///
/// Counts and a highest **Level** — never text. See ADR 0002.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityPayload {
    pub project_id: ProjectId,
    pub total: u64,
    pub counts: BTreeMap<Level, u64>,
    pub max_level: Option<Level>,
}

/// Whether a Project can be read right now (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StatusState {
    Online,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusPayload {
    pub project_id: ProjectId,
    pub state: StatusState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/* -------------------------------------------------------------------------- */
/* Emission                                                                    */
/* -------------------------------------------------------------------------- */

/// Where a poll's output goes.
///
/// A trait rather than a direct `AppHandle::emit` so [`WatcherState::poll`] can
/// be driven in a unit test against a temp directory with no Tauri runtime.
pub trait EventSink: Send + Sync + 'static {
    fn entry(&self, entry: &LogEntry);
    /// A whole poll's closed **Entries** at once.
    ///
    /// The default sends them one at a time, which is what a sink with no
    /// per-emit cost wants; [`TauriSink`] overrides it because crossing the IPC
    /// boundary is the cost being amortised.
    fn entries(&self, entries: &[LogEntry]) {
        for entry in entries {
            self.entry(entry);
        }
    }
    fn brk(&self, brk: &Break);
    fn activity(&self, payload: &ActivityPayload);
    /// Returns whether the payload actually reached the frontend.
    ///
    /// Status is the one event emitted **only on a transition**, so a failed
    /// emit is never made up for by the next one. The caller uses this answer to
    /// decide whether the transition has been announced or must be retried — an
    /// offline Project that never recovers would otherwise stay silently online
    /// in the sidebar forever.
    fn status(&self, payload: &StatusPayload) -> bool;
}

/// The real sink. Emit failures are reported, never swallowed: a dropped event
/// is a frontend that silently stops updating.
pub struct TauriSink {
    app: tauri::AppHandle,
}

impl TauriSink {
    pub fn new(app: tauri::AppHandle) -> Self {
        Self { app }
    }

    fn send<T: Serialize + Clone>(&self, event: &str, payload: &T) -> bool {
        use tauri::Emitter;
        match self.app.emit(event, payload.clone()) {
            Ok(()) => true,
            Err(err) => {
                eprintln!("logdeck: could not emit `{event}`: {err}");
                false
            }
        }
    }
}

impl EventSink for TauriSink {
    fn entry(&self, entry: &LogEntry) {
        self.send(EVENT_ENTRY, entry);
    }
    /// One event for the whole batch. An empty batch is not an event.
    fn entries(&self, entries: &[LogEntry]) {
        if entries.is_empty() {
            return;
        }
        self.send(EVENT_ENTRIES, &entries.to_vec());
    }
    fn brk(&self, brk: &Break) {
        self.send(EVENT_BREAK, brk);
    }
    fn activity(&self, payload: &ActivityPayload) {
        self.send(EVENT_ACTIVITY, payload);
    }
    fn status(&self, payload: &StatusPayload) -> bool {
        self.send(EVENT_STATUS, payload)
    }
}

/* -------------------------------------------------------------------------- */
/* Files                                                                       */
/* -------------------------------------------------------------------------- */

/// A `storage/logs` entry we are willing to read: `laravel.log`,
/// `laravel-YYYY-MM-DD.log`, or anything else the Project's channel writes with
/// a `.log` suffix. Dotfiles are skipped — `laravel/laravel` ships a
/// `storage/logs/.gitignore`.
fn is_log_name(name: &str) -> bool {
    !name.starts_with('.') && name.ends_with(".log")
}

/// A file name supplied by the frontend, turned into a path we will open.
///
/// Name only: no separator, no `..`, no absolute path. All file access goes
/// through Rust precisely so a runtime-chosen path cannot escape the folder the
/// user registered (BUILD-SPEC §7).
fn resolve_named(dir: &Path, name: &str) -> Result<PathBuf, String> {
    if !is_log_name(name)
        || name.contains(std::path::MAIN_SEPARATOR)
        || name.contains('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.contains("..")
    {
        return Err(format!("`{name}` is not a log file name"));
    }
    Ok(dir.join(name))
}

fn modified_secs(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

/// Re-resolve a registered **Project** root immediately before reading it.
///
/// The registry holds the path canonicalized *at registration*. A folder that
/// has since been moved away is the D9 case and shows up here as a
/// canonicalization failure; a folder replaced by a symlink to somewhere else is
/// a confinement failure and shows up as a path that no longer resolves to
/// itself. Every read — tailing *and* listing — goes through this, so neither
/// can be talked into reading a directory the user never chose.
fn canonical_root(project_root: &Path) -> Result<PathBuf, String> {
    let root = fs::canonicalize(project_root)
        .map_err(|err| format!("cannot resolve {}: {err}", project_root.display()))?;
    if root != project_root {
        return Err(format!(
            "{} no longer resolves to itself",
            project_root.display()
        ));
    }
    Ok(root)
}

/// Every log file in a **Project**'s `storage/logs/`, newest first.
pub fn list_files(project_root: &Path) -> Result<Vec<LogFile>, String> {
    let root = canonical_root(project_root)?;
    let dir = logs_dir(&root);
    let read = fs::read_dir(&dir).map_err(|err| format!("cannot read {}: {err}", dir.display()))?;

    let mut files: Vec<LogFile> = Vec::new();
    for entry in read {
        // One unreadable sibling must not hide the rest of the directory.
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_log_name(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        files.push(LogFile {
            name,
            bytes: meta.len(),
            modified: modified_secs(&meta),
        });
    }

    files.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| b.name.cmp(&a.name))
    });
    Ok(files)
}

/// The newest file by mtime, which is what `Latest` means (D5).
///
/// `Ok(None)` is a readable directory with nothing in it yet — a fresh Laravel
/// project that has not logged. That is not an outage, so it must not report
/// offline.
fn newest_log(dir: &Path) -> Result<Option<(String, PathBuf)>, String> {
    let read = fs::read_dir(dir).map_err(|err| format!("cannot read {}: {err}", dir.display()))?;

    let mut best: Option<(SystemTime, String)> = None;
    for entry in read {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_log_name(&name) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        // Name breaks an mtime tie, so two files written in the same coarse
        // filesystem tick resolve to the later date rather than to whichever
        // the directory happened to yield first.
        let better = match &best {
            Some((best_time, best_name)) => (mtime, &name) > (*best_time, best_name),
            None => true,
        };
        if better {
            best = Some((mtime, name));
        }
    }

    Ok(best.map(|(_, name)| {
        let path = dir.join(&name);
        (name, path)
    }))
}

/// Read `[start, end)` of a file.
fn read_range(path: &Path, start: u64, end: u64) -> io::Result<Vec<u8>> {
    if end <= start {
        return Ok(Vec::new());
    }
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(start))?;
    let mut buffer = Vec::with_capacity((end - start) as usize);
    file.take(end - start).read_to_end(&mut buffer)?;
    Ok(buffer)
}

/* -------------------------------------------------------------------------- */
/* The opening window (D6)                                                     */
/* -------------------------------------------------------------------------- */

/// The result of a backward walk: what to show, and where it starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    pub items: Vec<StreamItem>,
    /// Offset of the first **Entry** shown — what `load_earlier` pages back from.
    pub first_offset: u64,
    /// Where live tailing resumes. This is the *last* Entry's offset, not EOF:
    /// the poll re-reads it and re-emits it under the same id, so a record still
    /// being written is revised in place rather than duplicated (D2).
    pub next_offset: u64,
}

/// A parsed Entry that never carried a header — the tail of an Entry whose
/// header sits before the window we read.
fn is_headerless(entry: &LogEntry) -> bool {
    entry.timestamp.is_empty() && entry.env.is_empty()
}

/* -------------------------------------------------------------------------- */
/* Identity across a Break                                                     */
/* -------------------------------------------------------------------------- */

/// How many **Breaks** this watcher has passed. Zero until the first one, so an
/// untouched file produces exactly the ids `EntryId::new` describes.
///
/// `{file}:{offset}` alone is not unique within one **Session Record**: a
/// truncation resets the offset to 0 under the same file name, so the first
/// Entry written after a second `log:clear` would carry the id of the first
/// Entry written after the first one. The client upserts by id (D2), so that
/// collision would silently overwrite an Entry sitting above a Break — the loss
/// ADR 0001 exists to prevent, arrived at by identity rather than by an explicit
/// clear. Salting with the generation makes the two provably distinct.
type Generation = u64;

/// The file token an id is built from: the name itself before any Break, and a
/// generation-qualified name after one.
fn generation_token(file: &str, generation: Generation) -> String {
    if generation == 0 {
        file.to_owned()
    } else {
        format!("{file}@{generation}")
    }
}

fn stamped_entry_id(file: &str, offset: u64, generation: Generation) -> EntryId {
    EntryId::new(&generation_token(file, generation), offset)
}

fn stamped_break_id(file: &str, offset: u64, generation: Generation) -> BreakId {
    BreakId::new(&generation_token(file, generation), offset)
}

/// The inverse of [`stamped_entry_id`]: file, generation, and offset back out of
/// an id the client is holding.
///
/// `load_earlier` needs all three. The client knows an **Entry**'s `file` and
/// `offset` as fields, but not which generation stamped it — and after a Break
/// the oldest Entry held belongs to the *previous* file and generation, so
/// paging from its offset against the file currently being tailed reads the
/// wrong bytes. The id is the only thing that carries the whole triple, so it is
/// what the command takes.
///
/// The parse is unambiguous because a log file name always ends in `.log`
/// ([`is_log_name`]): the segment after the last `@` is a generation only when
/// it parses as a number, which `…log` never does.
fn decode_entry_id(id: &str) -> Result<(String, Generation, u64), String> {
    let malformed = || format!("`{id}` is not an Entry id");
    let (token, offset) = id.rsplit_once(':').ok_or_else(malformed)?;
    let offset: u64 = offset.parse().map_err(|_| malformed())?;

    match token.rsplit_once('@') {
        Some((file, generation)) => match generation.parse::<Generation>() {
            Ok(generation) if !file.is_empty() => Ok((file.to_owned(), generation, offset)),
            _ => Ok((token.to_owned(), 0, offset)),
        },
        None => Ok((token.to_owned(), 0, offset)),
    }
}

/// Walk backwards from `before`, doubling the read until 500 **Entries** or
/// 2 MB or the start of the file (D6).
///
/// Everything before the first header is discarded, so the window never opens
/// mid-stack-trace — unless dropping it would leave nothing at all, or the walk
/// reached the start of the file, where a headerless first Entry is genuinely
/// the file's own content.
pub fn read_window(
    project_id: &ProjectId,
    file: &str,
    path: &Path,
    before: u64,
    generation: Generation,
) -> io::Result<Window> {
    let mut chunk = WINDOW_CHUNK_BYTES;
    let mut start;
    let mut entries;

    loop {
        start = before.saturating_sub(chunk);
        let bytes = read_range(path, start, before)?;
        entries = crate::parser::parse_bytes(project_id, file, start, &bytes);

        let enough = entries.len() >= WINDOW_ENTRIES || chunk >= WINDOW_MAX_BYTES || start == 0;
        if enough {
            break;
        }
        chunk *= 2;
    }

    // Tidying the leading partial Entry away is a courtesy, not a rule. One
    // Entry whose context runs past the 2 MB cap — a logged request body, a
    // 40 000-frame trace — parses as a single headerless Entry, and discarding
    // it would hand back an empty window for a file that plainly has content,
    // losing it for good: live tailing would resume at EOF and `load_earlier`
    // would repeat the same empty answer. D6 promises whichever cap comes
    // first, which means a partial view, never no view.
    if start > 0 && entries.len() > 1 && entries.first().is_some_and(is_headerless) {
        entries.remove(0);
    }
    if entries.len() > WINDOW_ENTRIES {
        entries.drain(..entries.len() - WINDOW_ENTRIES);
    }
    for entry in &mut entries {
        entry.id = stamped_entry_id(&entry.file, entry.offset, generation);
    }

    let first_offset = entries.first().map(|entry| entry.offset).unwrap_or(before);
    let next_offset = entries.last().map(|entry| entry.offset).unwrap_or(before);

    Ok(Window {
        items: entries.into_iter().map(StreamItem::Entry).collect(),
        first_offset,
        next_offset,
    })
}

/* -------------------------------------------------------------------------- */
/* Activity                                                                    */
/* -------------------------------------------------------------------------- */

/// **Activity**: how much and how bad, never what.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Activity {
    pub total: u64,
    pub counts: BTreeMap<Level, u64>,
    pub max_level: Option<Level>,
}

impl Activity {
    fn record(&mut self, level: Level) {
        self.total += 1;
        *self.counts.entry(level).or_insert(0) += 1;
        self.max_level = Some(match self.max_level {
            Some(current) => current.max(level),
            None => level,
        });
    }

    fn clear(&mut self) {
        *self = Self::default();
    }

    fn payload(&self, project_id: &ProjectId) -> ActivityPayload {
        ActivityPayload {
            project_id: project_id.clone(),
            total: self.total,
            counts: self.counts.clone(),
            max_level: self.max_level,
        }
    }
}

/* -------------------------------------------------------------------------- */
/* The poll loop                                                               */
/* -------------------------------------------------------------------------- */

/// Whether this Project's text is kept or discarded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The Project the user is reading: parse fully, emit Entries and Breaks.
    Selected,
    /// Every other Project: parse for counts, discard the text (ADR 0002).
    Background,
}

/// Everything one watcher knows. Driven by [`WatcherState::poll`], which is the
/// whole of the watcher's behaviour and is exercised directly by the tests.
pub struct WatcherState {
    project_id: ProjectId,
    /// The Project root, already canonicalized by `project.rs`.
    root: PathBuf,
    target: Target,
    mode: Mode,
    /// The **Target** file currently being followed, once one has been resolved.
    file: Option<String>,
    offset: u64,
    /// The tail of a physical line longer than one poll's read, held until its
    /// `\n` arrives.
    ///
    /// The bytes are already past `offset` — `carry` always occupies exactly
    /// `[offset - carry.len(), offset)`. Feeding half a line to the parser and
    /// the other half next tick would insert a `\n` the file never contained
    /// into `context` and `raw`, and `raw` is what Copy sends verbatim (D1,
    /// BUILD-SPEC §2). Holding it costs no more than the Entry that will
    /// eventually own it.
    carry: Vec<u8>,
    accumulator: Option<Accumulator>,
    activity: Activity,
    /// The pending **Entry** already counted, so it is not counted again when it
    /// closes. Background mode only.
    counted_pending: Option<EntryId>,
    /// Id and length of the pending Entry as last emitted, so an unchanged
    /// pending Entry is not re-sent every 300 ms.
    emitted_pending: Option<(EntryId, usize)>,
    /// How many **Breaks** this watcher has passed, salting Entry and Break ids
    /// so a reset offset cannot reuse an id already spent (ADR 0001).
    generation: Generation,
    /// The last status observed, which is what the retry interval is chosen
    /// from. Not the same as what the frontend has been told — see `announced`.
    reported: Option<StatusState>,
    /// The last status the frontend actually received. Emission is on a
    /// transition only, so a transition whose emit failed has not happened as
    /// far as the sidebar is concerned and must be retried.
    announced: Option<(StatusState, Option<String>)>,
    backoff: Duration,
}

impl WatcherState {
    pub fn new(project_id: ProjectId, root: PathBuf, mode: Mode) -> Self {
        Self {
            project_id,
            root,
            target: Target::Latest,
            mode,
            file: None,
            offset: 0,
            carry: Vec::new(),
            accumulator: None,
            activity: Activity::default(),
            counted_pending: None,
            emitted_pending: None,
            generation: 0,
            reported: None,
            announced: None,
            backoff: POLL_INTERVAL,
        }
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn target(&self) -> &Target {
        &self.target
    }

    pub fn file(&self) -> Option<&str> {
        self.file.as_deref()
    }

    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    /// How long until the next poll: the base period while online, the backoff
    /// while not (D9).
    pub fn interval(&self) -> Duration {
        match self.reported {
            Some(StatusState::Offline) => self.backoff,
            _ => POLL_INTERVAL,
        }
    }

    /// Selecting a Project promotes its watcher; it does not start watching
    /// (ADR 0002). The **Activity** it accrued while unselected is spent.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
        self.emitted_pending = None;
        if mode == Mode::Selected {
            self.activity.clear();
            self.counted_pending = None;
        }
    }

    pub fn clear_activity(&mut self) {
        self.activity.clear();
    }

    /// Point the watcher at a different **Target** and start it over there.
    pub fn set_target(&mut self, target: Target) {
        self.target = target;
        self.detach();
    }

    /// Forget the current file so the next poll re-resolves and re-attaches.
    fn detach(&mut self) {
        self.file = None;
        self.offset = 0;
        self.carry.clear();
        self.accumulator = None;
        self.counted_pending = None;
        self.emitted_pending = None;
    }

    /// Attach to `file` at `offset` without treating it as a rotation. Used by
    /// `select_project` / `set_target`, which have just read a window and know
    /// exactly where live tailing should resume.
    pub fn attach(&mut self, file: String, offset: u64) {
        self.accumulator = Some(Accumulator::new(self.project_id.clone(), &file));
        self.file = Some(file);
        self.offset = offset;
        self.carry.clear();
        self.counted_pending = None;
        self.emitted_pending = None;
    }

    /// Read the opening window and resume live tailing from it (D6).
    ///
    /// Live tailing resumes at the *last* Entry rather than at EOF, so a record
    /// still being written is re-emitted under the same id and revised in place
    /// instead of appearing twice (D2).
    pub fn open(&mut self) -> Result<Vec<StreamItem>, String> {
        self.detach();
        let Some((name, path)) = self.resolve()? else {
            return Ok(Vec::new());
        };
        let len =
            file_len(&path).map_err(|err| format!("cannot stat {}: {err}", path.display()))?;
        let window = read_window(&self.project_id, &name, &path, len, self.generation)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        self.attach(name, window.next_offset);
        Ok(window.items)
    }

    /// Page another window back from the **Entry** with this id (D6). The
    /// **Target** is unchanged and live tailing is untouched.
    ///
    /// The page is resolved against the file *that Entry came from*, not against
    /// the file currently being tailed. After a **Break** the oldest Entry held
    /// belongs to the previous file and generation, and reading the current file
    /// at its offset would prepend bytes from the wrong file above the Break.
    /// Its generation is reused for the same reason: the page has to land in the
    /// same identity space as the Entries it sits directly above (ADR 0001).
    pub fn earlier(&self, before_id: &str) -> Result<Vec<StreamItem>, String> {
        let (name, generation, before) = decode_entry_id(before_id)?;
        if before == 0 {
            return Ok(Vec::new());
        }
        // The name arrives from the client, so it goes through the same
        // confinement check every other read does (BUILD-SPEC §7).
        let path = resolve_named(&logs_dir(&canonical_root(&self.root)?), &name)?;
        let window = read_window(&self.project_id, &name, &path, before, generation)
            .map_err(|err| format!("cannot read {}: {err}", path.display()))?;
        Ok(window.items)
    }

    /// The **Target** as a concrete file, or the reason there is none.
    ///
    /// The root is re-resolved every poll rather than trusted from the registry:
    /// a folder that was moved away is exactly the D9 case, and it shows up here
    /// as a canonicalization failure.
    fn resolve(&self) -> Result<Option<(String, PathBuf)>, String> {
        let root = canonical_root(&self.root)?;
        let dir = logs_dir(&root);
        match &self.target {
            Target::Latest => newest_log(&dir),
            Target::File(name) => {
                let path = resolve_named(&dir, name)?;
                if path.is_file() {
                    Ok(Some((name.clone(), path)))
                } else {
                    Err(format!("{} is not readable", path.display()))
                }
            }
        }
    }

    /// One tick. The whole of the watcher's behaviour (BUILD-SPEC §5).
    pub fn poll(&mut self, sink: &dyn EventSink) {
        match self.resolve() {
            Ok(Some((name, path))) => {
                self.report_online(sink);
                self.pump(sink, &name, &path);
            }
            // Readable, but nothing written yet. Online with nothing to do.
            Ok(None) => self.report_online(sink),
            Err(reason) => self.report_offline(sink, reason),
        }
    }

    fn report_online(&mut self, sink: &dyn EventSink) {
        self.backoff = POLL_INTERVAL;
        self.reported = Some(StatusState::Online);
        self.announce(sink, StatusState::Online, None);
    }

    /// A missing or unreadable path goes offline and keeps retrying, backing off
    /// 300 ms toward 5 s. It never gives up: removal is a deliberate user action
    /// (D9), so reattachment must happen unaided.
    fn report_offline(&mut self, sink: &dyn EventSink, reason: String) {
        if self.reported == Some(StatusState::Offline) {
            self.backoff = (self.backoff * 2).min(MAX_BACKOFF);
        } else {
            self.backoff = POLL_INTERVAL;
        }
        self.reported = Some(StatusState::Offline);
        self.announce(sink, StatusState::Offline, Some(reason));
    }

    /// Send a status only on a transition — but a transition the frontend never
    /// received is not one it can act on, so an emit that failed is retried on
    /// the next poll rather than assumed delivered.
    fn announce(&mut self, sink: &dyn EventSink, state: StatusState, reason: Option<String>) {
        let already = matches!(
            &self.announced,
            Some((announced, previous))
                if *announced == state && previous.as_deref() == reason.as_deref()
        );
        if already {
            return;
        }

        let delivered = sink.status(&StatusPayload {
            project_id: self.project_id.clone(),
            state,
            reason: reason.clone(),
        });
        if delivered {
            self.announced = Some((state, reason));
        }
    }

    /// Follow the resolved file: rotation, truncation, then the delta.
    fn pump(&mut self, sink: &dyn EventSink, name: &str, path: &Path) {
        if self.file.as_deref() != Some(name) {
            let first_attach = self.file.is_none();
            self.close_pending(sink);
            self.accumulator = Some(Accumulator::new(self.project_id.clone(), name));
            self.file = Some(name.to_owned());
            // Held bytes belong to the file we are leaving; they can never be
            // completed now, so they are dropped rather than prefixed onto the
            // new file's first line.
            self.carry.clear();
            self.counted_pending = None;
            self.emitted_pending = None;

            if first_attach {
                // Nothing has been shown for this Project yet, so history is not
                // ours to replay here: `select_project` reads the opening window
                // (D6), and a background Project's **Activity** counts what is
                // written from now on, not what was already there.
                self.offset = file_len(path).unwrap_or(0);
                return;
            }

            // A newer file appeared under `Latest` (D5). Everything above the
            // Break survives (ADR 0001), so the ids below it must not reuse the
            // ids above it — a rotation can land on a name we have already read.
            self.offset = 0;
            self.generation += 1;
            self.emit_break(sink, BreakKind::Rotated, name, 0);
        }

        let len = match file_len(path) {
            Ok(len) => len,
            Err(err) => {
                self.report_offline(sink, format!("cannot stat {}: {err}", path.display()));
                return;
            }
        };

        if len < self.offset {
            // A shrinking file *is* the truncation signal. The client buffer is
            // never cleared — a Break is inserted and everything above it stays
            // (D3, ADR 0001). The offset restarts at 0, so the generation moves
            // with it: without that, the first Entry of this clear would carry
            // the id of the first Entry of the last one and overwrite it in the
            // client's id-keyed **Session Record**.
            self.generation += 1;
            self.emit_break(sink, BreakKind::Cleared, name, self.offset);
            self.offset = 0;
            self.carry.clear();
            self.accumulator = Some(Accumulator::new(self.project_id.clone(), name));
            self.counted_pending = None;
            self.emitted_pending = None;
        }

        if len == self.offset {
            return;
        }

        let want = (len - self.offset).min(MAX_READ_PER_POLL);
        let bytes = match read_range(path, self.offset, self.offset + want) {
            Ok(bytes) => bytes,
            Err(err) => {
                self.report_offline(sink, format!("cannot read {}: {err}", path.display()));
                return;
            }
        };

        // Process only through the last `\n`, advancing the offset by exactly
        // the bytes consumed: a half-written record is re-read next tick rather
        // than parsed as garbage (BUILD-SPEC §5).
        match bytes.iter().rposition(|byte| *byte == b'\n') {
            Some(last) => self.ingest(sink, &bytes[..last + 1]),
            // A single physical line longer than one poll's read would stall
            // forever if it were re-read whole every tick, so its bytes are
            // taken off the file and *held* rather than handed to the parser.
            // Feeding the parser half a line now and the rest next tick would
            // append them as two lines, inserting a `\n` the file never
            // contained into `context` and `raw` — and `raw` is what Copy sends
            // verbatim (D1, BUILD-SPEC §2). It would split a UTF-8 sequence at
            // the cut too. Monolog writes a record per `fwrite()`, so this is
            // pathological rather than expected.
            None if want == MAX_READ_PER_POLL => {
                self.carry.extend_from_slice(&bytes);
                self.offset += bytes.len() as u64;
            }
            None => (),
        }
    }

    /// Feed the parser and do with the result whatever this **Mode** does.
    ///
    /// Any bytes held back by a previous poll are rejoined here, so the parser
    /// only ever sees whole lines and the Entry that owns them starts at the
    /// offset the held bytes came from — never at the artificial cut.
    fn ingest(&mut self, sink: &dyn EventSink, bytes: &[u8]) {
        if self.accumulator.is_none() {
            return;
        }
        let carry = std::mem::take(&mut self.carry);
        let start = self.offset - carry.len() as u64;
        let joined: Vec<u8>;
        let data: &[u8] = if carry.is_empty() {
            bytes
        } else {
            joined = carry.into_iter().chain(bytes.iter().copied()).collect();
            &joined
        };

        let Some(accumulator) = self.accumulator.as_mut() else {
            return;
        };
        let mut closed = accumulator.push_bytes(start, data);
        self.offset += bytes.len() as u64;

        match self.mode {
            Mode::Selected => {
                // One event for the tick, not one per Entry: a queue worker can
                // close thousands inside a single 300 ms poll, and the client
                // does a snapshot, a filter pass and a re-render per event.
                for entry in &mut closed {
                    entry.id = stamped_entry_id(&entry.file, entry.offset, self.generation);
                }
                sink.entries(&closed);
                self.emit_pending(sink);
            }
            Mode::Background => {
                // THE memory bound (ADR 0002). Only the Level of each Entry
                // survives this function; `closed` is dropped with its text at
                // the end of the statement.
                let levels: Vec<Level> = closed.iter().map(|entry| entry.level).collect();
                let ids: Vec<EntryId> = closed.into_iter().map(|entry| entry.id).collect();
                let pending = self
                    .accumulator
                    .as_ref()
                    .and_then(Accumulator::pending)
                    .map(|entry| (entry.id.clone(), entry.level));
                self.count(sink, ids, levels, pending);
            }
        }
    }

    /// Count closed **Entries** and the still-open one, exactly once each.
    ///
    /// The newest Entry is normally still pending — the next header is what
    /// closes it — so counting only closed Entries would leave the exception you
    /// just threw out of the badge until the next one arrived.
    fn count(
        &mut self,
        sink: &dyn EventSink,
        ids: Vec<EntryId>,
        levels: Vec<Level>,
        pending: Option<(EntryId, Level)>,
    ) {
        let before = self.activity.total;
        let mut closed = ids.into_iter().zip(levels);

        if let Some(counted) = self.counted_pending.take() {
            match closed.next() {
                // The Entry counted while pending has now closed.
                Some((id, _)) if id == counted => {}
                Some((_, level)) => self.activity.record(level),
                None => self.counted_pending = Some(counted),
            }
        }
        for (_, level) in closed {
            self.activity.record(level);
        }
        if let Some((id, level)) = pending {
            if self.counted_pending.as_ref() != Some(&id) {
                self.activity.record(level);
                self.counted_pending = Some(id);
            }
        }

        if self.activity.total != before {
            sink.activity(&self.activity.payload(&self.project_id));
        }
    }

    /// Emit an **Entry** the accumulator still owns, under the id this
    /// generation gives it. Before the first **Break** that is the id the
    /// accumulator already built, so the common case copies nothing.
    fn emit_entry(&self, sink: &dyn EventSink, entry: &LogEntry) {
        if self.generation == 0 {
            sink.entry(entry);
            return;
        }
        let mut stamped = entry.clone();
        stamped.id = stamped_entry_id(&stamped.file, stamped.offset, self.generation);
        sink.entry(&stamped);
    }

    /// Emit the pending **Entry** so the newest one appears immediately, and
    /// again whenever it grows — revised in place under the same id (D2).
    fn emit_pending(&mut self, sink: &dyn EventSink) {
        let signature = self
            .accumulator
            .as_ref()
            .and_then(Accumulator::pending)
            .map(|entry| (entry.id.clone(), entry.raw.len()));

        if signature == self.emitted_pending {
            return;
        }
        if let Some(entry) = self.accumulator.as_ref().and_then(Accumulator::pending) {
            self.emit_entry(sink, entry);
        }
        self.emitted_pending = signature;
    }

    /// Close the open **Entry** before a file boundary, so its final form is
    /// emitted rather than abandoned mid-trace.
    fn close_pending(&mut self, sink: &dyn EventSink) {
        let Some(accumulator) = self.accumulator.as_mut() else {
            return;
        };
        let Some(entry) = accumulator.flush() else {
            return;
        };
        match self.mode {
            Mode::Selected => {
                if self.emitted_pending.as_ref().map(|(id, len)| (id, *len))
                    != Some((&entry.id, entry.raw.len()))
                {
                    self.emit_entry(sink, &entry);
                }
            }
            Mode::Background => {
                if self.counted_pending.as_ref() != Some(&entry.id) {
                    self.activity.record(entry.level);
                    sink.activity(&self.activity.payload(&self.project_id));
                }
            }
        }
        self.counted_pending = None;
        self.emitted_pending = None;
    }

    /// Breaks belong to the **Session Record** of the Project being read. An
    /// unselected Project has no record to interrupt.
    fn emit_break(&self, sink: &dyn EventSink, kind: BreakKind, file: &str, offset: u64) {
        if self.mode != Mode::Selected {
            return;
        }
        sink.brk(&Break {
            id: stamped_break_id(file, offset, self.generation),
            project_id: self.project_id.clone(),
            kind,
            file: file.to_owned(),
        });
    }

    /// Bytes of **Entry** text this watcher is holding on to.
    ///
    /// The ADR 0002 assertion is written against this: in [`Mode::Background`]
    /// it must stay bounded by the single open Entry no matter how much the
    /// Project writes.
    #[cfg(test)]
    pub(crate) fn retained_text_bytes(&self) -> usize {
        self.accumulator
            .as_ref()
            .and_then(Accumulator::pending)
            .map(|entry| entry.raw.len() + entry.context.len() + entry.message.len())
            .unwrap_or(0)
    }

    /// Every byte this watcher is *still holding* after a poll returns.
    ///
    /// Broader than [`WatcherState::retained_text_bytes`]: it also counts the
    /// held partial line, the two pending-Entry bookmarks, the resolved file
    /// name, and the **Activity** histogram. ADR 0002's bound is a claim about
    /// the whole of a background watcher's retained state over time, not just
    /// about the open Entry, so the soak assertion is written against this.
    #[cfg(test)]
    pub(crate) fn retained_state_bytes(&self) -> usize {
        self.retained_text_bytes()
            + self.carry.len()
            + self.file.as_deref().map_or(0, str::len)
            + self
                .counted_pending
                .as_ref()
                .map_or(0, |id| id.as_str().len())
            + self
                .emitted_pending
                .as_ref()
                .map_or(0, |(id, _)| id.as_str().len())
            + self.activity.counts.len() * std::mem::size_of::<(Level, u64)>()
    }

    /// How many distinct **Levels** the **Activity** histogram holds. Bounded by
    /// the 9 variants of [`Level`] and by nothing else — which is the structural
    /// half of ADR 0002's bound.
    #[cfg(test)]
    pub(crate) fn activity_bucket_count(&self) -> usize {
        self.activity.counts.len()
    }
}

fn file_len(path: &Path) -> io::Result<u64> {
    fs::metadata(path).map(|meta| meta.len())
}

/* -------------------------------------------------------------------------- */
/* Threads                                                                     */
/* -------------------------------------------------------------------------- */

/// A poisoned watcher mutex means a poll panicked. The state behind it is plain
/// data with no invariant a panic could have broken halfway, and the alternative
/// is a Project that silently stops tailing, so the guard is recovered.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A sleep that a `stop` can cut short, so shutdown does not wait out a 5 s
/// backoff.
#[derive(Default)]
struct Halt {
    flag: AtomicBool,
    wake: (Mutex<bool>, Condvar),
}

impl Halt {
    fn stop(&self) {
        self.flag.store(true, Ordering::SeqCst);
        let (lock_, condvar) = &self.wake;
        *lock(lock_) = true;
        condvar.notify_all();
    }

    fn stopped(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    fn sleep(&self, duration: Duration) {
        let (lock_, condvar) = &self.wake;
        let guard = lock(lock_);
        let _ = condvar
            .wait_timeout_while(guard, duration, |stopped| !*stopped)
            .unwrap_or_else(PoisonError::into_inner);
    }
}

/// One **Project**'s watcher: its state, and the thread polling it.
pub struct WatcherHandle {
    state: Arc<Mutex<WatcherState>>,
    halt: Arc<Halt>,
    thread: Option<JoinHandle<()>>,
}

impl WatcherHandle {
    /// A handle exists only if its thread does.
    ///
    /// A `WatcherHandle` with no thread is a Project that looks watched and is
    /// not: `poll` runs nowhere, so no line is ever tailed and no
    /// `project:status` is ever emitted — the view goes on looking live while it
    /// has permanently stopped. The OS refusing a thread is therefore an error
    /// the caller must see, not a field set to `None`.
    fn spawn(state: WatcherState, sink: Arc<dyn EventSink>) -> io::Result<Self> {
        let state = Arc::new(Mutex::new(state));
        let halt = Arc::new(Halt::default());

        let thread = {
            let state = Arc::clone(&state);
            let halt = Arc::clone(&halt);
            thread::Builder::new()
                .name("logdeck-watcher".into())
                .spawn(move || {
                    while !halt.stopped() {
                        let wait = {
                            let mut guard = lock(&state);
                            guard.poll(sink.as_ref());
                            guard.interval()
                        };
                        halt.sleep(wait);
                    }
                })?
        };

        Ok(Self {
            state,
            halt,
            thread: Some(thread),
        })
    }

    pub fn with<R>(&self, act: impl FnOnce(&mut WatcherState) -> R) -> R {
        act(&mut lock(&self.state))
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.halt.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// `HashMap<ProjectId, WatcherHandle>` — every registered **Project** is
/// watched, and selection only changes a watcher's [`Mode`] (D8, ADR 0002).
#[derive(Default)]
pub struct Watchers {
    handles: Mutex<HashMap<ProjectId, WatcherHandle>>,
}

impl Watchers {
    /// Start watching a Project, or leave the existing watcher alone.
    pub fn ensure(&self, project_id: &ProjectId, root: &Path, sink: &Arc<dyn EventSink>) {
        self.ensure_with(project_id, root, sink, WatcherHandle::spawn)
    }

    /// [`Watchers::ensure`] with the spawn step injected, so the tests can drive
    /// the branch where the OS refuses a thread.
    fn ensure_with(
        &self,
        project_id: &ProjectId,
        root: &Path,
        sink: &Arc<dyn EventSink>,
        spawn: impl FnOnce(WatcherState, Arc<dyn EventSink>) -> io::Result<WatcherHandle>,
    ) {
        let mut handles = lock(&self.handles);
        if handles.contains_key(project_id) {
            return;
        }
        let state = WatcherState::new(project_id.clone(), root.to_path_buf(), Mode::Background);
        let err = match spawn(state, Arc::clone(sink)) {
            Ok(handle) => {
                handles.insert(project_id.clone(), handle);
                return;
            }
            Err(err) => err,
        };

        // Nothing is inserted, so this Project is not silently registered as
        // watched: every later `ensure` — from `add_project`, `select_project`,
        // `set_target`, `load_earlier`, `clear_activity` — tries again, and the
        // commands that need a watcher fail loudly meanwhile. The lock is
        // released first because emitting reaches the frontend.
        drop(handles);
        let reason = format!("could not start a watcher: {err}");
        eprintln!("logdeck: {project_id}: {reason}");
        sink.status(&StatusPayload {
            project_id: project_id.clone(),
            state: StatusState::Offline,
            reason: Some(reason),
        });
    }

    /// Stop watching a deregistered Project.
    pub fn stop(&self, project_id: &ProjectId) {
        let handle = lock(&self.handles).remove(project_id);
        drop(handle);
    }

    /// Promote one watcher and demote every other (ADR 0002).
    pub fn promote(&self, project_id: &ProjectId) {
        let handles = lock(&self.handles);
        for (id, handle) in handles.iter() {
            let mode = if id == project_id {
                Mode::Selected
            } else {
                Mode::Background
            };
            handle.with(|state| {
                if state.mode() != mode {
                    state.set_mode(mode);
                }
            });
        }
    }

    /// Act on one watcher's state. `None` when the Project is not registered.
    pub fn with<R>(
        &self,
        project_id: &ProjectId,
        act: impl FnOnce(&mut WatcherState) -> R,
    ) -> Option<R> {
        lock(&self.handles)
            .get(project_id)
            .map(|handle| handle.with(act))
    }

    /// Drop every watcher, joining its thread.
    pub fn stop_all(&self) {
        lock(&self.handles).clear();
    }
}

/* -------------------------------------------------------------------------- */
/* Tests                                                                       */
/* -------------------------------------------------------------------------- */

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Everything a poll emitted, in order.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Emitted {
        Entry(LogEntry),
        Break(Break),
        Activity(ActivityPayload),
        Status(StatusPayload),
    }

    /// The fake [`EventSink`]: records instead of emitting, so the poll loop is
    /// testable with no Tauri runtime.
    #[derive(Default)]
    pub(crate) struct Recorder {
        events: Mutex<Vec<Emitted>>,
        /// The size of every `log:entries` batch, in order. Recorded separately
        /// so the individual `Emitted::Entry` stream every other test reads
        /// stays exactly as it was.
        batches: Mutex<Vec<usize>>,
    }

    impl Recorder {
        pub(crate) fn events(&self) -> Vec<Emitted> {
            lock(&self.events).clone()
        }

        /// How many Entries each batched emit carried.
        pub(crate) fn batches(&self) -> Vec<usize> {
            lock(&self.batches).clone()
        }

        pub(crate) fn take(&self) -> Vec<Emitted> {
            std::mem::take(&mut *lock(&self.events))
        }

        pub(crate) fn entries(&self) -> Vec<LogEntry> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    Emitted::Entry(entry) => Some(entry),
                    _ => None,
                })
                .collect()
        }

        pub(crate) fn breaks(&self) -> Vec<Break> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    Emitted::Break(brk) => Some(brk),
                    _ => None,
                })
                .collect()
        }

        pub(crate) fn statuses(&self) -> Vec<StatusPayload> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    Emitted::Status(status) => Some(status),
                    _ => None,
                })
                .collect()
        }

        pub(crate) fn activities(&self) -> Vec<ActivityPayload> {
            self.events()
                .into_iter()
                .filter_map(|event| match event {
                    Emitted::Activity(activity) => Some(activity),
                    _ => None,
                })
                .collect()
        }
    }

    impl EventSink for Recorder {
        fn entry(&self, entry: &LogEntry) {
            lock(&self.events).push(Emitted::Entry(entry.clone()));
        }
        fn entries(&self, entries: &[LogEntry]) {
            if entries.is_empty() {
                return;
            }
            lock(&self.batches).push(entries.len());
            for entry in entries {
                self.entry(entry);
            }
        }
        fn brk(&self, brk: &Break) {
            lock(&self.events).push(Emitted::Break(brk.clone()));
        }
        fn activity(&self, payload: &ActivityPayload) {
            lock(&self.events).push(Emitted::Activity(payload.clone()));
        }
        fn status(&self, payload: &StatusPayload) -> bool {
            lock(&self.events).push(Emitted::Status(payload.clone()));
            true
        }
    }

    /// A sink whose `project:status` emit always fails — a transient webview or
    /// channel fault. Everything else lands.
    #[derive(Default)]
    pub(crate) struct StatusDropping {
        pub(crate) attempts: Mutex<Vec<StatusPayload>>,
    }

    impl EventSink for StatusDropping {
        fn entry(&self, _entry: &LogEntry) {}
        fn brk(&self, _brk: &Break) {}
        fn activity(&self, _payload: &ActivityPayload) {}
        fn status(&self, payload: &StatusPayload) -> bool {
            lock(&self.attempts).push(payload.clone());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::Recorder;
    use super::*;
    use crate::project::test_support::TempDir;
    use std::io::Write;

    /// Pins the wire shapes `src/lib/types.ts` assumed before this module
    /// existed. A divergence compiles cleanly on both sides and fails only when
    /// the command is first invoked, so prose in a doc comment is not enough.
    #[test]
    fn log_file_and_target_match_the_shapes_typescript_assumes() {
        let file = LogFile {
            name: "laravel-2026-08-14.log".into(),
            bytes: 2_146_304,
            modified: 1_786_000_080,
        };
        assert_eq!(
            serde_json::to_string(&file).expect("serialise LogFile"),
            r#"{"name":"laravel-2026-08-14.log","bytes":2146304,"modified":1786000080}"#,
            "modified must be epoch seconds, not an RFC 3339 string"
        );

        assert_eq!(
            serde_json::to_string(&Target::Latest).expect("serialise Latest"),
            r#""latest""#
        );
        assert_eq!(
            serde_json::to_string(&Target::File("laravel.log".into())).expect("serialise File"),
            r#"{"file":"laravel.log"}"#,
            "externally tagged and camelCase — not internally tagged, not PascalCase"
        );
    }

    /// A folder shaped like a Laravel root, with an empty `storage/logs`.
    fn laravel_root(temp: &TempDir, name: &str) -> PathBuf {
        let root = temp.child(name);
        fs::create_dir_all(logs_dir(&root)).expect("create storage/logs");
        fs::write(root.join("artisan"), "#!/usr/bin/env php\n").expect("write artisan");
        fs::canonicalize(&root).expect("canonicalize root")
    }

    fn append(path: &Path, text: &str) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open for append");
        file.write_all(text.as_bytes()).expect("append");
        file.flush().expect("flush");
    }

    fn entry_text(index: usize) -> String {
        format!(
            "[2026-08-14 01:28:{:02}] local.ERROR: boom {index}\n",
            index % 60
        )
    }

    fn watcher(root: &Path, mode: Mode) -> WatcherState {
        WatcherState::new(ProjectId::from_canonical(root), root.to_path_buf(), mode)
    }

    /// The first poll attaches at EOF; history is the opening window's job.
    fn attach(state: &mut WatcherState, sink: &Recorder) {
        state.poll(sink);
    }

    // ---- Offsets and partial lines ----------------------------------------

    #[test]
    fn the_offset_advances_only_past_the_last_newline() {
        let temp = TempDir::new("partial-line");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        append(&log, "[2026-08-14 01:28:00] local.INFO: seed\n");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        let attached = state.offset();
        sink.take();

        // A record caught mid-`fwrite()`: a whole line, then half of one.
        let whole = "[2026-08-14 01:28:01] local.ERROR: complete\n";
        let half = "[2026-08-14 01:28:02] local.ERROR: torn";
        append(&log, whole);
        append(&log, half);

        state.poll(&sink);

        assert_eq!(
            state.offset(),
            attached + whole.len() as u64,
            "the torn line is not consumed"
        );
        let emitted = sink.entries();
        assert_eq!(emitted.len(), 1, "{emitted:#?}");
        assert_eq!(emitted[0].message, "complete");
        assert!(
            !emitted.iter().any(|entry| entry.raw.contains("torn")),
            "a half-written line must not be parsed as garbage: {emitted:#?}"
        );

        // The rest of the record lands, and the whole line is re-read intact.
        sink.take();
        append(&log, " but finished\n");
        state.poll(&sink);

        let emitted = sink.entries();
        assert_eq!(
            emitted.last().map(|entry| entry.message.as_str()),
            Some("torn but finished"),
            "{emitted:#?}"
        );
        assert_eq!(
            state.offset(),
            file_len(&log).expect("len"),
            "everything through the last newline is now consumed"
        );
    }

    /// A line longer than one poll's read is the one case where the offset is
    /// advanced over bytes the parser has not seen. Those bytes are held, so the
    /// Entry that eventually owns them is byte-for-byte what the file holds —
    /// `raw` is what Copy sends verbatim (D1, BUILD-SPEC §2).
    #[test]
    fn a_line_longer_than_one_polls_read_is_not_split_by_a_spurious_newline() {
        let temp = TempDir::new("huge-line");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        // One physical line — a `dd()`ed request body — larger than the ceiling
        // on how much a single poll ingests.
        let header = "[2026-08-14 01:28:00] local.ERROR: ";
        let blob = "x".repeat(MAX_READ_PER_POLL as usize + 512 * 1024);
        let line = format!("{header}{blob}");
        append(&log, &line);

        // The first poll takes its 8 MB but hands the parser nothing: half a
        // line is not a line.
        state.poll(&sink);
        assert!(
            sink.take()
                .iter()
                .all(|event| matches!(event, super::test_support::Emitted::Status(_))),
            "no Entry is emitted from half a line"
        );
        assert_eq!(
            state.offset(),
            MAX_READ_PER_POLL,
            "the ceiling was taken off the file"
        );

        // The remainder is under the ceiling and still has no `\n`, so it waits.
        state.poll(&sink);
        assert!(sink.take().is_empty());

        append(&log, "\n");
        state.poll(&sink);

        let entries = sink.entries();
        assert_eq!(entries.len(), 1, "one line is one Entry");
        assert_eq!(
            entries[0].raw, line,
            "`raw` is the file's bytes verbatim — no newline the file never held"
        );
        assert!(
            !entries[0].raw.contains('\n') && entries[0].context.is_empty(),
            "the line was not split across the poll boundary"
        );
        assert_eq!(entries[0].offset, 0, "the Entry starts where the line does");
        assert_eq!(
            state.offset(),
            file_len(&log).expect("len"),
            "the whole line is consumed exactly once"
        );
    }

    /// The other half of the ceiling: a well-formed append larger than one
    /// poll's read is picked up over successive ticks, losing and duplicating
    /// nothing across the cut.
    #[test]
    fn an_append_larger_than_one_polls_read_resumes_on_the_next_tick() {
        let temp = TempDir::new("multi-chunk");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        // ~10 MB in one append: Entries wide enough that the cut lands inside
        // one of them, and few enough that the test stays cheap.
        let padding = "y".repeat(100 * 1024);
        let count = 100usize;
        let mut batch = String::new();
        for index in 0..count {
            batch.push_str(&entry_text(index));
            batch.push_str(&padding);
            batch.push('\n');
        }
        assert!(batch.len() > MAX_READ_PER_POLL as usize);
        append(&log, &batch);

        let mut polls = 0;
        while state.offset() < file_len(&log).expect("len") {
            state.poll(&sink);
            polls += 1;
            assert!(polls < 10, "the tail is making progress");
        }
        assert!(polls > 1, "the append really did span more than one poll");

        let mut seen: Vec<(EntryId, String)> = Vec::new();
        for entry in sink.entries() {
            // The pending Entry is re-emitted as it grows (D2), so identity is
            // what must be unique, not the number of events.
            if !seen.iter().any(|(id, _)| *id == entry.id) {
                seen.push((entry.id.clone(), entry.message.clone()));
            }
        }
        assert_eq!(seen.len(), count, "every Entry arrived exactly once");
        for (index, (_, message)) in seen.iter().enumerate() {
            assert_eq!(message, &format!("boom {index}"), "in order, none lost");
        }
    }

    #[test]
    fn the_pending_entry_is_emitted_immediately_and_revised_under_one_id() {
        let temp = TempDir::new("pending");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        append(&log, "[2026-08-14 01:28:00] local.ERROR: boom\n");
        state.poll(&sink);
        let first = sink.take();
        let opened = match first.first() {
            Some(super::test_support::Emitted::Entry(entry)) => entry.clone(),
            other => panic!("expected the pending Entry immediately, got {other:#?}"),
        };
        assert_eq!(opened.context, "");

        append(&log, "#0 /app/x.php(42)\n");
        state.poll(&sink);
        let revised = sink.entries();
        assert_eq!(revised.len(), 1, "{revised:#?}");
        assert_eq!(revised[0].id, opened.id, "revised in place (D2)");
        assert_eq!(revised[0].context, "#0 /app/x.php(42)");
    }

    /// A queue worker writing a tight loop closes thousands of **Entries**
    /// inside one 300 ms tick. Each one emitted separately costs the client a
    /// snapshot copy, a whole-buffer filter pass and a React commit, so an
    /// unbatched burst is quadratic in the buffer and freezes the view exactly
    /// while it is most worth watching.
    #[test]
    fn a_burst_of_entries_crosses_the_boundary_as_one_batched_event() {
        let temp = TempDir::new("burst-batching");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        // 2 000 short INFO lines in one append, well under the poll ceiling, so
        // a single poll closes all but the last of them.
        let count = 2_000usize;
        let burst: String = (0..count).map(entry_text).collect();
        assert!(burst.len() < MAX_READ_PER_POLL as usize);
        append(&log, &burst);

        state.poll(&sink);

        assert_eq!(
            sink.batches(),
            vec![count - 1],
            "one event carried the whole tick's closed Entries; the last one is \
             still pending and is emitted on its own (D2)"
        );
        let entries = sink.entries();
        assert_eq!(entries.len(), count, "nothing was lost to the batching");
        assert_eq!(entries[0].message, "boom 0");
        assert_eq!(entries[count - 1].message, format!("boom {}", count - 1));
        assert!(
            entries
                .windows(2)
                .all(|pair| pair[0].offset < pair[1].offset),
            "the batch preserves file order"
        );
    }

    #[test]
    fn an_idle_file_re_emits_nothing() {
        let temp = TempDir::new("idle");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        append(&log, "[2026-08-14 01:28:00] local.ERROR: boom\n");
        state.poll(&sink);
        sink.take();

        state.poll(&sink);
        state.poll(&sink);

        assert!(
            sink.events().is_empty(),
            "an unchanged pending Entry must not be re-sent every tick: {:#?}",
            sink.events()
        );
    }

    #[test]
    fn the_event_names_match_the_frozen_ipc_contract() {
        // A renamed event detaches the frontend silently (BUILD-SPEC §3).
        assert_eq!(EVENT_ENTRY, "log:entry");
        assert_eq!(EVENT_ENTRIES, "log:entries");
        assert_eq!(EVENT_BREAK, "log:break");
        assert_eq!(EVENT_ACTIVITY, "project:activity");
        assert_eq!(EVENT_STATUS, "project:status");
    }

    // ---- Truncation (D3, ADR 0001) ----------------------------------------

    #[test]
    fn truncation_breaks_the_record_without_clearing_the_client() {
        let temp = TempDir::new("truncate");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        append(&log, "[2026-08-14 01:28:00] local.ERROR: before\n");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        let before_offset = state.offset();
        assert!(before_offset > 0);
        sink.take();

        // `php artisan log:clear`, or an editor truncating on save.
        fs::write(&log, b"").expect("truncate");
        state.poll(&sink);

        let breaks = sink.breaks();
        assert_eq!(breaks.len(), 1, "{breaks:#?}");
        assert_eq!(breaks[0].kind, BreakKind::Cleared);
        assert_eq!(breaks[0].file, "laravel.log");
        assert_eq!(state.offset(), 0, "the offset resets to 0");

        // The only events are the Break and whatever arrives after it. Nothing
        // instructs the client to discard what it already holds (ADR 0001).
        let events = sink.take();
        assert_eq!(events.len(), 1, "{events:#?}");

        append(&log, "[2026-08-14 01:28:01] local.INFO: after\n");
        state.poll(&sink);
        let entries = sink.entries();
        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].message, "after");
        assert_eq!(entries[0].offset, 0, "the new file starts at 0");
    }

    // ---- Rotation (D5) ----------------------------------------------------

    #[test]
    fn a_newer_dated_file_breaks_the_record_and_is_followed() {
        let temp = TempDir::new("rotate");
        let root = laravel_root(&temp, "api");
        let dir = logs_dir(&root);
        let yesterday = dir.join("laravel-2026-08-13.log");
        append(&yesterday, "[2026-08-13 23:59:59] local.INFO: yesterday\n");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        assert_eq!(state.file(), Some("laravel-2026-08-13.log"));
        sink.take();

        // Midnight: the `daily` channel opens a new file.
        let today = dir.join("laravel-2026-08-14.log");
        append(&today, "[2026-08-14 00:00:00] local.ERROR: today\n");
        set_newer(&today, &yesterday);

        state.poll(&sink);

        let breaks = sink.breaks();
        assert_eq!(breaks.len(), 1, "{breaks:#?}");
        assert_eq!(breaks[0].kind, BreakKind::Rotated);
        assert_eq!(
            breaks[0].file, "laravel-2026-08-14.log",
            "the Break names the file in effect after it"
        );
        assert_eq!(state.file(), Some("laravel-2026-08-14.log"));

        let entries = sink.entries();
        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].message, "today", "the new file is read from 0");
        assert_eq!(entries[0].file, "laravel-2026-08-14.log");
    }

    /// Make `newer` unambiguously newer than `older`.
    ///
    /// The mtime is *stamped*, not nudged by touching the file. Writing zero
    /// bytes does not advance mtime on Linux — the kernel updates it only when
    /// a write actually transfers data — so a touch-and-retry loop spins until
    /// its guard runs out and then asserts. Which is precisely what it did in
    /// CI while passing on macOS, where the two fixtures were already written
    /// far enough apart to differ. Stamping a whole second past `older` clears
    /// any filesystem's granularity, and needs no sleeping to do it.
    ///
    /// The file must exist: every caller writes it first, and opening without
    /// `create` keeps it that way rather than silently stamping a new empty one.
    fn set_newer(newer: &Path, older: &Path) {
        let stamp = file_mtime(older) + Duration::from_secs(1);
        fs::OpenOptions::new()
            .write(true)
            .open(newer)
            .expect("open the fixture to stamp its mtime")
            .set_modified(stamp)
            .expect("stamp mtime");

        assert!(
            file_mtime(newer) > file_mtime(older),
            "the fixture must be newer than the file it rotates away from"
        );
    }

    /// Wait for a watcher thread to get somewhere. Generous: the assertion is
    /// that it happens unasked, not that it happens within some number of
    /// milliseconds on a loaded machine.
    fn wait_until(mut done: impl FnMut() -> bool) {
        for _ in 0..400 {
            if done() {
                return;
            }
            thread::sleep(POLL_INTERVAL / 4);
        }
    }

    fn file_mtime(path: &Path) -> SystemTime {
        fs::metadata(path)
            .and_then(|meta| meta.modified())
            .unwrap_or(UNIX_EPOCH)
    }

    // ---- The opening window (D6) ------------------------------------------

    #[test]
    fn the_backward_window_stops_at_five_hundred_entries() {
        let temp = TempDir::new("window-entries");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        let mut text = String::new();
        for index in 0..2_000 {
            text.push_str(&entry_text(index));
        }
        fs::write(&log, &text).expect("write log");

        let project_id = ProjectId::from_canonical(&root);
        let len = file_len(&log).expect("len");
        let window = read_window(&project_id, "laravel.log", &log, len, 0).expect("window");

        assert_eq!(window.items.len(), WINDOW_ENTRIES, "500 Entries, no more");
        let StreamItem::Entry(first) = &window.items[0] else {
            panic!("windows carry Entries")
        };
        assert!(
            !is_headerless(first),
            "the leading partial Entry is discarded: {first:#?}"
        );
        assert_eq!(first.message, "boom 1500");
        assert_eq!(window.first_offset, first.offset);
    }

    #[test]
    fn the_backward_window_stops_at_two_megabytes() {
        let temp = TempDir::new("window-bytes");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");

        // Entries far larger than 2 MB / 500, so the byte cap binds first.
        let frames = "#0 /var/www/app/Http/Controllers/UserController.php(42)\n".repeat(200);
        let mut text = String::new();
        for index in 0..700 {
            text.push_str(&entry_text(index));
            text.push_str(&frames);
        }
        fs::write(&log, &text).expect("write log");
        let len = file_len(&log).expect("len");
        assert!(
            len > 2 * WINDOW_MAX_BYTES,
            "the fixture must exceed the cap, got {len} bytes"
        );

        let project_id = ProjectId::from_canonical(&root);
        let window = read_window(&project_id, "laravel.log", &log, len, 0).expect("window");

        assert!(
            window.items.len() < WINDOW_ENTRIES,
            "the byte cap bound first, so fewer than 500 Entries came back"
        );
        assert!(
            len - window.first_offset <= WINDOW_MAX_BYTES,
            "no more than 2 MB is read"
        );
        let StreamItem::Entry(first) = &window.items[0] else {
            panic!("windows carry Entries")
        };
        assert!(
            !is_headerless(first),
            "the window never opens mid-trace: {:?}",
            first.message
        );
    }

    #[test]
    fn a_short_file_is_returned_whole_from_its_first_byte() {
        let temp = TempDir::new("window-short");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        let text = (0..3).map(entry_text).collect::<String>();
        fs::write(&log, &text).expect("write log");

        let project_id = ProjectId::from_canonical(&root);
        let len = file_len(&log).expect("len");
        let window = read_window(&project_id, "laravel.log", &log, len, 0).expect("window");

        assert_eq!(window.items.len(), 3, "{:#?}", window.items);
        assert_eq!(window.first_offset, 0, "the start of file is reached");
    }

    #[test]
    fn load_earlier_pages_back_from_a_supplied_offset() {
        let temp = TempDir::new("load-earlier");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        let text = (0..1_200).map(entry_text).collect::<String>();
        fs::write(&log, &text).expect("write log");

        let project_id = ProjectId::from_canonical(&root);
        let len = file_len(&log).expect("len");
        let first = read_window(&project_id, "laravel.log", &log, len, 0).expect("window");
        let earlier =
            read_window(&project_id, "laravel.log", &log, first.first_offset, 0).expect("earlier");

        let StreamItem::Entry(last_earlier) = earlier.items.last().expect("entries") else {
            panic!("windows carry Entries")
        };
        assert!(
            last_earlier.offset < first.first_offset,
            "the earlier page ends where the first began, with no overlap"
        );
        assert_eq!(last_earlier.message, "boom 699");
        assert_eq!(earlier.items.len(), WINDOW_ENTRIES);
    }

    /// The id is the only thing that carries file, generation and offset
    /// together, which is why `load_earlier` takes it rather than a bare offset.
    /// A log name always ends in `.log`, so the segment after the last `@` is a
    /// generation only when it parses as a number — never ambiguous.
    #[test]
    fn an_entry_id_decodes_back_into_the_file_generation_and_offset_it_was_stamped_from() {
        for (file, generation, offset) in [
            ("laravel.log", 0u64, 0u64),
            ("laravel.log", 0, 4_096),
            ("laravel-2026-08-14.log", 3, 184_320),
            // A file the user's channel named with the salt's own separator.
            ("laravel@2.log", 0, 12),
            ("laravel@2.log", 7, 12),
        ] {
            let id = stamped_entry_id(file, offset, generation);
            assert_eq!(
                decode_entry_id(id.as_str()).expect("decode"),
                (file.to_owned(), generation, offset),
                "round trip failed for {id}"
            );
        }

        assert!(decode_entry_id("laravel.log").is_err(), "no offset");
        assert!(decode_entry_id("laravel.log:nope").is_err(), "not a number");
    }

    /// The regression: `earlier` used to resolve the page against the file
    /// *currently being tailed* and stamp it with the *current* generation. The
    /// oldest Entry the client holds after a **Break** belongs to the previous
    /// file, so paging from its offset read the wrong file — and the page either
    /// vanished into the client's id dedupe or prepended Entries from the new
    /// file above the Break (ADR 0001).
    #[test]
    fn load_earlier_pages_the_file_the_entry_came_from_not_the_one_being_tailed() {
        let temp = TempDir::new("earlier-across-a-break");
        let root = laravel_root(&temp, "api");
        let dir = logs_dir(&root);

        // Yesterday's file, long enough that one window does not exhaust it.
        let yesterday = dir.join("laravel-2026-08-13.log");
        let history: String = (0..1_200)
            .map(|index| format!("[2026-08-13 23:59:59] local.INFO: old {index}\n"))
            .collect();
        fs::write(&yesterday, &history).expect("write yesterday");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        let opening = state.open().expect("opening window");
        let StreamItem::Entry(oldest_held) = opening.first().expect("a window") else {
            panic!("windows carry Entries")
        };
        let oldest_held = oldest_held.clone();
        assert_eq!(oldest_held.file, "laravel-2026-08-13.log");

        // Midnight: the `daily` channel rotates, and the Session Record breaks.
        let today = dir.join("laravel-2026-08-14.log");
        append(&today, "[2026-08-14 00:00:00] local.ERROR: today\n");
        set_newer(&today, &yesterday);
        state.poll(&sink);
        assert_eq!(sink.breaks().len(), 1, "the record broke");
        assert_eq!(state.file(), Some("laravel-2026-08-14.log"));

        // Now page back above the Break, from the oldest Entry actually held.
        let earlier = state
            .earlier(oldest_held.id.as_str())
            .expect("page earlier across the Break");

        assert!(!earlier.is_empty(), "the page must not come back empty");
        for item in &earlier {
            let StreamItem::Entry(entry) = item else {
                panic!("an earlier page carries Entries")
            };
            assert_eq!(
                entry.file, "laravel-2026-08-13.log",
                "the page comes from the file the Entry came from"
            );
            assert!(
                entry.offset < oldest_held.offset,
                "and sits strictly above what is already held"
            );
            assert!(
                entry.message.starts_with("old "),
                "not a line from today's file: {}",
                entry.message
            );
            assert_ne!(
                entry.id, oldest_held.id,
                "and cannot collide with what is held"
            );
        }
    }

    /// The `generation` salt exists for exactly this: a truncation resets the
    /// offset to 0 under the *same* file name, so without it the first Entry
    /// after a second `log:clear` would carry the id of the first Entry after
    /// the first one — and the client's id-keyed upsert would silently overwrite
    /// an Entry sitting above a Break, which is the loss ADR 0001 exists to
    /// prevent. Rotation tests do not cover this: a new file name would make the
    /// ids distinct on its own.
    #[test]
    fn a_second_truncation_of_the_same_file_cannot_reuse_the_first_ones_ids() {
        let temp = TempDir::new("double-truncate");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        append(&log, "[2026-08-14 01:28:00] local.ERROR: original\n");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        let mut ids = Vec::new();
        for round in 0..3 {
            fs::write(&log, b"").expect("truncate");
            state.poll(&sink);
            append(
                &log,
                &format!("[2026-08-14 01:28:0{round}] local.ERROR: round {round}\n"),
            );
            state.poll(&sink);

            let entry = sink
                .entries()
                .into_iter()
                .find(|entry| entry.message == format!("round {round}"))
                .unwrap_or_else(|| panic!("round {round} was emitted"));
            assert_eq!(entry.offset, 0, "every clear restarts the file at 0");
            ids.push(entry.id.clone());

            let brk = sink
                .breaks()
                .pop()
                .unwrap_or_else(|| panic!("round {round} broke the record"));
            assert_eq!(brk.kind, BreakKind::Cleared);
            sink.take();
        }

        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            ids.len(),
            "the first Entry after each clear must be distinct from the first \
             Entry after every earlier one, or the client overwrites an Entry \
             above a Break: {ids:?}"
        );
    }

    /// `canonical_root` re-resolves the registered root before every read, so a
    /// folder replaced by a symlink to somewhere else is refused rather than
    /// tailed. Nothing else in the suite ever builds a real symlink.
    #[cfg(unix)]
    #[test]
    fn a_project_root_replaced_by_a_symlink_is_refused_rather_than_read() {
        let temp = TempDir::new("symlink-escape");
        let root = laravel_root(&temp, "api");
        // Somewhere the user never chose, shaped so that reading it would work.
        let elsewhere = laravel_root(&temp, "elsewhere");
        append(
            &logs_dir(&elsewhere).join("laravel.log"),
            "[2026-08-14 01:28:00] local.ERROR: not yours\n",
        );

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        fs::remove_dir_all(&root).expect("remove the real root");
        std::os::unix::fs::symlink(&elsewhere, &root).expect("swap in a symlink");

        state.poll(&sink);

        assert!(
            sink.entries().is_empty(),
            "nothing from outside the registered folder is read: {:#?}",
            sink.entries()
        );
        let statuses = sink.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        assert_eq!(statuses[0].state, StatusState::Offline);
        assert!(
            statuses[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("no longer resolves to itself")),
            "the confinement failure is named: {statuses:#?}"
        );
        assert!(
            list_files(&root).is_err(),
            "listing is confined by the same check as tailing"
        );

        // Leave a real directory behind so the TempDir cleanup is not chasing a
        // symlink out of the fixture.
        fs::remove_file(&root).expect("remove the symlink");
    }

    // ---- Offline and reattachment (D9) ------------------------------------

    #[test]
    fn an_unavailable_path_goes_offline_backs_off_and_reattaches_unaided() {
        let temp = TempDir::new("offline");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        append(&log, "[2026-08-14 01:28:00] local.INFO: before\n");
        let away = temp.path().join("api-moved-away");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        assert_eq!(state.interval(), POLL_INTERVAL);
        sink.take();

        // `mv` the Project folder away.
        fs::rename(&root, &away).expect("move the project away");
        state.poll(&sink);

        let statuses = sink.take();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        let super::test_support::Emitted::Status(status) = &statuses[0] else {
            panic!("expected a status event, got {statuses:#?}")
        };
        assert_eq!(status.state, StatusState::Offline);
        assert!(status.reason.is_some(), "offline says why");
        assert_eq!(state.interval(), POLL_INTERVAL, "the first retry is prompt");

        // It keeps retrying, backing off toward 5 s and never giving up.
        let mut intervals = Vec::new();
        for _ in 0..8 {
            state.poll(&sink);
            intervals.push(state.interval());
        }
        assert!(
            intervals.windows(2).all(|pair| pair[1] >= pair[0]),
            "the backoff never shortens while offline: {intervals:?}"
        );
        assert_eq!(
            intervals.last(),
            Some(&MAX_BACKOFF),
            "it walks up to 5 s: {intervals:?}"
        );
        assert!(
            sink.take().is_empty(),
            "offline is reported on the transition, not every retry"
        );

        // `mv` it back. Nothing asks it to reattach.
        fs::rename(&away, &root).expect("move the project back");
        append(&log, "[2026-08-14 01:28:01] local.ERROR: after\n");
        state.poll(&sink);

        let statuses = sink.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        assert_eq!(statuses[0].state, StatusState::Online);
        assert_eq!(state.interval(), POLL_INTERVAL, "the backoff is spent");
        let entries = sink.entries();
        assert_eq!(
            entries.last().map(|entry| entry.message.as_str()),
            Some("after"),
            "tailing resumes where it left off: {entries:#?}"
        );
    }

    #[test]
    fn an_empty_logs_directory_is_online_with_nothing_to_read() {
        // A fresh Laravel project that has not logged yet is not an outage.
        let temp = TempDir::new("empty-logs");
        let root = laravel_root(&temp, "api");

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        state.poll(&sink);

        let statuses = sink.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        assert_eq!(statuses[0].state, StatusState::Online);
        assert_eq!(state.file(), None);
    }

    #[test]
    fn a_pinned_target_that_escapes_the_logs_directory_is_refused() {
        let temp = TempDir::new("pinned-escape");
        let root = laravel_root(&temp, "api");
        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();

        state.set_target(Target::File("../../../etc/passwd.log".into()));
        state.poll(&sink);

        let statuses = sink.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        assert_eq!(statuses[0].state, StatusState::Offline);
        assert_eq!(state.file(), None, "nothing outside storage/logs is opened");
    }

    // ---- Background mode: THE memory bound (ADR 0002) ----------------------

    #[test]
    fn a_background_watcher_retains_no_entry_text() {
        let temp = TempDir::new("background-memory");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Background);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        // ~10 MB, in the chunks a live tail would see it.
        let frames = "#0 /var/www/app/Http/Controllers/UserController.php(42)\n".repeat(20);
        let mut written = 0usize;
        let mut appended = 0u64;
        let mut index = 0usize;
        while written < 10 * 1024 * 1024 {
            let mut batch = String::new();
            while batch.len() < 512 * 1024 {
                batch.push_str(&entry_text(index));
                batch.push_str(&frames);
                index += 1;
                appended += 1;
            }
            written += batch.len();
            append(&log, &batch);
            state.poll(&sink);

            assert!(
                state.retained_text_bytes() < 8 * 1024,
                "a background watcher must retain no Entry text beyond the one \
                 still open — {} bytes retained after {written} bytes of log \
                 (ADR 0002)",
                state.retained_text_bytes()
            );
        }

        assert!(written >= 10 * 1024 * 1024, "the fixture really was 10 MB");
        assert!(
            sink.entries().is_empty(),
            "a background watcher emits no Entry text"
        );
        assert!(sink.breaks().is_empty());

        // What it *did* grow is counts.
        let activity = state.activity();
        assert_eq!(activity.total, appended, "every Entry was counted");
        assert_eq!(activity.max_level, Some(Level::Error));
        assert_eq!(activity.counts.get(&Level::Error), Some(&appended));
        assert_eq!(
            activity.counts.len(),
            1,
            "counts are one number per Level, not per Entry"
        );
        let last = sink.activities().pop().expect("activity was emitted");
        assert_eq!(last.total, appended);
        assert_eq!(last.max_level, Some(Level::Error));
    }

    #[test]
    fn background_counting_counts_each_entry_exactly_once() {
        let temp = TempDir::new("background-count");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Background);
        let sink = Recorder::default();
        attach(&mut state, &sink);

        // The newest Entry is still pending — counted now, not counted again
        // when the next header closes it.
        append(&log, "[2026-08-14 01:28:00] local.WARNING: one\n");
        state.poll(&sink);
        assert_eq!(state.activity().total, 1);
        assert_eq!(state.activity().max_level, Some(Level::Warning));

        append(&log, "#0 /app/x.php(42)\n");
        state.poll(&sink);
        assert_eq!(state.activity().total, 1, "a continuation is not an Entry");

        append(&log, "[2026-08-14 01:28:01] local.ERROR: two\n");
        state.poll(&sink);
        assert_eq!(state.activity().total, 2);
        assert_eq!(state.activity().max_level, Some(Level::Error));
        assert_eq!(state.activity().counts.get(&Level::Warning), Some(&1));
        assert_eq!(state.activity().counts.get(&Level::Error), Some(&1));

        state.clear_activity();
        assert_eq!(state.activity().total, 0);
        assert_eq!(state.activity().max_level, None);
    }

    #[test]
    fn promotion_and_demotion_change_mode_without_restarting_the_tail() {
        let temp = TempDir::new("promote");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let mut state = watcher(&root, Mode::Background);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        append(&log, "[2026-08-14 01:28:00] local.ERROR: unwatched\n");
        state.poll(&sink);
        let offset = state.offset();
        assert_eq!(state.activity().total, 1);
        sink.take();

        state.set_mode(Mode::Selected);

        assert_eq!(state.offset(), offset, "the tail keeps its place");
        assert_eq!(state.file(), Some("laravel.log"));
        assert_eq!(
            state.activity().total,
            0,
            "selection spends the Activity it accrued"
        );

        append(&log, "[2026-08-14 01:28:01] local.INFO: watched\n");
        state.poll(&sink);
        let entries = sink.entries();
        assert_eq!(
            entries.last().map(|entry| entry.message.as_str()),
            Some("watched"),
            "{entries:#?}"
        );
    }

    #[test]
    fn a_background_watcher_emits_no_break_when_its_file_rotates() {
        // A Project the user is not reading has no **Session Record** to break.
        let temp = TempDir::new("background-rotate");
        let root = laravel_root(&temp, "api");
        let dir = logs_dir(&root);
        let yesterday = dir.join("laravel-2026-08-13.log");
        append(&yesterday, "[2026-08-13 23:59:59] local.INFO: yesterday\n");

        let mut state = watcher(&root, Mode::Background);
        let sink = Recorder::default();
        attach(&mut state, &sink);
        sink.take();

        let today = dir.join("laravel-2026-08-14.log");
        append(&today, "[2026-08-14 00:00:00] local.ERROR: today\n");
        set_newer(&today, &yesterday);
        state.poll(&sink);

        assert!(sink.breaks().is_empty(), "{:#?}", sink.breaks());
        assert!(sink.entries().is_empty());
        assert_eq!(state.activity().total, 1, "it is still counted");
    }

    // ---- Files and targets -------------------------------------------------

    #[test]
    fn list_files_reports_bytes_and_epoch_seconds_newest_first() {
        let temp = TempDir::new("list-files");
        let root = laravel_root(&temp, "api");
        let dir = logs_dir(&root);
        fs::write(dir.join(".gitignore"), b"*\n").expect("write gitignore");
        fs::write(dir.join("notes.txt"), b"not a log\n").expect("write notes");
        let old = dir.join("laravel-2026-08-13.log");
        append(&old, "[2026-08-13 23:59:59] local.INFO: yesterday\n");
        let new = dir.join("laravel-2026-08-14.log");
        append(&new, "[2026-08-14 00:00:00] local.INFO: today\n");
        set_newer(&new, &old);

        let files = list_files(&root).expect("list");

        assert_eq!(
            files.len(),
            2,
            "only .log files, and no dotfiles: {files:#?}"
        );
        assert_eq!(files[0].name, "laravel-2026-08-14.log", "newest first");
        assert_eq!(files[0].bytes, file_len(&new).expect("len"));
        assert!(
            files[0].modified > 1_600_000_000,
            "modified is epoch seconds, not a formatted string: {}",
            files[0].modified
        );
        assert!(files[0].modified >= files[1].modified);
    }

    #[test]
    fn a_pinned_target_is_followed_instead_of_the_newest_file() {
        let temp = TempDir::new("pinned");
        let root = laravel_root(&temp, "api");
        let dir = logs_dir(&root);
        let pinned = dir.join("laravel-2026-08-13.log");
        append(&pinned, "[2026-08-13 23:59:59] local.INFO: yesterday\n");
        let newest = dir.join("laravel-2026-08-14.log");
        append(&newest, "[2026-08-14 00:00:00] local.INFO: today\n");
        set_newer(&newest, &pinned);

        let mut state = watcher(&root, Mode::Selected);
        let sink = Recorder::default();
        state.set_target(Target::File("laravel-2026-08-13.log".into()));
        attach(&mut state, &sink);
        sink.take();

        append(&newest, "[2026-08-14 00:00:01] local.ERROR: ignored\n");
        append(&pinned, "[2026-08-13 23:59:59] local.ERROR: followed\n");
        state.poll(&sink);

        assert_eq!(state.file(), Some("laravel-2026-08-13.log"));
        let entries = sink.entries();
        assert_eq!(entries.len(), 1, "{entries:#?}");
        assert_eq!(entries[0].message, "followed");
        assert!(sink.breaks().is_empty(), "a pin does not rotate");
    }

    #[test]
    fn target_serialises_as_the_shape_types_ts_already_assumes() {
        // `src/lib/types.ts` declared `Target` before this module existed:
        // externally tagged and camelCase. A divergence compiles cleanly on both
        // sides and fails only when `set_target` is first invoked.
        assert_eq!(
            serde_json::to_string(&Target::Latest).expect("serialise"),
            "\"latest\""
        );
        assert_eq!(
            serde_json::to_string(&Target::File("laravel.log".into())).expect("serialise"),
            "{\"file\":\"laravel.log\"}"
        );
        assert_eq!(
            serde_json::from_str::<Target>("\"latest\"").expect("deserialise"),
            Target::Latest
        );
        assert_eq!(
            serde_json::from_str::<Target>("{\"file\":\"laravel.log\"}").expect("deserialise"),
            Target::File("laravel.log".into())
        );
    }

    #[test]
    fn log_file_serialises_as_the_shape_types_ts_already_assumes() {
        let json = serde_json::to_string(&LogFile {
            name: "laravel.log".into(),
            bytes: 42,
            modified: 1_775_000_000,
        })
        .expect("serialise");

        assert_eq!(
            json, "{\"name\":\"laravel.log\",\"bytes\":42,\"modified\":1775000000}",
            "modified is epoch seconds, not RFC 3339"
        );
    }

    #[test]
    fn activity_and_status_payloads_are_camel_case() {
        let mut activity = Activity::default();
        activity.record(Level::Error);
        let json = serde_json::to_string(
            &activity.payload(&ProjectId::from_canonical(Path::new("/tmp/api"))),
        )
        .expect("serialise");
        assert!(json.contains("\"projectId\":\"/tmp/api\""), "{json}");
        assert!(json.contains("\"maxLevel\":\"error\""), "{json}");
        assert!(json.contains("\"counts\":{\"error\":1}"), "{json}");

        let json = serde_json::to_string(&StatusPayload {
            project_id: ProjectId::from_canonical(Path::new("/tmp/api")),
            state: StatusState::Offline,
            reason: Some("gone".into()),
        })
        .expect("serialise");
        assert_eq!(
            json, "{\"projectId\":\"/tmp/api\",\"state\":\"offline\",\"reason\":\"gone\"}",
            "{json}"
        );
    }

    // ---- The registry ------------------------------------------------------

    #[test]
    fn selecting_a_project_promotes_it_and_demotes_the_previous_one() {
        let temp = TempDir::new("registry");
        let first = laravel_root(&temp, "first");
        let second = laravel_root(&temp, "second");
        let sink: Arc<dyn EventSink> = Arc::new(Recorder::default());
        let watchers = Watchers::default();

        let first_id = ProjectId::from_canonical(&first);
        let second_id = ProjectId::from_canonical(&second);
        watchers.ensure(&first_id, &first, &sink);
        watchers.ensure(&second_id, &second, &sink);

        watchers.promote(&first_id);
        assert_eq!(watchers.with(&first_id, |s| s.mode()), Some(Mode::Selected));
        assert_eq!(
            watchers.with(&second_id, |s| s.mode()),
            Some(Mode::Background)
        );

        watchers.promote(&second_id);
        assert_eq!(
            watchers.with(&first_id, |s| s.mode()),
            Some(Mode::Background),
            "selecting demotes the previous one; it does not stop watching"
        );
        assert_eq!(
            watchers.with(&second_id, |s| s.mode()),
            Some(Mode::Selected)
        );

        // Both are still being watched (D8, ADR 0002).
        assert!(watchers.with(&first_id, |s| s.offset()).is_some());

        watchers.stop(&first_id);
        assert!(watchers.with(&first_id, |s| s.mode()).is_none());
        watchers.stop_all();
    }

    #[test]
    fn ensure_is_idempotent_and_keeps_the_running_watcher() {
        let temp = TempDir::new("ensure");
        let root = laravel_root(&temp, "api");
        let sink: Arc<dyn EventSink> = Arc::new(Recorder::default());
        let watchers = Watchers::default();
        let id = ProjectId::from_canonical(&root);

        watchers.ensure(&id, &root, &sink);
        watchers.promote(&id);
        watchers.ensure(&id, &root, &sink);

        assert_eq!(
            watchers.with(&id, |s| s.mode()),
            Some(Mode::Selected),
            "a second ensure must not replace a running watcher"
        );
        watchers.stop_all();
    }

    /// The OS refusing a thread must not leave a Project that *looks* watched.
    /// A threadless handle in the map would never poll, never emit a status, and
    /// never be replaced — `ensure` is a no-op once the key exists — so the view
    /// would go on looking live having permanently stopped.
    #[test]
    fn a_watcher_the_os_refuses_to_start_is_reported_offline_and_retried() {
        let temp = TempDir::new("spawn-refused");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let recorder = Arc::new(Recorder::default());
        let sink: Arc<dyn EventSink> = recorder.clone();
        let watchers = Watchers::default();
        let id = ProjectId::from_canonical(&root);

        watchers.ensure_with(&id, &root, &sink, |_, _| {
            Err(io::Error::other("cannot create a thread"))
        });

        assert_eq!(
            watchers.with(&id, |state| state.mode()),
            None,
            "a Project with no thread must not be registered as watched"
        );
        let statuses = recorder.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        assert_eq!(statuses[0].state, StatusState::Offline);
        assert!(
            statuses[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("could not start a watcher")),
            "the sidebar is told why: {statuses:#?}"
        );

        // The failure is not permanent: the next ensure tries again.
        watchers.ensure(&id, &root, &sink);
        wait_until(|| watchers.with(&id, |state| state.file().is_some()) == Some(true));
        watchers.stop_all();
    }

    /// Status is emitted only on a transition, so a transition whose emit failed
    /// is one the sidebar never heard about. It must be retried rather than
    /// assumed delivered — an offline Project that never recovers would
    /// otherwise stay silently online forever.
    #[test]
    fn a_status_whose_emit_failed_is_retried_until_it_lands() {
        let temp = TempDir::new("status-dropped");
        let root = laravel_root(&temp, "api");
        let mut state = watcher(&root, Mode::Selected);

        let dropping = super::test_support::StatusDropping::default();
        state.poll(&dropping);
        state.poll(&dropping);
        let attempts = lock(&dropping.attempts).clone();
        assert_eq!(
            attempts.len(),
            2,
            "an undelivered transition is retried: {attempts:#?}"
        );
        assert!(attempts.iter().all(|a| a.state == StatusState::Online));

        // Once it lands, it stops repeating.
        let sink = Recorder::default();
        state.poll(&sink);
        state.poll(&sink);
        let statuses = sink.statuses();
        assert_eq!(statuses.len(), 1, "{statuses:#?}");
        assert_eq!(statuses[0].state, StatusState::Online);
    }

    #[test]
    fn a_spawned_watcher_polls_on_its_own_and_stops_when_dropped() {
        let temp = TempDir::new("thread");
        let root = laravel_root(&temp, "api");
        let log = logs_dir(&root).join("laravel.log");
        fs::write(&log, b"").expect("create log");

        let recorder = Arc::new(Recorder::default());
        let sink: Arc<dyn EventSink> = recorder.clone();
        let watchers = Watchers::default();
        let id = ProjectId::from_canonical(&root);
        watchers.ensure(&id, &root, &sink);
        watchers.promote(&id);

        // The first poll attaches at EOF, so wait for it before appending —
        // otherwise the fixture is written *before* the watcher ever looked,
        // and a tail that skipped it would be behaving correctly.
        wait_until(|| watchers.with(&id, |state| state.file().is_some()) == Some(true));
        append(&log, "[2026-08-14 01:28:00] local.ERROR: live\n");

        let mut seen = Vec::new();
        wait_until(|| {
            seen = recorder.entries();
            !seen.is_empty()
        });
        assert_eq!(
            seen.last().map(|entry| entry.message.as_str()),
            Some("live"),
            "the thread tails without being asked: {:#?}",
            recorder.events()
        );

        watchers.stop_all();
        let after = recorder.entries().len();
        append(&log, "[2026-08-14 01:28:01] local.ERROR: ignored\n");
        thread::sleep(POLL_INTERVAL * 3);
        assert_eq!(
            recorder.entries().len(),
            after,
            "a stopped watcher stops polling"
        );
    }
}
