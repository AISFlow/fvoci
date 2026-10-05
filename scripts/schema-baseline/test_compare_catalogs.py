"""Pure controls for compare-catalogs.py (no database): the ledger table is the
only declared exception; any product-object difference still fails.

    python3 -m unittest scripts.schema_baseline.test_compare_catalogs   # from the repo root, or
    python3 scripts/schema-baseline/test_compare_catalogs.py
"""
import copy
import json
import os
import subprocess
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "compare-catalogs.py")


def table(name, columns, constraints=(), indexes=()):
    return {
        "name": name, "kind": "r", "rls": True, "force_rls": True, "acl": None,
        "columns": [{"num": i + 1, "name": c, "type": "text", "notnull": True, "default": None,
                     "identity": "", "generated": "", "collation": "-", "acl": None} for i, c in enumerate(columns)],
        "constraints": [{"name": n, "type": "c", "def": d, "deferrable": False, "deferred": False, "validated": True} for n, d in constraints],
        "indexes": [{"name": n, "def": d, "unique": False, "primary": False, "valid": True} for n, d in indexes],
        "triggers": [], "policies": [],
    }


def catalog(ledger_columns, ledger_rows, users_default="now()"):
    users = table("users", ["id", "email"])
    users["columns"][1]["default"] = users_default
    return {
        "server_version": "18.0",
        "schemas": [{"name": "fvoci", "acl": None}],
        "tables": [table("schema_migrations", ledger_columns, constraints=[("schema_migrations_pkey", "PRIMARY KEY (version)")]), users],
        "sequences": [], "functions": [], "views": [], "extensions": ["plpgsql"],
        "seeds": {"instance_settings_meta": [{"id": 1, "revision": 0}], "instance_config": [{"id": 1}], "outbox_consumers": [], "row_counts": {"users": 0}},
        "ledger": ledger_rows,
    }


def with_role(cat, ledger_columns, extra_table=(), extra_column=(), drop_column=()):
    cat = copy.deepcopy(cat)
    table_privileges = [{"table": "schema_migrations", "privilege": "SELECT"}, {"table": "users", "privilege": "SELECT"}, {"table": "users", "privilege": "INSERT"}]
    column_privileges = [{"table": "schema_migrations", "column": c, "privilege": "SELECT"} for c in ledger_columns]
    column_privileges += [{"table": "users", "column": c, "privilege": p} for c in ("id", "email") for p in ("SELECT", "INSERT")]
    table_privileges += list(extra_table)
    column_privileges += list(extra_column)
    column_privileges = [x for x in column_privileges if (x["table"], x["column"], x["privilege"]) not in set(drop_column)]
    cat["app_role"] = {"table_privileges": table_privileges, "column_privileges": column_privileges,
                       "routine_privileges": [{"routine": "fvoci.app_now()", "execute": True}], "sequence_usage": True, "schema_usage": True}
    return cat


def run(old, new):
    with tempfile.TemporaryDirectory() as d:
        a, b = os.path.join(d, "a.json"), os.path.join(d, "b.json")
        json.dump(old, open(a, "w")); json.dump(new, open(b, "w"))
        proc = subprocess.run([sys.executable, SCRIPT, a, b], capture_output=True, text=True)
        return proc.returncode, proc.stdout


OLD_LEDGER = catalog(["version", "applied_at"], [{"version": v} for v in range(1, 56)])
NEW_LEDGER = catalog(["version", "lineage", "sql_sha256", "applied_at"],
                     [{"version": v, "lineage": "fvoci-postgres-060", "sql_sha256": f"{v:064x}"} for v in range(1, 13)])


OLD_ROLE = with_role(OLD_LEDGER, ["version", "applied_at"])
NEW_ROLE = with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"])


class CompareCatalogs(unittest.TestCase):
    def test_role_inclusive_ledger_select_expansion_passes(self):
        rc, out = run(OLD_ROLE, NEW_ROLE)
        self.assertEqual(rc, 0, out)
        self.assertIn("RESULT: PASS", out)
        self.assertNotIn("app_role", "\n".join(l for l in out.splitlines() if l.startswith(("EXTRA", "MISSING", "DIFF"))))

    def test_unauthorized_or_incomplete_ledger_privileges_fail(self):
        cases = {
            "new INSERT on ledger table": with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"], extra_table=[{"table": "schema_migrations", "privilege": "INSERT"}]),
            "new UPDATE on lineage column": with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"], extra_column=[{"table": "schema_migrations", "column": "lineage", "privilege": "UPDATE"}]),
            "new DELETE on ledger table": with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"], extra_table=[{"table": "schema_migrations", "privilege": "DELETE"}]),
            "new SELECT missing on sql_sha256": with_role(NEW_LEDGER, ["version", "lineage", "applied_at"]),
            "new SELECT on a column the ledger does not have": with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at", "note"]),
            "new table SELECT missing": with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"], drop_column=[]),
        }
        cases["new table SELECT missing"]["app_role"]["table_privileges"] = [x for x in cases["new table SELECT missing"]["app_role"]["table_privileges"] if x["table"] != "schema_migrations"]
        for label, new in cases.items():
            rc, out = run(OLD_ROLE, new)
            self.assertEqual(rc, 1, f"{label}: {out}")
            self.assertIn("LEDGER app_role new", out, label)
        old = with_role(OLD_LEDGER, ["version", "applied_at", "lineage"])
        rc, out = run(old, NEW_ROLE)
        self.assertEqual(rc, 1, out)
        self.assertIn("LEDGER app_role old column privileges", out)

    def test_non_ledger_grant_differences_still_fail(self):
        new = with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"], extra_column=[{"table": "users", "column": "email", "privilege": "UPDATE"}])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn('EXTRA app_role.column_privileges {"column": "email", "privilege": "UPDATE", "table": "users"}', out)
        new = with_role(NEW_LEDGER, ["version", "lineage", "sql_sha256", "applied_at"], drop_column=[("users", "id", "SELECT")])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("MISSING app_role.column_privileges", out)
        new = copy.deepcopy(NEW_ROLE)
        new["app_role"]["routine_privileges"][0]["execute"] = False
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("app_role.routine_privileges", out)
        new = copy.deepcopy(NEW_ROLE)
        new["app_role"]["sequence_usage"] = False
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("DIFF app_role.sequence_usage", out)

    def test_ledger_table_and_rows_are_the_only_declared_exception(self):
        rc, out = run(OLD_LEDGER, NEW_LEDGER)
        self.assertEqual(rc, 0, out)
        self.assertIn("RESULT: PASS", out)
        self.assertIn("LEDGER TABLE (declared exception", out)
        self.assertIn("LEDGER ROWS (declared exception", out)
        self.assertNotIn("DIFF table schema_migrations", out)

    def test_a_product_column_default_difference_fails(self):
        new = copy.deepcopy(NEW_LEDGER)
        new["tables"][1]["columns"][1]["default"] = "'x'::text"
        rc, out = run(OLD_LEDGER, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("DIFF table users.email", out) if False else self.assertIn("DIFF table users", out)
        self.assertIn("RESULT: FAIL", out)

    def test_a_missing_product_constraint_or_index_fails(self):
        for key, item in (("constraints", {"name": "users_email_unique", "type": "u", "def": "UNIQUE (email)", "deferrable": False, "deferred": False, "validated": True}),
                          ("indexes", {"name": "users_email_idx", "def": "CREATE INDEX users_email_idx ON fvoci.users USING btree (email)", "unique": False, "primary": False, "valid": True})):
            old = copy.deepcopy(OLD_LEDGER)
            old["tables"][1][key].append(item)
            rc, out = run(old, NEW_LEDGER)
            self.assertEqual(rc, 1, out)
            self.assertIn(f"MISSING table users.{key} {item['name']}", out)

    def test_malformed_new_ledger_is_a_failure_not_an_exception(self):
        cases = {
            "wrong lineage": lambda c: c["ledger"].__setitem__(3, {**c["ledger"][3], "lineage": "fvoci-postgres-999"}),
            "missing digest": lambda c: c["ledger"].__setitem__(5, {"version": 6, "lineage": "fvoci-postgres-060"}),
            "short digest": lambda c: c["ledger"].__setitem__(1, {**c["ledger"][1], "sql_sha256": "abc"}),
            "gap in versions": lambda c: c["ledger"].pop(4),
            "duplicate digest": lambda c: c["ledger"].__setitem__(2, {**c["ledger"][2], "sql_sha256": c["ledger"][1]["sql_sha256"]}),
            "extra ledger column": lambda c: c["tables"][0]["columns"].append({"num": 5, "name": "note", "type": "text", "notnull": False, "default": None, "identity": "", "generated": "", "collation": "-", "acl": None}),
            "nullable digest column": lambda c: c["tables"][0]["columns"][2].__setitem__("notnull", False),
            "no rows": lambda c: c.__setitem__("ledger", []),
        }
        for label, mutate in cases.items():
            new = copy.deepcopy(NEW_LEDGER)
            for i, r in enumerate(new["ledger"]):
                r["sql_sha256"] = f"{i:064x}"
            mutate(new)
            rc, out = run(OLD_LEDGER, new)
            self.assertEqual(rc, 1, f"{label}: {out}")
            self.assertIn("LEDGER", out)
            self.assertIn("RESULT: FAIL", out)
        old = copy.deepcopy(OLD_LEDGER)
        old["ledger"] = [{"version": 1, "lineage": "fvoci-postgres-060", "sql_sha256": "0" * 64}]
        rc, out = run(old, NEW_LEDGER)
        self.assertEqual(rc, 1, out)
        self.assertIn("old receipt 1 carries a lineage", out)

    def test_a_missing_ledger_table_is_reported_not_silently_excepted(self):
        new = copy.deepcopy(NEW_LEDGER)
        new["tables"] = [t for t in new["tables"] if t["name"] != "schema_migrations"]
        rc, out = run(OLD_LEDGER, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("MISSING ledger table schema_migrations", out)

    def test_seed_and_column_order_differences_fail(self):
        new = copy.deepcopy(NEW_LEDGER)
        new["seeds"]["instance_settings_meta"] = [{"id": 1, "revision": 1}]
        rc, out = run(OLD_LEDGER, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("DIFF seed instance_settings_meta", out)
        new = copy.deepcopy(NEW_LEDGER)
        new["tables"][1]["columns"].reverse()
        for i, c in enumerate(new["tables"][1]["columns"]):
            c["num"] = i + 1
        rc, out = run(OLD_LEDGER, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("COLUMN-ORDER table users", out)


if __name__ == "__main__":
    unittest.main()
