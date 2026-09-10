//! The migration script, run against fixtures for both older layouts.
//!
//! The script is deliberately not a `tk` subcommand — it runs once per store —
//! so this is the only thing that keeps it working. The strongest check is not
//! here but in the binary: after converting, `tk check` must find nothing wrong,
//! which means the conversion produced documents this binary accepts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const SCRIPT: &str = "tools/migrate_to_v3.py";

/// The path to a usable `python3`, or `None` to skip.
fn python3() -> Option<PathBuf> {
    let out = Command::new("python3").arg("--version").output().ok()?;
    out.status.success().then(|| PathBuf::from("python3"))
}

fn migrate(store: &Path, args: &[&str]) -> Output {
    Command::new(python3().expect("python3"))
        .arg(SCRIPT)
        .args(args)
        .arg(store)
        .output()
        .expect("run the migration script")
}

fn tk(store: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tk"))
        .args(["--tasks-dir", &store.display().to_string()])
        .args(args)
        .output()
        .expect("run tk")
}

fn summary(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Read the entry whose title matches, as the store's own reader sees it.
fn entry_with_title(store: &Path, title: &str) -> serde_json::Value {
    let out = tk(store, &["-j", "ls", "-a"]);
    let envelope: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).expect("envelope");
    envelope["data"]
        .as_array()
        .expect("entries")
        .iter()
        .find(|entry| entry["title"] == title)
        .unwrap_or_else(|| panic!("no entry titled {title:?} in {}", envelope["data"]))
        .clone()
}

/// Entry files only: `<ref>-<slug>.json` with a valid tk ref. A v0 or v1 store
/// has `.json` files of its own, so counting those would hide the difference
/// between "converted" and "not yet converted".
fn entry_files(store: &Path) -> Vec<String> {
    let crockford = "0123456789abcdefghjkmnpqrstvwxyz";
    let mut names: Vec<String> = std::fs::read_dir(store)
        .expect("read store")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| {
            let Some((head, _)) = name
                .strip_suffix(".json")
                .and_then(|stem| stem.split_once('-'))
            else {
                return false;
            };
            head.len() == 4 && head.chars().all(|c| crockford.contains(c))
        })
        .collect();
    names.sort();
    names
}

/// Every `.json` file at the top level, whatever it is.
fn all_json_files(store: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(store)
        .expect("read store")
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".json"))
        .collect();
    names.sort();
    names
}

/// A v0 store: `config.json` plus one JSON document per task.
fn v0_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join(".tasks");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(
        store.join("config.json"),
        r#"{"project":"proj","version":1,"defaults":{"priority":"medium","labels":[]},"clean_after":{"enabled":true,"days":30}}"#,
    )
    .unwrap();
    let write = |ref_: &str, body: serde_json::Value| {
        std::fs::write(
            store.join(format!("proj-{ref_}.json")),
            serde_json::to_string_pretty(&body).unwrap(),
        )
        .unwrap();
    };
    write(
        "a7b3",
        serde_json::json!({
            "project": "proj", "ref": "a7b3", "title": "Rewrite the auth layer",
            "description": "A long description that v2 has no field for.",
            "status": "done", "priority": "high",
            "labels": ["Backend", "backend", "api"],
            "logs": ["2026-01-10: did a thing", {"ts": "2026-01-11T09:00:00Z", "msg": "second"}],
            "created_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-11T09:00:00Z",
            "completed_at": "2026-01-10T12:00:00Z",
            "checkpoint": "nearly there",
            "links": ["https://example.com/spec"],
            "acceptance": ["parity test passes", {"text": "docs updated"}],
            "evidence": ["ran the full suite"],
            "blocked_by": ["proj-b7c4"], "parent": "proj-c7d5",
            "assignees": ["nick"], "due_date": "2026-02-01", "estimate": 3,
            "previous_ids": ["proj-old1"], "external": {"url": "x"}
        }),
    );
    write(
        "b7c4",
        serde_json::json!({
            "project": "proj", "ref": "b7c4", "title": "Write the parser",
            "status": "cancelled", "priority": "low", "labels": [],
            "logs": null, "created_at": "2026-01-02T00:00:00Z", "updated_at": "2026-01-05T00:00:00Z"
        }),
    );
    write(
        "c7d5",
        serde_json::json!({
            "project": "proj", "ref": "c7d5", "title": "Deferred thing",
            "status": "deferred", "priority": "none", "labels": ["someday"],
            "created_at": "2026-01-03T00:00:00Z", "updated_at": "2026-01-04T00:00:00Z"
        }),
    );
    (dir, store)
}

/// A v1 store: `store.json` plus `records/<ulid>.jsonl` event logs.
fn v1_fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let store = dir.path().join(".tasks");
    let records = store.join("records");
    std::fs::create_dir_all(&records).unwrap();
    std::fs::write(store.join("store.json"), r#"{"format":2,"project":"proj"}"#).unwrap();

    let id = "01HZZZZZZZZZZZZZZZZZZZZZZZ";
    let created = serde_json::json!({
        "id": id, "alias": "z9y8", "legacy_aliases": ["proj-old2"], "project": "proj",
        "title": "V1 thing", "description": null, "status": "open", "priority": "medium",
        "labels": [], "parent": null, "blocked_by": [], "logs": [],
        "created_at": "2026-03-01T00:00:00Z", "updated_at": "2026-03-01T00:00:00Z",
        "completed_at": null, "archived_at": null, "checkpoint": null,
        "links": [], "acceptance": [], "evidence": []
    });
    let events = [
        serde_json::json!({"ts":"2026-03-01T00:00:00Z","writer":"w","op":"created","data":created}),
        serde_json::json!({"ts":"2026-03-02T00:00:00Z","writer":"w","op":"log","data":{"msg":"first"}}),
        // `status`, `checkpoint`, and `labels.add` carry a bare value: v1's
        // folder deserialised the event's `data` directly as the field type.
        serde_json::json!({"ts":"2026-03-03T00:00:00Z","writer":"w","op":"labels.add","data":["backend"]}),
        serde_json::json!({"ts":"2026-03-04T00:00:00Z","writer":"w","op":"checkpoint","data":"halfway"}),
        serde_json::json!({"ts":"2026-03-05T00:00:00Z","writer":"w","op":"acceptance.add","data":["works"]}),
        serde_json::json!({"ts":"2026-03-06T00:00:00Z","writer":"w","op":"status","data":"done"}),
    ];
    let body: String = events.iter().map(|event| format!("{event}\n")).collect();
    std::fs::write(records.join(format!("{id}.jsonl")), body).unwrap();
    (dir, store)
}

#[test]
fn a_v0_store_converts_to_the_documented_shape() {
    if python3().is_none() {
        eprintln!("skipping: python3 is not available");
        return;
    }
    let (_tmp, store) = v0_fixture();

    // A dry run changes nothing.
    let out = migrate(&store, &["--dry-run"]);
    assert!(out.status.success(), "{}", summary(&out));
    assert!(!store.join(".tk.json").exists(), "a dry run writes nothing");
    assert!(
        entry_files(&store).is_empty(),
        "a dry run converts nothing: {:?}",
        entry_files(&store)
    );
    assert_eq!(
        all_json_files(&store).len(),
        4,
        "and the sources are still where they were: {:?}",
        all_json_files(&store)
    );

    let out = migrate(&store, &[]);
    assert!(out.status.success(), "{}", summary(&out));
    let text = summary(&out);
    assert!(text.contains("entries converted:   3"), "{text}");
    // Dropped fields are counted, never silently discarded.
    for field in [
        "description",
        "priority",
        "project",
        "due_date",
        "estimate",
        "parent",
    ] {
        assert!(
            text.contains(field),
            "{field} should be reported as dropped: {text}"
        );
    }

    // The format gate accepts what it produced.
    let out = tk(&store, &["check"]);
    assert!(
        out.status.success(),
        "the migrated store must be valid: {}",
        summary(&out)
    );

    // Every entry, including the closed ones, is there.
    assert_eq!(
        entry_files(&store).len(),
        3,
        "files: {:?}\nsummary: {text}",
        entry_files(&store)
    );
    let ids = std::fs::read_to_string(store.join(".tk.json")).unwrap();
    assert!(ids.contains("\"format\": 3"), "{ids}");

    // The done entry kept its completion time and everything v2 has a field for.
    let done = entry_with_title(&store, "Rewrite the auth layer");
    assert_eq!(done["state"], "done");
    assert_eq!(done["done"], serde_json::json!("2026-01-10T12:00:00Z"));
    assert_eq!(
        done["labels"],
        serde_json::json!(["api", "backend"]),
        "labels normalise"
    );
    // Fields v3 does not have become marked log entries, so nothing is lost and
    // nothing pretends to be something a reader can filter on.
    for gone in ["status", "acceptance", "evidence", "checkpoint"] {
        assert!(
            done.get(gone).is_none(),
            "{gone} is not part of the record: {done}"
        );
    }
    let log = done["log"].as_array().expect("a log");
    let messages: Vec<&str> = log.iter().filter_map(|l| l["msg"].as_str()).collect();
    assert!(messages.contains(&"status: nearly there"), "{messages:?}");
    assert!(
        messages.contains(&"acceptance: parity test passes"),
        "{messages:?}"
    );
    assert!(
        messages.contains(&"acceptance: docs updated"),
        "an object's text is read: {messages:?}"
    );
    assert!(
        messages.contains(&"verified: ran the full suite"),
        "evidence -> log: {messages:?}"
    );
    assert_eq!(log[0]["msg"], "did a thing", "a legacy log string is split");
    assert!(
        log[0]["ts"].as_str().unwrap().starts_with("2026-01-10"),
        "the date comes out of the string: {log:?}"
    );
    assert_eq!(log[1]["msg"], "second");

    // A blocker is remapped to the new store's ref for the same entry.
    let parser = entry_with_title(&store, "Write the parser");
    assert_eq!(done["blocked_by"], serde_json::json!([parser["ref"]]));
    assert_eq!(parser["state"], "dropped", "cancelled becomes dropped");
    assert!(
        parser["done"].as_str().is_some(),
        "a dropped entry has a time"
    );
    assert!(
        parser["log"].as_array().unwrap().is_empty(),
        "a null log list"
    );

    let deferred = entry_with_title(&store, "Deferred thing");
    assert_eq!(deferred["state"], "open", "deferred becomes open");
    assert_eq!(deferred["done"], serde_json::Value::Null);

    // Sources are moved, never deleted.
    assert!(store.join("legacy").is_dir());
    assert!(store.join("legacy/config.json").exists());
    assert_eq!(
        entry_files(&store).len(),
        3,
        "the old top-level task files are not still sitting there"
    );
    let moved = std::fs::read_dir(store.join("legacy")).unwrap().count();
    assert_eq!(moved, 4, "config.json plus three task files");
}

#[test]
fn a_v1_store_converts_by_folding_its_events() {
    if python3().is_none() {
        eprintln!("skipping: python3 is not available");
        return;
    }
    let (_tmp, store) = v1_fixture();

    let out = migrate(&store, &[]);
    assert!(out.status.success(), "{}", summary(&out));
    assert!(
        tk(&store, &["check"]).status.success(),
        "{}",
        summary(&tk(&store, &["check"]))
    );

    let entry = entry_with_title(&store, "V1 thing");
    assert_eq!(entry["state"], "done", "the folded status");
    assert_eq!(entry["labels"], serde_json::json!(["backend"]));
    let messages: Vec<&str> = entry["log"]
        .as_array()
        .expect("a log")
        .iter()
        .filter_map(|l| l["msg"].as_str())
        .collect();
    assert!(messages.contains(&"status: halfway"), "{messages:?}");
    assert!(messages.contains(&"acceptance: works"), "{messages:?}");
    assert_eq!(entry["log"][0]["msg"], "first");
    assert_eq!(entry["log"][0]["ts"], "2026-03-02T00:00:00Z");
    assert!(
        entry["done"].as_str().is_some(),
        "a closed entry has a time"
    );
    assert_eq!(
        entry["ref"], "z9y8",
        "a valid, free v1 alias is reused as the ref"
    );

    // The old ULID and the alias both map, so an old reference still resolves.
    let map = std::fs::read_to_string(store.join("MIGRATION.md")).unwrap();
    assert!(map.contains("01HZZZZZZZZZZZZZZZZZZZZZZZ"), "{map}");
    assert!(map.contains("proj-old2"), "{map}");
    assert!(map.contains("z9y8"), "{map}");

    // The old layout is gone, not read as empty by the next run.
    assert!(!store.join("store.json").exists());
    assert!(!store.join("records").exists());
    assert!(store.join("legacy/store.json").exists());
    assert!(store.join("legacy/records").is_dir());
}

#[test]
fn converting_twice_is_refused() {
    if python3().is_none() {
        eprintln!("skipping: python3 is not available");
        return;
    }
    let (_tmp, store) = v0_fixture();
    assert!(migrate(&store, &[]).status.success());
    let out = migrate(&store, &[]);
    assert!(!out.status.success(), "a second run must refuse");
    assert!(summary(&out).contains(".tk.json"), "{}", summary(&out));
}

/// An empty store is refused, because converting nothing is more likely a wrong
/// directory than an intention. `tk init` starts a fresh store instead.
#[test]
fn an_empty_store_is_refused_rather_than_converted() {
    if python3().is_none() {
        eprintln!("skipping: python3 is not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let store = dir.path().join(".tasks");
    std::fs::create_dir_all(&store).unwrap();
    std::fs::write(store.join("config.json"), r#"{"project":"proj"}"#).unwrap();
    let out = migrate(&store, &[]);
    assert!(!out.status.success());
    assert!(summary(&out).contains("no entries"), "{}", summary(&out));
    // Nothing was written, so the directory is still recognisably v0.
    assert!(!store.join(".tk.json").exists());
    assert!(!store.join("legacy").exists());
}
