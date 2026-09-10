//! CLI-level tests: help drift, error codes, end-to-end flows, and the
//! concurrency behaviour the store design is built around.
//!
//! The concurrency tests matter more than they did in v1. Now that a mutation
//! rewrites a whole document, two writers that both read before either wrote can
//! lose one of the changes, so the store lock is load-bearing rather than an
//! optimization — these tests assert that it is actually held across the
//! read-modify-write, and that nothing is silently dropped.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

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
    for (alias, name) in [
        ("ls", "list"),
        ("new", "add"),
        ("rdy", "ready"),
        ("log", "note"),
        ("tag", "label"),
        ("rm", "purge"),
        ("ck", "check"),
    ] {
        let via_alias = usage::test::help(spec, &[alias], usage::test::Page::Long);
        let via_name = usage::test::help(spec, &[name], usage::test::Page::Long);
        assert_eq!(via_alias, via_name, "{alias} should be {name}");
    }
}

#[test]
fn a_bad_state_is_refused_where_a_human_reads_it() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Alpha");
    let out = run_in(&dir, &["state", &r#ref, "active"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("invalid state"), "{text}");
    // The machine kind must not lead the human message.
    assert!(!text.starts_with("invalid_input"), "{text}");
    // `active` was a v1 state, and the message says what to use instead.
    assert!(text.contains("open, done, or dropped"), "{text}");
    assert_eq!(
        failure_code(&dir, &["state", &r#ref, "active"]),
        "invalid_input"
    );
}

// --- Harness ---------------------------------------------------------------

fn init(dir: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_tk"))
        .args(["-C", &dir.display().to_string(), "init"])
        .output()
        .expect("spawn tk init");
    assert!(out.status.success(), "init failed: {}", stderr(&out));
}

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tk"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn tk")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn ok_in(dir: &Path, args: &[&str]) -> String {
    let out = run_in(dir, args);
    assert!(
        out.status.success(),
        "tk {args:?} failed: {}\n{}",
        stdout(&out),
        stderr(&out)
    );
    stdout(&out)
}

/// The `data` field of a successful `--json` envelope.
fn payload(dir: &Path, args: &[&str]) -> serde_json::Value {
    let mut with_json = vec!["-j"];
    with_json.extend_from_slice(args);
    let text = ok_in(dir, &with_json);
    let envelope: serde_json::Value =
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("not JSON: {e}\n{text}"));
    assert_eq!(envelope["ok"], serde_json::json!(true), "{text}");
    // One shape everywhere: a reader indexes the same keys on every command.
    for key in ["ok", "command", "rev", "data", "issues", "error_code"] {
        assert!(envelope.get(key).is_some(), "{key} missing from {text}");
    }
    envelope["data"].clone()
}

/// The `error_code` of a failing `--json` envelope.
fn failure_code(dir: &Path, args: &[&str]) -> String {
    let mut with_json = vec!["-j"];
    with_json.extend_from_slice(args);
    let out = run_in(dir, &with_json);
    assert!(!out.status.success(), "expected failure for {args:?}");
    let text = stdout(&out);
    let envelope: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|e| panic!("not JSON: {e}\nstdout: {text}\nstderr: {}", stderr(&out)));
    assert_eq!(envelope["ok"], serde_json::json!(false));
    // The same key set as a success, so a reader never shape-checks. `data` may
    // carry the structured result of the failure (`check`'s findings, `apply`'s
    // report); a plain failure leaves it null.
    for key in ["ok", "command", "rev", "data", "issues", "error_code"] {
        assert!(envelope.get(key).is_some(), "{key} missing from {text}");
    }
    envelope["error_code"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

fn add(dir: &Path, title: &str) -> String {
    ok_in(dir, &["add", title, "-q"]).trim().to_owned()
}

fn entry_file(dir: &Path, r#ref: &str) -> PathBuf {
    let tasks = dir.join(".tasks");
    std::fs::read_dir(&tasks)
        .expect("read store")
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(&format!("{ref}-")))
        })
        .unwrap_or_else(|| panic!("no file for {ref}"))
}

fn read_entry(dir: &Path, r#ref: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(entry_file(dir, r#ref)).expect("read entry");
    serde_json::from_str(&text).expect("parse entry")
}

fn store() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().to_path_buf();
    init(&path);
    (dir, path)
}

// --- End-to-end ------------------------------------------------------------

#[test]
fn a_task_goes_from_open_to_done() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Rewrite the auth layer");
    assert_eq!(r#ref.len(), 4);

    // Open, unblocked, and visible.
    assert!(ok_in(&dir, &["ls"]).contains("Rewrite the auth layer"));
    assert!(ok_in(&dir, &["ready"]).contains(&r#ref));

    ok_in(&dir, &["note", &r#ref, "started with", "the JWT approach"]);
    ok_in(&dir, &["status", &r#ref, "halfway"]);
    ok_in(&dir, &["label", &r#ref, "+backend"]);
    ok_in(&dir, &["accept", &r#ref, "parity test passes"]);

    let entry = read_entry(&dir, &r#ref);
    assert_eq!(entry["state"], "open");
    assert_eq!(entry["status"], "halfway");
    assert_eq!(entry["labels"], serde_json::json!(["backend"]));
    assert_eq!(entry["log"][0]["msg"], "started with the JWT approach");
    assert_eq!(entry["acceptance"][0], "parity test passes");
    assert_eq!(entry["done"], serde_json::Value::Null);

    let done = ok_in(&dir, &["done", &r#ref]);
    assert!(done.contains("done"), "{done}");
    let entry = read_entry(&dir, &r#ref);
    assert_eq!(entry["state"], "done");
    assert!(entry["done"].as_str().is_some(), "closing stamps a time");

    // Done entries leave the default view and `ready`.
    assert!(!ok_in(&dir, &["ls"]).contains("Rewrite the auth layer"));
    assert!(ok_in(&dir, &["ls", "-a"]).contains("Rewrite the auth layer"));
    assert!(!ok_in(&dir, &["ready"]).contains(&r#ref));

    // Reopening forgets the completion time.
    ok_in(&dir, &["state", &r#ref, "open"]);
    assert_eq!(read_entry(&dir, &r#ref)["done"], serde_json::Value::Null);
    assert!(ok_in(&dir, &["ready"]).contains(&r#ref));
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

#[test]
fn the_document_on_disk_is_the_documented_shape() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Rewrite the auth layer");
    // A fresh entry has no status; the key is omitted rather than written empty.
    let fresh = std::fs::read_to_string(entry_file(&dir, &r#ref)).unwrap();
    assert!(!fresh.contains("\"status\""), "{fresh}");
    ok_in(&dir, &["status", &r#ref, "halfway"]);
    let raw = std::fs::read_to_string(entry_file(&dir, &r#ref)).unwrap();
    assert!(raw.ends_with("}\n"), "a file ends with a newline: {raw:?}");
    // Read the order from the text: a parsed map sorts its keys.
    let keys: Vec<&str> = raw
        .lines()
        .filter_map(|line| line.strip_prefix("  \""))
        .filter_map(|line| line.split('"').next())
        .collect();
    assert_eq!(
        keys,
        vec![
            "ref",
            "title",
            "state",
            "labels",
            "created",
            "updated",
            "done",
            "blocked_by",
            "status",
            "acceptance",
            "log"
        ],
        "key order is part of the format"
    );
    assert_eq!(
        entry_file(&dir, &r#ref)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        format!("{ref}-rewrite-the-auth-layer.json", ref = r#ref)
    );
}

#[test]
fn adding_an_entry_takes_everything_in_one_call() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Alpha");
    let other = ok_in(
        &dir,
        &[
            "add",
            "Beta",
            "-q",
            "-l",
            "backend,api",
            "--accept",
            "works,is tested",
            "--status",
            "waiting on review",
            "-b",
            &r#ref,
        ],
    )
    .trim()
    .to_owned();
    assert!(r#ref != other);

    let entry = read_entry(&dir, &other);
    assert_eq!(entry["labels"], serde_json::json!(["api", "backend"]));
    assert_eq!(
        entry["acceptance"],
        serde_json::json!(["works", "is tested"])
    );
    assert_eq!(entry["status"], "waiting on review");
    assert_eq!(entry["blocked_by"], serde_json::json!([r#ref]));
    assert!(!ok_in(&dir, &["ready"]).contains(&other), "it is blocked");
}

#[test]
fn blocking_gates_readiness_and_unblocking_restores_it() {
    let (_tmp, dir) = store();
    let a = add(&dir, "Alpha");
    let b = add(&dir, "Beta");
    ok_in(&dir, &["block", &b, &a]);

    assert!(ok_in(&dir, &["ls"]).contains("[blocked]"));
    assert_eq!(payload(&dir, &["ready"]).as_array().unwrap().len(), 1);
    assert_eq!(
        payload(&dir, &["ls", "--blocked"])
            .as_array()
            .unwrap()
            .len(),
        1
    );

    // Finishing the blocker is enough: readiness follows state, not deletion.
    ok_in(&dir, &["done", &a]);
    let ready = payload(&dir, &["ready"]);
    assert_eq!(ready.as_array().unwrap().len(), 1);
    assert_eq!(ready[0]["ref"], serde_json::json!(b));
    assert!(!ok_in(&dir, &["ls"]).contains("[blocked]"));

    // Reopening the blocker blocks it again.
    ok_in(&dir, &["state", &a, "open"]);
    assert!(ok_in(&dir, &["ls"]).contains("[blocked]"));
    ok_in(&dir, &["unblock", &b, &a]);
    assert!(!ok_in(&dir, &["ls"]).contains("[blocked]"));
}

#[test]
fn a_blocking_loop_is_refused_with_the_loop_in_the_message() {
    let (_tmp, dir) = store();
    let a = add(&dir, "Alpha");
    let b = add(&dir, "Beta");
    ok_in(&dir, &["block", &b, &a]);

    let out = run_in(&dir, &["block", &a, &b]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("loop"), "{text}");
    assert!(text.contains("->"), "the loop is spelled out: {text}");
    // Nothing was written.
    assert_eq!(read_entry(&dir, &a)["blocked_by"], serde_json::json!([]));
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

#[test]
fn editing_many_fields_happens_in_one_write() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Alpha");
    let before = read_entry(&dir, &r#ref);

    ok_in(
        &dir,
        &[
            "edit",
            &r#ref,
            "--title",
            "Alpha, revised",
            "--status",
            "blocked on review",
            "--add-label",
            "backend",
            "--add-accept",
            "parity test passes",
            "-n",
            "picked it up",
        ],
    );
    let after = read_entry(&dir, &r#ref);
    assert_eq!(after["title"], "Alpha, revised");
    assert_eq!(after["status"], "blocked on review");
    assert_eq!(after["labels"], serde_json::json!(["backend"]));
    assert_eq!(
        after["acceptance"],
        serde_json::json!(["parity test passes"])
    );
    assert_eq!(after["log"][0]["msg"], "picked it up");
    assert_eq!(after["created"], before["created"], "created never moves");
    // The file does not move on a title change: the ref is the identity.
    assert_eq!(
        entry_file(&dir, &r#ref)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        format!("{ref}-alpha.json", ref = r#ref)
    );
    // An explicit slug does move it.
    ok_in(&dir, &["edit", &r#ref, "--slug", "something-else"]);
    assert_eq!(
        entry_file(&dir, &r#ref)
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        format!("{ref}-something-else.json", ref = r#ref)
    );
    assert_eq!(read_entry(&dir, &r#ref)["created"], before["created"]);
}

#[test]
fn resolving_by_title_works_and_ambiguity_is_an_error() {
    let (_tmp, dir) = store();
    let a = add(&dir, "Rewrite the auth layer");
    let b = add(&dir, "Parse the auth header");

    assert_eq!(
        ok_in(&dir, &["show", "Rewrite"]).trim().lines().next(),
        Some(format!("{a}  Rewrite the auth layer").as_str())
    );
    assert_eq!(
        failure_code(&dir, &["show", "auth"]),
        "ambiguous",
        "two matches is an error, not a guess"
    );
    assert_eq!(
        failure_code(&dir, &["show", "nothing like it"]),
        "not_found"
    );
    assert!(ok_in(&dir, &["show", &b]).contains("Parse the auth header"));
}

#[test]
fn purge_removes_an_entry_and_unblocks_what_waited_on_it() {
    let (_tmp, dir) = store();
    let a = add(&dir, "Alpha");
    let b = add(&dir, "Beta");
    ok_in(&dir, &["block", &b, &a]);

    let out = ok_in(&dir, &["purge", &a]);
    assert!(out.contains(&a), "{out}");
    assert!(out.contains(&b), "it reports what it unblocked: {out}");
    assert!(
        read_entry(&dir, &b)["blocked_by"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(payload(&dir, &["ls"]).as_array().unwrap().len(), 1);
    assert_eq!(failure_code(&dir, &["show", &a]), "not_found");
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

#[test]
fn a_purge_dry_run_changes_nothing() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Alpha");
    let out = ok_in(&dir, &["purge", &r#ref, "--dry-run"]);
    assert!(out.contains("would delete"), "{out}");
    assert!(read_entry(&dir, &r#ref)["title"].as_str().is_some());
}

// --- Format gate -----------------------------------------------------------

#[test]
fn an_older_store_is_refused_with_the_reason_and_the_remedy() {
    let (_tmp, dir) = store();
    // A legacy store is one without our own store file; drop it and the entries.
    std::fs::remove_file(dir.join(".tasks/.tk.json")).unwrap();

    // A v1 layout: store.json plus records/.
    std::fs::create_dir_all(dir.join(".tasks/records")).unwrap();
    std::fs::write(dir.join(".tasks/store.json"), r#"{"format":2}"#).unwrap();
    let out = run_in(&dir, &["ls"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("v1"), "{text}");
    assert!(text.contains("migrate_to_v3.py"), "{text}");
    assert_eq!(failure_code(&dir, &["ls"]), "not_a_store");

    // A v0 layout: config.json.
    std::fs::remove_file(dir.join(".tasks/store.json")).unwrap();
    std::fs::remove_dir_all(dir.join(".tasks/records")).unwrap();
    std::fs::write(dir.join(".tasks/config.json"), "{}").unwrap();
    let out = run_in(&dir, &["ls"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("v0"), "{}", stderr(&out));

    // A format number this binary does not write.
    std::fs::remove_file(dir.join(".tasks/config.json")).unwrap();
    std::fs::write(dir.join(".tasks/.tk.json"), r#"{"format":9}"#).unwrap();
    assert!(stderr(&run_in(&dir, &["ls"])).contains("format 9"));
}

#[test]
fn a_store_selected_explicitly_must_exist() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("nowhere/.tasks");
    let out = Command::new(env!("CARGO_BIN_EXE_tk"))
        .args(["--tasks-dir", &missing.display().to_string(), "ls"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("will not create"), "{text}");
}

// --- Error codes and revisions --------------------------------------------

#[test]
fn error_codes_are_stable_and_machine_readable() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Alpha");

    // A stale revision: the only defense against a whole-file clobber.
    let rev = payload(&dir, &["show", &r#ref])["rev"]
        .as_str()
        .unwrap()
        .to_owned();
    ok_in(&dir, &["status", &r#ref, "something changed"]);
    assert_eq!(
        failure_code(&dir, &["status", &r#ref, "again", "--if-rev", &rev]),
        "stale_revision"
    );
    // The current revision is accepted.
    let rev = payload(&dir, &["show", &r#ref])["rev"]
        .as_str()
        .unwrap()
        .to_owned();
    ok_in(&dir, &["status", &r#ref, "and again", "--if-rev", &rev]);

    assert_eq!(failure_code(&dir, &["show", "nope"]), "not_found");
    // An ordinary failure has no payload, and still keeps every key.
    let out = run_in(&dir, &["-j", "show", "nope"]);
    let envelope: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(envelope["data"], serde_json::Value::Null);
    assert_eq!(envelope["issues"].as_array().unwrap().len(), 1);
    assert_eq!(failure_code(&dir, &["edit", &r#ref]), "invalid_input");
    assert_eq!(
        failure_code(&dir, &["state", &r#ref, "silly"]),
        "invalid_input"
    );
}

#[test]
fn check_reports_findings_and_exits_nonzero() {
    let (_tmp, dir) = store();
    let a = add(&dir, "Alpha");
    let b = add(&dir, "Beta");

    // Break the store by hand: a dangling blocker, a self-block, a stray file.
    let mut entry = read_entry(&dir, &a);
    entry["blocked_by"] = serde_json::json!(["zzzz", a]);
    std::fs::write(
        entry_file(&dir, &a),
        serde_json::to_string_pretty(&entry).unwrap(),
    )
    .unwrap();
    std::fs::write(dir.join(".tasks/stray.json"), "{}").unwrap();
    let _ = b;

    let out = run_in(&dir, &["check"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    for expected in ["zzzz", "blocks itself", "stray.json"] {
        assert!(text.contains(expected), "{expected} missing from:\n{text}");
    }
    // The same findings travel in the failure's envelope.
    assert_eq!(failure_code(&dir, &["check"]), "check_failed");
    let envelope: serde_json::Value = {
        let out = run_in(&dir, &["-j", "check"]);
        serde_json::from_str(&stdout(&out)).unwrap()
    };
    let findings = envelope["data"]["findings"].as_array().unwrap();
    assert!(
        findings
            .iter()
            .any(|f| f.as_str().unwrap_or_default().contains("zzzz")),
        "{envelope}"
    );
    assert_eq!(envelope["data"]["clean"], serde_json::json!(false));
}

#[test]
fn a_human_reader_gets_sentences_and_a_machine_gets_a_code() {
    let (_tmp, dir) = store();
    let out = run_in(&dir, &["show", "nope"]);
    assert!(!out.status.success());
    let text = stderr(&out);
    assert!(text.contains("no entry matches"), "{text}");
    // The machine kind does not lead the human message.
    assert!(!text.contains("not_found"), "{text}");
}

// --- Keys tk does not know ------------------------------------------------

#[test]
fn a_hand_added_field_survives_a_tk_write() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Alpha");
    let path = entry_file(&dir, &r#ref);
    let mut entry: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    entry["assignee"] = serde_json::json!("nick");
    std::fs::write(&path, serde_json::to_string_pretty(&entry).unwrap()).unwrap();

    ok_in(&dir, &["status", &r#ref, "picked up"]);
    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(after["assignee"], serde_json::json!("nick"));
    assert_eq!(after["status"], "picked up");
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

// --- Batches --------------------------------------------------------------

fn apply(dir: &Path, batch: &str) -> Output {
    apply_with(dir, &["apply", "-j"], batch)
}

fn apply_with(dir: &Path, args: &[&str], batch: &str) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_tk"))
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn tk apply");
    child
        .stdin
        .as_mut()
        .expect("stdin")
        .write_all(batch.as_bytes())
        .expect("write batch");
    child.wait_with_output().expect("apply output")
}

#[test]
fn a_batch_applies_in_order_and_reports_what_it_did() {
    let (_tmp, dir) = store();
    let out = apply(
        &dir,
        r#"{"intents": [
            {"op": "add", "title": "Alpha", "labels": ["backend"]},
            {"op": "add", "title": "Beta"},
            {"op": "block", "ref": "beta", "blocker": "alpha"},
            {"op": "note", "ref": "beta", "message": "waiting"},
            {"op": "state", "ref": "alpha", "state": "done"}
        ]}"#,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let envelope: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(envelope["data"]["applied"].as_array().unwrap().len(), 5);
    assert_eq!(envelope["data"]["failed"], serde_json::Value::Null);
    assert_eq!(envelope["data"]["dry_run"], serde_json::json!(false));
    let note = envelope["data"]["note"].as_str().unwrap();
    assert!(note.contains("not a transaction"), "{note}");

    // The batch really did the work.
    assert_eq!(payload(&dir, &["ready"]).as_array().unwrap().len(), 1);
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

#[test]
fn a_batch_and_the_commands_agree() {
    // Commands and batch intents call the same operations; this is the test that
    // caught them disagreeing in v1.
    let (_tmp, batch_dir) = store();
    let (_tmp2, cmd_dir) = store();

    let batch = r#"{"intents": [
        {"op": "add", "title": "Rewrite the auth layer"},
        {"op": "add", "title": "Write the parser"},
        {"op": "edit", "ref": "auth", "title": "Rewrite the auth layer, properly",
         "status": "halfway", "add_labels": ["backend"],
         "add_acceptance": ["parity test passes"], "note": "started"},
        {"op": "block", "ref": "parser", "blocker": "auth"},
        {"op": "state", "ref": "parser", "state": "dropped"}
    ]}"#;
    let out = apply(&batch_dir, batch);
    assert!(out.status.success(), "{}", stderr(&out));

    let auth = add(&cmd_dir, "Rewrite the auth layer");
    let parser = add(&cmd_dir, "Write the parser");
    ok_in(
        &cmd_dir,
        &[
            "edit",
            &auth,
            "--title",
            "Rewrite the auth layer, properly",
            "--status",
            "halfway",
            "--add-label",
            "backend",
            "--add-accept",
            "parity test passes",
            "-n",
            "started",
        ],
    );
    ok_in(&cmd_dir, &["block", &parser, &auth]);
    ok_in(&cmd_dir, &["state", &parser, "dropped"]);

    let documents = |dir: &Path| -> Vec<serde_json::Value> {
        let mut out: Vec<serde_json::Value> = payload(dir, &["ls", "-a"])
            .as_array()
            .unwrap()
            .iter()
            .map(|view| {
                let mut doc = view.clone();
                doc["ref"] = serde_json::json!("");
                doc["blocked_by"] = serde_json::json!([]);
                doc["rev"] = serde_json::json!("");
                doc["file"] = serde_json::json!("");
                // Wall-clock stamps differ between the two stores.
                doc["created"] = serde_json::json!("");
                doc["updated"] = serde_json::json!("");
                doc["done"] = serde_json::json!(doc["done"].is_string());
                if let Some(log) = doc["log"].as_array_mut() {
                    for line in log {
                        line["ts"] = serde_json::json!("");
                    }
                }
                doc.as_object_mut().unwrap().remove("blocking");
                doc.as_object_mut().unwrap().remove("unresolved_blockers");
                doc
            })
            .collect();
        out.sort_by(|a, b| a["title"].as_str().cmp(&b["title"].as_str()));
        out
    };
    assert_eq!(documents(&batch_dir), documents(&cmd_dir));
}

#[test]
fn a_batch_stops_at_the_first_failure_and_says_so() {
    let (_tmp, dir) = store();
    let out = apply(
        &dir,
        r#"{"intents": [
            {"op": "add", "title": "Alpha"},
            {"op": "note", "ref": "nope", "message": "x"},
            {"op": "add", "title": "Beta"}
        ]}"#,
    );
    assert!(!out.status.success());
    let envelope: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(envelope["data"]["applied"].as_array().unwrap().len(), 1);
    assert_eq!(envelope["data"]["failed"]["index"], serde_json::json!(1));
    assert_eq!(envelope["data"]["not_attempted"], serde_json::json!(1));
    assert_eq!(envelope["error_code"], "not_found");
    // What ran, stayed: no rollback, and only one entry exists.
    assert_eq!(payload(&dir, &["ls"]).as_array().unwrap().len(), 1);
}

#[test]
fn a_dry_run_batch_writes_nothing() {
    let (_tmp, dir) = store();
    let out = apply_with(
        &dir,
        &["apply", "-j", "--dry-run"],
        r#"{"intents": [{"op": "add", "title": "Alpha"}, {"op": "add", "title": "Beta"}]}"#,
    );
    assert!(out.status.success(), "{}", stderr(&out));
    let envelope: serde_json::Value = serde_json::from_str(&stdout(&out)).unwrap();
    assert_eq!(envelope["data"]["dry_run"], serde_json::json!(true));
    assert_eq!(envelope["data"]["applied"].as_array().unwrap().len(), 2);
    assert!(payload(&dir, &["ls"]).as_array().unwrap().is_empty());
}

#[test]
fn a_malformed_batch_is_refused_before_anything_is_written() {
    let (_tmp, dir) = store();
    for bad in [
        "",
        "{not json",
        r#"{"intents": [{"op": "nope"}]}"#,
        r#"[{"op": "note", "ref": "a7b3"}]"#,
    ] {
        let out = apply(&dir, bad);
        assert!(!out.status.success(), "{bad:?} should fail");
        assert!(payload(&dir, &["ls"]).as_array().unwrap().is_empty());
    }
}

// --- Concurrency ----------------------------------------------------------

/// A lock held by another process makes a writer wait, because a mutation is a
/// read-modify-write of a whole document.
#[test]
fn a_held_lock_makes_a_writer_wait_but_not_a_reader() {
    let (_tmp, dir) = store();
    let holder = Command::new(env!("CARGO_BIN_EXE_tk"))
        .arg("-C")
        .arg(&dir)
        .args(["lock", "--", "sleep", "1"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lock holder");
    // Give the holder time to acquire the lock.
    std::thread::sleep(Duration::from_millis(250));

    // A read does not take the lock, so it is fast.
    let started = Instant::now();
    assert!(ok_in(&dir, &["ls"]).contains("no entries"));
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "reads do not wait for the lock (took {:?})",
        started.elapsed()
    );

    // A write waits for it, and then succeeds.
    let started = Instant::now();
    let r#ref = add(&dir, "Waited for the lock");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(400),
        "the write waited ({waited:?})"
    );
    assert_eq!(read_entry(&dir, &r#ref)["title"], "Waited for the lock");

    holder.wait_with_output().expect("holder exits");
}

/// Eight processes appending to one entry: with whole-file writes and a lock,
/// every append must survive. Without the lock this loses updates.
#[test]
fn concurrent_log_appends_are_not_lost() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Contended");

    let children: Vec<_> = (0..8)
        .map(|n| {
            let dir = dir.clone();
            let r#ref = r#ref.clone();
            std::thread::spawn(move || run_in(&dir, &["note", &r#ref, &format!("writer {n}")]))
        })
        .collect();
    for child in children {
        let out = child.join().expect("thread");
        assert!(out.status.success(), "{}", stderr(&out));
    }

    let entry = read_entry(&dir, &r#ref);
    let logs = entry["log"].as_array().expect("a log");
    assert_eq!(logs.len(), 8, "every append survived: {logs:?}");
    let mut messages: Vec<&str> = logs.iter().filter_map(|l| l["msg"].as_str()).collect();
    messages.sort_unstable();
    assert_eq!(
        messages,
        (0..8).map(|n| format!("writer {n}")).collect::<Vec<_>>()
    );
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

/// Concurrent opposite blocks: one must win, and the store must not end up in a
/// loop. Whichever order the lock grants, the loser is refused.
#[test]
fn concurrent_opposite_blocks_never_create_a_loop() {
    let (_tmp, dir) = store();
    let a = add(&dir, "Alpha");
    let b = add(&dir, "Beta");

    let spawn = |from: String, blocker: String| {
        let dir = dir.clone();
        std::thread::spawn(move || run_in(&dir, &["block", &from, &blocker]))
    };
    let first = spawn(b.clone(), a.clone());
    let second = spawn(a.clone(), b.clone());
    let results = [first.join().unwrap(), second.join().unwrap()];

    assert_eq!(
        results.iter().filter(|out| out.status.success()).count(),
        1,
        "exactly one of the two directions is accepted"
    );
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

/// Concurrent label deltas: `+x` and `-y` are read-modify-write, so the lock is
/// what keeps them all. A bare label *replaces* a set that was read earlier, so
/// it can legitimately drop a concurrent delta — that boundary is documented,
/// and this test pins the safe form.
#[test]
fn concurrent_label_deltas_are_not_lost() {
    let (_tmp, dir) = store();
    let r#ref = add(&dir, "Contended");
    ok_in(&dir, &["label", &r#ref, "base"]);

    let children: Vec<_> = (0..8)
        .map(|n| {
            let dir = dir.clone();
            let r#ref = r#ref.clone();
            std::thread::spawn(move || run_in(&dir, &["label", &r#ref, &format!("+tag{n}")]))
        })
        .collect();
    for child in children {
        let out = child.join().expect("thread");
        assert!(out.status.success(), "{}", stderr(&out));
    }

    let entry = read_entry(&dir, &r#ref);
    let labels = entry["labels"].as_array().expect("labels");
    assert_eq!(labels.len(), 9, "base plus eight deltas: {labels:?}");
    for n in 0..8 {
        assert!(
            labels.contains(&serde_json::json!(format!("tag{n}"))),
            "{labels:?}"
        );
    }
}

/// Eight processes creating entries at once: every ref is unique, and every
/// entry survives.
#[test]
fn concurrent_creates_allocate_unique_refs() {
    let (_tmp, dir) = store();
    let children: Vec<_> = (0..8)
        .map(|n| {
            let dir = dir.clone();
            std::thread::spawn(move || ok_in(&dir, &["add", &format!("Entry {n}"), "-q"]))
        })
        .collect();
    let refs: Vec<String> = children
        .into_iter()
        .map(|child| child.join().expect("thread").trim().to_owned())
        .collect();

    let mut unique = refs.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 8, "refs are unique: {refs:?}");
    assert_eq!(payload(&dir, &["ls"]).as_array().unwrap().len(), 8);
    assert_eq!(
        ok_in(&dir, &["check"]).trim(),
        "ok: the store is consistent"
    );
}

// --- Config ---------------------------------------------------------------

#[test]
fn config_shows_the_store_and_remembers_directory_aliases() {
    let (_tmp, dir) = store();
    add(&dir, "Alpha");
    let text = ok_in(&dir, &["config"]);
    assert!(text.contains("Format:  3"), "{text}");
    assert!(text.contains("Entries: 1"), "{text}");

    ok_in(&dir, &["config", "alias", "work", "/tmp"]);
    let text = ok_in(&dir, &["config"]);
    assert!(text.contains("work"), "{text}");
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".tasks/.tk.json")).unwrap())
            .unwrap();
    assert_eq!(raw["format"], serde_json::json!(3));
    assert_eq!(raw["aliases"]["work"], serde_json::json!("/tmp"));

    ok_in(&dir, &["config", "alias", "work"]);
    let raw: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join(".tasks/.tk.json")).unwrap())
            .unwrap();
    assert!(
        raw.get("aliases").is_none(),
        "removing the last alias drops the map: {raw}"
    );
}
