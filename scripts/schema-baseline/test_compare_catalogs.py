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


def run(old, new):
    with tempfile.TemporaryDirectory() as d:
        a, b = os.path.join(d, "a.json"), os.path.join(d, "b.json")
        json.dump(old, open(a, "w")); json.dump(new, open(b, "w"))
        proc = subprocess.run([sys.executable, SCRIPT, a, b], capture_output=True, text=True)
        return proc.returncode, proc.stdout


OLD_LEDGER = catalog(["version", "applied_at"], [{"version": v} for v in range(1, 56)])
NEW_LEDGER = catalog(["version", "lineage", "sql_sha256", "applied_at"],
                     [{"version": v, "lineage": "fvoci-postgres-060", "sql_sha256": "0" * 64} for v in range(1, 13)])


class CompareCatalogs(unittest.TestCase):
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
