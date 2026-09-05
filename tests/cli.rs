//! CLI-level tests: help drift, diagnostics, and end-to-end flows.

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
}
