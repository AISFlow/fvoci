#!/usr/bin/env python3
"""Pure actual restart query/guard controls; no DB, driver import or subprocess."""
import ast
import copy
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[2]


def restart_block():
    tree = ast.parse((ROOT / 'scripts/selected-backend-ci/restart_checkpoint.py').read_text())
    function = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == 'restart_same_app')
    branch = next(n.orelse for n in ast.walk(function) if isinstance(n, ast.If) and n.orelse
                  and isinstance(n.orelse[0], ast.Assign)
                  and any(isinstance(t, ast.Name) and t.id == 'facts' for t in n.orelse[0].targets))
    query = branch[0].value.args[0].value
    guards = []
    for node in branch[1:]:
        if not isinstance(node, ast.Assert):break
        guards.append(node)
    assert len(guards) == 4
    return query, compile(ast.fix_missing_locations(ast.Module(body=[branch[0], *guards], type_ignores=[])), '<actual-restart-guards>', 'exec')


class RestartLedgerTest(unittest.TestCase):
    def setUp(self):
        self.query, self.code = restart_block()
        self.facts = {'user':'owned_app', 'version':'180003', 'superuser':False, 'bypassrls':False,
                      'owns_schema':False, 'owns_tables':False, 'versions':list(range(1, 13)),
                      'ledger':[[v, 'fvoci-postgres-060', format(v, '064x')] for v in range(1, 13)],
                      'rls':{n:{'enabled':True, 'forced':n in ('revisions','wiki_create_commands')}
                             for n in ('documents','document_states','document_collab_updates','wiki_create_commands','revisions')}}

    def run_guard(self, actual, *, old=False, refused=False):
        calls = []
        def query(sql, *, app):
            self.assertIs(app, True)
            self.assertEqual(sql, self.query)
            calls.append(sql)
            if refused:raise PermissionError('owned app query refused')
            result = copy.deepcopy(actual)
            if old:result.pop('ledger')  # Original reader omitted this result key.
            return result
        g = {'pg_sql':query, 'role':'owned_app', 'receipt':{'actual_restricted_role_schema_rls':self.facts}}
        exec(self.code, {'g':g})
        self.assertEqual(len(calls), 1)

    def test_original_missing_ledger_fails_but_complete_restart_facts_pass(self):
        with self.assertRaises(AssertionError):self.run_guard(self.facts, old=True)
        self.run_guard(self.facts)

    def test_same_literal_ordered_ledger_query_as_initial_restricted_role_reader(self):
        tree = ast.parse((ROOT / 'scripts/selected-backend-ci/current-postgres-driver.py').read_text())
        producer = next(n.value.args[0].value for n in ast.walk(tree) if isinstance(n, ast.Assign)
                        and any(isinstance(t, ast.Name) and t.id == 'flags' for t in n.targets))
        self.assertEqual(' '.join(self.query.split()), ' '.join(producer.split()))
        self.assertIn('jsonb_build_array(version,lineage,sql_sha256) ORDER BY version', self.query)

    def test_missing_changed_reordered_and_added_ledger_rows_refuse(self):
        for field in ('missing','lineage','digest','version','reordered','extra'):
            with self.subTest(field=field):
                changed = copy.deepcopy(self.facts)
                if field == 'missing':changed.pop('ledger')
                elif field == 'lineage':changed['ledger'][0][1] = 'foreign-lineage'
                elif field == 'digest':changed['ledger'][0][2] = 'f'*64
                elif field == 'version':changed['ledger'][0][0] = 99
                elif field == 'reordered':changed['ledger'].reverse()
                else:changed['ledger'].append([13,'fvoci-postgres-060','f'*64])
                with self.assertRaises(AssertionError):self.run_guard(changed)

    def test_role_ownership_version_and_rls_changes_refuse(self):
        for field in ('user','version','superuser','bypassrls','owns_schema','owns_tables','versions','rls'):
            with self.subTest(field=field):
                changed = copy.deepcopy(self.facts)
                changed[field] = (True if isinstance(changed[field],bool) else None)
                with self.assertRaises(AssertionError):self.run_guard(changed)
        changed = copy.deepcopy(self.facts);changed['rls']['revisions']['forced'] = False
        with self.assertRaises(AssertionError):self.run_guard(changed)

    def test_actual_app_query_refusal_is_not_replaced_with_partial_or_owner_facts(self):
        with self.assertRaises(PermissionError):self.run_guard(self.facts, refused=True)


if __name__ == '__main__':unittest.main()
