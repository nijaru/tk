#!/usr/bin/env python3
"""One-shot conversion of a tk store to the format-3 JSON layout.

Deliberately *not* a `tk` subcommand, for the same reason the earlier v0 -> v1
script was not: it runs once per store and then has no reason to exist. Keeping
it in the binary would mean a permanent reader for a format nobody writes, plus
its tests and its version gate.

  v3: .tasks/.tk.json ({"format": 3}) + .tasks/<ref>-<slug>.json per entry

Two source layouts are handled:

  v0: .tasks/config.json + .tasks/<project>-<ref>.json        (one doc per task)
  v1: .tasks/store.json (format 2) + .tasks/records/<ulid>.jsonl

The spec this implements is authoritative. In particular it is the JSON format,
not the Markdown draft: one pretty-printed JSON document per entry, filenames
`<ref>-<slug>.json`, and a `.tk.json` carrying `{"format": 3}`.

What it does:

  * converts both layouts into one JSON file per entry;
  * reuses a legacy 4-character ref/alias as the new ref when it is valid and
    free, and otherwise generates a fresh Crockford base32 ref;
  * rewrites `blocked_by` from old handles (v0 `project-ref`, v1 ULID/id/alias)
    to new refs, dropping and reporting anything that does not resolve;
  * folds v1 JSONL events exactly as `src/record.rs` does, so the projected
    state matches what the v1 binary would have shown;
  * moves every consumed source file into `<store>/legacy/` -- never deletes;
  * writes `<store>/MIGRATION.md` mapping every old handle to its new ref;
  * writes `<store>/.tk.json`.

Usage:
    tools/migrate_to_v3.py [--dry-run] STORE_DIR
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import random
import re
import shutil
import sys
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path

# --- constants --------------------------------------------------------------

# Crockford base32: no i, l, o, u.
CROCKFORD = "0123456789abcdefghjkmnpqrstvwxyz"
CROCKFORD_SET = frozenset(CROCKFORD)
REF_LEN = 4
SLUG_MAX = 60
FORMAT = 3
# A v3 entry filename: <4 crockford chars>-<slug>.json.
ENTRY_NAME = re.compile(rf"^[{CROCKFORD}]{{4}}-.*\.json$")
# Fixed seed so a --dry-run plan is byte-for-byte the plan a real run writes.
RNG_SEED = 0x7A2B3C4D

# The v1 fold vocabulary, copied from src/record.rs.
LIST_ADD = {
    "labels.add": "labels",
    "links.add": "links",
    "acceptance.add": "acceptance",
    "evidence.add": "evidence",
    "block.add": "blocked_by",
}
LIST_REMOVE = {
    "labels.remove": "labels",
    "links.remove": "links",
    "acceptance.remove": "acceptance",
    "evidence.remove": "evidence",
    "block.remove": "blocked_by",
}
LIST_SET = {
    "labels.set": "labels",
    "links.set": "links",
    "acceptance.set": "acceptance",
    "evidence.set": "evidence",
}

# Fields the target format keeps, per source layout. Everything else is dropped
# and counted so nothing disappears silently.
V0_MAPPED = {
    "title",
    "ref",
    "status",
    "labels",
    "logs",
    "created_at",
    "updated_at",
    "completed_at",
    "checkpoint",
    "acceptance",
    "evidence",
    "blocked_by",
    "previous_ids",
}
# `project` is used to build the legacy handle but is still dropped on purpose.
V1_MAPPED = {
    "id",
    "alias",
    "legacy_aliases",
    "title",
    "status",
    "state",
    "labels",
    "logs",
    "created_at",
    "updated_at",
    "completed_at",
    "checkpoint",
    "blocked_by",
    "acceptance",
    "evidence",
}

_NOW = ""


class MigrationError(Exception):
    """A refusal or a source problem; printed to stderr, exit 1."""


# --- time -------------------------------------------------------------------


def now_rfc3339() -> str:
    return (
        dt.datetime.now(dt.timezone.utc)
        .replace(microsecond=0)
        .isoformat()
        .replace("+00:00", "Z")
    )


def is_parseable_ts(value: object) -> bool:
    if not isinstance(value, str):
        return False
    text = value.strip()
    if not text:
        return False
    candidate = text[:-1] + "+00:00" if text[-1] in "Zz" else text
    try:
        dt.datetime.fromisoformat(candidate)
    except ValueError:
        return False
    return True


def keep_ts(value: object) -> str | None:
    """Return a source timestamp verbatim when it parses; never reformat it."""
    if isinstance(value, str) and is_parseable_ts(value):
        return value.strip()
    return None


def resolve_ts(value: object, fallback: object = None) -> str:
    """A parseable source timestamp, else the fallback, else now."""
    return keep_ts(value) or keep_ts(fallback) or _NOW


# --- small value helpers ----------------------------------------------------


def as_text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, str):
        return value
    return str(value)


def as_optional_text(value: object) -> str | None:
    if value is None:
        return None
    if isinstance(value, str):
        return value
    return str(value)


def as_text_list(value: object) -> list[str]:
    if not isinstance(value, list):
        return []
    return [as_text(item) for item in value]


def entry_text(item: object) -> str | None:
    """An acceptance/evidence item as a plain string, or None to skip it."""
    if isinstance(item, str):
        text = item.strip()
        return text or None
    if isinstance(item, dict):
        for key in ("text", "title", "description"):
            value = item.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
    return None


def normalize_label(value: object) -> str | None:
    if not isinstance(value, (str, int, float)):
        return None
    out = []
    for ch in str(value).strip().lower():
        if ("a" <= ch <= "z") or ("0" <= ch <= "9") or ch == "-":
            out.append(ch)
        else:
            out.append("-")
    cleaned = re.sub(r"-{2,}", "-", "".join(out)).strip("-")
    return cleaned or None


def normalize_labels(values: object) -> tuple[list[str], int]:
    if not isinstance(values, list):
        return [], 0
    seen: list[str] = []
    dropped = 0
    for value in values:
        label = normalize_label(value)
        if label is None:
            dropped += 1
        elif label not in seen:
            seen.append(label)
    return sorted(seen), dropped


def valid_ref(value: object) -> bool:
    return (
        isinstance(value, str)
        and len(value) == REF_LEN
        and all(ch in CROCKFORD_SET for ch in value)
    )


def slugify(title: object) -> str:
    out: list[str] = []
    pending = False
    for ch in as_text(title):
        if ch.isascii() and ch.isalnum():
            if pending and out:
                out.append("-")
            pending = False
            out.append(ch.lower())
        else:
            pending = True
    slug = "".join(out)
    if len(slug) > SLUG_MAX:
        slug = slug[:SLUG_MAX].rstrip("-")
    return slug or "entry"


def parse_legacy_log_string(value: str) -> tuple[str, str]:
    """`"<ts>: <msg>"`, a bare timestamp, or a plain message."""
    if is_parseable_ts(value):
        return value.strip(), ""
    for index, ch in enumerate(value):
        if ch == ":" and is_parseable_ts(value[:index]):
            return value[:index].strip(), value[index + 1 :].lstrip(" \t")
    return "", value


def normalize_log_item(item: object, event_ts: object = None) -> dict | None:
    """A log entry as `{"ts", "msg"}`; None when the item carries nothing."""
    if item is None:
        return None
    if isinstance(item, dict):
        ts = as_text(item.get("ts"))
        msg = as_text(item.get("msg"))
        return {"ts": ts or as_text(event_ts), "msg": msg}
    if isinstance(item, str):
        ts, msg = parse_legacy_log_string(item)
        return {"ts": ts or as_text(event_ts), "msg": msg}
    return None


# --- v1 event fold (mirrors src/record.rs) ----------------------------------


def fold_v1(events: list[dict]) -> tuple[dict, int]:
    state: dict | None = None
    unknown = 0
    for event in events:
        op = event.get("op")
        data = event.get("data")
        ts = event.get("ts")
        if op in ("created", "snapshot"):
            if not isinstance(data, dict):
                raise MigrationError("a created/snapshot event has non-object data")
            state = dict(data)
            continue
        if state is None:
            raise MigrationError("a record has an event before its created event")
        if op == "project":
            state["project"] = as_text(data)
        elif op == "title":
            state["title"] = as_text(data)
        elif op == "description":
            state["description"] = as_optional_text(data)
        elif op == "priority":
            state["priority"] = data
        elif op == "status":
            raw = as_text(data).strip().lower()
            state["status"] = raw
            # Completion time is derived from the transition, exactly as v1.
            state["completed_at"] = ts if raw == "done" else None
        elif op == "checkpoint":
            checkpoint = as_optional_text(data)
            state["checkpoint"] = (
                checkpoint if checkpoint and checkpoint.strip() else None
            )
        elif op == "log":
            entry = normalize_log_item(data, ts)
            if entry is not None:
                state.setdefault("logs", []).append(entry)
        elif op in LIST_ADD:
            target = state.setdefault(LIST_ADD[op], [])
            if not isinstance(target, list):
                target = state[LIST_ADD[op]] = []
            for item in as_text_list(data):
                item = item.strip()
                if item and item not in target:
                    target.append(item)
        elif op in LIST_REMOVE:
            target = state.get(LIST_REMOVE[op])
            if not isinstance(target, list):
                continue
            doomed = [item.strip() for item in as_text_list(data)]
            state[LIST_REMOVE[op]] = [
                item for item in target if not any(d == item for d in doomed)
            ]
        elif op in LIST_SET:
            state[LIST_SET[op]] = as_text_list(data)
        elif op == "parent.set":
            state["parent"] = as_text(data)
        elif op == "parent.clear":
            state["parent"] = None
        elif op == "archived":
            state["archived_at"] = ts
        elif op == "unarchived":
            state["archived_at"] = None
        else:
            # An op this script does not know is a newer writer's business.
            # v1 returns before touching updated_at, so neither do we.
            unknown += 1
            continue
        state["updated_at"] = ts
    if state is None:
        raise MigrationError("a record has no created event")
    return state, unknown


def map_state(raw: object) -> str:
    text = as_text(raw).strip().lower()
    if text == "done":
        return "done"
    if text in ("closed", "cancelled", "canceled"):
        return "dropped"
    return "open"


# --- the plan ---------------------------------------------------------------


@dataclass
class Entry:
    source: Path
    kind: str
    legacy_ref: object
    old_handles: list[str]
    identity_keys: list[str]
    title: object
    state_raw: object
    created_raw: object
    updated_raw: object
    completed_raw: object
    labels_raw: object
    checkpoint: object
    acceptance_raw: object
    evidence_raw: object
    logs: list[dict]
    blocked_raw: object
    dropped_fields: Counter = field(default_factory=Counter)
    damaged: list[str] = field(default_factory=list)
    skipped_logs: int = 0
    ref: str = ""
    reused: bool = False
    slug: str = ""
    filename: str = ""
    document: dict = field(default_factory=dict)
    refs_remapped: int = 0
    refs_dropped: list[str] = field(default_factory=list)
    acceptance_skipped: int = 0
    evidence_skipped: int = 0
    labels_dropped: int = 0


def add_dropped(counter: Counter, fields: set[str], source: dict) -> None:
    for key in source:
        if key not in fields and not key.startswith("_"):
            counter[key] += 1


def parse_v0(store: Path) -> tuple[list[Entry], list[Path], dict]:
    config_path = store / "config.json"
    try:
        config = json.loads(config_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise MigrationError(f"{config_path}: cannot read v0 config ({exc})")
    if not isinstance(config, dict):
        raise MigrationError(f"{config_path}: v0 config is not a JSON object")

    entries: list[Entry] = []
    consumed: list[Path] = [config_path]
    for path in sorted(store.glob("*.json")):
        if path.name.startswith(".") or path.name in ("config.json", "store.json"):
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            raise MigrationError(f"{path}: not valid JSON ({exc})")
        if not isinstance(data, dict) or "project" not in data or "ref" not in data:
            # A stray JSON file is not a task document; leave it alone.
            continue
        consumed.append(path)
        entries.append(build_v0_entry(path, data))
    return entries, consumed, config


def build_v0_entry(path: Path, data: dict) -> Entry:
    project = as_text(data.get("project"))
    ref = as_text(data.get("ref"))
    old_id = f"{project}-{ref}" if project else ref

    old_handles: list[str] = []
    for handle in [old_id, ref, *(data.get("previous_ids") or [])]:
        text = as_text(handle).strip()
        if text and text not in old_handles:
            old_handles.append(text)

    dropped: Counter = Counter()
    add_dropped(dropped, V0_MAPPED, data)

    logs: list[dict] = []
    skipped = 0
    raw_logs = data.get("logs")
    if isinstance(raw_logs, list):
        for item in raw_logs:
            entry = normalize_log_item(item)
            if entry is None:
                skipped += 1
            else:
                logs.append(entry)

    return Entry(
        source=path,
        kind="v0",
        legacy_ref=ref,
        old_handles=old_handles,
        identity_keys=list(old_handles),
        title=data.get("title"),
        state_raw=data.get("status"),
        created_raw=data.get("created_at"),
        updated_raw=data.get("updated_at"),
        completed_raw=data.get("completed_at"),
        labels_raw=data.get("labels"),
        checkpoint=data.get("checkpoint"),
        acceptance_raw=data.get("acceptance"),
        evidence_raw=data.get("evidence"),
        logs=logs,
        blocked_raw=data.get("blocked_by"),
        dropped_fields=dropped,
        skipped_logs=skipped,
    )


def parse_v1(store: Path) -> tuple[list[Entry], list[Path], dict]:
    store_path = store / "store.json"
    try:
        config = json.loads(store_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise MigrationError(f"{store_path}: cannot read v1 store config ({exc})")
    if not isinstance(config, dict) or config.get("format") != 2:
        raise MigrationError(
            f"{store_path}: expected a format-2 v1 store, found "
            f"{config.get('format') if isinstance(config, dict) else config!r}"
        )
    records = store / "records"
    if not records.is_dir():
        raise MigrationError(f"{records}: v1 store has no records directory")

    entries: list[Entry] = []
    for path in sorted(records.glob("*.jsonl")):
        entries.append(build_v1_entry(path))
    return entries, [store_path, records], config


def build_v1_entry(path: Path) -> Entry:
    raw = path.read_text(encoding="utf-8")
    torn = bool(raw) and not raw.endswith("\n")
    if torn:
        cut = raw.rfind("\n")
        raw = raw[: cut + 1] if cut != -1 else ""

    events: list[dict] = []
    damaged: list[str] = []
    for number, line in enumerate(raw.split("\n"), start=1):
        if not line.strip():
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError as exc:
            damaged.append(f"line {number}: {exc}")
            continue
        if not isinstance(event, dict):
            damaged.append(f"line {number}: not a JSON object")
            continue
        events.append(event)

    try:
        state, _unknown = fold_v1(events)
    except MigrationError as exc:
        raise MigrationError(f"{path}: {exc}")

    ident = as_text(state.get("id")) or path.stem
    alias = as_text(state.get("alias"))
    legacy_aliases = [
        as_text(item).strip()
        for item in (state.get("legacy_aliases") or [])
        if as_text(item).strip()
    ]

    old_handles: list[str] = []
    for handle in [alias, ident, *legacy_aliases]:
        if handle and handle not in old_handles:
            old_handles.append(handle)

    dropped: Counter = Counter()
    add_dropped(dropped, V1_MAPPED, state)

    logs: list[dict] = []
    skipped = 0
    for item in state.get("logs") or []:
        entry = normalize_log_item(item)
        if entry is None:
            skipped += 1
        else:
            logs.append(entry)

    # The v1 fold already put a {ts,msg} shape on the state; the fold's own
    # event timestamps are preserved, and `parse_legacy_log_string` inside
    # `normalize_log_item` is a no-op for them.

    state_raw = state.get("status", state.get("state"))

    return Entry(
        source=path,
        kind="v1",
        legacy_ref=alias,
        old_handles=old_handles,
        identity_keys=[ident, alias, *legacy_aliases],
        title=state.get("title"),
        state_raw=state_raw,
        created_raw=state.get("created_at"),
        updated_raw=state.get("updated_at"),
        completed_raw=state.get("completed_at"),
        labels_raw=state.get("labels"),
        checkpoint=state.get("checkpoint"),
        acceptance_raw=state.get("acceptance"),
        evidence_raw=state.get("evidence"),
        logs=logs,
        blocked_raw=state.get("blocked_by"),
        dropped_fields=dropped,
        damaged=damaged,
        skipped_logs=skipped,
    )


# --- ref assignment and document building -----------------------------------


def assign_refs(entries: list[Entry]) -> tuple[int, int]:
    """Reuse a valid, unique legacy ref; generate the rest. Returns counts."""
    counts = Counter(
        entry.legacy_ref
        for entry in entries
        if valid_ref(entry.legacy_ref)
    )
    taken: set[str] = set()

    for entry in entries:
        candidate = entry.legacy_ref
        if valid_ref(candidate) and counts[candidate] == 1 and candidate not in taken:
            entry.ref = candidate
            entry.reused = True
            taken.add(candidate)

    rng = random.Random(RNG_SEED)
    reused = 0
    generated = 0
    for entry in entries:
        if entry.reused:
            reused += 1
            continue
        while True:
            candidate = "".join(rng.choice(CROCKFORD) for _ in range(REF_LEN))
            if candidate not in taken:
                break
        entry.ref = candidate
        taken.add(candidate)
        generated += 1
    return reused, generated


def assign_filenames(entries: list[Entry], reserved: set[str]) -> None:
    used: set[str] = set()
    for entry in entries:
        entry.slug = slugify(entry.title)
        base = f"{entry.ref}-{entry.slug}"
        name = f"{base}.json"
        suffix = 1
        while name in used or name in reserved:
            suffix += 1
            name = f"{base}-{suffix}.json"
        used.add(name)
        entry.filename = name


def build_handle_map(entries: list[Entry]) -> dict[str, str]:
    handle_map: dict[str, str] = {}
    for entry in entries:
        for key in entry.identity_keys:
            if key and key not in handle_map:
                handle_map[key] = entry.ref
    return handle_map


def build_document(entry: Entry, handle_map: dict[str, str], refs: set[str]) -> None:
    updated = resolve_ts(entry.updated_raw)
    created = resolve_ts(entry.created_raw, updated)
    state = map_state(entry.state_raw)

    done = None
    if state in ("done", "dropped"):
        done = resolve_ts(entry.completed_raw, updated)

    labels, labels_dropped = normalize_labels(entry.labels_raw)

    blocked_by: list[str] = []
    for raw in entry.blocked_raw or []:
        key = as_text(raw).strip()
        if not key:
            continue
        if key in handle_map:
            target = handle_map[key]
        elif valid_ref(key) and key in refs:
            target = key
        else:
            entry.refs_dropped.append(key)
            continue
        if target not in blocked_by:
            blocked_by.append(target)
        entry.refs_remapped += 1

    acceptance: list[str] = []
    for item in entry.acceptance_raw or []:
        text = entry_text(item)
        if text is None:
            entry.acceptance_skipped += 1
        else:
            acceptance.append(text)

    log: list[dict] = []
    for item in entry.logs:
        log.append(
            {
                "ts": resolve_ts(item.get("ts"), updated),
                "msg": as_text(item.get("msg")),
            }
        )
    for item in entry.evidence_raw or []:
        text = entry_text(item)
        if text is None:
            entry.evidence_skipped += 1
        else:
            log.append({"ts": updated, "msg": f"verified: {text}"})

    checkpoint = as_text(entry.checkpoint).strip()

    document: dict = {}
    document["ref"] = entry.ref
    document["title"] = as_text(entry.title)
    document["state"] = state
    document["labels"] = labels
    document["created"] = created
    document["updated"] = updated
    document["done"] = done
    document["blocked_by"] = blocked_by
    if checkpoint:
        document["status"] = checkpoint
    document["acceptance"] = acceptance
    document["log"] = log

    entry.document = document
    entry.labels_dropped = labels_dropped


# --- writing ----------------------------------------------------------------


def write_text_atomic(path: Path, text: str) -> None:
    tmp = path.with_name(".tmp." + path.name)
    tmp.write_text(text, encoding="utf-8")
    os.replace(tmp, path)


def write_entries(store: Path, entries: list[Entry]) -> None:
    for entry in entries:
        text = json.dumps(entry.document, indent=2, ensure_ascii=False) + "\n"
        write_text_atomic(store / entry.filename, text)


def write_migration_md(store: Path, handle_map: dict[str, str]) -> None:
    lines = [
        "# Migration map",
        "",
        "Legacy handles from the pre-format-3 store, mapped to the ref that",
        "replaced them. This file is informational and safe to delete once no",
        "old reference needs looking up.",
        "",
    ]
    for handle in sorted(handle_map):
        lines.append(f"{handle} -> {handle_map[handle]}")
    lines.append("")
    write_text_atomic(store / "MIGRATION.md", "\n".join(lines))


def write_format_marker(store: Path) -> None:
    write_text_atomic(store / ".tk.json", json.dumps({"format": FORMAT}) + "\n")


def unique_destination(path: Path) -> Path:
    if not path.exists():
        return path
    stem, suffix = path.stem, path.suffix
    number = 1
    while True:
        candidate = path.with_name(f"{stem}.{number}{suffix}")
        if not candidate.exists():
            return candidate
        number += 1


def move_to_legacy(store: Path, sources: list[Path]) -> None:
    legacy = store / "legacy"
    legacy.mkdir(exist_ok=True)
    for source in sources:
        shutil.move(str(source), str(unique_destination(legacy / source.name)))


# --- reporting --------------------------------------------------------------


def print_plan(store: Path, entries: list[Entry], kind: str, dry: bool) -> None:
    verb = "Would convert" if dry else "Converting"
    print(f"{verb} {len(entries)} {kind} entr{'y' if len(entries) == 1 else 'ies'} in {store}:")
    for entry in entries:
        handle = entry.old_handles[0] if entry.old_handles else entry.source.name
        origin = "reused" if entry.reused else "new"
        print(
            f"  {handle:<32} -> {entry.ref}  {entry.filename}  "
            f"[{entry.document['state']}, {origin}]"
        )
        if entry.damaged:
            for problem in entry.damaged:
                print(f"      damaged: {problem}")


def print_summary(
    entries: list[Entry],
    reused: int,
    generated: int,
    handle_map: dict[str, str],
    kind: str,
    config: dict,
) -> None:
    dropped_refs: list[str] = []
    fields: Counter = Counter()
    labels_dropped = 0
    acceptance_skipped = 0
    evidence_skipped = 0
    logs_skipped = 0
    refs_remapped = 0
    for entry in entries:
        dropped_refs.extend(entry.refs_dropped)
        fields.update(entry.dropped_fields)
        labels_dropped += entry.labels_dropped
        acceptance_skipped += entry.acceptance_skipped
        evidence_skipped += entry.evidence_skipped
        logs_skipped += entry.skipped_logs
        refs_remapped += entry.refs_remapped

    print()
    print("Summary")
    print(f"  entries converted:   {len(entries)} ({kind})")
    print(f"  refs reused:         {reused}")
    print(f"  refs generated:      {generated}")
    print(f"  references remapped: {refs_remapped}")
    print(f"  references dropped:  {len(dropped_refs)}")
    for handle in dropped_refs:
        print(f"    - {handle}")
    if fields:
        rendered = ", ".join(f"{name} {count}" for name, count in sorted(fields.items()))
        print(f"  fields dropped:      {rendered}")
    else:
        print("  fields dropped:      none")
    if logs_skipped:
        print(f"  log entries skipped: {logs_skipped}")
    if acceptance_skipped:
        print(f"  acceptance skipped:  {acceptance_skipped}")
    if evidence_skipped:
        print(f"  evidence skipped:    {evidence_skipped}")
    if labels_dropped:
        print(f"  labels skipped:      {labels_dropped}")
    print(f"  old handles mapped:  {len(handle_map)} (see MIGRATION.md)")
    if kind == "v0":
        dropped_config = sorted(k for k in config if k not in ("format",))
        if dropped_config:
            print(f"  store config dropped: {', '.join(dropped_config)}")


# --- entry point ------------------------------------------------------------


def run(store: Path, dry_run: bool) -> int:
    global _NOW
    _NOW = now_rfc3339()

    if (store / ".tk.json").exists():
        raise MigrationError(
            f"{store} already has .tk.json; it is already a format-3 store"
        )

    if (store / "store.json").exists():
        entries, consumed, config = parse_v1(store)
        kind = "v1"
    elif (store / "config.json").exists():
        entries, consumed, config = parse_v0(store)
        kind = "v0"
    else:
        raise MigrationError(
            f"{store} is not a tk store: no store.json (v1) or config.json (v0)"
        )

    # Refuse if v3 entry files are already present. Source files that happen to
    # look like entry names are excluded -- they are consumed, not collisions.
    # This runs before the empty-store check so a stray entry file still refuses.
    source_names = {path.name for path in consumed}
    conflicts = sorted(
        path.name
        for path in store.glob("*.json")
        if ENTRY_NAME.match(path.name)
        and not path.name.startswith(".")
        and path.name not in source_names
    )
    if conflicts:
        raise MigrationError(
            f"{store} already contains entry files: {', '.join(conflicts)}; "
            "refusing to overwrite them"
        )

    if not entries:
        raise MigrationError(f"{store}: no entries found to convert")

    reused, generated = assign_refs(entries)
    reserved = {path.name for path in store.iterdir() if path.is_file()}
    assign_filenames(entries, reserved)

    handle_map = build_handle_map(entries)
    refs = {entry.ref for entry in entries}
    for entry in entries:
        build_document(entry, handle_map, refs)

    print_plan(store, entries, kind, dry_run)

    if not dry_run:
        write_entries(store, entries)
        write_migration_md(store, handle_map)
        write_format_marker(store)
        move_to_legacy(store, consumed)

    print_summary(entries, reused, generated, handle_map, kind, config)

    if dry_run:
        print("\nDry run: nothing written.")
    else:
        print(f"\nConverted {len(entries)} entries; sources moved to {store / 'legacy'}")
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("store", type=Path, help="the .tasks directory to convert")
    parser.add_argument(
        "--dry-run", action="store_true", help="print the plan and write nothing"
    )
    args = parser.parse_args(argv)

    store = args.store
    if not store.is_dir():
        print(f"not a directory: {store}", file=sys.stderr)
        return 1
    try:
        return run(store, args.dry_run)
    except MigrationError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
