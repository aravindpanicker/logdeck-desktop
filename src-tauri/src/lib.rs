pub mod model;
pub mod parser;
pub mod project;
pub mod watcher;

/// The BUILD-SPEC §9 verification matrix, executed against real files in a temp
/// directory. Test-only: it ships no behaviour, it only pins the shipped
/// behaviour to the table in the spec.
#[cfg(test)]
mod verification;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager, State};

use crate::model::{Project, ProjectId, StreamItem};
use crate::watcher::{EventSink, LogFile, Target, TauriSink, Watchers};

/// Process-wide state managed by Tauri.
///
/// Rust owns the watchers and therefore owns the **Project** list; a
/// client-side copy would be a second source of truth.
#[derive(Default)]
pub struct AppState {
    /// The registered **Projects**, in the order the user added them.
    pub projects: Mutex<Vec<Project>>,
    /// One watcher per Project. Every registered Project is watched; selection
    /// only changes a watcher's mode (D8, ADR 0002).
    pub watchers: Watchers,
    /// Why the registry must not be written, if it must not be.
    ///
    /// Set at startup when the persisted registry could not be read *and* could
    /// not be moved aside. The file on disk may still hold every Project the
    /// user registered; `projects` is empty only because the read failed, so
    /// saving would replace their registry with that emptiness. Refusing is the
    /// only outcome that keeps the file recoverable.
    pub write_refused: Mutex<Option<String>>,
}

/// A poisoned registry mutex means another thread panicked mid-update. There is
/// no safe recovery, so the command reports it rather than papering over it.
const POISONED: &str = "the project registry is unavailable after an internal error";

/// Shown to the user when a write is refused because the saved registry could
/// not be read. Losing the Project list is the one failure they cannot undo.
const WRITE_REFUSED: &str = "refusing to change the project registry: the saved registry could not be read and could not be moved aside, so writing now would overwrite it";

/// Where the registry is persisted in production. Tests inject a temp dir
/// straight into `project::save` / `project::load` instead.
fn registry_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path()
        .app_config_dir()
        .map_err(|err| format!("cannot locate the configuration directory: {err}"))
}

#[tauri::command]
fn list_projects(state: State<'_, AppState>) -> Result<Vec<Project>, String> {
    let projects = state.projects.lock().map_err(|_| POISONED.to_string())?;
    Ok(projects.clone())
}

/// Change the registry, persist it, and only then let the change be seen.
///
/// The mutation runs against a copy. Memory is updated from that copy after the
/// save succeeds, so a failed write leaves the frontend and the file agreeing:
/// a Project the user was told was not added is not in `list_projects` either,
/// and one they were told was not removed is still there. Anything else lets
/// the UI show state that disappears at the next restart.
///
/// Persisting is skipped when the mutation changed nothing, so a no-op add or
/// an unknown-id removal does not rewrite the file.
fn update_registry<T>(
    dir: &Path,
    state: &AppState,
    mutate: impl FnOnce(&mut Vec<Project>) -> Result<T, String>,
) -> Result<T, String> {
    if let Some(reason) = state
        .write_refused
        .lock()
        .map_err(|_| POISONED.to_string())?
        .as_ref()
    {
        return Err(reason.clone());
    }

    let mut projects = state.projects.lock().map_err(|_| POISONED.to_string())?;
    let mut candidate = projects.clone();
    let outcome = mutate(&mut candidate)?;

    if candidate != *projects {
        project::save(dir, &candidate).map_err(|err| err.to_string())?;
        *projects = candidate;
    }

    Ok(outcome)
}

/// Registration warns, never blocks (D4): a folder that is not a Laravel root
/// is still added, carrying the **Health** that says why.
#[tauri::command]
fn add_project(
    app: AppHandle,
    state: State<'_, AppState>,
    path: String,
) -> Result<Project, String> {
    let dir = registry_dir(&app)?;
    let project = update_registry(&dir, state.inner(), |projects| {
        project::add(projects, &path).map_err(|err| err.to_string())
    })?;
    // A Project is watched from the moment it is registered (D8).
    state
        .watchers
        .ensure(project.id(), project.path(), &sink_for(&app));
    Ok(project)
}

#[tauri::command]
fn remove_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    let dir = registry_dir(&app)?;
    update_registry(&dir, state.inner(), |projects| {
        project::remove(projects, &project_id);
        Ok(())
    })?;
    // Deregistration is the only thing that stops a watcher (ADR 0002).
    state
        .watchers
        .stop(&ProjectId::from_canonical(Path::new(&project_id)));
    Ok(())
}

/* -------------------------------------------------------------------------- */
/* Stream commands                                                             */
/* -------------------------------------------------------------------------- */

fn sink_for(app: &AppHandle) -> Arc<dyn EventSink> {
    Arc::new(TauriSink::new(app.clone()))
}

/// The canonical root of a registered **Project**.
///
/// Read from the registry rather than taken from the frontend: a path the
/// client made up must never reach the filesystem (BUILD-SPEC §7).
fn project_root(state: &AppState, project_id: &str) -> Result<PathBuf, String> {
    let projects = state.projects.lock().map_err(|_| POISONED.to_string())?;
    projects
        .iter()
        .find(|project| project.id().as_str() == project_id)
        .map(|project| project.path().to_path_buf())
        .ok_or_else(|| format!("`{project_id}` is not a registered project"))
}

/// The id a **Project**'s watcher is keyed by, with the watcher started if it is
/// somehow not running.
///
/// Every stream command derives the watched id here and nowhere else. Deriving
/// it twice is how one command ends up looking under a key another command never
/// writes — the watcher keeps streaming while paging and retargeting report that
/// the Project is not being watched.
fn watched_id(app: &AppHandle, state: &AppState, project_id: &str) -> Result<ProjectId, String> {
    let root = project_root(state, project_id)?;
    let id = ProjectId::from_canonical(&root);
    state.watchers.ensure(&id, &root, &sink_for(app));
    Ok(id)
}

/// A Project that is registered but has no running watcher — the OS refused the
/// thread. Reported rather than papered over: a watcher that does not exist is a
/// view that has silently stopped updating.
fn not_watched(project_id: &str) -> String {
    format!("`{project_id}` is not being watched")
}

/// Act on a **Project**'s watcher, starting it if it is somehow not running.
fn with_watcher<R>(
    app: &AppHandle,
    state: &AppState,
    project_id: &str,
    act: impl FnOnce(&mut watcher::WatcherState) -> R,
) -> Result<R, String> {
    let id = watched_id(app, state, project_id)?;
    state
        .watchers
        .with(&id, act)
        .ok_or_else(|| not_watched(project_id))
}

/// Every log file in the Project's `storage/logs/`, newest first, for the
/// **Target** picker (D5).
#[tauri::command]
fn list_log_files(state: State<'_, AppState>, project_id: String) -> Result<Vec<LogFile>, String> {
    let root = project_root(state.inner(), &project_id)?;
    watcher::list_files(&root)
}

/// Promote this Project's watcher and return its opening window (D6).
///
/// Selection never starts or stops watching — it promotes one watcher and
/// demotes the previous one (ADR 0002).
#[tauri::command]
fn select_project(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<Vec<StreamItem>, String> {
    let id = watched_id(&app, state.inner(), &project_id)?;
    state.watchers.promote(&id);
    state
        .watchers
        .with(&id, |watcher| watcher.open())
        .ok_or_else(|| not_watched(&project_id))?
}

/// Pin a file, or go back to following the newest one (D5).
#[tauri::command]
fn set_target(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    target: Target,
) -> Result<Vec<StreamItem>, String> {
    with_watcher(&app, state.inner(), &project_id, |watcher| {
        watcher.set_target(target);
        watcher.open()
    })?
}

/// Page another 500 **Entries** back from the **Entry** with this id (D6).
///
/// The id rather than a bare offset: it carries the file and the id generation
/// as well, and after a **Break** the oldest Entry the client holds belongs to a
/// different file from the one being tailed. An offset alone would be read
/// against the wrong file.
#[tauri::command]
fn load_earlier(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
    before_id: String,
) -> Result<Vec<StreamItem>, String> {
    with_watcher(&app, state.inner(), &project_id, |watcher| {
        watcher.earlier(&before_id)
    })?
}

/// Spend a Project's **Activity** badge (D8).
#[tauri::command]
fn clear_activity(
    app: AppHandle,
    state: State<'_, AppState>,
    project_id: String,
) -> Result<(), String> {
    with_watcher(&app, state.inner(), &project_id, |watcher| {
        watcher.clear_activity()
    })
}

/// Start a watcher for every registered **Project** (D8).
///
/// Watching is not conditional on being selected: the sidebar can only tell you
/// that a Project you are *not* reading threw an exception because its watcher
/// was already running (ADR 0002).
fn start_watchers(app: &AppHandle) {
    let state = app.state::<AppState>();
    let sink = sink_for(app);
    let roots: Vec<(ProjectId, PathBuf)> = match state.projects.lock() {
        Ok(projects) => projects
            .iter()
            .map(|project| (project.id().clone(), project.path().to_path_buf()))
            .collect(),
        Err(_) => {
            eprintln!("logdeck: {POISONED}");
            return;
        }
    };

    for (id, root) in roots {
        state.watchers.ensure(&id, &root, &sink);
    }
}

/// Restore the persisted registry into [`AppState`].
///
/// A failure here is loud but not fatal — the app still opens. What it must
/// never do is leave an empty registry that a later save is willing to write:
/// the file on disk is the user's Project list, and an empty in-memory list
/// after a failed read means "unknown", not "none". So either the old file is
/// preserved somewhere else, or writing is refused until the next launch.
fn restore_registry(app: &AppHandle) {
    let dir = match registry_dir(app) {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("logdeck: {err}");
            return;
        }
    };

    let state = app.state::<AppState>();
    restore_registry_from(&dir, state.inner());
}

/// The body of [`restore_registry`], taking the directory and state directly so
/// tests can drive it without an `AppHandle`.
fn restore_registry_from(dir: &Path, state: &AppState) {
    let err = match project::load(dir) {
        Ok(projects) => {
            match state.projects.lock() {
                Ok(mut registry) => *registry = projects,
                Err(_) => eprintln!("logdeck: {POISONED}"),
            }
            return;
        }
        Err(err) => err,
    };

    eprintln!("logdeck: could not read the project registry: {err}");

    // Quarantine applies only to a file that exists and does not parse. Any
    // other failure — permission denied, a lock, a flaky disk — says nothing
    // about the contents, so moving it is not a safe thing to attempt.
    let preserved = matches!(err, project::RegistryError::Corrupt { .. })
        && match project::quarantine(dir) {
            Ok(moved) => {
                eprintln!("logdeck: kept a copy at {}", moved.display());
                true
            }
            Err(err) => {
                eprintln!("logdeck: could not preserve it: {err}");
                false
            }
        };

    if !preserved {
        let reason = format!("{WRITE_REFUSED} ({err})");
        eprintln!("logdeck: {reason}");
        // A poisoned lock here is itself a refusal: `update_registry` fails on
        // the same lock, so the guarantee holds either way.
        if let Ok(mut refused) = state.write_refused.lock() {
            *refused = Some(reason);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // No `opener`: nothing in the app opens a path or a URL with the OS
        // handler, and an unused capability is only attack surface (BUILD-SPEC
        // §7 — the same discipline that keeps the `fs` plugin out).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(AppState::default())
        .setup(|app| {
            restore_registry(app.handle());
            start_watchers(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_projects,
            add_project,
            remove_project,
            list_log_files,
            select_project,
            set_target,
            load_earlier,
            clear_activity
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// These drive `update_registry` and `restore_registry_from` directly rather
/// than the `#[tauri::command]` wrappers, which need an `AppHandle`. The
/// wrappers hold no logic of their own beyond resolving the config directory.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::test_support::{as_str, TempDir};
    use std::fs;

    /// A config directory that cannot exist: `save` fails at `create_dir_all`.
    fn unwritable_config(temp: &TempDir) -> PathBuf {
        let blocked = temp.path().join("config");
        fs::write(&blocked, b"this is a file, not a directory").expect("write blocker");
        blocked
    }

    fn registry_with(path: &Path) -> Vec<Project> {
        let mut projects = Vec::new();
        project::add(&mut projects, &as_str(path)).expect("seed the registry");
        projects
    }

    /// Every stream command reaches the filesystem through `project_root`, so an
    /// id the frontend made up — stale, mistyped, or hostile — must fail rather
    /// than fall back to some other registered Project's folder.
    #[test]
    fn an_unregistered_project_id_is_refused_rather_than_resolved_to_another() {
        let temp = TempDir::new("unregistered-id");
        let root = temp.child("api");
        let state = AppState::default();
        *state.projects.lock().expect("lock") = registry_with(&root);

        let err = project_root(&state, "/no/such/project")
            .expect_err("an unregistered id must not resolve to a path");
        assert!(
            err.contains("is not a registered project"),
            "unhelpful message: {err}"
        );

        let registered = state.projects.lock().expect("lock")[0]
            .id()
            .as_str()
            .to_owned();
        let resolved = project_root(&state, &registered).expect("the registered id resolves");
        assert_eq!(
            resolved,
            state.projects.lock().expect("lock")[0].path(),
            "and it resolves to its own folder, not another's"
        );
    }

    #[test]
    fn a_failed_save_does_not_leave_the_added_project_in_memory() {
        let temp = TempDir::new("add-rollback");
        let root = temp.child("api");
        let config = unwritable_config(&temp);
        let state = AppState::default();

        let result = update_registry(&config, &state, |projects| {
            project::add(projects, &as_str(&root)).map_err(|err| err.to_string())
        });

        assert!(result.is_err(), "the save failed, so the add failed");
        assert!(
            state.projects.lock().expect("lock").is_empty(),
            "a Project the user was told was not added must not show up in list_projects"
        );
    }

    #[test]
    fn a_failed_save_does_not_remove_the_project_from_memory() {
        let temp = TempDir::new("remove-rollback");
        let root = temp.child("api");
        let config = unwritable_config(&temp);
        let state = AppState::default();
        let seeded = registry_with(&root);
        let id = as_str(&root);
        *state.projects.lock().expect("lock") = seeded;

        let result = update_registry(&config, &state, |projects| {
            project::remove(projects, &id);
            Ok(())
        });

        assert!(result.is_err(), "the save failed, so the removal failed");
        assert_eq!(
            state.projects.lock().expect("lock").len(),
            1,
            "a Project the user was told was not removed is still registered"
        );
    }

    #[test]
    fn removing_an_unknown_project_needs_no_write_at_all() {
        // Nothing changed, so nothing is persisted — an unwritable config dir
        // must not turn a no-op into an error.
        let temp = TempDir::new("remove-noop");
        let config = unwritable_config(&temp);
        let state = AppState::default();

        let result = update_registry(&config, &state, |projects| {
            project::remove(projects, "/no/such/project");
            Ok(())
        });

        assert!(result.is_ok(), "got {result:?}");
    }

    #[test]
    fn a_registry_that_could_not_be_read_refuses_every_later_project_write() {
        // The regression: an I/O failure that is neither "missing" nor
        // "corrupt" used to be logged and forgotten, leaving an empty registry
        // that the next add happily wrote over the user's real one.
        let temp = TempDir::new("unreadable-registry");
        let config = temp.child("config");
        let occupied = config.join(project::REGISTRY_FILE_NAME);
        fs::create_dir_all(&occupied).expect("occupy the registry path");
        let root = temp.child("api");
        let state = AppState::default();

        restore_registry_from(&config, &state);

        let result = update_registry(&config, &state, |projects| {
            project::add(projects, &as_str(&root)).map_err(|err| err.to_string())
        });

        let err = result.expect_err("the write must be refused");
        assert!(err.contains(WRITE_REFUSED), "unhelpful message: {err}");
        assert!(
            state.projects.lock().expect("lock").is_empty(),
            "the refused add left nothing behind"
        );
        assert!(occupied.exists(), "the unreadable registry is untouched");
    }

    #[test]
    fn a_corrupt_registry_that_cannot_be_quarantined_refuses_project_writes() {
        let temp = TempDir::new("stuck-quarantine");
        let config = temp.child("config");
        let registry = config.join(project::REGISTRY_FILE_NAME);
        fs::write(&registry, b"{not json at all").expect("write registry");
        let occupied = config.join(project::QUARANTINE_FILE_NAME);
        fs::create_dir_all(&occupied).expect("occupy the quarantine path");
        fs::write(occupied.join("keep"), b"not empty").expect("write occupant");
        let root = temp.child("api");
        let state = AppState::default();

        restore_registry_from(&config, &state);

        let result = update_registry(&config, &state, |projects| {
            project::add(projects, &as_str(&root)).map_err(|err| err.to_string())
        });

        assert!(
            result
                .expect_err("the write must be refused")
                .contains(WRITE_REFUSED),
            "a corrupt registry that stayed put must not be overwritten"
        );
        assert_eq!(
            fs::read(&registry).expect("read"),
            b"{not json at all",
            "the last recoverable copy survives"
        );
    }

    #[test]
    fn a_quarantined_registry_still_lets_the_user_add_a_project() {
        // The refusal must not outlive the danger: once the unreadable file is
        // safely aside, saving a fresh registry destroys nothing.
        let temp = TempDir::new("after-quarantine");
        let config = temp.child("config");
        fs::write(
            config.join(project::REGISTRY_FILE_NAME),
            b"{not json at all",
        )
        .expect("write registry");
        let root = temp.child("api");
        let state = AppState::default();

        restore_registry_from(&config, &state);

        let added = update_registry(&config, &state, |projects| {
            project::add(projects, &as_str(&root)).map_err(|err| err.to_string())
        });

        assert!(added.is_ok(), "got {added:?}");
        assert_eq!(state.projects.lock().expect("lock").len(), 1);
        assert_eq!(
            fs::read(config.join(project::QUARANTINE_FILE_NAME)).expect("read"),
            b"{not json at all",
            "the corrupt file is still recoverable"
        );
    }

    #[test]
    fn a_first_run_with_no_registry_file_allows_project_writes() {
        let temp = TempDir::new("first-run");
        let config = temp.child("config");
        let root = temp.child("api");
        let state = AppState::default();

        restore_registry_from(&config, &state);

        let added = update_registry(&config, &state, |projects| {
            project::add(projects, &as_str(&root)).map_err(|err| err.to_string())
        });

        assert!(added.is_ok(), "got {added:?}");
        assert_eq!(
            project::load(&config).expect("load").len(),
            1,
            "the Project reached disk"
        );
    }
}
