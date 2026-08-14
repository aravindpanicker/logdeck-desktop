//! Domain types shared across the Rust side and serialised over IPC.
//!
//! Every payload type renames to camelCase at the serde boundary: `project_id`
//! in Rust is `projectId` in TypeScript. A mismatch compiles cleanly and fails
//! silently in the frontend, so the round-trip is pinned by a test below.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The severity a Laravel **Entry** was written at.
///
/// Ordered: comparison drives the **Activity** rollup (highest Level across a
/// batch) and filter thresholds. Variants are declared least- to most-severe
/// and `Ord` is derived from that declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Level {
    /// No severity, rather than a low one.
    ///
    /// Carried by content that never announced a Level: a fragment with no
    /// Monolog header (a PHP fatal, `dd()` output, stderr redirected into the
    /// log) or a header whose level text is not one of the eight PSR levels.
    ///
    /// Declared first so it ranks below `Debug` and can never inflate the
    /// **Activity** rollup. It is deliberately its own variant rather than a
    /// fallback onto `Info`: an unheaded fatal error is not something the
    /// application logged at INFO, and collapsing the two would both misreport
    /// it and hide it behind an INFO filter.
    Unknown,
    Debug,
    Info,
    Notice,
    Warning,
    Error,
    Critical,
    Alert,
    Emergency,
}

/// Whether a **Project** can currently be read, and if not, why.
///
/// An unhealthy Project is still registered (D4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Health {
    Ok,
    NoLogsDir,
    NotLaravel,
    Unavailable(String),
}

/// Identity of a **Project**: its canonicalized absolute path.
///
/// A newtype rather than a bare `String` so the compiler can tell it apart from
/// an [`EntryId`] or a [`BreakId`] — three distinct identity spaces that would
/// otherwise be interchangeable. `#[serde(transparent)]` means it is an
/// ordinary string on the wire, so the IPC contract in BUILD-SPEC §3 is
/// unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(String);

impl ProjectId {
    /// The path must already be canonicalized; canonicalization is I/O and
    /// belongs to `project.rs`, which keeps this module pure.
    pub fn from_canonical(path: &Path) -> Self {
        Self(path.to_string_lossy().into_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProjectId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of an **Entry**: `"{file}:{offset}"`.
///
/// The format is constructed here rather than described in a comment, so it
/// cannot drift from the spec. Composite rather than a session counter because
/// an Entry's identity must survive a file boundary (ADR 0001).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct EntryId(String);

impl EntryId {
    pub fn new(file: &str, offset: u64) -> Self {
        Self(format!("{file}:{offset}"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Identity of a **Break**: `"{file}:{offset}:break"`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BreakId(String);

impl BreakId {
    pub fn new(file: &str, offset: u64) -> Self {
        Self(format!("{file}:{offset}:break"))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BreakId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A folder the user has registered, identified by its absolute path.
///
/// Construct through [`Project::new`], which derives `id` and `label` from the
/// path so the two cannot drift apart. Fields are private for that reason.
/// `serde` deserialisation bypasses the constructor, so `project.rs` re-derives
/// identity when loading the persisted registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    id: ProjectId,
    label: String,
    path: PathBuf,
    health: Health,
}

impl Project {
    /// `canonical` must already be canonicalized (see [`ProjectId::from_canonical`]).
    ///
    /// `label` starts as the basename; `project.rs` appends the parent segment
    /// when two Projects collide, which needs the whole registry and so cannot
    /// happen here.
    pub fn new(canonical: PathBuf, health: Health) -> Self {
        let id = ProjectId::from_canonical(&canonical);
        let label = canonical
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| id.as_str().to_owned());

        Self {
            id,
            label,
            path: canonical,
            health,
        }
    }

    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn health(&self) -> &Health {
        &self.health
    }

    /// Used when two Projects share a basename (`CONTEXT.md`, **Project**).
    pub fn set_label(&mut self, label: String) {
        self.label = label;
    }

    /// A Project's Health changes at runtime — it goes offline and recovers (D9).
    pub fn set_health(&mut self, health: Health) {
        self.health = health;
    }
}

/// One logical log event: a header together with the context and stack frames
/// beneath it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LogEntry {
    pub id: EntryId,
    pub project_id: ProjectId,
    pub file: String,
    pub offset: u64,
    /// Verbatim as parsed, not normalised.
    pub timestamp: String,
    pub env: String,
    pub level: Level,
    /// Header remainder, first line only.
    pub message: String,
    /// Following lines, joined with `\n`.
    pub context: String,
    /// Full verbatim text including the header — what Copy sends (D1).
    /// Stored rather than reconstructed: re-serialising from parts would
    /// normalise whitespace a developer pasting into a bug report may need.
    pub raw: String,
}

/// Why a **Break** was inserted into the **Session Record**.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BreakKind {
    Cleared,
    Rotated,
}

/// A point in the **Session Record** where the underlying source
/// discontinued. Entries either side of a Break are unrelated in time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Break {
    pub id: BreakId,
    pub project_id: ProjectId,
    pub kind: BreakKind,
    /// The file in effect *after* the Break.
    pub file: String,
}

/// What the stream carries over IPC: either an **Entry** or a **Break**.
///
/// Internally tagged so the frontend can discriminate on `type` while reading
/// the payload's own fields off the same object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum StreamItem {
    Entry(LogEntry),
    Break(Break),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entry() -> LogEntry {
        LogEntry {
            id: EntryId::new("laravel-2026-08-14.log", 184_320),
            project_id: ProjectId::from_canonical(Path::new("/Users/dev/projects/api")),
            file: "laravel-2026-08-14.log".into(),
            offset: 184_320,
            timestamp: "2026-08-14 01:28:00".into(),
            env: "local".into(),
            level: Level::Error,
            message: "Undefined variable $user".into(),
            context: "#0 /app/Http/Controllers/UserController.php(42)".into(),
            raw: "[2026-08-14 01:28:00] local.ERROR: Undefined variable $user\n#0 /app/Http/Controllers/UserController.php(42)".into(),
        }
    }

    #[test]
    fn level_orders_from_unknown_to_emergency() {
        assert!(
            Level::Unknown < Level::Debug,
            "Unknown must rank below every real severity so it cannot inflate the Activity rollup"
        );
        assert!(Level::Debug < Level::Info);
        assert!(Level::Info < Level::Notice);
        assert!(Level::Notice < Level::Warning);
        assert!(Level::Warning < Level::Error);
        assert!(Level::Error < Level::Critical);
        assert!(Level::Critical < Level::Alert);
        assert!(Level::Alert < Level::Emergency);

        let all = [
            Level::Unknown,
            Level::Debug,
            Level::Info,
            Level::Notice,
            Level::Warning,
            Level::Error,
            Level::Critical,
            Level::Alert,
            Level::Emergency,
        ];
        assert_eq!(all.len(), 9);
        assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(all.iter().max(), Some(&Level::Emergency));
        assert_eq!(all.iter().min(), Some(&Level::Unknown));
    }

    #[test]
    fn log_entry_serialises_snake_case_fields_as_camel_case() {
        let json = serde_json::to_string(&sample_entry()).expect("serialise LogEntry");

        assert!(
            json.contains("projectId"),
            "expected camelCase `projectId` in {json}"
        );
        assert!(
            !json.contains("project_id"),
            "snake_case `project_id` leaked into {json}"
        );

        let back: LogEntry = serde_json::from_str(&json).expect("deserialise LogEntry");
        assert_eq!(back, sample_entry());
    }

    #[test]
    fn level_serialises_as_lower_camel_case() {
        assert_eq!(
            serde_json::to_string(&Level::Emergency).unwrap(),
            "\"emergency\""
        );
    }

    #[test]
    fn break_serialises_project_id_as_camel_case() {
        let brk = Break {
            id: BreakId::new("laravel.log", 0),
            project_id: ProjectId::from_canonical(Path::new("/Users/dev/projects/api")),
            kind: BreakKind::Rotated,
            file: "laravel-2026-08-14.log".into(),
        };
        let json = serde_json::to_string(&brk).expect("serialise Break");

        assert!(json.contains("projectId"), "{json}");
        assert!(json.contains("\"rotated\""), "{json}");

        let back: Break = serde_json::from_str(&json).expect("deserialise Break");
        assert_eq!(back, brk);
    }

    #[test]
    fn stream_item_is_tagged_and_round_trips() {
        let entry = StreamItem::Entry(sample_entry());
        let json = serde_json::to_string(&entry).expect("serialise StreamItem::Entry");
        assert!(json.contains("\"type\":\"entry\""), "{json}");
        assert!(json.contains("projectId"), "{json}");
        assert_eq!(
            serde_json::from_str::<StreamItem>(&json).unwrap(),
            entry,
            "entry round-trip"
        );

        let brk = StreamItem::Break(Break {
            id: BreakId::new("laravel.log", 0),
            project_id: ProjectId::from_canonical(Path::new("/Users/dev/projects/api")),
            kind: BreakKind::Cleared,
            file: "laravel.log".into(),
        });
        let json = serde_json::to_string(&brk).expect("serialise StreamItem::Break");
        assert!(json.contains("\"type\":\"break\""), "{json}");
        assert_eq!(serde_json::from_str::<StreamItem>(&json).unwrap(), brk);
    }

    #[test]
    fn health_variants_serialise_as_camel_case() {
        assert_eq!(serde_json::to_string(&Health::Ok).unwrap(), "\"ok\"");
        assert_eq!(
            serde_json::to_string(&Health::NoLogsDir).unwrap(),
            "\"noLogsDir\""
        );
        assert_eq!(
            serde_json::to_string(&Health::NotLaravel).unwrap(),
            "\"notLaravel\""
        );
        assert_eq!(
            serde_json::to_string(&Health::Unavailable("gone".into())).unwrap(),
            "{\"unavailable\":\"gone\"}"
        );
    }

    #[test]
    fn project_round_trips() {
        let project = Project::new(PathBuf::from("/Users/dev/projects/api"), Health::Ok);
        let json = serde_json::to_string(&project).expect("serialise Project");
        assert_eq!(
            serde_json::from_str::<Project>(&json).unwrap(),
            project,
            "{json}"
        );
    }

    #[test]
    fn project_derives_id_and_label_from_its_path() {
        let project = Project::new(PathBuf::from("/Users/dev/projects/api"), Health::Ok);

        assert_eq!(project.id().as_str(), "/Users/dev/projects/api");
        assert_eq!(project.label(), "api");
        assert_eq!(
            project.id().as_str(),
            project.path().to_string_lossy(),
            "id must stay tied to path"
        );
    }

    #[test]
    fn entry_and_break_ids_use_the_documented_format() {
        assert_eq!(
            EntryId::new("laravel-2026-08-14.log", 184_320).as_str(),
            "laravel-2026-08-14.log:184320"
        );
        assert_eq!(
            BreakId::new("laravel-2026-08-14.log", 0).as_str(),
            "laravel-2026-08-14.log:0:break"
        );
    }

    #[test]
    fn ids_are_transparent_on_the_wire() {
        // The newtypes exist for the Rust compiler only. If they ever serialise
        // as wrapper objects the frozen IPC contract in BUILD-SPEC §3 breaks.
        assert_eq!(
            serde_json::to_string(&EntryId::new("laravel.log", 42)).unwrap(),
            "\"laravel.log:42\""
        );
        assert_eq!(
            serde_json::to_string(&BreakId::new("laravel.log", 42)).unwrap(),
            "\"laravel.log:42:break\""
        );
        assert_eq!(
            serde_json::to_string(&ProjectId::from_canonical(Path::new("/tmp/api"))).unwrap(),
            "\"/tmp/api\""
        );
    }
}
