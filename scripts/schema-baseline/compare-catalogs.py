#!/usr/bin/env python3
"""Semantic comparison of two PostgreSQL catalog dumps written by
tests/schema_baseline_integration.rs::postgres_catalog_dump.

    python3 scripts/schema-baseline/compare-catalogs.py OLD.json NEW.json [--report REPORT.md]

OLD is the database installed by the retired development lineage (001..056),
NEW is the database installed by the fvoci-postgres-060 baseline. The ledger
is expected to differ (new lineage) and is reported, not compared. Everything
else must match: tables/columns (type, typmod, default, null, identity,
collation, column ACL), constraints (pg_get_constraintdef), indexes
(pg_get_indexdef), triggers, policies, RLS flags, table ACLs, sequences,
functions (signature, language, result, arguments, security/volatility/
parallel/strict/leakproof/config, body, ACL), views, extensions, seeds, row
counts and, when dumped, the app role's effective privileges. Exit 0 only when
no semantic difference remains. Raw DDL text is never compared.
"""
import argparse
import json
import sys


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def index_by(items, key):
    out = {}
    for item in items:
        name = item[key]
        if name in out:
            raise SystemExit(f"duplicate {key} {name!r}")
        out[name] = item
    return out


def diff_lists(label, old, new, key, report, nested=None):
    old_map = index_by(old, key)
    new_map = index_by(new, key)
    for name in sorted(set(old_map) - set(new_map)):
        report.append(f"MISSING {label} {name}")
    for name in sorted(set(new_map) - set(old_map)):
        report.append(f"EXTRA {label} {name}")
    for name in sorted(set(old_map) & set(new_map)):
        a, b = old_map[name], new_map[name]
        for field in sorted(set(a) | set(b)):
            if nested and field in nested:
                diff_lists(f"{label} {name}.{field}", a.get(field) or [], b.get(field) or [], nested[field], report)
                continue
            if a.get(field) != b.get(field):
                report.append(f"DIFF {label} {name}.{field}: old={a.get(field)!r} new={b.get(field)!r}")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("old")
    parser.add_argument("new")
    parser.add_argument("--report")
    args = parser.parse_args()
    old, new = load(args.old), load(args.new)
    report = []
    if old.get("server_version") != new.get("server_version"):
        report.append(f"NOTE server_version old={old.get('server_version')} new={new.get('server_version')} (same-version comparison expected)")
    diff_lists("schema", old["schemas"], new["schemas"], "name", report)
    diff_lists(
        "table",
        old["tables"],
        new["tables"],
        "name",
        report,
        nested={"columns": "name", "constraints": "name", "indexes": "name", "triggers": "name", "policies": "name"},
    )
    # Column order is semantic for this comparison (SELECT * / INSERT without a
    # column list); report it separately from per-column facts.
    old_tables = index_by(old["tables"], "name")
    new_tables = index_by(new["tables"], "name")
    for name in sorted(set(old_tables) & set(new_tables)):
        old_order = [c["name"] for c in old_tables[name]["columns"]]
        new_order = [c["name"] for c in new_tables[name]["columns"]]
        if old_order != new_order:
            report.append(f"COLUMN-ORDER table {name}: old={old_order} new={new_order}")
    diff_lists("sequence", old["sequences"], new["sequences"], "name", report)
    diff_lists("function", old["functions"], new["functions"], "signature", report)
    diff_lists("view", old["views"], new["views"], "name", report)
    if old["extensions"] != new["extensions"]:
        report.append(f"DIFF extensions old={old['extensions']} new={new['extensions']}")
    for seed in sorted(set(old["seeds"]) | set(new["seeds"])):
        if old["seeds"].get(seed) != new["seeds"].get(seed):
            report.append(f"DIFF seed {seed}: old={old['seeds'].get(seed)!r} new={new['seeds'].get(seed)!r}")
    if ("app_role" in old) != ("app_role" in new):
        report.append("NOTE app_role privileges dumped for only one side")
    elif "app_role" in old:
        a, b = old["app_role"], new["app_role"]
        for field in ("table_privileges", "column_privileges", "routine_privileges"):
            sa = {json.dumps(x, sort_keys=True) for x in a[field]}
            sb = {json.dumps(x, sort_keys=True) for x in b[field]}
            for item in sorted(sa - sb):
                report.append(f"MISSING app_role.{field} {item}")
            for item in sorted(sb - sa):
                report.append(f"EXTRA app_role.{field} {item}")
        for field in ("sequence_usage", "schema_usage"):
            if a.get(field) != b.get(field):
                report.append(f"DIFF app_role.{field}: old={a.get(field)!r} new={b.get(field)!r}")
    ledger_note = f"LEDGER (not compared) old={old.get('ledger')} new={new.get('ledger')}"
    semantic = [line for line in report if not line.startswith("NOTE")]
    lines = [
        "# Catalog comparison",
        f"old: {args.old}",
        f"new: {args.new}",
        f"semantic differences: {len(semantic)}",
        "",
        *report,
        "",
        ledger_note,
        "",
        "RESULT: " + ("PASS" if not semantic else "FAIL"),
    ]
    text = "\n".join(lines) + "\n"
    if args.report:
        with open(args.report, "w", encoding="utf-8") as handle:
            handle.write(text)
    sys.stdout.write(text)
    return 0 if not semantic else 1


if __name__ == "__main__":
    sys.exit(main())
