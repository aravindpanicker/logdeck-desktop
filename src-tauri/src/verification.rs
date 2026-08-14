//! Phase 9 — the BUILD-SPEC §9 verification matrix, executed.
//!
//! §9 is written as a manual script against `npm run tauri dev`. Nothing in this
//! environment can click a native dialog or read the system clipboard, so every
//! row that is really a claim about *behaviour* rather than about *pixels* is
//! driven here against real files in a temp directory, through the same
//! `WatcherState`, `Watchers` and `project` code the app runs.
//!
//! Fixtures never touch the repo: every one is created under
//! [`crate::project::test_support::TempDir`], which removes itself on drop.
//!
//! Test names carry their matrix row so a failure points straight back at the
//! table.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::model::{BreakKind, Health, Level, ProjectId, StreamItem};
use crate::project::test_support::TempDir;
use crate::project::{self, logs_dir};
use crate::watcher::test_support::Recorder;
use crate::watcher::{
    EventSink, Mode, StatusState, Target, WatcherState, Watchers, EVENT_ACTIVITY, EVENT_BREAK,
    EVENT_ENTRY, EVENT_STATUS,
};

/* -------------------------------------------------------------------------- */
/* Fixtures                                                                    */
/* -------------------------------------------------------------------------- */

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

fn watcher(root: &Path, mode: Mode) -> WatcherState {
    WatcherState::new(ProjectId::from_canonical(root), root.to_path_buf(), mode)
}

/// A real Laravel exception as Monolog flattens it: one header, a JSON context
/// line, `[stacktrace]`, and `frames` numbered frames — one `fwrite()`, one
/// physical block, and one **Entry**.
fn trace_entry(frames: usize) -> String {
    let mut text = String::from(
        "[2026-08-14 01:28:07] local.ERROR: Undefined variable $user \
         {\"exception\":\"[object] (ErrorException(code: 0): Undefined variable $user \
         at /app/Http/Controllers/UserController.php:42)\"}\n[stacktrace]\n",
    );
    for frame in 0..frames {
        text.push_str(&format!(
            "#{frame} /app/vendor/laravel/framework/src/Illuminate/Routing/Controller.php({frame}): \
             App\\Http\\Controllers\\UserController->show()\n"
        ));
    }
    text
}

/// Spin until `check` holds, or fail with `what`. Used only where the point of
/// the row is that **nothing drove the app** — a live watcher thread on its own
/// 300 ms cadence.
fn wait_for(what: &str, timeout: Duration, mut check: impl FnMut() -> bool) -> Duration {
    let started = Instant::now();
    while started.elapsed() < timeout {
        if check() {
            return started.elapsed();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("timed out after {timeout:?} waiting for {what}");
}

/* -------------------------------------------------------------------------- */
/* Row 1 — add a folder with no storage/logs (D4)                              */
/* -------------------------------------------------------------------------- */

#[test]
fn row_01_a_folder_with_no_storage_logs_is_added_and_marked_inert() {
    let temp = TempDir::new("row01");

    // A Laravel root whose `storage/logs` was never created.
    let bare = temp.child("no-logs-dir");
    fs::write(bare.join("artisan"), "#!/usr/bin/env php\n").expect("write artisan");
    let bare = fs::canonicalize(&bare).expect("canonicalize");

    // …and a folder that is not a Laravel project at all.
    let stranger = fs::canonicalize(temp.child("not-laravel")).expect("canonicalize");

    let mut projects = Vec::new();
    let added = project::add(&mut projects, &bare.to_string_lossy()).expect("add is not refused");
    let other =
        project::add(&mut projects, &stranger.to_string_lossy()).expect("add is not refused");

    // Registration warns, never blocks (D4): both are Projects.
    assert_eq!(projects.len(), 2, "both folders are registered");
    assert_eq!(
        added.health(),
        &Health::NoLogsDir,
        "the reason is recorded, not the rejection"
    );
    assert_eq!(other.health(), &Health::NotLaravel);
    assert_eq!(added.label(), "no-logs-dir", "it is a nameable Project");

    // Inert: a watcher over it resolves nothing and reports offline with a
    // reason rather than tailing anything.
    let sink = Recorder::default();
    let mut state = watcher(&bare, Mode::Selected);
    state.poll(&sink);

    let statuses = sink.statuses();
    assert_eq!(statuses.len(), 1, "{statuses:#?}");
    assert_eq!(statuses[0].state, StatusState::Offline);
    let reason = statuses[0].reason.as_deref().unwrap_or_default();
    assert!(
        reason.contains("storage/logs") || reason.contains("cannot read"),
        "the inert Project carries its reason: {reason:?}"
    );
    assert!(sink.entries().is_empty(), "nothing is tailed");
}

/* -------------------------------------------------------------------------- */
/* Row 2 — append an Entry, with nothing driving the app                       */
/* -------------------------------------------------------------------------- */

#[test]
fn row_02_an_appended_entry_appears_with_no_interaction() {
    let temp = TempDir::new("row02");
    let root = laravel_root(&temp, "api");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 01:28:00] local.INFO: booted\n");

    let id = ProjectId::from_canonical(&root);
    let recorder = Arc::new(Recorder::default());
    let sink: Arc<dyn EventSink> = recorder.clone();

    // A live watcher thread on its own 300 ms cadence. Nothing below calls
    // `poll` — the only action taken is writing to the file.
    let watchers = Watchers::default();
    watchers.ensure(&id, &root, &sink);
    watchers.promote(&id);
    wait_for("the watcher to attach", Duration::from_secs(5), || {
        watchers.with(&id, |state| state.file().is_some()) == Some(true)
    });
    recorder.take();

    append(
        &log,
        "[2026-08-14 01:28:01] local.ERROR: queue worker died\n",
    );

    let took = wait_for(
        "the appended Entry to arrive",
        Duration::from_secs(5),
        || {
            recorder
                .entries()
                .iter()
                .any(|entry| entry.message == "queue worker died")
        },
    );

    let arrived = recorder
        .entries()
        .into_iter()
        .find(|entry| entry.message == "queue worker died")
        .expect("the Entry arrived");
    assert_eq!(arrived.level, Level::Error);
    assert_eq!(arrived.env, "local");
    assert!(
        took < Duration::from_secs(2),
        "it arrived unaided within one or two poll periods, in {took:?}"
    );

    watchers.stop_all();
}

/* -------------------------------------------------------------------------- */
/* Row 3 — a 47-line trace is exactly one Entry                                */
/* -------------------------------------------------------------------------- */

#[test]
fn row_03_a_forty_seven_line_trace_forms_exactly_one_entry() {
    let temp = TempDir::new("row03");
    let root = laravel_root(&temp, "api");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 01:28:00] local.INFO: booted\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    state.poll(&sink);
    sink.take();

    // The header, the JSON context, `[stacktrace]`, and 45 frames: 47 lines.
    let trace = trace_entry(45);
    assert_eq!(trace.lines().count(), 47, "the fixture really is 47 lines");
    append(&log, &trace);
    // A following header, so the trace Entry closes rather than staying pending.
    append(&log, "[2026-08-14 01:28:08] local.INFO: recovered\n");
    state.poll(&sink);

    let entries = sink.entries();
    let trace_entries: Vec<_> = entries
        .iter()
        .filter(|entry| entry.raw.contains("Undefined variable $user"))
        .collect();

    // Revisions share an id (D2), so "exactly one Entry" is one *identity*.
    let ids: std::collections::BTreeSet<_> = trace_entries
        .iter()
        .map(|entry| entry.id.as_str())
        .collect();
    assert_eq!(
        ids.len(),
        1,
        "the whole trace is one Entry, not one per line: {ids:#?}"
    );

    let final_form = trace_entries.last().expect("the trace was emitted");
    assert_eq!(final_form.level, Level::Error);
    // Verbatim, minus only the newline that *separates* this record from the
    // next — that byte belongs to the file, not to the event.
    assert_eq!(
        final_form.raw,
        trace.trim_end_matches('\n'),
        "`raw` is the file's bytes verbatim — what Copy sends (D1)"
    );
    assert_eq!(final_form.raw.lines().count(), 47);
    assert_eq!(
        final_form.context.lines().count(),
        46,
        "the header stays the message; the other 46 lines are context"
    );
    assert!(final_form.context.contains("[stacktrace]"));
    assert!(
        final_form.context.contains("#44 "),
        "the last frame is there"
    );
    assert_eq!(final_form.message, {
        let header = trace.lines().next().expect("header");
        header[header.find("local.ERROR: ").expect("level") + "local.ERROR: ".len()..].to_owned()
    });
}

/* -------------------------------------------------------------------------- */
/* Row 4 — truncate -s 0 (D3, ADR 0001)                                        */
/* -------------------------------------------------------------------------- */

#[test]
fn row_04_truncation_breaks_the_record_and_nothing_above_it_is_retracted() {
    let temp = TempDir::new("row04");
    let root = laravel_root(&temp, "api");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 01:28:00] local.INFO: booted\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    state.poll(&sink);
    sink.take();

    append(
        &log,
        "[2026-08-14 01:28:01] local.ERROR: before the clear\n",
    );
    state.poll(&sink);
    let above: Vec<_> = sink
        .entries()
        .into_iter()
        .map(|entry| entry.id.as_str().to_owned())
        .collect();
    assert!(!above.is_empty(), "there is something above the Break");
    sink.take();

    // `php artisan log:clear`, or an editor truncating on save.
    fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&log)
        .expect("truncate -s 0");
    append(
        &log,
        "[2026-08-14 01:28:02] local.WARNING: after the clear\n",
    );
    state.poll(&sink);

    let breaks = sink.breaks();
    assert_eq!(breaks.len(), 1, "{breaks:#?}");
    assert_eq!(breaks[0].kind, BreakKind::Cleared);
    assert_eq!(breaks[0].file, "laravel.log");
    assert_eq!(state.offset(), fs::metadata(&log).expect("stat").len());

    // Everything above survives because nothing ever tells the client to drop
    // it: the contract has four events and none of them is a clear.
    for event in [EVENT_ENTRY, EVENT_BREAK, EVENT_ACTIVITY, EVENT_STATUS] {
        assert!(
            !event.contains("clear") && !event.contains("reset"),
            "{event} would be a way to wipe the Session Record"
        );
    }

    // …and the ids below the Break cannot collide with the ids above it, which
    // is the only other way an id-keyed client could lose a row.
    let below: Vec<_> = sink
        .entries()
        .into_iter()
        .map(|entry| entry.id.as_str().to_owned())
        .collect();
    assert!(!below.is_empty(), "the post-clear Entry arrived");
    for id in &below {
        assert!(
            !above.contains(id),
            "id {id} below the Break would overwrite an Entry above it"
        );
    }
}

/* -------------------------------------------------------------------------- */
/* Row 5 — a newer dated file (D5)                                             */
/* -------------------------------------------------------------------------- */

#[test]
fn row_05_the_target_follows_a_newer_dated_file_across_a_break() {
    let temp = TempDir::new("row05");
    let root = laravel_root(&temp, "api");
    let dir = logs_dir(&root);
    let yesterday = dir.join("laravel-2026-08-13.log");
    append(&yesterday, "[2026-08-13 23:59:00] local.INFO: yesterday\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    state.poll(&sink);
    assert_eq!(state.file(), Some("laravel-2026-08-13.log"));
    append(
        &yesterday,
        "[2026-08-13 23:59:30] local.INFO: still yesterday\n",
    );
    state.poll(&sink);
    sink.take();

    // Midnight: the `daily` channel rolls over.
    let today = dir.join("laravel-2026-08-14.log");
    append(&today, "[2026-08-14 00:00:01] local.ERROR: today\n");
    state.poll(&sink);

    let breaks = sink.breaks();
    assert_eq!(breaks.len(), 1, "{breaks:#?}");
    assert_eq!(breaks[0].kind, BreakKind::Rotated);
    assert_eq!(
        breaks[0].file, "laravel-2026-08-14.log",
        "the Break names the file in effect *after* it"
    );
    assert_eq!(
        state.file(),
        Some("laravel-2026-08-14.log"),
        "the Target followed the newest file (D5)"
    );

    let entries = sink.entries();
    assert!(
        entries.iter().any(|entry| entry.message == "today"),
        "and is tailing it: {entries:#?}"
    );
    assert!(
        entries
            .iter()
            .all(|entry| entry.file == "laravel-2026-08-14.log"),
        "everything after the Break belongs to the new file: {entries:#?}"
    );

    // Pinning overrides the follow, and gives the pinned file back (D5).
    state.set_target(Target::File("laravel-2026-08-13.log".into()));
    state.poll(&sink);
    assert_eq!(state.file(), Some("laravel-2026-08-13.log"));
}

/* -------------------------------------------------------------------------- */
/* Row 6 — a second registered Project (D8)                                    */
/* -------------------------------------------------------------------------- */

#[test]
fn row_06_a_second_project_reports_activity_count_and_highest_level() {
    let temp = TempDir::new("row06");
    let reading = laravel_root(&temp, "reading");
    let other = laravel_root(&temp, "other");
    let other_log = logs_dir(&other).join("laravel.log");
    append(&other_log, "[2026-08-14 01:00:00] local.INFO: seed\n");

    let sink = Recorder::default();
    let mut selected = watcher(&reading, Mode::Selected);
    let mut background = watcher(&other, Mode::Background);
    selected.poll(&sink);
    background.poll(&sink);
    sink.take();

    // The Project the user is *not* looking at throws.
    append(&other_log, "[2026-08-14 01:28:00] local.DEBUG: cache hit\n");
    append(
        &other_log,
        "[2026-08-14 01:28:01] local.WARNING: slow query\n",
    );
    append(&other_log, &trace_entry(45));
    append(&other_log, "[2026-08-14 01:28:09] local.INFO: recovered\n");
    background.poll(&sink);

    let activity = sink.activities();
    let latest = activity.last().expect("Activity was reported");
    assert_eq!(latest.project_id, ProjectId::from_canonical(&other));
    assert_eq!(
        latest.total, 4,
        "count of Entries, not of lines: {latest:#?}"
    );
    assert_eq!(
        latest.max_level,
        Some(Level::Error),
        "the badge shows the highest Level, not the last one"
    );
    assert_eq!(latest.counts.get(&Level::Debug), Some(&1));
    assert_eq!(latest.counts.get(&Level::Warning), Some(&1));
    assert_eq!(latest.counts.get(&Level::Error), Some(&1));
    assert_eq!(latest.counts.get(&Level::Info), Some(&1));

    // "How much and how bad, never what": no Entry text was emitted, and the
    // 47-line trace left nothing behind.
    assert!(
        sink.entries().is_empty(),
        "a background Project emits counts only (ADR 0002)"
    );
    assert!(
        sink.breaks().is_empty(),
        "and has no Session Record to break"
    );
    assert!(
        background.retained_text_bytes() < 4096,
        "retained {} bytes after a 47-line trace",
        background.retained_text_bytes()
    );

    // Selecting it spends the Activity — the badge clears, the text starts.
    background.set_mode(Mode::Selected);
    assert_eq!(background.activity().total, 0);
    assert_eq!(background.activity().max_level, None);
}

/* -------------------------------------------------------------------------- */
/* Row 7 — mv the folder away, then back (D9)                                  */
/* -------------------------------------------------------------------------- */

#[test]
fn row_07_a_moved_project_goes_offline_and_reattaches_unaided() {
    let temp = TempDir::new("row07");
    let root = laravel_root(&temp, "api");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 01:28:00] local.INFO: booted\n");
    let elsewhere = temp.path().join("api-moved-away");

    let id = ProjectId::from_canonical(&root);
    let recorder = Arc::new(Recorder::default());
    let sink: Arc<dyn EventSink> = recorder.clone();

    let watchers = Watchers::default();
    watchers.ensure(&id, &root, &sink);
    watchers.promote(&id);
    wait_for("the watcher to come online", Duration::from_secs(5), || {
        recorder
            .statuses()
            .iter()
            .any(|status| status.state == StatusState::Online)
    });
    recorder.take();

    // `mv api api-moved-away`
    fs::rename(&root, &elsewhere).expect("move the Project away");
    wait_for("the offline transition", Duration::from_secs(10), || {
        recorder
            .statuses()
            .iter()
            .any(|status| status.state == StatusState::Offline)
    });
    let offline = recorder
        .statuses()
        .into_iter()
        .find(|status| status.state == StatusState::Offline)
        .expect("offline");
    assert!(
        offline.reason.is_some(),
        "the sidebar is told why: {offline:#?}"
    );

    // The retry interval backs off rather than spinning at 300 ms (D9).
    wait_for("the backoff to grow", Duration::from_secs(10), || {
        watchers
            .with(&id, |state| state.interval() > Duration::from_millis(300))
            .unwrap_or(false)
    });

    // `mv api-moved-away api`. Nothing else is touched: no re-add, no reselect.
    recorder.take();
    fs::rename(&elsewhere, &root).expect("move the Project back");
    append(
        &log,
        "[2026-08-14 01:29:00] local.ERROR: back from the dead\n",
    );

    wait_for("the unaided reattach", Duration::from_secs(15), || {
        recorder
            .statuses()
            .iter()
            .any(|status| status.state == StatusState::Online)
    });
    wait_for("tailing to resume", Duration::from_secs(15), || {
        recorder
            .entries()
            .iter()
            .any(|entry| entry.message == "back from the dead")
    });

    assert_eq!(
        watchers.with(&id, |state| state.interval()),
        Some(Duration::from_millis(300)),
        "the backoff resets once the path returns"
    );

    watchers.stop_all();
}

/* -------------------------------------------------------------------------- */
/* Row 8 — a string that occurs only inside a trace (D7)                       */
/* -------------------------------------------------------------------------- */

/// The Rust half of D7: search reads `raw`, so the text a reader searches for
/// has to *be* in `raw` even when it is 40 frames down and collapsed. The
/// reveal, the match count and the highlighting are the frontend's half and are
/// covered by `src/lib/highlight.test.ts` plus the throwaway component run
/// recorded in the Phase 9 report.
#[test]
fn row_08_a_string_only_inside_a_trace_is_present_in_the_searchable_raw() {
    let temp = TempDir::new("row08");
    let root = laravel_root(&temp, "api");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 01:28:00] local.INFO: booted\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    state.poll(&sink);
    sink.take();

    append(&log, &trace_entry(45));
    append(&log, "[2026-08-14 01:28:09] local.INFO: recovered\n");
    state.poll(&sink);

    let needle = "Illuminate/Routing/Controller.php(31)";
    let hit = sink
        .entries()
        .into_iter()
        .rfind(|entry| entry.raw.contains(needle))
        .expect("an Entry matches on text only its trace contains");

    assert!(
        !hit.message.contains(needle),
        "the needle is not on the visible line"
    );
    assert!(
        hit.context.contains(needle),
        "it is down in the collapsed context, which is what makes D7 necessary"
    );
    assert_eq!(hit.level, Level::Error);
}

/* -------------------------------------------------------------------------- */
/* Row 9 — copy (D1)                                                           */
/* -------------------------------------------------------------------------- */

/// The Rust half of D1. What Copy sends is `entry.raw` and nothing else, so the
/// question "is the full trace present?" is answered here: `raw` is byte-for-byte
/// the record the file holds, header and every frame included. Whether the
/// system clipboard then hands those bytes back on paste is the manual step.
#[test]
fn row_09_the_bytes_copy_sends_are_the_whole_entry_verbatim() {
    let temp = TempDir::new("row09");
    let root = laravel_root(&temp, "api");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 01:28:00] local.INFO: booted\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    state.poll(&sink);
    sink.take();

    let trace = trace_entry(45);
    append(&log, &trace);
    append(&log, "[2026-08-14 01:28:09] local.INFO: recovered\n");
    state.poll(&sink);

    let entry = sink
        .entries()
        .into_iter()
        .rfind(|entry| entry.raw.contains("Undefined variable $user"))
        .expect("the trace Entry");

    assert_eq!(
        entry.raw,
        trace.trim_end_matches('\n'),
        "verbatim, not reassembled from the parts — only the record separator is dropped"
    );
    assert!(entry.raw.starts_with("[2026-08-14 01:28:07] local.ERROR:"));
    assert!(entry.raw.contains("[stacktrace]"));
    for frame in 0..45 {
        assert!(
            entry.raw.contains(&format!("#{frame} /app/vendor/laravel")),
            "frame #{frame} is in what Copy sends"
        );
    }
    assert_eq!(
        entry.raw.lines().count(),
        47,
        "all 47 lines reach the clipboard, not just the visible one"
    );
}

/* -------------------------------------------------------------------------- */
/* Row 10 — restart (ADR 0002)                                                 */
/* -------------------------------------------------------------------------- */

#[test]
fn row_10_projects_persist_across_a_restart_and_activity_does_not() {
    let temp = TempDir::new("row10");
    let config = temp.child("config");
    let alpha = laravel_root(&temp, "alpha");
    let beta = laravel_root(&temp, "beta");
    let log = logs_dir(&beta).join("laravel.log");
    append(&log, "[2026-08-14 01:00:00] local.INFO: seed\n");

    // --- session one -------------------------------------------------------
    let mut projects = Vec::new();
    project::add(&mut projects, &alpha.to_string_lossy()).expect("add alpha");
    project::add(&mut projects, &beta.to_string_lossy()).expect("add beta");
    project::save(&config, &projects).expect("persist the registry");

    let sink = Recorder::default();
    let mut background = watcher(&beta, Mode::Background);
    background.poll(&sink);
    append(&log, "[2026-08-14 01:28:00] local.ERROR: exploded\n");
    append(&log, "[2026-08-14 01:28:01] local.INFO: recovered\n");
    background.poll(&sink);
    assert_eq!(
        background.activity().total,
        2,
        "Activity accrued this session"
    );
    assert_eq!(background.activity().max_level, Some(Level::Error));

    // Activity is not persisted because it is not persistable: it is not part
    // of what `save` writes, and there is no path from it into the file.
    let written =
        fs::read_to_string(config.join("projects.json")).expect("the registry file exists on disk");
    for absent in [
        "total",
        "counts",
        "maxLevel",
        "activity",
        "EMERGENCY",
        "ERROR",
    ] {
        assert!(
            !written.contains(absent),
            "the persisted registry must not carry Activity: found {absent:?} in {written}"
        );
    }

    // --- restart -----------------------------------------------------------
    drop(background);
    let reloaded = project::load(&config).expect("load the registry");

    assert_eq!(reloaded.len(), 2, "both Projects survived the restart");
    let ids: Vec<_> = reloaded
        .iter()
        .map(|project| project.id().as_str().to_owned())
        .collect();
    assert!(ids.contains(&alpha.to_string_lossy().into_owned()));
    assert!(ids.contains(&beta.to_string_lossy().into_owned()));
    assert!(
        reloaded
            .iter()
            .all(|project| project.health() == &Health::Ok),
        "Health is re-derived on load, not trusted from the file"
    );

    // A fresh session's watcher starts with a quiet sidebar, not a backlog.
    let fresh = watcher(&beta, Mode::Background);
    assert_eq!(fresh.activity().total, 0);
    assert_eq!(fresh.activity().max_level, None);
    assert!(fresh.activity().counts.is_empty());

    // …and its first poll attaches at EOF, so what was already in the file is
    // not replayed into the badge either.
    let sink = Recorder::default();
    let mut fresh = fresh;
    fresh.poll(&sink);
    assert_eq!(fresh.activity().total, 0, "{:#?}", sink.activities());
    assert_eq!(fresh.offset(), fs::metadata(&log).expect("stat").len());
}

/* -------------------------------------------------------------------------- */
/* Soak — ADR 0002's bound holds over TIME                                     */
/* -------------------------------------------------------------------------- */

/// The existing `a_background_watcher_retains_no_entry_text` test proves the
/// bound across **one** large append. This proves it across **many hundreds of
/// poll cycles** with a write landing between every pair of them, which is the
/// shape the bound is actually claimed in: idle memory must not become a
/// function of how long the app has been open.
///
/// The assertion is deterministic — retained *structure*, not wall-clock RSS —
/// because an RSS reading is an allocator statistic, not a statement about what
/// this watcher is holding.
#[test]
fn soak_a_background_watcher_stays_bounded_over_many_poll_cycles() {
    const CYCLES: usize = 750;
    /// Every cycle writes one full 47-line exception plus three short Entries.
    const ENTRIES_PER_CYCLE: u64 = 4;

    let temp = TempDir::new("soak-background");
    let root = laravel_root(&temp, "noisy");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 00:00:00] local.INFO: seed\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Background);
    state.poll(&sink);

    // The high-water mark of everything this watcher holds, sampled after each
    // of the 750 polls.
    let mut peak = 0usize;
    let mut peak_at = 0usize;
    let mut peak_buckets = 0usize;

    for cycle in 0..CYCLES {
        append(
            &log,
            &format!("[2026-08-14 01:28:00] local.DEBUG: cache hit {cycle}\n"),
        );
        append(
            &log,
            &format!("[2026-08-14 01:28:01] local.WARNING: slow query {cycle}\n"),
        );
        append(&log, &trace_entry(45));
        append(
            &log,
            &format!("[2026-08-14 01:28:09] local.INFO: recovered {cycle}\n"),
        );
        state.poll(&sink);

        let retained = state.retained_state_bytes();
        if retained > peak {
            peak = retained;
            peak_at = cycle;
        }
        peak_buckets = peak_buckets.max(state.activity_bucket_count());
    }

    let grew_to = fs::metadata(&log).expect("stat").len();
    // Sanity: the soak really did push a lot of log through the watcher.
    assert!(
        grew_to > 4 * 1024 * 1024,
        "the file only reached {grew_to} bytes — the soak is not exercising anything"
    );

    // The bound. One open Entry, one held partial line, one file name, two id
    // bookmarks, and at most nine Level buckets — and nothing that scales with
    // `CYCLES` or with `grew_to`.
    assert!(
        peak < 16 * 1024,
        "a background watcher retained {peak} bytes at cycle {peak_at} after {grew_to} bytes of \
         log across {CYCLES} polls — ADR 0002's discard has eroded"
    );
    assert!(
        peak_buckets <= 9,
        "the Activity histogram grew to {peak_buckets} buckets; it is bounded by the 9 Levels"
    );

    // Counts are the only thing that grows, and they grow as integers.
    let total = state.activity().total;
    assert_eq!(
        total,
        CYCLES as u64 * ENTRIES_PER_CYCLE,
        "every Entry was counted exactly once across {CYCLES} polls"
    );
    assert_eq!(state.activity().max_level, Some(Level::Error));

    // And the watcher is still correct at the end, not merely small: it is at
    // EOF and still counting.
    assert_eq!(state.offset(), grew_to);
    assert!(
        sink.entries().is_empty(),
        "no Entry text ever left the watcher"
    );

    println!(
        "soak: {CYCLES} polls, {grew_to} bytes of log, {total} Entries counted, \
         peak retained state {peak} bytes (at cycle {peak_at}), {peak_buckets} Level buckets"
    );
}

/// The same shape for the *selected* watcher, which is allowed to hold one open
/// Entry but not a session's worth of them. The client-side 2000-Entry cap is
/// the other half of this and is pinned by `src/hooks/__tests__/streamBuffer.test.ts`.
#[test]
fn soak_a_selected_watcher_holds_only_the_open_entry() {
    const CYCLES: usize = 750;

    let temp = TempDir::new("soak-selected");
    let root = laravel_root(&temp, "noisy");
    let log = logs_dir(&root).join("laravel.log");
    append(&log, "[2026-08-14 00:00:00] local.INFO: seed\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    state.poll(&sink);

    let mut peak = 0usize;
    for cycle in 0..CYCLES {
        append(&log, &trace_entry(45));
        append(
            &log,
            &format!("[2026-08-14 01:28:09] local.INFO: recovered {cycle}\n"),
        );
        state.poll(&sink);
        // The recorder is the *client*, not the watcher; drained so the test
        // measures what the watcher holds rather than what the fake sink kept.
        sink.take();
        peak = peak.max(state.retained_state_bytes());
    }

    let grew_to = fs::metadata(&log).expect("stat").len();
    assert!(
        peak < 16 * 1024,
        "a selected watcher retained {peak} bytes after {grew_to} bytes across {CYCLES} polls"
    );
    println!(
        "soak(selected): {CYCLES} polls, {grew_to} bytes of log, peak retained state {peak} bytes"
    );
}

/* -------------------------------------------------------------------------- */
/* CONTEXT.md conformance                                                      */
/* -------------------------------------------------------------------------- */

/// `CONTEXT.md` says a **Session Record** spans any number of Breaks, and that a
/// Break is either cleared or rotated. Both kinds in one record, in order, with
/// every Entry still distinct.
#[test]
fn a_session_record_spans_both_kinds_of_break() {
    let temp = TempDir::new("session-record");
    let root = laravel_root(&temp, "api");
    let dir = logs_dir(&root);
    let today = dir.join("laravel-2026-08-14.log");
    append(&today, "[2026-08-14 01:00:00] local.INFO: one\n");

    let sink = Recorder::default();
    let mut state = watcher(&root, Mode::Selected);
    // `select_project`'s path: the opening window is the start of the record.
    let opening = state.open().expect("the opening window");
    append(&today, "[2026-08-14 01:00:01] local.INFO: two\n");
    state.poll(&sink);

    // Cleared…
    fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&today)
        .expect("truncate");
    append(&today, "[2026-08-14 01:00:02] local.INFO: three\n");
    state.poll(&sink);

    // …then rotated.
    let tomorrow = dir.join("laravel-2026-08-15.log");
    append(&tomorrow, "[2026-08-15 00:00:00] local.INFO: four\n");
    state.poll(&sink);

    let kinds: Vec<_> = sink.breaks().into_iter().map(|brk| brk.kind).collect();
    assert_eq!(kinds, vec![BreakKind::Cleared, BreakKind::Rotated]);

    // Every Entry in the record is still separately addressable across both.
    let record: Vec<StreamItem> = opening
        .into_iter()
        .chain(sink.events().into_iter().filter_map(|event| match event {
            crate::watcher::test_support::Emitted::Entry(entry) => Some(StreamItem::Entry(entry)),
            crate::watcher::test_support::Emitted::Break(brk) => Some(StreamItem::Break(brk)),
            _ => None,
        }))
        .collect();
    let ids: std::collections::BTreeSet<String> = record
        .iter()
        .map(|item| match item {
            StreamItem::Entry(entry) => entry.id.as_str().to_owned(),
            StreamItem::Break(brk) => brk.id.as_str().to_owned(),
        })
        .collect();
    let messages: std::collections::BTreeSet<&str> = record
        .iter()
        .filter_map(|item| match item {
            StreamItem::Entry(entry) => Some(entry.message.as_str()),
            StreamItem::Break(_) => None,
        })
        .collect();
    assert_eq!(
        messages,
        ["four", "one", "three", "two"].into_iter().collect(),
        "nothing above either Break was lost"
    );
    assert_eq!(
        ids.len(),
        6,
        "four Entries and two Breaks, all distinctly addressable: {ids:#?}"
    );
}
