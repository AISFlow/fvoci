#!/usr/bin/env python3
"""Semantic comparison of two PostgreSQL catalog dumps written by
tests/schema_baseline_integration.rs::postgres_catalog_dump.

    python3 scripts/schema-baseline/compare-catalogs.py OLD.json NEW.json [--report REPORT.md]

OLD is the database installed by the retired development lineage (001..056),
NEW is the database installed by the fvoci-postgres-060 baseline. The ONLY
declared exception is the ledger table fvoci.schema_migrations itself: its
columns, constraints and own indexes differ by design ((version, applied_at)
versus (version, lineage, sql_sha256, applied_at) with CHECKs), and its rows are
the lineage receipts. The exception is granted only after an explicit transition
validation: the old ledger must have the retired shape (version, applied_at) and
the new ledger exactly (version, lineage, sql_sha256, applied_at) NOT NULL with
contiguous receipts from 1, the single lineage fvoci-postgres-060 and distinct
64-hex digests; otherwise the ledger difference is reported as a failure. Both
sides' ledger definitions and rows are printed in the report, never compared as
application catalog. When the app role's privileges are dumped, its ledger
privileges are validated the same way (exactly SELECT on the table and on each
ledger column of that side's shape, so the new side gains SELECT on lineage and
sql_sha256 and nothing else) and every other grant is compared strictly. No
other object is excluded or normalized. Everything else must match: tables/columns (type, typmod, default, null, identity,
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

import re

LEDGER_TABLE = "schema_migrations"

NEW_LINEAGE = "fvoci-postgres-060"
NEW_LEDGER_COLUMNS = ["version", "lineage", "sql_sha256", "applied_at"]
OLD_LEDGER_COLUMNS = ["version", "applied_at"]
HEX64 = re.compile(r"^[0-9a-f]{64}$")


def validate_ledger_transition(old_tables, old_rows, new_tables, new_rows):
    """The ledger is excluded from the application-equality check only when it is
    exactly the expected transition: the retired shape on the old side, and on the
    new side the fvoci-postgres-060 ledger with contiguous receipts, one lineage and
    64-hex digests. Anything else is a failure, never a silent exception."""
    problems = []
    if len(old_tables) == 1:
        cols = [c["name"] for c in old_tables[0]["columns"]]
        if cols != OLD_LEDGER_COLUMNS:
            problems.append(f"LEDGER old table columns {cols} are not the retired shape {OLD_LEDGER_COLUMNS}")
    if len(new_tables) == 1:
        cols = [c["name"] for c in new_tables[0]["columns"]]
        if cols != NEW_LEDGER_COLUMNS:
            problems.append(f"LEDGER new table columns {cols} are not {NEW_LEDGER_COLUMNS}")
        notnull = {c["name"]: c["notnull"] for c in new_tables[0]["columns"]}
        for name in NEW_LEDGER_COLUMNS:
            if not notnull.get(name, False):
                problems.append(f"LEDGER new table column {name} must be NOT NULL")
    if not isinstance(new_rows, list) or not new_rows:
        problems.append("LEDGER new rows missing")
        return problems
    versions = [r.get("version") for r in new_rows]
    if versions != list(range(1, len(new_rows) + 1)):
        problems.append(f"LEDGER new receipts are not contiguous from 1: {versions}")
    lineages = sorted({r.get("lineage") for r in new_rows})
    if lineages != [NEW_LINEAGE]:
        problems.append(f"LEDGER new receipts carry lineage(s) {lineages}, expected [{NEW_LINEAGE!r}]")
    for r in new_rows:
        digest = r.get("sql_sha256")
        if not isinstance(digest, str) or not HEX64.match(digest):
            problems.append(f"LEDGER new receipt {r.get('version')} has no 64-hex sql_sha256: {digest!r}")
    if len({r.get("sql_sha256") for r in new_rows}) != len(new_rows):
        problems.append("LEDGER new receipts repeat a digest")
    if isinstance(old_rows, list):
        for r in old_rows:
            if "lineage" in r:
                problems.append(f"LEDGER old receipt {r.get('version')} carries a lineage; the old side must be the retired shape")
                break
    return problems


def validate_ledger_privileges(old_role, new_role):
    """The app role's ledger privileges are validated explicitly, not ignored:
    table privileges on schema_migrations must be exactly SELECT on both sides and
    column privileges exactly SELECT on every ledger column of that side's shape
    (the new side therefore gains SELECT on lineage and sql_sha256 and nothing
    else). Any write privilege, any missing SELECT or any other column fails."""
    problems = []
    for side, role, columns in (("old", old_role, OLD_LEDGER_COLUMNS), ("new", new_role, NEW_LEDGER_COLUMNS)):
        table_privs = sorted({x["privilege"] for x in role["table_privileges"] if x.get("table") == LEDGER_TABLE})
        if table_privs != ["SELECT"]:
            problems.append(f"LEDGER app_role {side} table privileges on {LEDGER_TABLE} are {table_privs}, expected ['SELECT'] only")
        column_privs = sorted((x["column"], x["privilege"]) for x in role["column_privileges"] if x.get("table") == LEDGER_TABLE)
        expected = sorted((c, "SELECT") for c in columns)
        if column_privs != expected:
            problems.append(f"LEDGER app_role {side} column privileges on {LEDGER_TABLE} are {column_privs}, expected {expected}")
    return problems


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
    old_ledger = [t for t in old["tables"] if t["name"] == LEDGER_TABLE]
    new_ledger = [t for t in new["tables"] if t["name"] == LEDGER_TABLE]
    if len(old_ledger) != 1 or len(new_ledger) != 1:
        report.append(f"MISSING ledger table {LEDGER_TABLE} on one side (old={len(old_ledger)} new={len(new_ledger)})")
    report.extend(validate_ledger_transition(old_ledger, old.get("ledger"), new_ledger, new.get("ledger")))
    product_old = [t for t in old["tables"] if t["name"] != LEDGER_TABLE]
    product_new = [t for t in new["tables"] if t["name"] != LEDGER_TABLE]
    diff_lists(
        "table",
        product_old,
        product_new,
        "name",
        report,
        nested={"columns": "name", "constraints": "name", "indexes": "name", "triggers": "name", "policies": "name"},
    )
    # Column order is semantic for this comparison (SELECT * / INSERT without a
    # column list); report it separately from per-column facts.
    old_tables = index_by(product_old, "name")
    new_tables = index_by(product_new, "name")
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
        report.extend(validate_ledger_privileges(a, b))
        for field in ("table_privileges", "column_privileges", "routine_privileges"):
            sa = {json.dumps(x, sort_keys=True) for x in a[field] if x.get("table") != LEDGER_TABLE}
            sb = {json.dumps(x, sort_keys=True) for x in b[field] if x.get("table") != LEDGER_TABLE}
            for item in sorted(sa - sb):
                report.append(f"MISSING app_role.{field} {item}")
            for item in sorted(sb - sa):
                report.append(f"EXTRA app_role.{field} {item}")
        for field in ("sequence_usage", "schema_usage"):
            if a.get(field) != b.get(field):
                report.append(f"DIFF app_role.{field}: old={a.get(field)!r} new={b.get(field)!r}")
    ledger_note = "\n".join([
        f"LEDGER TABLE (declared exception, printed not compared) old={json.dumps(old_ledger, sort_keys=True)}",
        f"LEDGER TABLE (declared exception, printed not compared) new={json.dumps(new_ledger, sort_keys=True)}",
        f"LEDGER ROWS (declared exception, printed not compared) old={old.get('ledger')} new={new.get('ledger')}",
    ])
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
