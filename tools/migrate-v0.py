#!/usr/bin/env python3
"""One-shot conversion of a v0 tk store to the v1 record layout.

Deliberately *not* a `tk` subcommand. It runs once per store and then has no
reason to exist: keeping it in the binary would mean a permanent reader for a
format nobody writes, plus tests for it, plus a version gate to maintain. The
v1 binary refuses a v0 store outright, so this script is the path across, and
it is meant to be deleted after the cutover.

  v0: .tasks/config.json + .tasks/<project>-<ref>.json   (one document per task)
  v1: .tasks/store.json (format 2) + .tasks/records/<ulid>.jsonl

What it preserves:

  * every task's title, description, labels, status, and logs
    (legacy `cancelled` becomes `closed`)
  * the old handle: the 4-character ref becomes the v1 alias when it is unique
    and unambiguous, and the old `project-ref` ID stays resolvable as a legacy
    alias either way, so references written before the migration still work
  * the dependency graph, by rewriting `blocked_by` and `parent` from old IDs
    to new ones

Usage:
    tools/migrate-v0.py [--dry-run] STORE_DIR

It refuses to touch a store that is already v1, and moves the old files into
`STORE_DIR/legacy/` rather than deleting them.
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import secrets
import shutil
import sys
from pathlib import Path

CROCKFORD = "0123456789abcdefghjkmnpqrstvwxyz"
FORMAT = 2
WRITER = "migrate-0000"
ALIAS_LEN = 4
REF_LEN = 4

STATUSES = {"deferred", "open", "active", "done", "closed"}
STATUS_ALIASES = {"cancelled": "closed", "canceled": "closed", "todo": "open", "": "open"}


# --- identity ---------------------------------------------------------------


def encode_ulid(value: int) -> str:
    """128-bit value -> 26 Crockford characters, as the Rust side does."""
    out = []
    for _ in range(26):
        out.append(CROCKFORD[value & 0x1F])
        value >>= 5
    return "".join(reversed(out))


def new_id(ts_ms: int) -> str:
    value = ((ts_ms & ((1 << 48) - 1)) << 80) | secrets.randbits(80)
    return encode_ulid(value)


def is_valid_alias(value: str) -> bool:
    return len(value) == ALIAS_LEN and all(c in CROCKFORD for c in value)


def is_valid_id(value: str) -> bool:
    return len(value) == 26 and all(c in CROCKFORD for c in value)


# --- reading the v0 store ---------------------------------------------------


def now_ms() -> int:
    return int(dt.datetime.now(dt.timezone.utc).timestamp() * 1000)


def parse_ts(value: str, fallback_ms: int) -> str:
    """Keep a parseable legacy timestamp; otherwise stamp it now."""
    if isinstance(value, str) and value:
        text = value.strip()
        try:
            parsed = dt.datetime.fromisoformat(text.replace("Z", "+00:00"))
            if parsed.tzinfo is None:
                parsed = parsed.replace(tzinfo=dt.timezone.utc)
            return parsed.astimezone(dt.timezone.utc).isoformat().replace("+00:00", "Z")
        except ValueError:
            pass
    stamp = dt.datetime.fromtimestamp(fallback_ms / 1000, dt.timezone.utc)
    return stamp.isoformat().replace("+00:00", "Z")


def load_legacy_tasks(store: Path) -> list[dict]:
    tasks = []
    for path in sorted(store.glob("*.json")):
        if path.name in {"config.json", "store.json"}:
            continue
        try:
            data = json.loads(path.read_text())
        except json.JSONDecodeError as exc:
            raise SystemExit(f"{path}: not valid JSON ({exc})") from exc
        if not isinstance(data, dict) or "project" not in data or "ref" not in data:
            print(f"  skipping {path.name}: not a v0 task document", file=sys.stderr)
            continue
        data["_path"] = path
        data["_old_id"] = f"{data['project']}-{data['ref']}"
        tasks.append((data["_old_id"], data))
    return [t for _, t in tasks]


def normalise_status(value) -> str:
    text = str(value or "").strip().lower()
    text = STATUS_ALIASES.get(text, text)
    return text if text in STATUSES else "open"


def legacy_logs(raw) -> list[dict]:
    """Legacy logs were objects, plain strings, or `"<timestamp>: <message>"`."""
    if not isinstance(raw, list):
        return []
    out = []
    for entry in raw:
        if isinstance(entry, dict):
            out.append({"ts": str(entry.get("ts", "")), "msg": str(entry.get("msg", ""))})
        elif isinstance(entry, str):
            ts, msg = "", entry
            if ":" in entry:
                head, _, tail = entry.partition(":")
                parsed = None
                for fmt in ("%Y-%m-%d", "%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"):
                    try:
                        parsed = dt.datetime.strptime(head.strip(), fmt)
                        break
                    except ValueError:
                        continue
                if parsed is not None:
                    ts, msg = head.strip(), tail.strip()
            out.append({"ts": ts, "msg": msg})
    return out


# --- planning ---------------------------------------------------------------


def plan(tasks: list[dict], config: dict) -> dict:
    """Assign a v1 identity to every legacy task, then rewrite references."""
    id_map = {task["_old_id"]: new_id(now_ms()) for task in tasks}
    ref_counts: dict[str, int] = {}
    for task in tasks:
        ref = str(task["ref"])
        ref_counts[ref] = ref_counts.get(ref, 0) + 1

    taken: set[str] = set()
    for task in tasks:
        ref = str(task["ref"])
        # Reuse the old handle when it is a valid, unique v1 alias: `tk show a7b3`
        # keeps working for most tasks after the migration.
        if is_valid_alias(ref) and ref_counts[ref] == 1 and ref not in taken:
            task["_alias"] = ref
        else:
            alias = "".join(secrets.choice(CROCKFORD) for _ in range(ALIAS_LEN))
            while alias in taken:
                alias = "".join(secrets.choice(CROCKFORD) for _ in range(ALIAS_LEN))
            task["_alias"] = alias
        taken.add(task["_alias"])

    for task in tasks:
        ref = str(task["ref"])
        aliases = [task["_old_id"]]
        for previous in task.get("previous_ids") or []:
            if isinstance(previous, str) and previous and previous not in aliases:
                aliases.append(previous)
        # The bare ref is only safe to keep when nothing else answers to it.
        if ref != task["_alias"] and ref_counts[ref] == 1:
            aliases.append(ref)
        task["_legacy_aliases"] = aliases

        def remap(entry):
            return id_map.get(entry, entry) if isinstance(entry, str) else entry

        task["_blocked_by"] = [remap(b) for b in (task.get("blocked_by") or [])]
        parent = task.get("parent")
        task["_parent"] = remap(parent) if parent else None

    return {
        "id_map": id_map,
        "config": config,
    }


def build_created(task: dict) -> dict:
    stamp = parse_ts(task.get("created_at", ""), now_ms())
    updated = parse_ts(task.get("updated_at", "") or task.get("created_at", ""), now_ms())
    state = {
        "id": task["_new_id"],
        "alias": task["_alias"],
        "legacy_aliases": task["_legacy_aliases"],
        "project": str(task.get("project", "tk")),
        "title": str(task.get("title", "")),
        "status": normalise_status(task.get("status")),
        "priority": int(task.get("priority", 3) or 3),
        "labels": [str(x) for x in (task.get("labels") or [])],
        "parent": task["_parent"],
        "blocked_by": task["_blocked_by"],
        "logs": [],
        "created_at": stamp,
        "updated_at": updated,
        "completed_at": task.get("completed_at"),
        "archived_at": task.get("archived_at"),
        "checkpoint": task.get("checkpoint"),
        "links": [str(x) for x in (task.get("links") or [])],
        "acceptance": [str(x) for x in (task.get("acceptance") or [])],
        "evidence": [str(x) for x in (task.get("evidence") or [])],
    }
    description = task.get("description")
    if description:
        state["description"] = str(description)
    return state


# --- writing ----------------------------------------------------------------


def event(ts: str, op: str, data) -> str:
    return json.dumps({"ts": ts, "writer": WRITER, "op": op, "data": data}, separators=(",", ":"))


def write_store(store: Path, tasks: list[dict], config: dict, dry_run: bool) -> None:
    records = store / "records"
    if not dry_run:
        records.mkdir(parents=True, exist_ok=True)

    store_config = {
        "format": FORMAT,
        "version": 1,
        "project": config.get("project", "tk"),
        "defaults": {
            key: value
            for key, value in (config.get("defaults") or {}).items()
            if key in {"priority", "labels"}
        },
        "clean_after": config.get("clean_after", 14),
    }
    if config.get("aliases"):
        store_config["aliases"] = config["aliases"]

    for task in tasks:
        state = build_created(task)
        path = records / f"{state['id']}.jsonl"
        lines = [event(state["created_at"], "created", state)]
        for entry in legacy_logs(task.get("logs")):
            stamp = parse_ts(entry["ts"], now_ms()) if entry["ts"] else state["updated_at"]
            lines.append(event(stamp, "log", {"ts": stamp, "msg": entry["msg"]}))
        if not dry_run:
            path.write_text("\n".join(lines) + "\n")

    if not dry_run:
        (store / "store.json").write_text(json.dumps(store_config, indent=2) + "\n")
        gitignore = store / ".gitignore"
        if not gitignore.exists():
            gitignore.write_text("# tk lock artifacts\n.lock\n.tmp.*\n")
        legacy = store / "legacy"
        legacy.mkdir(exist_ok=True)
        for path in list(store.glob("*.json")):
            if path.name != "store.json":
                shutil.move(str(path), str(legacy / path.name))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("store", type=Path, help="the .tasks directory to convert")
    parser.add_argument(
        "--dry-run", action="store_true", help="report what would change, write nothing"
    )
    args = parser.parse_args()

    store = args.store
    if not store.is_dir():
        print(f"not a directory: {store}", file=sys.stderr)
        return 1
    if (store / "store.json").exists():
        print(
            f"{store} is already a v1 store (store.json exists); refusing to run twice",
            file=sys.stderr,
        )
        return 1
    if (store / "records").is_dir() and any((store / "records").iterdir()):
        print(f"{store}/records is not empty; refusing to mix layouts", file=sys.stderr)
        return 1

    config_path = store / "config.json"
    config = {}
    if config_path.exists():
        config = json.loads(config_path.read_text())

    tasks = load_legacy_tasks(store)
    if not tasks:
        print(f"no v0 task documents found in {store}", file=sys.stderr)
        return 1

    result = plan(tasks, config)
    id_map = result["id_map"]
    for task in tasks:
        task["_new_id"] = id_map[task["_old_id"]]
        task["_id_map"] = id_map
        if not is_valid_id(task["_new_id"]):
            print(f"internal error: generated a malformed ID {task['_new_id']}", file=sys.stderr)
            return 1

    print(f"{'Would migrate' if args.dry_run else 'Migrating'} {len(tasks)} task(s):")
    for task in tasks:
        print(f"  {task['_old_id']:<28} -> {task['_new_id']} ({task['_alias']})")
    write_store(store, tasks, config, args.dry_run)

    if args.dry_run:
        print("\nDry run: nothing written.")
    else:
        print(f"\nWrote {len(tasks)} record(s) to {store / 'records'}")
        print(f"Old files kept in {store / 'legacy'} — delete them once satisfied.")
        print("Run 'tk check' to confirm the store is clean.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
