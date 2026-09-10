//! The v0 → v1 migration is a one-shot script, not a `tk` subcommand, so its
//! coverage lives here: build a v0 store, convert it, and check that the v1
//! binary is happy with the result.
//!
//! Skipped when `python3` is unavailable, rather than failing on a machine that
//! cannot run the script at all.

use std::path::{Path, PathBuf};
use std::process::Command;

fn python() -> Option<String> {
    for candidate in ["python3", "python"] {
        let ok = Command::new(candidate)
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success());
        if ok {
            return Some(candidate.to_owned());
        }
    }
    None
}

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_tk"))
}

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

/// A v0 store with the shapes that actually appear in the wild: a legacy
/// string log with an embedded timestamp, an unknown field, a `cancelled`
/// status, a cross-task reference, and two projects sharing a ref.
fn write_v0_store(dir: &Path) {
    let tasks = store_dir(dir);
    std::fs::create_dir_all(&tasks).expect("mkdir");
    std::fs::write(
        tasks.join("config.json"),
        r#"{"version":1,"project":"demo","defaults":{"priority":3,"labels":["x"],"assignees":[]},"clean_after":14}"#,
    )
    .expect("config");
    std::fs::write(
        tasks.join("demo-a7b3.json"),
        r#"{"project":"demo","ref":"a7b3","title":"Implement auth","status":"active",
            "priority":1,"labels":["backend"],"assignees":["nick"],"blocked_by":[],
            "logs":["2026-01-10: started","just a note"],
            "created_at":"2026-01-10T12:00:00Z","updated_at":"2026-02-01T09:30:00Z",
            "checkpoint":"halfway","links":["docs/x.md"],"acceptance":["tests pass"],
            "evidence":[],"external":{"github":{"number":1}}}"#,
    )
    .expect("task a");
    std::fs::write(
        tasks.join("demo-b7c4.json"),
        r#"{"project":"demo","ref":"b7c4","title":"Write tests","status":"cancelled",
            "priority":2,"blocked_by":["demo-a7b3"],"logs":[],
            "created_at":"2026-01-11T12:00:00Z","updated_at":"2026-01-12T12:00:00Z",
            "completed_at":"2026-01-12T12:00:00Z"}"#,
    )
    .expect("task b");
    std::fs::write(
        tasks.join("other-b7c4.json"),
        r#"{"project":"other","ref":"b7c4","title":"Duplicate ref elsewhere","status":"open",
            "priority":3,"blocked_by":[],"logs":[],
            "created_at":"2026-01-13T12:00:00Z","updated_at":"2026-01-13T12:00:00Z"}"#,
    )
    .expect("task c");
}

fn migrate(dir: &Path, extra: &[&str]) -> std::process::Output {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("tools/migrate-v0.py");
    let mut command = Command::new(python().expect("python3 checked by caller"));
    command
        .arg(script)
        .arg(".tasks")
        .args(extra)
        .current_dir(dir);
    command.output().expect("spawn migration")
}

#[test]
fn a_v0_store_migrates_into_a_clean_v1_store() {
    let Some(_) = python() else {
        eprintln!("skipping: python3 is not available to run tools/migrate-v0.py");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    write_v0_store(dir.path());

    // A dry run must not touch anything.
    let dry = migrate(dir.path(), &["--dry-run"]);
    assert!(
        dry.status.success(),
        "{}",
        String::from_utf8_lossy(&dry.stderr)
    );
    assert!(!store_dir(dir.path()).join("store.json").exists());
    assert!(!store_dir(dir.path()).join("records").exists());

    let out = migrate(dir.path(), &[]);
    assert!(
        out.status.success(),
        "migration failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // The v1 binary accepts the result, with no findings.
    ok_in(dir.path(), &["check"]);

    // Fields survive, including the legacy log split and the unknown field
    // being ignored rather than fatal.
    let a = ok_in(dir.path(), &["show", "demo-a7b3", "--json"]);
    let a: serde_json::Value = serde_json::from_str(&a).expect("show --json");
    let a = &a["data"];
    assert_eq!(a["title"], "Implement auth");
    assert_eq!(a["status"], "active");
    assert_eq!(a["priority"], serde_json::json!(1));
    assert_eq!(a["checkpoint"], "halfway");
    assert_eq!(a["links"], serde_json::json!(["docs/x.md"]));
    assert_eq!(a["acceptance"], serde_json::json!(["tests pass"]));
    assert_eq!(
        a["legacy_aliases"],
        serde_json::json!(["demo-a7b3"]),
        "the old ID must stay resolvable"
    );
    let logs: Vec<&str> = a["logs"]
        .as_array()
        .expect("logs")
        .iter()
        .map(|l| l["msg"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(logs, vec!["started", "just a note"], "{a}");

    // The old handle keeps working: the ref became the alias.
    assert_eq!(a["alias"], "a7b3");
    ok_in(dir.path(), &["show", "a7b3"]);

    // References were rewritten to new IDs, not left pointing at v0 names.
    let b = ok_in(dir.path(), &["show", "demo-b7c4", "--json"]);
    let b: serde_json::Value = serde_json::from_str(&b).expect("show --json");
    let b = &b["data"];
    assert_eq!(b["status"], "closed", "`cancelled` becomes `closed`");
    assert_eq!(b["completed_at"], "2026-01-12T12:00:00Z");
    let blocked_by = b["blocked_by"].as_array().expect("blocked_by");
    assert_eq!(blocked_by.len(), 1);
    let blocker = blocked_by[0].as_str().expect("blocker id");
    assert!(blocker.starts_with("01"), "remapped to a ULID: {blocker}");
    assert_eq!(
        blocker,
        a["id"].as_str().unwrap(),
        "the blocker must be the migrated task"
    );

    // Two projects sharing a ref cannot both keep it, so neither bare ref
    // resolves ambiguously.
    let c = ok_in(dir.path(), &["show", "other-b7c4", "--json"]);
    let c: serde_json::Value = serde_json::from_str(&c).expect("show --json");
    assert_ne!(c["data"]["alias"], "b7c4");
    let list = ok_in(dir.path(), &["list", "-a"]);
    assert_eq!(
        list.matches("b7c4").count(),
        0,
        "no bare ref collision: {list}"
    );

    // Old files are kept, not deleted, and a second run refuses.
    assert!(store_dir(dir.path()).join("legacy/demo-a7b3.json").exists());
    let again = migrate(dir.path(), &[]);
    assert!(!again.status.success(), "a second migration must refuse");
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already a v1 store"),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
}

#[test]
fn a_v1_store_is_never_migrated() {
    let Some(_) = python() else {
        eprintln!("skipping: python3 is not available to run tools/migrate-v0.py");
        return;
    };
    let dir = tempfile::tempdir().expect("tempdir");
    ok_in(dir.path(), &["init", "-P", "demo"]);
    ok_in(dir.path(), &["add", "already v1"]);

    let out = migrate(dir.path(), &[]);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("already a v1 store"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    // The v1 store is untouched.
    ok_in(dir.path(), &["check"]);
    assert!(ok_in(dir.path(), &["list"]).contains("already v1"));
}
