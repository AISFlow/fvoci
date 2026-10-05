"""Pure controls for compare-catalogs.py (no database): the ledger table's own
structure and rows are the only declared exception; the ledger's ACL authority,
the app role's identity and every grant stay strict; any product-object
difference still fails.

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
OWNER = "fvoci_owner"
APP = "fvoci_app_cmp"
NEW_COLS = ["version", "lineage", "sql_sha256", "applied_at"]
OLD_COLS = ["version", "applied_at"]


def acl(entries):
    return "{" + ",".join(entries) + "}"


LEDGER_ACL = acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r/{OWNER}"])
USERS_ACL = acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=arwd/{OWNER}"])


def table(name, columns, constraints=(), indexes=(), table_acl=None):
    return {
        "name": name, "kind": "r", "rls": True, "force_rls": True, "acl": table_acl,
        "columns": [{"num": i + 1, "name": c, "type": "text", "notnull": True, "default": None,
                     "identity": "", "generated": "", "collation": "-", "acl": None} for i, c in enumerate(columns)],
        "constraints": [{"name": n, "type": "c", "def": d, "deferrable": False, "deferred": False, "validated": True} for n, d in constraints],
        "indexes": [{"name": n, "def": d, "unique": False, "primary": False, "valid": True} for n, d in indexes],
        "triggers": [], "policies": [],
    }


def catalog(ledger_columns, ledger_rows, users_default="now()"):
    users = table("users", ["id", "email"], table_acl=USERS_ACL)
    users["columns"][1]["default"] = users_default
    return {
        "server_version": "18.0",
        "schemas": [{"name": "fvoci", "acl": None}],
        "tables": [table("schema_migrations", ledger_columns, constraints=[("schema_migrations_pkey", "PRIMARY KEY (version)")], table_acl=LEDGER_ACL), users],
        "sequences": [], "functions": [], "views": [], "extensions": ["plpgsql"],
        "seeds": {"instance_settings_meta": [{"id": 1, "revision": 0}], "instance_config": [{"id": 1}], "outbox_consumers": [], "row_counts": {"users": 0}},
        "ledger": ledger_rows,
    }


def tp(table_name, privilege, grantor=OWNER, grantable="NO", schema="fvoci"):
    return {"schema": schema, "table": table_name, "privilege": privilege, "grantor": grantor, "grantable": grantable}


def cp(table_name, column, privilege, grantor=OWNER, grantable="NO", schema="fvoci"):
    return {"schema": schema, "table": table_name, "column": column, "privilege": privilege, "grantor": grantor, "grantable": grantable}


def attributes(**overrides):
    base = {"exists": True, "superuser": False, "inherit": True, "createrole": False, "createdb": False,
            "login": True, "replication": False, "bypassrls": False, "member_of": []}
    base.update(overrides)
    return base


def with_role(cat, ledger_columns, role=APP, grantor=OWNER, ledger_grantable="NO", extra_table=(), extra_column=(), drop_column=(), attrs=None):
    cat = copy.deepcopy(cat)
    table_privileges = [tp("schema_migrations", "SELECT", grantor, ledger_grantable), tp("users", "SELECT", grantor), tp("users", "INSERT", grantor)]
    column_privileges = [cp("schema_migrations", c, "SELECT", grantor, ledger_grantable) for c in ledger_columns]
    column_privileges += [cp("users", c, p, grantor) for c in ("id", "email") for p in ("SELECT", "INSERT")]
    table_privileges += list(extra_table)
    column_privileges += list(extra_column)
    column_privileges = [x for x in column_privileges if (x["table"], x["column"], x["privilege"]) not in set(drop_column)]
    cat["app_role"] = {"role": role, "attributes": attrs or attributes(),
                       "schema_privileges": {"fvoci": {"usage": True, "create": False}, "public": {"usage": True, "create": False}},
                       "table_privileges": table_privileges, "column_privileges": column_privileges,
                       "routine_privileges": [{"routine": "fvoci.app_now()", "execute": True}], "sequence_usage": True, "schema_usage": True}
    return cat


def run(old, new):
    with tempfile.TemporaryDirectory() as d:
        a, b = os.path.join(d, "a.json"), os.path.join(d, "b.json")
        with open(a, "w") as handle:
            json.dump(old, handle)
        with open(b, "w") as handle:
            json.dump(new, handle)
        proc = subprocess.run([sys.executable, SCRIPT, a, b], capture_output=True, text=True)
        return proc.returncode, proc.stdout


OLD_LEDGER = catalog(OLD_COLS, [{"version": v} for v in range(1, 56)])
NEW_LEDGER = catalog(NEW_COLS, [{"version": v, "lineage": "fvoci-postgres-060", "sql_sha256": f"{v:064x}"} for v in range(1, 13)])
OLD_ROLE = with_role(OLD_LEDGER, OLD_COLS)
NEW_ROLE = with_role(NEW_LEDGER, NEW_COLS)


def ledger_table(cat):
    return next(t for t in cat["tables"] if t["name"] == "schema_migrations")


class CompareCatalogs(unittest.TestCase):
    # --- original controls (ledger-only exception; product objects strict) ---
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
        self.assertIn("DIFF table users", out)
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
            "extra ledger column": lambda c: ledger_table(c)["columns"].append({"num": 5, "name": "note", "type": "text", "notnull": False, "default": None, "identity": "", "generated": "", "collation": "-", "acl": None}),
            "nullable digest column": lambda c: ledger_table(c)["columns"][2].__setitem__("notnull", False),
            "no rows": lambda c: c.__setitem__("ledger", []),
        }
        for label, mutate in cases.items():
            new = copy.deepcopy(NEW_LEDGER)
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

    # --- role-inclusive controls ---
    def test_role_inclusive_ledger_select_expansion_passes(self):
        rc, out = run(OLD_ROLE, NEW_ROLE)
        self.assertEqual(rc, 0, out)
        self.assertIn("RESULT: PASS", out)
        self.assertNotIn("app_role", "\n".join(l for l in out.splitlines() if l.startswith(("EXTRA", "MISSING", "DIFF"))))

    def test_unauthorized_or_incomplete_ledger_privileges_fail(self):
        cases = {
            "new INSERT on ledger table": with_role(NEW_LEDGER, NEW_COLS, extra_table=[tp("schema_migrations", "INSERT")]),
            "new UPDATE on lineage column": with_role(NEW_LEDGER, NEW_COLS, extra_column=[cp("schema_migrations", "lineage", "UPDATE")]),
            "new DELETE on ledger table": with_role(NEW_LEDGER, NEW_COLS, extra_table=[tp("schema_migrations", "DELETE")]),
            "new SELECT missing on sql_sha256": with_role(NEW_LEDGER, ["version", "lineage", "applied_at"]),
            "new SELECT on a column the ledger does not have": with_role(NEW_LEDGER, NEW_COLS + ["note"]),
            "new table SELECT missing": with_role(NEW_LEDGER, NEW_COLS),
            "new ledger SELECT grantable (table and columns)": with_role(NEW_LEDGER, NEW_COLS, ledger_grantable="YES"),
            "new ledger column SELECT grantable only": with_role(NEW_LEDGER, NEW_COLS, extra_column=[cp("schema_migrations", "version", "SELECT", grantable="YES")], drop_column=[("schema_migrations", "version", "SELECT")]),
            "new ledger grant from a second grantor": with_role(NEW_LEDGER, NEW_COLS, extra_table=[tp("schema_migrations", "SELECT", grantor="other_admin")], ),
        }
        cases["new table SELECT missing"]["app_role"]["table_privileges"] = [x for x in cases["new table SELECT missing"]["app_role"]["table_privileges"] if x["table"] != "schema_migrations"]
        for label, new in cases.items():
            rc, out = run(OLD_ROLE, new)
            self.assertEqual(rc, 1, f"{label}: {out}")
            self.assertIn("LEDGER app_role new", out, label)
        old = with_role(OLD_LEDGER, OLD_COLS + ["lineage"])
        rc, out = run(old, NEW_ROLE)
        self.assertEqual(rc, 1, out)
        self.assertIn("LEDGER app_role old column privileges", out)
        old = with_role(OLD_LEDGER, OLD_COLS, grantor="owner_a")
        new = with_role(NEW_LEDGER, NEW_COLS, grantor="owner_b")
        rc, out = run(old, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("LEDGER grantor differs across sides", out)

    def test_non_ledger_grant_differences_still_fail(self):
        new = with_role(NEW_LEDGER, NEW_COLS, extra_column=[cp("users", "email", "UPDATE")])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn('EXTRA app_role.column_privileges', out)
        self.assertIn('"privilege": "UPDATE"', out)
        new = with_role(NEW_LEDGER, NEW_COLS, drop_column=[("users", "id", "SELECT")])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("MISSING app_role.column_privileges", out)
        new = with_role(NEW_LEDGER, NEW_COLS, extra_table=[tp("users", "SELECT", schema="public")])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn('EXTRA app_role.table_privileges', out)
        self.assertIn('"schema": "public"', out)
        new = with_role(NEW_LEDGER, NEW_COLS, extra_table=[tp("users", "SELECT", grantable="YES")], )
        new["app_role"]["table_privileges"] = [x for x in new["app_role"]["table_privileges"] if not (x["table"] == "users" and x["privilege"] == "SELECT" and x["grantable"] == "NO")]
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn('"grantable": "YES"', out)
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
        new = copy.deepcopy(NEW_ROLE)
        new["app_role"]["schema_privileges"]["public"]["create"] = True
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("DIFF app_role.schema_privileges", out)

    def test_role_identity_must_match_and_stay_unprivileged(self):
        new = with_role(NEW_LEDGER, NEW_COLS, role="other_app")
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("ROLE identity differs", out)
        for flag in ("superuser", "bypassrls", "createrole", "createdb", "replication"):
            old = with_role(OLD_LEDGER, OLD_COLS, attrs=attributes(**{flag: True}))
            new = with_role(NEW_LEDGER, NEW_COLS, attrs=attributes(**{flag: True}))
            rc, out = run(old, new)
            self.assertEqual(rc, 1, f"{flag}: {out}")
            self.assertIn(f"{flag}=True", out)
        new = with_role(NEW_LEDGER, NEW_COLS, attrs=attributes(inherit=False))
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("ROLE attributes differ", out)
        new = with_role(NEW_LEDGER, NEW_COLS, attrs=attributes(member_of=["pg_read_all_data"]))
        old = with_role(OLD_LEDGER, OLD_COLS, attrs=attributes(member_of=["pg_read_all_data"]))
        rc, out = run(old, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("is a member of", out)
        new = with_role(NEW_LEDGER, NEW_COLS, attrs=attributes(exists=False))
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out)
        self.assertIn("role absent", out)

    def test_missing_or_unexpected_metadata_fails(self):
        new = copy.deepcopy(NEW_ROLE); del new["app_role"]["attributes"]
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("metadata keys", out)
        new = copy.deepcopy(NEW_ROLE); new["app_role"]["note"] = "x"
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("missing/unexpected", out)
        for field in ("grantable", "grantor", "schema"):
            new = copy.deepcopy(NEW_ROLE); del new["app_role"]["column_privileges"][0][field]
            rc, out = run(OLD_ROLE, new)
            self.assertEqual(rc, 1, f"{field}: {out}"); self.assertIn("metadata keys differ", out)
        new = copy.deepcopy(NEW_ROLE); new["app_role"]["table_privileges"][0]["extra"] = 1
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("metadata keys differ", out)
        new = copy.deepcopy(NEW_ROLE); new["app_role"]["table_privileges"][1]["grantable"] = "MAYBE"
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("are not NO/YES", out)
        new = copy.deepcopy(NEW_ROLE); del new["app_role"]["attributes"]["bypassrls"]
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("attributes incomplete", out)

    def test_ledger_acl_authority_is_never_discarded(self):
        cases = {
            "foreign grantee read": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r/{OWNER}", f"other=r/{OWNER}"]), "foreign grantee 'other'"),
            "another recipient write": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r/{OWNER}", f"other=w/{OWNER}"]), "foreign grantee 'other'"),
            "PUBLIC read": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r/{OWNER}", f"=r/{OWNER}"]), "grants PUBLIC"),
            "app role grant option": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r*/{OWNER}"]), "expected exactly 'r'"),
            "app role write": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=rw/{OWNER}"]), "expected exactly 'r'"),
            "app role granted by non-owner": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r/other_admin"]), "not the owner"),
            "two full entries": (acl([f"{OWNER}=arwdDxtm/{OWNER}", f"other=arwdDxtm/other", f"{APP}=r/{OWNER}"]), "owner entries"),
            "no app role entry": (acl([f"{OWNER}=arwdDxtm/{OWNER}"]), "no entry for the app role"),
            "no acl": (None, "has no acl"),
            "quoted identifier": ('{"we ird"=r/' + OWNER + "}", "unparsable"),
        }
        for label, (table_acl, expected) in cases.items():
            old = copy.deepcopy(OLD_ROLE); new = copy.deepcopy(NEW_ROLE)
            ledger_table(old)["acl"] = table_acl; ledger_table(new)["acl"] = table_acl
            rc, out = run(old, new)
            self.assertEqual(rc, 1, f"{label}: {out}")
            self.assertIn(expected, out, label)
        new = copy.deepcopy(NEW_ROLE); ledger_table(new)["acl"] = acl([f"{OWNER}=arwdDxtm/{OWNER}", f"{APP}=r/{OWNER}", f"other=r/{OWNER}"])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("LEDGER TABLE acl differs", out)
        new = copy.deepcopy(NEW_ROLE); ledger_table(new)["rls"] = False
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("LEDGER TABLE rls differs", out)
        new = copy.deepcopy(NEW_ROLE); ledger_table(new)["columns"][1]["acl"] = acl([f"{APP}=r/{OWNER}"])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("new column lineage carries a column acl", out)
        new = copy.deepcopy(NEW_ROLE); ledger_table(new)["columns"][0]["acl"] = acl([f"{APP}=w/{OWNER}"])
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("LEDGER TABLE column version acl differs", out)
        self.assertIn("new column version carries a column acl", out)
        # Same unsafe column authority on BOTH sides must fail even though equal.
        for label, column_acl in (("foreign write", "{foreign_app=w/postgres}"), ("PUBLIC write", "{=w/postgres}"),
                                  ("app read-only column grant", acl([f"{APP}=r/{OWNER}"])), ("foreign read", "{other=r/postgres}")):
            old = copy.deepcopy(OLD_ROLE); new = copy.deepcopy(NEW_ROLE)
            ledger_table(old)["columns"][0]["acl"] = column_acl; ledger_table(new)["columns"][0]["acl"] = column_acl
            rc, out = run(old, new)
            self.assertEqual(rc, 1, f"{label}: {out}")
            self.assertIn("old column version carries a column acl", out, label)
            self.assertIn("new column version carries a column acl", out, label)
            self.assertNotIn("column version acl differs", out, label)
        new = copy.deepcopy(NEW_ROLE); ledger_table(new)["policies"] = [{"name": "p", "cmd": "*", "permissive": True, "roles": [APP], "public": False, "using": "true", "check": None}]
        rc, out = run(OLD_ROLE, new)
        self.assertEqual(rc, 1, out); self.assertIn("LEDGER TABLE policies differs", out)


if __name__ == "__main__":
    unittest.main()
