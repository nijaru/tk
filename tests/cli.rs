//! CLI-level tests: help drift, diagnostics, end-to-end flows, and the
//! concurrency/store-integrity regressions that motivated the mutation guard.

use std::path::Path;
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
            cwd: std::path::PathBuf::from("/nonexistent-tk-test"),
            root: std::path::PathBuf::from("/nonexistent-tk-test"),
            tasks_dir: std::path::PathBuf::from("/nonexistent-tk-test/.tasks"),
            exists: false,
            source: tk::store::StoreSource::Discovered,
            worktree: false,
        },
        json: false,
        color: false,
    }
}

// --- End-to-end flows --------------------------------------------------------

fn tk() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("tk").expect("tk binary")
}

fn init_project(dir: &tempfile::TempDir, project: &str) {
    tk().arg("-C")
        .arg(dir.path())
        .args(["init", "-P", project])
        .assert()
        .success()
        .stdout(predicates::str::contains("Initialized"));
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

    at().args(["add", "Implement auth", "-p", "1"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Created task demo-"));
    at().args(["add", "Write tests", "-p", "2"])
        .assert()
        .success();

    // Ref-suffix resolution: grab refs from JSON output.
    let out = at()
        .arg("list")
        .arg("--json")
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let tasks: Vec<serde_json::Value> = serde_json::from_slice(&out).expect("list --json");
    assert_eq!(tasks.len(), 2);
    let ref_of = |title: &str| {
        tasks
            .iter()
            .find(|t| t["title"] == title)
            .and_then(|t| t["ref"].as_str())
            .expect("ref")
            .to_owned()
    };
    let auth = ref_of("Implement auth");
    let tests = ref_of("Write tests");

    // Blocked task is not ready.
    at().args(["block", &tests, &auth]).assert().success();
    at().arg("ready")
        .assert()
        .success()
        .stdout(predicates::str::contains("Implement auth"))
        .stdout(predicates::str::contains("Write tests").not());

    // Completing the blocker unblocks the test task.
    at().args(["done", &auth]).assert().success();
    at().arg("ready")
        .assert()
        .success()
        .stdout(predicates::str::contains("Write tests"));

    // Detail view renders without raw timestamps.
    at().args(["show", &tests]).assert().success().stdout(
        predicates::str::contains("Blockers:").and(predicates::str::contains("(resolved)")),
    );

    // Integrity is clean.
    at().arg("check")
        .assert()
        .success()
        .stdout(predicates::str::contains("No integrity issues"));
}

#[test]
fn ambiguous_and_missing_ids_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_project(&dir, "demo");
    let at = || {
        let mut c = tk();
        c.arg("-C").arg(dir.path());
        c
    };
    at().args(["show", "nope"]).assert().failure().stderr(
        predicates::str::contains("task not found").or(predicates::str::contains("no .tasks")),
    );
}

#[test]
fn legacy_go_files_read_cleanly() {
    // A task file written by the old Go binary (unknown `external` field,
    // legacy string log entry, `cancelled` status) must load and self-heal.
    let dir = tempfile::tempdir().expect("tempdir");
    let tasks = dir.path().join(".tasks");
    std::fs::create_dir_all(&tasks).expect("mkdir");
    std::fs::write(
        tasks.join("config.json"),
        r#"{"version":1,"project":"demo","defaults":{"priority":3,"labels":[],"assignees":[]},"clean_after":14}"#,
    )
    .expect("config");
    std::fs::write(
        tasks.join("demo-old1.json"),
        r#"{"project":"demo","ref":"old1","title":"Legacy task","status":"cancelled",
            "priority":2,"labels":[],"assignees":[],"blocked_by":[],"logs":["2026-01-10: old note"],
            "created_at":"2026-01-10T12:00:00Z","updated_at":"2026-01-10T12:00:00Z",
            "external":{"github":{"number":1}}}"#,
    )
    .expect("task");

    let mut c = tk();
    c.arg("-C")
        .arg(dir.path())
        .args(["show", "old1"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Legacy task"))
        .stdout(predicates::str::contains("old note"));
    // Reading must not rewrite the file: unknown fields survive a read-only query.
    let raw = std::fs::read_to_string(tasks.join("demo-old1.json")).expect("read");
    assert!(raw.contains("external"), "{raw}");
}

// --- Regression helpers ------------------------------------------------------

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tk"))
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

fn store_dir(dir: &Path) -> std::path::PathBuf {
    dir.join(".tasks")
}

fn add_task(dir: &Path, title: &str) -> String {
    let out = ok_in(dir, &["add", title]);
    out.trim()
        .strip_prefix("Created task ")
        .and_then(|rest| rest.split(':').next())
        .expect("created task id")
        .trim()
        .to_owned()
}

fn read_task(dir: &Path, id: &str) -> serde_json::Value {
    let raw =
        std::fs::read_to_string(store_dir(dir).join(format!("{id}.json"))).expect("task file");
    serde_json::from_str(&raw).expect("task json")
}

fn show_json(dir: &Path, id: &str) -> serde_json::Value {
    serde_json::from_str(&ok_in(dir, &["show", id, "--json"])).expect("show --json")
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

// --- Regressions: concurrent mutations must not be lost ----------------------

#[test]
fn concurrent_log_appends_are_not_lost() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "shared log");

    const WRITERS: usize = 8;
    let children: Vec<_> = (0..WRITERS)
        .map(|i| {
            bin()
                .arg("-C")
                .arg(dir.path())
                .args(["log", &id, &format!("entry-{i}")])
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

    let task = read_task(dir.path(), &id);
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
    assert_eq!(
        seen.len(),
        WRITERS,
        "duplicate or missing entries: {seen:?}"
    );
}

#[test]
fn concurrent_label_edits_are_not_lost() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "shared labels");

    const WRITERS: usize = 8;
    let children: Vec<_> = (0..WRITERS)
        .map(|i| {
            bin()
                .arg("-C")
                .arg(dir.path())
                .args(["edit", &id, "-l", &format!("+label{i}")])
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

    let task = read_task(dir.path(), &id);
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
fn a_held_lock_blocks_another_writer() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "lock target");

    let ctx = tk::store::Ctx::at_tasks_dir(store_dir(dir.path()));
    ctx.require().expect("store exists");
    let guard = ctx.lock_store().expect("take store lock");

    let mut child = bin()
        .arg("-C")
        .arg(dir.path())
        .args(["log", &id, "blocked-writer"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn log");
    std::thread::sleep(Duration::from_millis(400));
    assert!(
        child.try_wait().expect("try_wait").is_none(),
        "an append completed while the store lock was held"
    );

    drop(guard);
    let status = child.wait().expect("wait");
    assert!(
        status.success(),
        "append failed after the lock was released"
    );
    let task = read_task(dir.path(), &id);
    assert_eq!(task["logs"].as_array().expect("logs").len(), 1);
}

// --- Regressions: read side must not destroy state ---------------------------

#[test]
fn show_does_not_delete_a_missing_blocker_reference() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let blocker = add_task(dir.path(), "prerequisite");
    let dependent = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent, &blocker]);

    // The prerequisite disappears (cleaned, hand-deleted, or synced away).
    std::fs::remove_file(store_dir(dir.path()).join(format!("{blocker}.json")))
        .expect("remove prerequisite");

    let before = std::fs::read_to_string(store_dir(dir.path()).join(format!("{dependent}.json")))
        .expect("dependent file");
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

    let after = std::fs::read_to_string(store_dir(dir.path()).join(format!("{dependent}.json")))
        .expect("dependent file");
    assert_eq!(before, after, "a read-only query rewrote the task file");

    // A missing prerequisite is unresolved, never "ready".
    let ready = ok_in(dir.path(), &["ready"]);
    assert!(
        !ready.contains(&dependent),
        "dependent must not be ready: {ready}"
    );
}

#[test]
fn check_exits_nonzero_on_findings() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let blocker = add_task(dir.path(), "prerequisite");
    let dependent = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent, &blocker]);
    std::fs::remove_file(store_dir(dir.path()).join(format!("{blocker}.json")))
        .expect("remove prerequisite");

    let human = run_in(dir.path(), &["check"]);
    assert!(!human.status.success(), "check must fail on findings");
    let text = String::from_utf8_lossy(&human.stdout).into_owned();
    assert!(text.contains("missing task"), "{text}");

    let json = run_in(dir.path(), &["check", "--json"]);
    assert!(!json.status.success(), "check --json must fail on findings");
    let report: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("check --json payload");
    assert_eq!(report["ok"], serde_json::json!(false));
    assert!(
        report["issues"].as_array().is_some_and(|i| !i.is_empty()),
        "{report}"
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
fn repair_drops_dangling_references_only_when_asked() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let blocker = add_task(dir.path(), "prerequisite");
    let dependent = add_task(dir.path(), "dependent");
    ok_in(dir.path(), &["block", &dependent, &blocker]);
    std::fs::remove_file(store_dir(dir.path()).join(format!("{blocker}.json")))
        .expect("remove prerequisite");

    // Plain repair reports but does not silently drop the reference.
    ok_in(dir.path(), &["repair", &dependent]);
    let task = read_task(dir.path(), &dependent);
    assert_eq!(
        task["blocked_by"],
        serde_json::json!([blocker]),
        "plain repair must not drop references"
    );

    ok_in(dir.path(), &["repair", &dependent, "--drop-missing"]);
    let task = read_task(dir.path(), &dependent);
    assert_eq!(task["blocked_by"], serde_json::json!([]));
    ok_in(dir.path(), &["check"]);
}

#[test]
fn concurrent_opposite_blocks_never_create_a_cycle() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let a = add_task(dir.path(), "alpha");
    let b = add_task(dir.path(), "beta");

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
    let first = spawn(&b, &a);
    let second = spawn(&a, &b);
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
    let a = add_task(dir.path(), "alpha");
    let b = add_task(dir.path(), "beta");

    // Write a cycle directly, the way a hand edit or an old writer could.
    for (this, other) in [(&a, &b), (&b, &a)] {
        let path = store_dir(dir.path()).join(format!("{this}.json"));
        let mut value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("task file"))
                .expect("json");
        value["blocked_by"] = serde_json::json!([other]);
        std::fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("write");
    }

    let out = run_in(dir.path(), &["check"]);
    assert!(!out.status.success(), "a cycle must fail the check");
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("dependency cycle"), "{text}");
}

// --- Regressions: stale intent ----------------------------------------------

#[test]
fn edit_rejects_a_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "concurrent intent");

    let rev = show_json(dir.path(), &id)["rev"]
        .as_str()
        .expect("rev")
        .to_owned();
    ok_in(dir.path(), &["edit", &id, "-t", "first", "--if-rev", &rev]);

    // The revision is from before the first edit: applying it would silently
    // overwrite a change this writer never saw.
    let stale = run_in(dir.path(), &["edit", &id, "-t", "second", "--if-rev", &rev]);
    assert!(!stale.status.success(), "stale edit must be rejected");
    let err = flatten(&String::from_utf8_lossy(&stale.stderr));
    assert!(err.contains("changed since it was read"), "{err}");
    assert_eq!(read_task(dir.path(), &id)["title"], "first");

    // The current revision still works.
    let fresh = show_json(dir.path(), &id)["rev"]
        .as_str()
        .expect("rev")
        .to_owned();
    ok_in(
        dir.path(),
        &["edit", &id, "-t", "third", "--if-rev", &fresh],
    );
    assert_eq!(read_task(dir.path(), &id)["title"], "third");
}

// --- Regressions: store selection -------------------------------------------

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
    assert!(store.join("config.json").exists());

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
fn linked_worktree_refuses_to_bootstrap() {
    let dir = tempfile::tempdir().expect("tempdir");
    // A linked worktree (or submodule) has `.git` as a file pointing elsewhere.
    std::fs::write(
        dir.path().join(".git"),
        "gitdir: /elsewhere/.git/worktrees/x\n",
    )
    .expect("gitdir file");

    let out = bin()
        .arg("-C")
        .arg(dir.path())
        .args(["add", "shadow store"])
        .output()
        .expect("spawn tk");
    assert!(!out.status.success(), "bootstrap in a worktree must fail");
    let err = flatten(&String::from_utf8_lossy(&out.stderr));
    assert!(err.contains("linked worktree"), "{err}");
    assert!(
        !store_dir(dir.path()).exists(),
        "tk created a worktree-local store"
    );

    // Deliberate creation still works.
    ok_in(dir.path(), &["init", "-P", "demo"]);
    assert!(store_dir(dir.path()).exists());
}

#[test]
fn path_reports_the_selected_store() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);

    let discovered = ok_in(dir.path(), &["path", "--json"]);
    let value: serde_json::Value = serde_json::from_str(&discovered).expect("path --json");
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
    assert_eq!(value["source"], "flag");
}

// --- Task detail: checkpoint, links, acceptance, evidence --------------------

#[test]
fn checkpoint_is_replaced_not_appended() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "checkpointed");

    ok_in(dir.path(), &["checkpoint", &id, "first pass done"]);
    ok_in(dir.path(), &["log", &id, "historical note"]);
    ok_in(
        dir.path(),
        &["checkpoint", &id, "second pass: blocked on review"],
    );

    let shown = show_json(dir.path(), &id);
    assert_eq!(shown["checkpoint"], "second pass: blocked on review");
    assert_eq!(shown["logs"].as_array().expect("logs").len(), 1);

    let out = ok_in(dir.path(), &["checkpoint", &id]);
    assert!(out.contains("second pass: blocked on review"), "{out}");

    ok_in(dir.path(), &["checkpoint", &id, "--clear"]);
    assert_eq!(
        show_json(dir.path(), &id)["checkpoint"],
        serde_json::Value::Null
    );
}

#[test]
fn checkpoint_rejects_a_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "stale checkpoint");
    let rev = show_json(dir.path(), &id)["rev"]
        .as_str()
        .expect("rev")
        .to_owned();

    ok_in(dir.path(), &["checkpoint", &id, "fresh", "--if-rev", &rev]);
    let stale = run_in(
        dir.path(),
        &["checkpoint", &id, "overwrite", "--if-rev", &rev],
    );
    assert!(!stale.status.success(), "stale checkpoint must be rejected");
    assert_eq!(show_json(dir.path(), &id)["checkpoint"], "fresh");
}

#[test]
fn links_acceptance_and_evidence_round_trip() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = add_task(dir.path(), "documented");
    let reference = "agent-context/projects/x/research/y.md";

    ok_in(dir.path(), &["link", &id, reference]);
    ok_in(dir.path(), &["accept", &id, "parity test passes"]);
    ok_in(dir.path(), &["accept", &id, "docs updated"]);
    ok_in(dir.path(), &["evidence", &id, "cargo test --all-targets"]);

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
    ok_in(dir.path(), &["link", &id, reference]);
    assert_eq!(
        show_json(dir.path(), &id)["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );

    let detail = ok_in(dir.path(), &["show", &id]);
    assert!(detail.contains(reference), "{detail}");
    assert!(detail.contains("parity test passes"), "{detail}");

    ok_in(dir.path(), &["accept", &id, "--remove", "docs updated"]);
    assert_eq!(
        show_json(dir.path(), &id)["acceptance"],
        serde_json::json!(["parity test passes"])
    );
    ok_in(dir.path(), &["unlink", &id, reference]);
    assert_eq!(show_json(dir.path(), &id)["links"], serde_json::json!([]));
    ok_in(dir.path(), &["evidence", &id, "--clear"]);
    assert_eq!(
        show_json(dir.path(), &id)["evidence"],
        serde_json::json!([])
    );
}

// --- Archival and stable references -----------------------------------------

fn done_task(dir: &Path, title: &str) -> String {
    let id = add_task(dir, title);
    ok_in(dir, &["done", &id]);
    id
}

#[test]
fn archive_requires_a_terminal_status_and_hides_from_active_views() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let open = add_task(dir.path(), "still open");
    let out = run_in(dir.path(), &["archive", &open]);
    assert!(!out.status.success(), "an open task must not archive");

    let done = done_task(dir.path(), "finished");
    ok_in(dir.path(), &["archive", &done]);
    assert!(show_json(dir.path(), &done)["archived_at"].is_string());

    // Other tasks may still reference an archived task, and resolution works.
    let dependent = add_task(dir.path(), "depends on archived");
    ok_in(dir.path(), &["block", &dependent, &done]);
    let ready = ok_in(dir.path(), &["ready"]);
    assert!(ready.contains(&dependent), "{ready}");
    ok_in(dir.path(), &["check"]);

    let listed = ok_in(dir.path(), &["list", "-s", "done"]);
    assert!(
        !listed.contains(&done),
        "archived task in the default list: {listed}"
    );
    let archived = ok_in(dir.path(), &["list", "--archived"]);
    assert!(archived.contains(&done), "{archived}");
    assert!(archived.contains("[archived]"), "{archived}");

    ok_in(dir.path(), &["unarchive", &done]);
    let listed = ok_in(dir.path(), &["list", "-s", "done"]);
    assert!(listed.contains(&done), "{listed}");
}

#[test]
fn clean_archives_instead_of_deleting() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let id = done_task(dir.path(), "old work");

    let out = ok_in(dir.path(), &["clean", "--older-than", "0"]);
    assert!(out.contains("Archived 1"), "{out}");
    assert!(show_json(dir.path(), &id)["archived_at"].is_string());
    assert!(
        store_dir(dir.path()).join(format!("{id}.json")).exists(),
        "clean deleted the record instead of archiving it"
    );

    // Purge still deletes, including already-archived records.
    let out = ok_in(dir.path(), &["clean", "--older-than", "0", "--purge"]);
    assert!(out.contains("Purged 1"), "{out}");
    assert!(!store_dir(dir.path()).join(format!("{id}.json")).exists());
}

#[test]
fn moved_task_still_resolves_by_its_previous_id() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let old_id = add_task(dir.path(), "relocated");

    let out = ok_in(dir.path(), &["mv", &old_id, "other"]);
    assert!(out.contains("Moved"), "{out}");
    let moved = show_json(dir.path(), &old_id);
    assert_eq!(moved["project"], "other");
    assert_eq!(moved["previous_ids"], serde_json::json!([old_id]));

    // A reference written before the move still points at the task.
    ok_in(dir.path(), &["check"]);
}

#[test]
fn previous_id_shadowing_a_live_task_is_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    let live = add_task(dir.path(), "live");
    let other = add_task(dir.path(), "other");

    let path = store_dir(dir.path()).join(format!("{other}.json"));
    let raw = std::fs::read_to_string(&path).expect("task file");
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("json");
    value["previous_ids"] = serde_json::json!([live]);
    std::fs::write(&path, serde_json::to_string_pretty(&value).expect("json")).expect("write");

    let out = run_in(dir.path(), &["check"]);
    assert!(
        !out.status.success(),
        "shadowed previous_ids must be reported"
    );
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("previous_ids"),
        "{}",
        String::from_utf8_lossy(&out.stdout)
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
