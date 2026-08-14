//! The **Project** registry: validation, identity, and persistence.
//!
//! A **Project** is a folder the user registered, identified by its
//! canonicalized absolute path. Registration is by *intent*, not by validity:
//! a folder that turns out to hold no logs is still a Project, carrying the
//! [`Health`] that says why (D4). Nothing in this module rejects a folder for
//! being un-Laravel-shaped — Lumen, a custom `LOG_PATH`, and bind-mounted
//! layouts all have to stay usable.
//!
//! Identity is the canonicalized path, so registering the same folder twice is
//! a no-op rather than a duplicate or an error.
//!
//! All file access in the app goes through Rust rather than the `fs` plugin,
//! whose scopes are fixed at compile time while Project paths are chosen at
//! runtime (BUILD-SPEC §7). This module is therefore the place where a
//! frontend-supplied path is turned into something trusted: it must be
//! absolute, it is canonicalized before use, and the canonical form is what is
//! stored and later read from.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::model::{Health, Project};

/// The registry file inside the configuration directory.
pub const REGISTRY_FILE_NAME: &str = "projects.json";

/// Where a corrupt registry is moved so a later save cannot overwrite it.
pub const QUARANTINE_FILE_NAME: &str = "projects.json.corrupt";

/// Everything that can go wrong registering or persisting **Projects**.
///
/// Deliberately explicit: a swallowed I/O error here loses the user's Project
/// list silently, which is the one failure they cannot recover from.
#[derive(Debug)]
pub enum RegistryError {
    /// A relative path arrived from the frontend. Resolving it against the
    /// process's working directory is exactly the class of hole avoiding the
    /// `fs` plugin was meant to close, so it is refused instead.
    NotAbsolute(String),
    /// The path does not resolve — missing, or not readable.
    Unresolvable {
        path: String,
        source: io::Error,
    },
    /// The path resolves to something that is not a folder.
    NotADirectory(PathBuf),
    Io {
        path: PathBuf,
        source: io::Error,
    },
    /// The registry file exists but is not readable as a registry. Never
    /// treated as "empty": that would wipe the user's Projects on next save.
    Corrupt {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotAbsolute(path) => {
                write!(f, "project path must be absolute, got `{path}`")
            }
            Self::Unresolvable { path, source } => {
                write!(f, "cannot resolve `{path}`: {source}")
            }
            Self::NotADirectory(path) => {
                write!(f, "`{}` is not a folder", path.display())
            }
            Self::Io { path, source } => {
                write!(f, "i/o error on `{}`: {source}", path.display())
            }
            Self::Corrupt { path, source } => {
                write!(
                    f,
                    "registry at `{}` is unreadable: {source}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unresolvable { source, .. } | Self::Io { source, .. } => Some(source),
            Self::Corrupt { source, .. } => Some(source),
            Self::NotAbsolute(_) | Self::NotADirectory(_) => None,
        }
    }
}

/// Turn a frontend-supplied path into one we are willing to read from.
///
/// Absolute-only, then canonicalized: the stored path has no `..` segments and
/// no unresolved symlinks left in it, so every later read is confined to the
/// folder the user actually chose.
///
/// Residual risk, accepted: this resolves the path at one instant, and `std::fs`
/// offers no atomic open-and-verify. A folder swapped for a symlink after
/// registration is not detected here. [`load`] re-canonicalizes on every start,
/// which bounds the window to one session; a reader that opens files under a
/// stored path (Phase 6) should re-resolve immediately before reading rather
/// than trust the value cached in `AppState`.
pub fn canonicalize_project_path(raw: &str) -> Result<PathBuf, RegistryError> {
    let candidate = Path::new(raw);
    if !candidate.is_absolute() {
        return Err(RegistryError::NotAbsolute(raw.to_owned()));
    }

    let canonical = fs::canonicalize(candidate).map_err(|source| RegistryError::Unresolvable {
        path: raw.to_owned(),
        source,
    })?;

    if !canonical.is_dir() {
        return Err(RegistryError::NotADirectory(canonical));
    }

    Ok(canonical)
}

/// Why a **Project** can or cannot be read right now.
///
/// A Laravel root has an `artisan` file and a `storage/logs` directory — the
/// `laravel/laravel` skeleton commits `storage/logs/.gitignore`, so that
/// directory exists after a fresh clone even with no log files in it. Missing
/// either is reported, never rejected (D4).
pub fn health_of(canonical: &Path) -> Health {
    if !canonical.is_dir() {
        return Health::Unavailable(format!("{} is not a readable folder", canonical.display()));
    }
    if !canonical.join("artisan").is_file() {
        return Health::NotLaravel;
    }
    if !logs_dir(canonical).is_dir() {
        return Health::NoLogsDir;
    }
    Health::Ok
}

/// Where a **Project** writes its logs.
pub fn logs_dir(project_path: &Path) -> PathBuf {
    project_path.join("storage").join("logs")
}

/// The one place a canonical path becomes a **Project**.
///
/// [`add`] and [`load`] must derive `Health` identically — if they drift, the
/// same folder reports differently after a restart than it did when added,
/// which is exactly the second source of truth BUILD-SPEC §8 exists to avoid.
fn project_at(canonical: PathBuf) -> Project {
    let health = health_of(&canonical);
    Project::new(canonical, health)
}

/// Register a folder, or return the existing **Project** if it is already
/// registered. Identity is the canonicalized path, so a second add of the same
/// folder yields one Project — not a duplicate, and not an error.
pub fn add(projects: &mut Vec<Project>, raw_path: &str) -> Result<Project, RegistryError> {
    let candidate = project_at(canonicalize_project_path(raw_path)?);

    if let Some(index) = projects
        .iter()
        .position(|project| project.id() == candidate.id())
    {
        // Re-adding is how a user re-checks a folder they fixed, so refresh
        // Health; the label is left alone because it is registry-wide.
        projects[index].set_health(candidate.health().clone());
        return Ok(projects[index].clone());
    }

    projects.push(candidate);
    disambiguate_labels(projects);

    Ok(projects
        .last()
        .expect("registry is non-empty immediately after a push")
        .clone())
}

/// Deregister a **Project**. Removing an unknown id is a no-op; returns whether
/// anything was removed.
pub fn remove(projects: &mut Vec<Project>, project_id: &str) -> bool {
    let before = projects.len();
    projects.retain(|project| project.id().as_str() != project_id);
    let removed = projects.len() != before;
    if removed {
        // A label only carries a parent segment while a collision exists.
        disambiguate_labels(projects);
    }
    removed
}

/// Label every **Project** with the shortest trailing path suffix that is
/// unique across the registry.
///
/// A label starts as the basename; `~/work/acme/api` and `~/side/acme/api` are
/// both `api`, so both grow a parent segment until they differ. This needs the
/// whole registry and so cannot live in `Project::new`.
pub fn disambiguate_labels(projects: &mut [Project]) {
    let segments: Vec<Vec<String>> = projects
        .iter()
        .map(|project| path_segments(project.path()))
        .collect();
    let mut depths = vec![1usize; projects.len()];

    let labels = loop {
        let labels: Vec<String> = projects
            .iter()
            .enumerate()
            .map(|(index, project)| label_at_depth(project, &segments[index], depths[index]))
            .collect();

        let mut groups: HashMap<&str, Vec<usize>> = HashMap::new();
        for (index, label) in labels.iter().enumerate() {
            groups.entry(label.as_str()).or_default().push(index);
        }

        // Only a colliding group grows, and only while it has a segment left to
        // grow into — otherwise two paths that differ solely in their root
        // would loop forever.
        let growable: Vec<usize> = groups
            .values()
            .filter(|group| group.len() > 1)
            .flat_map(|group| group.iter().copied())
            .filter(|&index| depths[index] < segments[index].len())
            .collect();

        if growable.is_empty() {
            break labels;
        }
        for index in growable {
            depths[index] += 1;
        }
    };

    for (project, label) in projects.iter_mut().zip(labels) {
        project.set_label(label);
    }
}

fn path_segments(path: &Path) -> Vec<String> {
    path.components()
        .filter_map(|component| match component {
            Component::Normal(segment) => Some(segment.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect()
}

fn label_at_depth(project: &Project, segments: &[String], depth: usize) -> String {
    if segments.is_empty() {
        // A path with no normal components (a bare root) has no basename to
        // shorten to; the full path is the only honest label.
        return project.path().to_string_lossy().into_owned();
    }
    let take = depth.clamp(1, segments.len());
    segments[segments.len() - take..].join("/")
}

/// Persist the registry as JSON under `dir`.
///
/// The directory is a parameter rather than something this function digs out of
/// an `AppHandle`, so tests can inject a temp dir. Production passes
/// `app_config_dir()`.
pub fn save(dir: &Path, projects: &[Project]) -> Result<(), RegistryError> {
    fs::create_dir_all(dir).map_err(|source| RegistryError::Io {
        path: dir.to_path_buf(),
        source,
    })?;

    let path = dir.join(REGISTRY_FILE_NAME);
    let json = serde_json::to_vec_pretty(projects).map_err(|source| RegistryError::Corrupt {
        path: path.clone(),
        source,
    })?;

    // Write beside the target and rename, so an interrupted save leaves the
    // previous registry intact rather than a truncated one.
    let temp = dir.join(format!("{REGISTRY_FILE_NAME}.tmp"));
    fs::write(&temp, &json).map_err(|source| RegistryError::Io {
        path: temp.clone(),
        source,
    })?;
    fs::rename(&temp, &path).map_err(|source| RegistryError::Io {
        path: path.clone(),
        source,
    })
}

/// Load the registry from `dir`.
///
/// A missing file is an empty registry, not an error — that is simply first
/// run. A file that exists but does not parse *is* an error, so the caller can
/// preserve it instead of overwriting it with an empty list.
///
/// `serde` bypasses `Project::new`, so identity is re-derived here: a registry
/// edited by hand, or written before a folder was renamed, must not produce a
/// **Project** whose id or label disagrees with its path. `Health` is likewise
/// recomputed — it is a fact about the filesystem now, not when we last saved.
pub fn load(dir: &Path) -> Result<Vec<Project>, RegistryError> {
    let path = dir.join(REGISTRY_FILE_NAME);

    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(RegistryError::Io { path, source }),
    };

    let stored: Vec<Project> =
        serde_json::from_slice(&bytes).map_err(|source| RegistryError::Corrupt {
            path: path.clone(),
            source,
        })?;

    let mut projects: Vec<Project> = Vec::with_capacity(stored.len());
    for project in stored {
        let rebuilt = match canonicalize_project_path(&project.path().to_string_lossy()) {
            Ok(canonical) => project_at(canonical),
            // The folder moved or went away. It stays registered — removal is a
            // deliberate user action (D9) — carrying the reason it cannot be read.
            Err(err) => Project::new(
                project.path().to_path_buf(),
                Health::Unavailable(err.to_string()),
            ),
        };

        let id = rebuilt.id().clone();
        if projects.iter().any(|existing| existing.id() == &id) {
            // Two stored entries that canonicalize to the same folder are one
            // Project, same as adding it twice.
            continue;
        }
        projects.push(rebuilt);
    }

    disambiguate_labels(&mut projects);
    Ok(projects)
}

/// Move an unreadable registry aside so a later [`save`] cannot overwrite it.
///
/// Returns the path the file was moved to.
pub fn quarantine(dir: &Path) -> Result<PathBuf, RegistryError> {
    let from = dir.join(REGISTRY_FILE_NAME);
    let to = dir.join(QUARANTINE_FILE_NAME);
    fs::rename(&from, &to).map_err(|source| RegistryError::Io { path: from, source })?;
    Ok(to)
}

/// Fixtures shared by this module's tests and the command tests in `lib.rs`,
/// which exercise the same persistence paths from the other side.
#[cfg(test)]
pub(crate) mod test_support {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    /// A self-cleaning temp directory.
    ///
    /// Hand-rolled rather than pulled in as a dev-dependency because
    /// `Cargo.toml` belongs to another phase. Fixtures never touch the repo —
    /// `.gitignore` ignores `*.log`, so a committed log fixture would be
    /// invisible until it broke something.
    pub(crate) struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let unique = format!(
                "logdeck-{}-{}-{}-{}",
                tag,
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("system clock after the epoch")
                    .as_nanos(),
                COUNTER.fetch_add(1, Ordering::Relaxed),
            );
            let path = std::env::temp_dir().join(unique);
            fs::create_dir_all(&path).expect("create temp dir");
            // The OS temp dir is a symlink on macOS; canonicalize so test
            // expectations match the canonical paths the registry stores.
            let path = fs::canonicalize(&path).expect("canonicalize temp dir");
            Self { path }
        }

        pub(crate) fn path(&self) -> &Path {
            &self.path
        }

        pub(crate) fn child(&self, relative: &str) -> PathBuf {
            let child = self.path.join(relative);
            fs::create_dir_all(&child).expect("create child dir");
            child
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    pub(crate) fn as_str(path: &Path) -> String {
        path.to_string_lossy().into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::{as_str, TempDir};
    use super::*;

    /// A folder shaped like a Laravel root: `artisan` plus `storage/logs`.
    fn laravel_fixture(root: &Path) {
        fs::create_dir_all(root.join("storage").join("logs")).expect("create storage/logs");
        fs::write(root.join("artisan"), "#!/usr/bin/env php\n").expect("write artisan");
    }

    #[test]
    fn a_folder_without_artisan_is_still_added_with_the_reason() {
        let temp = TempDir::new("not-laravel");
        let root = temp.child("plain-folder");
        let mut projects = Vec::new();

        let project = add(&mut projects, &as_str(&root)).expect("registration warns, never blocks");

        assert_eq!(
            projects.len(),
            1,
            "the folder is registered despite failing validation"
        );
        assert_eq!(project.health(), &Health::NotLaravel);
        assert_eq!(project.label(), "plain-folder");
        assert_eq!(project.path(), root);
    }

    #[test]
    fn a_laravel_root_without_a_logs_dir_reports_no_logs_dir() {
        let temp = TempDir::new("no-logs");
        let root = temp.child("api");
        fs::write(root.join("artisan"), "#!/usr/bin/env php\n").expect("write artisan");
        let mut projects = Vec::new();

        let project = add(&mut projects, &as_str(&root)).expect("add");

        assert_eq!(project.health(), &Health::NoLogsDir);
    }

    #[test]
    fn a_laravel_root_with_an_empty_logs_dir_is_healthy() {
        // laravel/laravel commits storage/logs/.gitignore, so the directory
        // exists after a fresh clone with no log files in it.
        let temp = TempDir::new("healthy");
        let root = temp.child("api");
        laravel_fixture(&root);
        let mut projects = Vec::new();

        let project = add(&mut projects, &as_str(&root)).expect("add");

        assert_eq!(project.health(), &Health::Ok);
    }

    #[test]
    fn adding_the_same_path_twice_yields_one_project() {
        let temp = TempDir::new("duplicate");
        let root = temp.child("api");
        laravel_fixture(&root);
        let mut projects = Vec::new();

        let first = add(&mut projects, &as_str(&root)).expect("first add");
        let second =
            add(&mut projects, &as_str(&root)).expect("second add is a no-op, not an error");

        assert_eq!(projects.len(), 1);
        assert_eq!(first.id(), second.id());
    }

    #[test]
    fn adding_the_same_folder_through_a_dot_segment_is_still_one_project() {
        // Identity is the canonicalized path, so a differently spelled route to
        // the same folder must not register twice.
        let temp = TempDir::new("dot-segment");
        let root = temp.child("api");
        laravel_fixture(&root);
        let mut projects = Vec::new();

        add(&mut projects, &as_str(&root)).expect("add");
        let spelled_differently = format!("{}/./", as_str(&root));
        add(&mut projects, &spelled_differently).expect("add");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].path(), root);
    }

    #[test]
    fn two_folders_named_api_get_distinct_labels() {
        let temp = TempDir::new("collision");
        let work = temp.child("work/api");
        let side = temp.child("side/api");
        let mut projects = Vec::new();

        add(&mut projects, &as_str(&work)).expect("add work");
        add(&mut projects, &as_str(&side)).expect("add side");

        assert_eq!(projects[0].label(), "work/api");
        assert_eq!(projects[1].label(), "side/api");
    }

    #[test]
    fn labels_keep_growing_until_they_actually_differ() {
        // ~/work/acme/api and ~/side/acme/api share both the basename and its
        // parent, so one parent segment is not enough.
        let temp = TempDir::new("deep-collision");
        let work = temp.child("work/acme/api");
        let side = temp.child("side/acme/api");
        let mut projects = Vec::new();

        add(&mut projects, &as_str(&work)).expect("add work");
        add(&mut projects, &as_str(&side)).expect("add side");

        assert_eq!(projects[0].label(), "work/acme/api");
        assert_eq!(projects[1].label(), "side/acme/api");
    }

    #[test]
    fn a_lone_project_keeps_its_basename_as_its_label() {
        let temp = TempDir::new("basename");
        let root = temp.child("work/acme/api");
        let mut projects = Vec::new();

        add(&mut projects, &as_str(&root)).expect("add");

        assert_eq!(projects[0].label(), "api");
    }

    #[test]
    fn removing_a_colliding_project_restores_the_plain_basename() {
        let temp = TempDir::new("uncollide");
        let work = temp.child("work/api");
        let side = temp.child("side/api");
        let mut projects = Vec::new();
        add(&mut projects, &as_str(&work)).expect("add work");
        add(&mut projects, &as_str(&side)).expect("add side");
        assert_ne!(projects[0].label(), "api");

        let removed = remove(&mut projects, &as_str(&side));

        assert!(removed);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].label(), "api");
    }

    #[test]
    fn removing_an_unknown_id_is_a_no_op() {
        let temp = TempDir::new("remove-unknown");
        let root = temp.child("api");
        let mut projects = Vec::new();
        add(&mut projects, &as_str(&root)).expect("add");

        assert!(!remove(&mut projects, "/no/such/project"));
        assert_eq!(projects.len(), 1);
    }

    #[test]
    fn save_then_load_round_trips_through_a_temp_dir() {
        let temp = TempDir::new("round-trip");
        let config = temp.child("config");
        let healthy = temp.child("work/api");
        laravel_fixture(&healthy);
        let plain = temp.child("side/notes");

        let mut projects = Vec::new();
        add(&mut projects, &as_str(&healthy)).expect("add healthy");
        add(&mut projects, &as_str(&plain)).expect("add plain");
        save(&config, &projects).expect("save");

        let loaded = load(&config).expect("load");

        assert_eq!(loaded, projects, "the registry survives a restart");
    }

    #[test]
    fn loading_re_derives_identity_when_the_stored_id_disagrees_with_the_path() {
        let temp = TempDir::new("stale-id");
        let config = temp.child("config");
        let root = temp.child("work/api");
        laravel_fixture(&root);

        // A registry written before a rename, or edited by hand.
        let hand_written = format!(
            r#"[{{"id":"/stale/elsewhere/old-name","label":"old-name","path":{},"health":"ok"}}]"#,
            serde_json::to_string(&as_str(&root)).expect("encode path")
        );
        fs::write(config.join(REGISTRY_FILE_NAME), hand_written).expect("write registry");

        let loaded = load(&config).expect("load");

        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].id().as_str(),
            as_str(&root),
            "id is re-derived from the path, never trusted from the file"
        );
        assert_eq!(loaded[0].label(), "api", "label is re-derived too");
        assert_eq!(loaded[0].health(), &Health::Ok, "health is recomputed");
    }

    #[test]
    fn loading_re_runs_collision_disambiguation() {
        let temp = TempDir::new("load-collision");
        let config = temp.child("config");
        let work = temp.child("work/api");
        let side = temp.child("side/api");

        let hand_written = format!(
            r#"[{{"id":"x","label":"api","path":{},"health":"ok"}},
                {{"id":"y","label":"api","path":{},"health":"ok"}}]"#,
            serde_json::to_string(&as_str(&work)).expect("encode"),
            serde_json::to_string(&as_str(&side)).expect("encode"),
        );
        fs::write(config.join(REGISTRY_FILE_NAME), hand_written).expect("write registry");

        let loaded = load(&config).expect("load");

        assert_eq!(loaded.len(), 2);
        assert_ne!(loaded[0].label(), loaded[1].label());
    }

    #[test]
    fn loading_a_registry_whose_folder_vanished_keeps_the_project_as_unavailable() {
        let temp = TempDir::new("vanished");
        let config = temp.child("config");
        let gone = temp.path().join("work/api");

        let hand_written = format!(
            r#"[{{"id":"x","label":"api","path":{},"health":"ok"}}]"#,
            serde_json::to_string(&as_str(&gone)).expect("encode"),
        );
        fs::write(config.join(REGISTRY_FILE_NAME), hand_written).expect("write registry");

        let loaded = load(&config).expect("load");

        assert_eq!(loaded.len(), 1, "removal stays a deliberate user action");
        assert!(matches!(loaded[0].health(), Health::Unavailable(_)));
    }

    #[test]
    fn a_missing_registry_file_loads_as_empty_not_as_an_error() {
        let temp = TempDir::new("first-run");
        let config = temp.child("config");

        let loaded = load(&config).expect("first run is not a failure");

        assert!(loaded.is_empty());
    }

    #[test]
    fn a_corrupt_registry_is_an_error_rather_than_a_silent_wipe() {
        let temp = TempDir::new("corrupt");
        let config = temp.child("config");
        fs::write(config.join(REGISTRY_FILE_NAME), b"{not json at all").expect("write registry");

        let result = load(&config);

        assert!(matches!(result, Err(RegistryError::Corrupt { .. })));
        assert!(
            config.join(REGISTRY_FILE_NAME).exists(),
            "the unreadable file is left on disk for the user to recover"
        );
    }

    #[test]
    fn quarantine_moves_a_corrupt_registry_out_of_the_way_of_the_next_save() {
        let temp = TempDir::new("quarantine");
        let config = temp.child("config");
        fs::write(config.join(REGISTRY_FILE_NAME), b"{not json at all").expect("write registry");

        let moved = quarantine(&config).expect("quarantine");

        assert!(moved.exists());
        assert!(!config.join(REGISTRY_FILE_NAME).exists());
        assert_eq!(fs::read(&moved).expect("read"), b"{not json at all");
    }

    #[test]
    fn two_stored_entries_that_resolve_to_the_same_folder_collapse_into_one() {
        // A hand-edited or twice-written registry can spell one folder two ways.
        // Identity is the canonicalized path, so loading must behave exactly
        // like adding it twice: one Project, the first entry surviving.
        let temp = TempDir::new("stored-duplicate");
        let config = temp.child("config");
        let root = temp.child("work/api");
        laravel_fixture(&root);

        let hand_written = format!(
            r#"[{{"id":"x","label":"first","path":{},"health":"ok"}},
                {{"id":"y","label":"second","path":{},"health":"ok"}}]"#,
            serde_json::to_string(&as_str(&root)).expect("encode"),
            serde_json::to_string(&format!("{}/./", as_str(&root))).expect("encode"),
        );
        fs::write(config.join(REGISTRY_FILE_NAME), hand_written).expect("write registry");

        let loaded = load(&config).expect("load");

        assert_eq!(loaded.len(), 1, "one folder is one Project");
        assert_eq!(loaded[0].path(), root);
        assert_eq!(
            loaded[0].label(),
            "api",
            "the label is re-derived, not kept"
        );
    }

    #[test]
    fn save_surfaces_an_io_error_rather_than_swallowing_it() {
        // A registry directory that cannot be created stands in for any
        // unwritable config dir. Losing this error loses the user's Projects
        // without telling them.
        let temp = TempDir::new("unwritable");
        let blocked = temp.path().join("config");
        fs::write(&blocked, b"this is a file, not a directory").expect("write blocker");

        let result = save(&blocked, &[]);

        assert!(
            matches!(result, Err(RegistryError::Io { .. })),
            "expected an Io error, got {result:?}"
        );
    }

    #[test]
    fn load_surfaces_an_io_error_instead_of_reporting_an_empty_registry() {
        // Only a *missing* file means "first run". Any other read failure must
        // not be flattened into an empty registry, or the next save wipes it.
        let temp = TempDir::new("unreadable");
        let config = temp.child("config");
        fs::create_dir_all(config.join(REGISTRY_FILE_NAME)).expect("occupy the registry path");

        let result = load(&config);

        assert!(
            matches!(result, Err(RegistryError::Io { .. })),
            "expected an Io error, got {result:?}"
        );
    }

    #[test]
    fn quarantine_reports_failure_rather_than_pretending_it_moved_the_file() {
        let temp = TempDir::new("quarantine-fails");
        let config = temp.child("config");
        fs::write(config.join(REGISTRY_FILE_NAME), b"{not json at all").expect("write registry");
        // Something already occupies the destination and cannot be replaced by
        // a file, so the rename fails.
        let occupied = config.join(QUARANTINE_FILE_NAME);
        fs::create_dir_all(&occupied).expect("occupy the quarantine path");
        fs::write(occupied.join("keep"), b"not empty").expect("write occupant");

        let result = quarantine(&config);

        assert!(
            matches!(result, Err(RegistryError::Io { .. })),
            "expected an Io error, got {result:?}"
        );
        assert!(
            config.join(REGISTRY_FILE_NAME).exists(),
            "the unreadable registry is still where it was"
        );
    }

    #[test]
    fn a_relative_path_is_refused_rather_than_resolved() {
        // Resolving a relative segment against the process cwd is the hole that
        // avoiding the fs plugin's static scopes was meant to close.
        let mut projects = Vec::new();

        let result = add(&mut projects, "../../etc");

        assert!(matches!(result, Err(RegistryError::NotAbsolute(_))));
        assert!(projects.is_empty());
    }

    #[test]
    fn a_path_that_does_not_exist_is_refused() {
        let temp = TempDir::new("missing");
        let missing = temp.path().join("nope");
        let mut projects = Vec::new();

        let result = add(&mut projects, &as_str(&missing));

        assert!(matches!(result, Err(RegistryError::Unresolvable { .. })));
    }

    #[test]
    fn a_file_is_not_a_project() {
        let temp = TempDir::new("file");
        let file = temp.path().join("laravel.txt");
        fs::write(&file, b"not a folder").expect("write file");
        let mut projects = Vec::new();

        let result = add(&mut projects, &as_str(&file));

        assert!(matches!(result, Err(RegistryError::NotADirectory(_))));
    }

    #[test]
    fn stored_paths_carry_no_traversal_segments() {
        let temp = TempDir::new("traversal");
        let root = temp.child("work/api");
        let mut projects = Vec::new();

        let sideways = format!("{}/../api", as_str(&root));
        let project = add(&mut projects, &sideways).expect("add");

        assert_eq!(project.path(), root, "the canonical form is what is stored");
        assert!(!as_str(project.path()).contains(".."));
    }

    #[test]
    fn re_adding_a_project_refreshes_its_health() {
        let temp = TempDir::new("refresh");
        let root = temp.child("api");
        let mut projects = Vec::new();
        let before = add(&mut projects, &as_str(&root)).expect("add");
        assert_eq!(before.health(), &Health::NotLaravel);

        laravel_fixture(&root);
        let after = add(&mut projects, &as_str(&root)).expect("re-add");

        assert_eq!(projects.len(), 1);
        assert_eq!(after.health(), &Health::Ok);
        assert_eq!(projects[0].health(), &Health::Ok);
    }
}
