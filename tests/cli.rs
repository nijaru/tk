//! CLI-level tests: help drift, diagnostics, end-to-end flows, and the
//! concurrency guarantees the store design is built around.
//!
//! The concurrency tests are the interesting ones. v1 appends events with
//! `O_APPEND` and takes the store lock only for operations that must see a
//! consistent store, so these tests assert both halves of that split: plain
//! appends proceed while another process holds the lock, and validation-
//! dependent operations wait for it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use predicates::prelude::PredicateBooleanExt;
use usage::RunWith as _;

use tk::cli::Cli as TkCli;

// --- Spec / help drift -----------------------------------------------------

#[test]
fn help_tree_snapshot() {
    let tree = usage::test::help_tree(TkCli::spec(), usage::test::Page::Long);
    insta::assert_snapshot!(tree);
}

#[test]
fn aliases_resolve_in_help() {
    // Users ask the way they type: alias paths must render the same page.
    let spec = TkCli::spec();
    let via_alias = usage::test::help(spec, &["ls"], usage::test::Page::Long);
    let via_name = usage::test::help(spec, &["list"], usage::test::Page::Long);
    assert_eq!(via_alias, via_name);
}

#[test]
fn diagnostics_read_like_users_see_them() {
    let words = usage::test::argv(["add", "Some title", "-p", "bogus"]);
    let cli = TkCli::parse_from(&words.words()).expect("priority is a string to the parser");
    let err = cli
        .command
        .run_with(dummy_ctx())
        .expect_err("bogus priority must fail");
    assert!(err.to_string().contains("invalid priority"), "{err:?}");
}

fn dummy_ctx() -> tk::cli::AppCtx {
    tk::cli::AppCtx {
        store: tk::store::Ctx {
            cwd: PathBuf::from("/nonexistent-tk-test"),
            root: PathBuf::from("/nonexistent-tk-test"),
            tasks_dir: PathBuf::from("/nonexistent-tk-test/.tasks"),
            exists: false,
            source: tk::store::StoreSource::Discovered,
            worktree: false,
        },
        json: false,
        color: false,
    }
}

// --- End-to-end flows ------------------------------------------------------

fn tk() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("tk").expect("tk binary")
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tk"))
}

fn init_project(dir: &tempfile::TempDir, project: &str) {
    tk().arg("-C")
        .arg(dir.path())
        .args(["init", "-P", project])
        .assert()
        .success()
        .stdout(predicates::str::contains("Initialized"));
}

/// Run tk in `dir` and return the raw output.
fn run_in(dir: &Path, args: &[&str]) -> std::process::Output {
    bin()
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn tk")
}

fn ok_in(dir: &Path, args: &[&str]) -> String {
    let out = run_in(dir, args);
    assert!(
        out.status.success(),
        "tk {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn store_dir(dir: &Path) -> PathBuf {
    dir.join(".tasks")
}

fn records_dir(dir: &Path) -> PathBuf {
    store_dir(dir).join("records")
}

/// Create a task and return `(alias, id)`.
fn add_task(dir: &Path, title: &str) -> (String, String) {
    let out = ok_in(dir, &["add", title]);
    // "Created task <alias> (<id>)"
    let rest = out
        .trim()
        .strip_prefix("Created task ")
        .expect("created task line");
    let (alias, id) = rest.split_once(" (").expect("alias and id");
    (alias.to_owned(), id.trim_end_matches(')').to_owned())
}

/// Parse a `--json` run as the envelope every command answers with.
fn envelope(dir: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    if !full.contains(&"--json") {
        full.push("--json");
    }
    serde_json::from_str(&ok_in(dir, &full)).expect("json envelope")
}

/// Parse a `--json` run, unwrapping the envelope's payload.
fn payload(dir: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = args.to_vec();
    if !full.contains(&"--json") {
        full.push("--json");
    }
    envelope(dir, &full)["data"].clone()
}

fn show_json(dir: &Path, id: &str) -> serde_json::Value {
    payload(dir, &["show", id])
}

fn list_json(dir: &Path, args: &[&str]) -> Vec<serde_json::Value> {
    let mut full = vec!["list", "-a"];
    full.extend_from_slice(args);
    payload(dir, &full).as_array().cloned().unwrap_or_default()
}

fn record_path(dir: &Path, id: &str) -> PathBuf {
    records_dir(dir).join(format!("{id}.jsonl"))
}

/// Every event line of a record, parsed.
fn read_events(dir: &Path, id: &str) -> Vec<serde_json::Value> {
    let raw = std::fs::read_to_string(record_path(dir, id)).expect("record file");
    raw.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("event line"))
        .collect()
}

/// Append an event the way a concurrent writer or a hand edit would.
fn append_event(dir: &Path, id: &str, op: &str, data: serde_json::Value) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(record_path(dir, id))
        .expect("open record");
    let line = serde_json::json!({
        "ts": "2026-01-10T00:00:00.000000000Z",
        "writer": "test-0000",
        "op": op,
        "data": data,
    });
    writeln!(file, "{line}").expect("append event");
}

/// Collapse miette's box drawing and line wrapping so diagnostics can be
/// matched by their words rather than their layout.
fn flatten(text: &str) -> String {
    text.replace(
        [
            '│', '┌', '┐', '└', '┘', '├', '┤', '─', '╰', '╭', '╯', '╮', '┬', '┴', '┼',
        ],
        " ",
    )
    .split_whitespace()
    .collect::<Vec<_>>()
    .join(" ")
}

#[test]
fn full_task_lifecycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let at = || {
        let mut c = tk();
        c.arg("-C").arg(dir.path());
        c
    };

    let (auth_alias, auth) = add_task(dir.path(), "Implement auth");
    let (tests_alias, _tests) = add_task(dir.path(), "Write tests");
    assert_eq!(auth_alias.len(), 4, "aliases are 4 characters");
    assert_eq!(auth.len(), 26, "IDs are ULIDs");

    at().args(["block", &tests_alias, &auth_alias])
        .assert()
        .success()
        .stdout(predicates::str::contains(&auth_alias));

    // Blocked task is not ready.
    at().arg("ready")
        .assert()
        .success()
        .stdout(predicates::str::contains("Implement auth"))
        .stdout(predicates::str::contains("Write tests").not());

    // Completing the blocker unblocks the dependent.
    at().args(["done", &auth_alias]).assert().success();
    at().arg("ready")
        .assert()
        .success()
        .stdout(predicates::str::contains("Write tests"));

    // Detail view renders the blocker as the alias the user typed.
    let detail = ok_in(dir.path(), &["show", &tests_alias]);
    assert!(detail.contains("(resolved)"), "{detail}");
    assert!(
        detail.contains(&format!("Blockers:    {auth_alias}")),
        "blockers must render as aliases: {detail}"
    );

    // Resolution works by alias, by full ID, and by ID prefix. The prefix has
    // to be long enough to clear the tasks created in the same millisecond.
    for reference in [auth_alias.clone(), auth.clone(), auth[..16].to_owned()] {
        ok_in(dir.path(), &["show", &reference]);
    }

    ok_in(dir.path(), &["check"]);
}

#[test]
fn a_record_is_an_append_only_event_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (alias, id) = add_task(dir.path(), "logged");

    ok_in(dir.path(), &["log", &alias, "first note"]);
    ok_in(dir.path(), &["edit", &alias, "-t", "renamed"]);
    ok_in(dir.path(), &["done", &alias]);

    let events = read_events(dir.path(), &id);
    let ops: Vec<&str> = events
        .iter()
        .map(|e| e["op"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ops, vec!["created", "log", "title", "status"], "{ops:?}");

    // Every event carries who wrote it and when.
    for event in &events {
        assert!(event["writer"].is_string(), "{event}");
        assert!(event["ts"].is_string(), "{event}");
    }
    // The first line is the complete initial state, so a record is
    // self-describing from the top.
    assert_eq!(events[0]["data"]["title"], "logged");
    assert_eq!(events[0]["data"]["alias"], alias);

    // Nothing was overwritten: the original title is still on disk.
    let raw = std::fs::read_to_string(record_path(dir.path(), &id)).expect("record");
    assert!(raw.contains("\"title\":\"logged\""), "{raw}");
    assert_eq!(show_json(dir.path(), &id)["title"], "renamed");
}

#[test]
fn ambiguous_and_missing_ids_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let out = run_in(dir.path(), &["show", "nope"]);
    assert!(!out.status.success());
    let err = flatten(&String::from_utf8_lossy(&out.stderr));
    assert!(err.contains("task not found"), "{err}");
}

// --- The store refuses a legacy layout instead of mangling it ---------------

#[test]
fn a_legacy_store_is_refused_not_mangled() {
    // The previous layout: config.json plus one JSON document per task.
    let dir = tempfile::tempdir().expect("tempdir");
    let tasks = store_dir(dir.path());
    std::fs::create_dir_all(&tasks).expect("mkdir");
    std::fs::write(
        tasks.join("config.json"),
        r#"{"version":1,"project":"demo","defaults":{"priority":3,"labels":[],"assignees":[]},"clean_after":14}"#,
    )
    .expect("config");
    let legacy = tasks.join("demo-old1.json");
    std::fs::write(
        &legacy,
        r#"{"project":"demo","ref":"old1","title":"Legacy task","status":"open",
            "priority":3,"created_at":"2026-01-10T12:00:00Z","updated_at":"2026-01-10T12:00:00Z"}"#,
    )
    .expect("task");

    for args in [vec!["list"], vec!["check"], vec!["show", "old1"]] {
        let out = run_in(dir.path(), &args);
        assert!(!out.status.success(), "tk {args:?} must refuse a v0 store");
        let err = flatten(&String::from_utf8_lossy(&out.stderr));
        assert!(err.contains("not a tk v1 store"), "tk {args:?}: {err}");
        assert!(err.contains("config.json"), "tk {args:?}: {err}");
        assert!(err.contains("migrate-v0.py"), "tk {args:?}: {err}");
    }

    // `init` must not paper over it either.
    let out = run_in(dir.path(), &["init", "-P", "demo"]);
    assert!(!out.status.success(), "init must refuse a legacy store");
    assert!(
        !tasks.join("store.json").exists(),
        "init wrote a v1 store over a legacy layout"
    );
    // And nothing was rewritten.
    let raw = std::fs::read_to_string(&legacy).expect("legacy task");
    assert!(raw.contains("Legacy task"), "{raw}");
}

#[test]
fn a_bare_directory_is_refused_with_a_clear_reason() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(store_dir(dir.path())).expect("mkdir");
    let out = run_in(dir.path(), &["list"]);
    assert!(!out.status.success());
    let err = flatten(&String::from_utf8_lossy(&out.stderr));
    assert!(err.contains("store.json is missing"), "{err}");
}

// --- Regressions: appends must not be lost without a lock -------------------

#[test]
fn concurrent_log_appends_are_not_lost() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "shared log");

    const WRITERS: usize = 8;
    let children: Vec<_> = (0..WRITERS)
        .map(|i| {
            bin()
                .arg("-C")
                .arg(dir.path())
                .args(["log", &alias, &format!("entry-{i}")])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn log")
        })
        .collect();

    for child in children {
        let out = child.wait_with_output().expect("wait");
        assert!(
            out.status.success(),
            "a concurrent log writer failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let task = show_json(dir.path(), &id);
    let logs = task["logs"].as_array().expect("logs");
    assert_eq!(
        logs.len(),
        WRITERS,
        "every successful append must be retained: {logs:?}"
    );
    let mut seen: Vec<String> = logs
        .iter()
        .map(|l| l["msg"].as_str().unwrap_or_default().to_owned())
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(seen.len(), WRITERS, "duplicate or missing: {seen:?}");
}

#[test]
fn concurrent_label_deltas_are_not_lost() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "shared labels");

    const WRITERS: usize = 8;
    let children: Vec<_> = (0..WRITERS)
        .map(|i| {
            bin()
                .arg("-C")
                .arg(dir.path())
                .args(["edit", &alias, "-l", &format!("+label{i}")])
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .spawn()
                .expect("spawn edit")
        })
        .collect();

    for child in children {
        let out = child.wait_with_output().expect("wait");
        assert!(
            out.status.success(),
            "a concurrent label edit failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    let task = show_json(dir.path(), &id);
    let labels: Vec<String> = task["labels"]
        .as_array()
        .expect("labels")
        .iter()
        .map(|l| l.as_str().unwrap_or_default().to_owned())
        .collect();
    for i in 0..WRITERS {
        assert!(
            labels.contains(&format!("label{i}")),
            "label{i} was lost: {labels:?}"
        );
    }
}

#[test]
fn lock_free_appends_do_not_wait_for_the_store_lock() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (a_alias, _) = add_task(dir.path(), "first");
    let (b_alias, _) = add_task(dir.path(), "second");

    let ctx = tk::store::Ctx::at_tasks_dir(store_dir(dir.path()));
    ctx.require().expect("store exists");
    let guard = ctx.lock_store().expect("take store lock");

    // A log append is last-writer-wins and takes no lock, so it must complete
    // even while this process holds the lock.
    let out = bin()
        .arg("-C")
        .arg(dir.path())
        .args(["log", &a_alias, "not blocked"])
        .output()
        .expect("spawn log");
    assert!(
        out.status.success(),
        "a lock-free append must not wait for the store lock: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // A graph change validates other records, so it must wait.
    let mut blocked = bin()
        .arg("-C")
        .arg(dir.path())
        .args(["block", &b_alias, &a_alias])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn block");
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        blocked.try_wait().expect("try_wait").is_none(),
        "a lock-requiring operation ran while the store lock was held"
    );

    drop(guard);
    assert!(blocked.wait().expect("wait").success());
}

#[test]
fn concurrent_opposite_blocks_never_create_a_cycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (a_alias, _) = add_task(dir.path(), "alpha");
    let (b_alias, _) = add_task(dir.path(), "beta");

    // Each writer validates the graph under the same lock as its write, so
    // whichever loses the race must observe the other's edge.
    let spawn = |id: &str, blocker: &str| {
        bin()
            .arg("-C")
            .arg(dir.path())
            .args(["block", id, blocker])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn block")
    };
    let first = spawn(&b_alias, &a_alias);
    let second = spawn(&a_alias, &b_alias);
    let outputs: Vec<_> = [first, second]
        .into_iter()
        .map(|c| c.wait_with_output().expect("wait"))
        .collect();

    let failures: Vec<String> = outputs
        .iter()
        .filter(|o| !o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stderr).into_owned())
        .collect();
    assert_eq!(failures.len(), 1, "exactly one direction must lose");
    assert!(
        failures[0].contains("circular dependency"),
        "{}",
        failures[0]
    );

    ok_in(dir.path(), &["check"]);
}

#[test]
fn check_reports_a_dependency_cycle_on_disk() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (_, a) = add_task(dir.path(), "alpha");
    let (_, b) = add_task(dir.path(), "beta");

    // A cycle written directly, the way a hand edit or a broken tool would.
    append_event(dir.path(), &a, "block.add", serde_json::json!([b]));
    append_event(dir.path(), &b, "block.add", serde_json::json!([a]));

    let out = run_in(dir.path(), &["check"]);
    assert!(!out.status.success(), "a cycle must fail the check");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("dependency cycle"), "{text}");
}

// --- Regressions: read side must not destroy state ---------------------------

#[test]
fn show_does_not_delete_a_missing_blocker_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (blocker_alias, blocker) = add_task(dir.path(), "prerequisite");
    let (dependent_alias, dependent) = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent_alias, &blocker_alias]);

    // The prerequisite disappears (purged, hand-deleted, or synced away).
    std::fs::remove_file(record_path(dir.path(), &blocker)).expect("remove prerequisite");

    let before = std::fs::read_to_string(record_path(dir.path(), &dependent)).expect("record");
    let shown = show_json(dir.path(), &dependent);
    assert_eq!(
        shown["unresolved_blockers"],
        serde_json::json!([blocker]),
        "the query must report the unresolved blocker: {shown}"
    );
    assert!(shown["blocked_by_incomplete"].as_bool().unwrap_or(false));
    assert!(
        shown["issues"].as_array().is_some_and(|i| !i.is_empty()),
        "show must report the issue: {shown}"
    );

    let after = std::fs::read_to_string(record_path(dir.path(), &dependent)).expect("record");
    assert_eq!(before, after, "a read-only query rewrote the record");

    // A missing prerequisite is unresolved, never "ready".
    let ready = ok_in(dir.path(), &["ready"]);
    assert!(
        !ready.contains(&dependent_alias),
        "dependent must not be ready: {ready}"
    );
}

#[test]
fn check_exits_nonzero_on_findings() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (blocker_alias, blocker) = add_task(dir.path(), "prerequisite");
    let (dependent_alias, _) = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent_alias, &blocker_alias]);
    std::fs::remove_file(record_path(dir.path(), &blocker)).expect("remove prerequisite");

    let human = run_in(dir.path(), &["check"]);
    assert!(!human.status.success(), "check must fail on findings");
    let text = String::from_utf8_lossy(&human.stdout).into_owned();
    assert!(text.contains("missing task"), "{text}");

    let json = run_in(dir.path(), &["check", "--json"]);
    assert!(!json.status.success(), "check --json must fail on findings");
    let report: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("check --json payload");
    // A failing command still answers in the envelope, naming the failure.
    assert_eq!(report["ok"], serde_json::json!(false));
    assert_eq!(report["error_code"], "check_failed");
    assert_eq!(report["command"], "check");
    assert_eq!(report["data"]["ok"], serde_json::json!(false));
    assert!(
        report["data"]["issues"]
            .as_array()
            .is_some_and(|i| !i.is_empty()),
        "{report}"
    );
}

// --- Recovery ---------------------------------------------------------------

#[test]
fn recover_drops_only_the_torn_line() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "interrupted");

    // Simulate a write that was killed before its newline landed.
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(record_path(dir.path(), &id))
        .expect("open record");
    file.write_all(b"{\"ts\":\"2026-01-11T00:00:00Z\",\"writer\":\"dead-0000\",\"op\":\"log\",")
        .expect("partial write");
    drop(file);

    // The task is still readable: the partial line is not applied.
    let shown = show_json(dir.path(), &id);
    assert_eq!(shown["title"], "interrupted");

    let report = payload(dir.path(), &["recover", &alias, "--dry-run"]);
    assert_eq!(report["repaired"].as_array().map(Vec::len), Some(1));
    assert!(report["bytes_dropped"].as_u64().unwrap_or(0) > 0);
    assert_eq!(report["dry_run"], serde_json::json!(true));
    // A dry run must change nothing.
    let raw = std::fs::read_to_string(record_path(dir.path(), &id)).expect("record");
    assert!(!raw.ends_with('\n'), "dry run rewrote the record");

    ok_in(dir.path(), &["recover", &alias]);
    let repaired = std::fs::read_to_string(record_path(dir.path(), &id)).expect("record");
    assert!(repaired.ends_with('\n'), "recovery must leave a clean tail");
    assert_eq!(
        read_events(dir.path(), &id).len(),
        1,
        "only created remains"
    );
    ok_in(dir.path(), &["check"]);
}

// --- Deletion, archival, retention ------------------------------------------

#[test]
fn purge_refuses_while_referenced_and_scrubs_on_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (blocker_alias, blocker) = add_task(dir.path(), "prerequisite");
    let (dependent_alias, dependent) = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent_alias, &blocker_alias]);

    let out = run_in(dir.path(), &["purge", &blocker_alias, "-f"]);
    assert!(
        !out.status.success(),
        "purging a referenced task must be refused"
    );
    let err = flatten(&String::from_utf8_lossy(&out.stderr));
    assert!(err.contains("referenced by"), "{err}");
    assert!(err.contains("--scrub"), "{err}");
    assert!(record_path(dir.path(), &blocker).exists());

    let report = payload(dir.path(), &["purge", &blocker_alias, "-f", "--scrub"]);
    assert_eq!(report["references_scrubbed"], serde_json::json!(1));
    assert!(!record_path(dir.path(), &blocker).exists());
    assert_eq!(
        show_json(dir.path(), &dependent)["blocked_by"],
        serde_json::json!([])
    );
    ok_in(dir.path(), &["check"]);
}

#[test]
fn purge_never_deletes_unattended_without_force() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "keep me");

    // stdin is not a terminal here, so the guard must refuse rather than prompt.
    let out = run_in(dir.path(), &["purge", &alias]);
    assert!(!out.status.success(), "must not delete without -f");
    let err = flatten(&String::from_utf8_lossy(&out.stderr));
    assert!(err.contains("without -f"), "{err}");
    assert!(record_path(dir.path(), &id).exists());

    ok_in(dir.path(), &["purge", &alias, "-f"]);
    assert!(!record_path(dir.path(), &id).exists());
}

#[test]
fn archive_requires_a_terminal_status_and_hides_from_active_views() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (open_alias, _) = add_task(dir.path(), "still open");
    let out = run_in(dir.path(), &["archive", &open_alias]);
    assert!(!out.status.success(), "an open task must not archive");

    let (done_alias, done) = add_task(dir.path(), "finished");
    ok_in(dir.path(), &["done", &done_alias]);
    ok_in(dir.path(), &["archive", &done_alias]);
    assert!(show_json(dir.path(), &done)["archived_at"].is_string());

    // Other tasks may still reference an archived task, and resolution works.
    let (dependent_alias, _) = add_task(dir.path(), "depends on archived");
    ok_in(dir.path(), &["block", &dependent_alias, &done_alias]);
    let ready = ok_in(dir.path(), &["ready"]);
    assert!(ready.contains(&dependent_alias), "{ready}");
    ok_in(dir.path(), &["check"]);

    let listed = ok_in(dir.path(), &["list", "-s", "done"]);
    assert!(
        !listed.contains(&done_alias),
        "archived task in the default list: {listed}"
    );
    let archived = ok_in(dir.path(), &["list", "--archived"]);
    assert!(archived.contains(&done_alias), "{archived}");
    assert!(archived.contains("[archived]"), "{archived}");

    ok_in(dir.path(), &["unarchive", &done_alias]);
    let listed = ok_in(dir.path(), &["list", "-s", "done"]);
    assert!(listed.contains(&done_alias), "{listed}");
}

#[test]
fn clean_archives_instead_of_deleting() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "old work");
    ok_in(dir.path(), &["done", &alias]);

    let out = ok_in(dir.path(), &["clean", "--older-than", "0"]);
    assert!(out.contains("Archived 1"), "{out}");
    assert!(show_json(dir.path(), &id)["archived_at"].is_string());
    assert!(
        record_path(dir.path(), &id).exists(),
        "clean deleted the record instead of archiving it"
    );

    let out = ok_in(dir.path(), &["clean", "--older-than", "0", "--purge"]);
    assert!(out.contains("Purged 1"), "{out}");
    assert!(!record_path(dir.path(), &id).exists());
}

#[test]
fn moving_a_task_keeps_its_identity_and_references() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (blocker_alias, blocker) = add_task(dir.path(), "relocated");
    let (dependent_alias, _) = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent_alias, &blocker_alias]);

    ok_in(dir.path(), &["mv", &blocker_alias, "other"]);
    let moved = show_json(dir.path(), &blocker);
    assert_eq!(moved["project"], "other");
    assert_eq!(moved["alias"], serde_json::json!(blocker_alias));
    assert_eq!(
        moved["id"],
        serde_json::json!(blocker),
        "identity must not move"
    );

    // The reference was never rewritten, because it never pointed at a name.
    ok_in(dir.path(), &["check"]);
    // And a project rename is equally free of reference churn.
    ok_in(
        dir.path(),
        &["config", "project", "rename", "other", "third"],
    );
    ok_in(dir.path(), &["check"]);
    assert_eq!(show_json(dir.path(), &blocker)["project"], "third");
}

// --- Related edges: task-to-task, not documents -----------------------------

#[test]
fn relate_records_a_non_blocking_edge() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (a_alias, a) = add_task(dir.path(), "alpha");
    let (b_alias, b) = add_task(dir.path(), "beta");

    ok_in(dir.path(), &["relate", &a_alias, &b_alias]);
    assert_eq!(show_json(dir.path(), &a)["related"], serde_json::json!([b]));

    // A relation is "see also", not a constraint: both stay ready.
    let ready = ok_in(dir.path(), &["ready"]);
    assert!(
        ready.contains(&a_alias) && ready.contains(&b_alias),
        "{ready}"
    );

    // Human output names the relation by alias.
    let detail = ok_in(dir.path(), &["show", &a_alias]);
    assert!(
        detail.contains(&format!("Related:     {b_alias}")),
        "{detail}"
    );

    // It is one-way: nothing was written to the other record.
    assert_eq!(show_json(dir.path(), &b)["related"], serde_json::json!([]));

    // Adding it twice does not duplicate it.
    ok_in(dir.path(), &["relate", &a_alias, &b_alias]);
    assert_eq!(
        show_json(dir.path(), &a)["related"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    ok_in(dir.path(), &["unrelate", &a_alias, &b_alias]);
    assert_eq!(show_json(dir.path(), &a)["related"], serde_json::json!([]));
    ok_in(dir.path(), &["check"]);
}

#[test]
fn purge_refuses_while_a_relation_points_at_the_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (a_alias, a) = add_task(dir.path(), "alpha");
    let (b_alias, b) = add_task(dir.path(), "beta");
    ok_in(dir.path(), &["relate", &a_alias, &b_alias]);

    let out = run_in(dir.path(), &["purge", &b_alias, "-f"]);
    assert!(!out.status.success(), "a relation must hold the record");
    assert!(record_path(dir.path(), &b).exists());

    ok_in(dir.path(), &["purge", &b_alias, "-f", "--scrub"]);
    assert_eq!(show_json(dir.path(), &a)["related"], serde_json::json!([]));
    ok_in(dir.path(), &["check"]);
}

#[test]
fn a_relation_to_a_missing_task_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (a_alias, _) = add_task(dir.path(), "alpha");
    let (b_alias, b) = add_task(dir.path(), "beta");
    ok_in(dir.path(), &["relate", &a_alias, &b_alias]);

    std::fs::remove_file(record_path(dir.path(), &b)).expect("remove related");
    let out = run_in(dir.path(), &["check"]);
    assert!(
        !out.status.success(),
        "a dangling relation must be reported"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("related to missing task"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
}

// --- Stale intent -----------------------------------------------------------

#[test]
fn edit_rejects_a_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "concurrent intent");

    let rev = show_json(dir.path(), &id)["rev"]
        .as_str()
        .expect("rev")
        .to_owned();
    ok_in(
        dir.path(),
        &["edit", &alias, "-t", "first", "--if-rev", &rev],
    );

    // The revision is from before the first edit: applying it would silently
    // overwrite a change this writer never saw.
    let stale = run_in(
        dir.path(),
        &["edit", &alias, "-t", "second", "--if-rev", &rev],
    );
    assert!(!stale.status.success(), "stale edit must be rejected");
    let err = flatten(&String::from_utf8_lossy(&stale.stderr));
    assert!(err.contains("changed since it was read"), "{err}");
    assert_eq!(show_json(dir.path(), &id)["title"], "first");

    // The current revision still works, including for a multi-field edit.
    let fresh = show_json(dir.path(), &id)["rev"]
        .as_str()
        .expect("rev")
        .to_owned();
    ok_in(
        dir.path(),
        &["edit", &alias, "-t", "third", "-p", "1", "--if-rev", &fresh],
    );
    let after = show_json(dir.path(), &id);
    assert_eq!(after["title"], "third");
    assert_eq!(after["priority"], serde_json::json!(1));
}

#[test]
fn a_revision_changes_when_another_writer_appends() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "watched");

    let first = show_json(dir.path(), &id)["rev"]
        .as_str()
        .unwrap()
        .to_owned();
    ok_in(dir.path(), &["log", &alias, "someone else moved it"]);
    let second = show_json(dir.path(), &id)["rev"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first, second, "an append must move the revision");

    // The old token is now stale, and says so.
    let out = run_in(
        dir.path(),
        &["checkpoint", &alias, "too late", "--if-rev", &first],
    );
    assert!(!out.status.success());
    assert_eq!(
        show_json(dir.path(), &id)["checkpoint"],
        serde_json::Value::Null
    );
}

// --- Store selection --------------------------------------------------------

#[test]
fn add_never_bootstraps_a_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = run_in(dir.path(), &["add", "should not exist"]);
    assert!(!out.status.success(), "add must not create a store");
    let err = flatten(&String::from_utf8_lossy(&out.stderr));
    assert!(err.contains("no .tasks/ directory found"), "{err}");
    assert!(!store_dir(dir.path()).exists());
}

#[test]
fn missing_explicit_store_fails_instead_of_creating() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("central").join(".tasks");

    let out = bin()
        .arg("--tasks-dir")
        .arg(&store)
        .args(["add", "should not exist"])
        .output()
        .expect("spawn tk");
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("task store not found"), "{err}");
    assert!(!store.exists(), "tk created an explicitly designated store");

    // init is the deliberate exception.
    let out = bin()
        .arg("--tasks-dir")
        .arg(&store)
        .args(["init", "-P", "central"])
        .output()
        .expect("spawn tk");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(store.join("store.json").exists());

    let out = bin()
        .arg("--tasks-dir")
        .arg(&store)
        .args(["add", "now it works"])
        .output()
        .expect("spawn tk");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn explicit_missing_store_fails_on_reads_too() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = dir.path().join("absent").join(".tasks");
    for args in [
        vec!["list"],
        vec!["ready"],
        vec!["config", "show"],
        vec!["check"],
    ] {
        let out = bin()
            .arg("--tasks-dir")
            .arg(&store)
            .args(&args)
            .output()
            .expect("spawn tk");
        assert!(
            !out.status.success(),
            "tk {args:?} must fail on an explicit missing store"
        );
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("task store not found"),
            "tk {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn linked_worktree_does_not_grow_its_own_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A linked worktree (or submodule) has `.git` as a file pointing elsewhere.
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: /elsewhere/.git/worktrees/x\n",
    )
    .expect("gitdir file");

    let out = run_in(dir.path(), &["add", "shadow store"]);
    assert!(!out.status.success(), "add must fail without a store");
    assert!(
        !store_dir(dir.path()).exists(),
        "tk created a worktree-local store"
    );

    // Deliberate creation still works here.
    ok_in(dir.path(), &["init", "-P", "demo"]);
    assert!(store_dir(dir.path()).exists());
}

#[test]
fn path_reports_the_selected_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);

    let discovered = envelope(dir.path(), &["path", "--json"]);
    let value = &discovered["data"];
    assert_eq!(discovered["ok"], serde_json::json!(true));
    assert_eq!(discovered["command"], "path");
    assert_eq!(value["source"], "discovered");
    assert_eq!(value["exists"], serde_json::json!(true));
    assert_eq!(
        value["tasks_dir"].as_str().expect("tasks_dir"),
        store_dir(dir.path()).to_string_lossy()
    );

    let store = store_dir(dir.path());
    let out = bin()
        .arg("--tasks-dir")
        .arg(&store)
        .args(["path", "--json"])
        .output()
        .expect("spawn tk");
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("path --json");
    assert_eq!(value["data"]["source"], "flag");
}

// --- Task detail: checkpoint, links, acceptance, evidence --------------------

#[test]
fn checkpoint_is_replaced_not_appended() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "checkpointed");

    ok_in(dir.path(), &["checkpoint", &alias, "first pass done"]);
    ok_in(dir.path(), &["log", &alias, "historical note"]);
    ok_in(
        dir.path(),
        &["checkpoint", &alias, "second pass: blocked on review"],
    );

    let shown = show_json(dir.path(), &id);
    assert_eq!(shown["checkpoint"], "second pass: blocked on review");
    assert_eq!(shown["logs"].as_array().expect("logs").len(), 1);

    let out = ok_in(dir.path(), &["checkpoint", &alias]);
    assert!(out.contains("second pass: blocked on review"), "{out}");

    ok_in(dir.path(), &["checkpoint", &alias, "--clear"]);
    assert_eq!(
        show_json(dir.path(), &id)["checkpoint"],
        serde_json::Value::Null
    );
}

#[test]
fn links_acceptance_and_evidence_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "documented");
    let reference = "agent-context/projects/x/research/y.md";

    ok_in(dir.path(), &["link", &alias, reference]);
    ok_in(dir.path(), &["accept", &alias, "parity test passes"]);
    ok_in(dir.path(), &["accept", &alias, "docs updated"]);
    ok_in(
        dir.path(),
        &["evidence", &alias, "cargo test --all-targets"],
    );

    let shown = show_json(dir.path(), &id);
    assert_eq!(shown["links"], serde_json::json!([reference]));
    assert_eq!(
        shown["acceptance"],
        serde_json::json!(["parity test passes", "docs updated"])
    );
    assert_eq!(
        shown["evidence"],
        serde_json::json!(["cargo test --all-targets"])
    );

    // Adding the same value twice does not duplicate it.
    ok_in(dir.path(), &["link", &alias, reference]);
    assert_eq!(
        show_json(dir.path(), &id)["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let detail = ok_in(dir.path(), &["show", &alias]);
    assert!(detail.contains(reference), "{detail}");
    assert!(detail.contains("parity test passes"), "{detail}");

    ok_in(dir.path(), &["accept", &alias, "--remove", "docs updated"]);
    assert_eq!(
        show_json(dir.path(), &id)["acceptance"],
        serde_json::json!(["parity test passes"])
    );
    ok_in(dir.path(), &["unlink", &alias, reference]);
    assert_eq!(show_json(dir.path(), &id)["links"], serde_json::json!([]));
    ok_in(dir.path(), &["evidence", &alias, "--clear"]);
    assert_eq!(
        show_json(dir.path(), &id)["evidence"],
        serde_json::json!([])
    );
}

#[test]
fn labels_replace_and_delta_agree() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let (alias, id) = add_task(dir.path(), "labelled");

    ok_in(dir.path(), &["edit", &alias, "-l", "alpha,beta"]);
    assert_eq!(
        show_json(dir.path(), &id)["labels"],
        serde_json::json!(["alpha", "beta"])
    );
    ok_in(dir.path(), &["edit", &alias, "-l", "+gamma"]);
    assert_eq!(
        show_json(dir.path(), &id)["labels"],
        serde_json::json!(["alpha", "beta", "gamma"])
    );
    ok_in(dir.path(), &["edit", &alias, "--remove-label", "beta"]);
    assert_eq!(
        show_json(dir.path(), &id)["labels"],
        serde_json::json!(["alpha", "gamma"])
    );
    // A bare value replaces the whole set.
    ok_in(dir.path(), &["edit", &alias, "-l", "only"]);
    assert_eq!(
        show_json(dir.path(), &id)["labels"],
        serde_json::json!(["only"])
    );

    // Filtering still finds it.
    let found = list_json(dir.path(), &["-l", "only"]);
    assert_eq!(found.len(), 1);
}

// --- One envelope for every command -----------------------------------------

/// Every `--json` run answers with the same keys, whatever the command.
fn assert_envelope_shape(value: &serde_json::Value, command: &str) {
    let mut keys: Vec<&str> = value
        .as_object()
        .unwrap_or_else(|| panic!("not an object: {value}"))
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(
        keys,
        vec!["command", "data", "error_code", "issues", "ok", "rev"],
        "{value}"
    );
    assert_eq!(value["command"], command, "{value}");
}

#[test]
fn every_command_answers_in_the_same_envelope() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (alias, _id) = add_task(dir.path(), "enveloped");

    for (command, args) in [
        ("add", vec!["add", "another"]),
        ("list", vec!["list"]),
        ("ready", vec!["ready"]),
        ("show", vec!["show", &alias]),
        ("log", vec!["log", &alias, "note"]),
        ("checkpoint", vec!["checkpoint", &alias, "mid"]),
        ("link", vec!["link", &alias, "docs/x.md"]),
        ("accept", vec!["accept", &alias, "works"]),
        ("evidence", vec!["evidence", &alias, "cargo test"]),
        ("check", vec!["check"]),
        ("path", vec!["path"]),
        ("config", vec!["config", "show"]),
    ] {
        let args = args.iter().map(|a| a.as_ref()).collect::<Vec<&str>>();
        let value = envelope(dir.path(), &args);
        assert_envelope_shape(&value, command);
        assert_eq!(value["ok"], serde_json::json!(true), "tk {args:?}: {value}");
    }

    // A relation to itself is rejected — and the failure is still an envelope.
    let out = run_in(dir.path(), &["relate", &alias, &alias, "--json"]);
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_envelope_shape(&value, "relate");
    assert_eq!(value["ok"], serde_json::json!(false));
}

#[test]
fn json_failures_name_their_kind() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (alias, id) = add_task(dir.path(), "watched");

    // Not found.
    let out = run_in(dir.path(), &["show", "nope", "--json"]);
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_eq!(value["ok"], serde_json::json!(false));
    assert_eq!(value["error_code"], "not_found", "{value}");
    assert!(
        value["issues"][0]
            .as_str()
            .is_some_and(|m| m.contains("task not found")),
        "the envelope carries the message too: {value}"
    );

    // A stale revision is its own kind, so a caller can retry rather than
    // conclude the task does not exist.
    let rev = show_json(dir.path(), &id)["rev"]
        .as_str()
        .unwrap()
        .to_owned();
    ok_in(dir.path(), &["log", &alias, "moved on"]);
    let out = run_in(
        dir.path(),
        &["checkpoint", &alias, "late", "--if-rev", &rev, "--json"],
    );
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_eq!(value["error_code"], "stale_revision", "{value}");
}

#[test]
fn a_legacy_store_failure_says_which_kind_it_is() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(store_dir(dir.path())).expect("mkdir");
    std::fs::write(
        store_dir(dir.path()).join("config.json"),
        r#"{"version":1,"project":"demo"}"#,
    )
    .expect("config");

    let out = run_in(dir.path(), &["list", "--json"]);
    assert!(!out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_eq!(value["error_code"], "not_a_v1_store", "{value}");
    assert!(
        value["issues"][0]
            .as_str()
            .is_some_and(|m| m.contains("migrate-v0.py")),
        "{value}"
    );
}

// --- Batch application ------------------------------------------------------

/// Run `tk apply` with a request body on stdin.
fn apply_in(dir: &Path, body: &str, extra: &[&str]) -> std::process::Output {
    let mut child = bin()
        .arg("-C")
        .arg(dir)
        .arg("apply")
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn apply");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(body.as_bytes())
        .expect("write batch");
    child.wait_with_output().expect("wait")
}

fn apply_ok(dir: &Path, body: &str) -> serde_json::Value {
    let out = apply_in(dir, body, &["--json"]);
    assert!(
        out.status.success(),
        "apply failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("apply envelope")
}

#[test]
fn apply_runs_a_whole_change_in_one_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (a_alias, a_id) = add_task(dir.path(), "alpha");
    let (b_alias, b_id) = add_task(dir.path(), "beta");

    let body = serde_json::json!({"intents": [
        {"op": "add", "title": "gamma", "labels": ["batched"]},
        {"op": "block", "id": a_alias, "blocker": b_alias},
        {"op": "checkpoint", "id": a_alias, "text": "in flight"},
        {"op": "log", "id": a_alias, "msg": "batched note"},
        {"op": "status", "id": a_alias, "status": "active"},
    ]})
    .to_string();

    let value = apply_ok(dir.path(), &body);
    assert_eq!(value["command"], "apply");
    assert_eq!(value["data"]["dry_run"], serde_json::json!(false));
    assert_eq!(value["data"]["applied"].as_array().unwrap().len(), 5);

    // Every intent landed, and five invocations became one.
    let a = show_json(dir.path(), &a_id);
    assert_eq!(a["status"], "active");
    assert_eq!(a["checkpoint"], "in flight");
    assert_eq!(a["blocked_by"], serde_json::json!([b_id]));
    assert_eq!(a["logs"][0]["msg"], "batched note");
    assert_eq!(list_json(dir.path(), &[]).len(), 3);
    ok_in(dir.path(), &["check"]);
}

#[test]
fn apply_validates_the_whole_batch_before_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    add_task(dir.path(), "existing");

    // The second intent cannot resolve, so the first must not be written.
    let body = serde_json::json!({"intents": [
        {"op": "add", "title": "should not exist"},
        {"op": "log", "id": "zzzz", "msg": "nowhere"},
    ]})
    .to_string();
    let out = apply_in(dir.path(), &body, &[]);
    assert!(!out.status.success(), "a bad batch must be rejected");

    let tasks = list_json(dir.path(), &[]);
    assert_eq!(tasks.len(), 1, "nothing may be written: {tasks:?}");
}

#[test]
fn apply_rejects_a_cycle_whole() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (a_alias, a_id) = add_task(dir.path(), "alpha");
    let (b_alias, b_id) = add_task(dir.path(), "beta");

    let body = serde_json::json!({"intents": [
        {"op": "block", "id": a_alias, "blocker": b_alias},
        {"op": "block", "id": b_alias, "blocker": a_alias},
    ]})
    .to_string();
    let out = apply_in(dir.path(), &body, &[]);
    assert!(!out.status.success(), "a cyclic batch must be rejected");

    // Neither edge was written: a rejected batch leaves no half-graph.
    assert_eq!(
        show_json(dir.path(), &a_id)["blocked_by"],
        serde_json::json!([])
    );
    assert_eq!(
        show_json(dir.path(), &b_id)["blocked_by"],
        serde_json::json!([])
    );
}

#[test]
fn apply_dry_run_reports_without_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (alias, id) = add_task(dir.path(), "untouched");

    let body = serde_json::json!({"intents": [
        {"op": "add", "title": "planned"},
        {"op": "log", "id": alias, "msg": "planned note"},
    ]})
    .to_string();
    let out = apply_in(dir.path(), &body, &["--json", "--dry-run"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).expect("envelope");
    assert_eq!(value["data"]["dry_run"], serde_json::json!(true), "{value}");
    assert_eq!(value["data"]["applied"].as_array().unwrap().len(), 2);

    assert_eq!(
        list_json(dir.path(), &[]).len(),
        1,
        "a dry run wrote a task"
    );
    assert_eq!(show_json(dir.path(), &id)["logs"], serde_json::json!([]));
}

#[test]
fn apply_rejects_a_misspelled_intent_field() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (alias, id) = add_task(dir.path(), "typo target");

    // `message` is not `msg`: silently doing nothing would hide the mistake.
    let body = serde_json::json!({"intents": [
        {"op": "log", "id": alias, "message": "typo"},
    ]})
    .to_string();
    let out = apply_in(dir.path(), &body, &[]);
    assert!(!out.status.success(), "a misspelled field must fail");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("message"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(show_json(dir.path(), &id)["logs"], serde_json::json!([]));
}

#[test]
fn apply_honours_a_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let (alias, id) = add_task(dir.path(), "contended");
    let rev = show_json(dir.path(), &id)["rev"]
        .as_str()
        .unwrap()
        .to_owned();

    // Someone else moves the record first.
    ok_in(dir.path(), &["log", &alias, "another writer"]);

    let body = serde_json::json!({"intents": [
        {"op": "checkpoint", "id": alias, "text": "late", "if_rev": rev},
    ]})
    .to_string();
    let out = apply_in(dir.path(), &body, &[]);
    assert!(!out.status.success(), "a stale intent must be rejected");
    assert_eq!(
        show_json(dir.path(), &id)["checkpoint"],
        serde_json::Value::Null
    );
}

// --- Lock guard for external sync -------------------------------------------

#[cfg(unix)]
#[test]
fn lock_runs_a_command_and_propagates_its_status() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);

    let marker = dir.path().join("marker");
    let out = bin()
        .arg("-C")
        .arg(dir.path())
        .args([
            "lock",
            "--",
            "sh",
            "-c",
            &format!("touch {}", marker.display()),
        ])
        .output()
        .expect("spawn tk lock");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(marker.exists(), "child command did not run");

    let out = bin()
        .arg("-C")
        .arg(dir.path())
        .args(["lock", "--", "sh", "-c", "exit 7"])
        .output()
        .expect("spawn tk lock");
    assert_eq!(out.status.code(), Some(7), "child status must propagate");
}

#[cfg(unix)]
#[test]
fn lock_scan_guards_every_store_under_a_root() {
    let root = tempfile::tempdir().expect("tempdir");
    for name in ["a", "b"] {
        let project = root.path().join(name);
        std::fs::create_dir_all(&project).expect("mkdir");
        ok_in(&project, &["init", "-P", name]);
    }

    let ctx = tk::store::Ctx::at_tasks_dir(root.path().join("a").join(".tasks"));
    let guard = ctx.lock_store().expect("lock first store");

    let mut child = bin()
        .args(["lock", "--scan"])
        .arg(root.path())
        .args(["--", "sh", "-c", "exit 0"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn tk lock --scan");
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "scan lock ignored a store that was already locked"
    );
    drop(guard);
    assert!(child.wait().expect("wait").success());
}
