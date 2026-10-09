#!/usr/bin/env python3
"""Pure restart query/CLI/failure controls; no DB, runtime driver or external command."""
import ast
import copy
import contextlib
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import tempfile
from unittest.mock import Mock, patch
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



class RestartCLIAndFailureTest(unittest.TestCase):
    """Execute the maintained helper with subprocess/HTTP/authorization mocked."""
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory();self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        spec = importlib.util.spec_from_file_location('restart_pure_fixture', ROOT/'scripts/selected-backend-ci/restart_checkpoint.py')
        self.helper = importlib.util.module_from_spec(spec)
        with patch.dict(os.environ, {'FVOCI_CI_OWNER':'pure-owner'}):spec.loader.exec_module(self.helper)
        tree = ast.parse((ROOT/'scripts/selected-backend-ci/current-postgres-driver.py').read_text())
        kept = [n for n in tree.body if (isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and
                t.id in ('KNOWN_ON_BROWSER_TEST','KNOWN_BROWSER_SUFFIXES','KNOWN_BROWSER_STATUSES') for t in n.targets)) or
                (isinstance(n,ast.FunctionDef) and n.name == 'known_browser_checkpoint')]
        state = {};exec(compile(ast.fix_missing_locations(ast.Module(body=kept,type_ignores=[])),'<maintained-projection>','exec'),state)
        self.project = state['known_browser_checkpoint']
        self.cli = self.root/'node_modules/playwright/cli.js';self.cli.parent.mkdir(parents=True)
        self.cli.write_text('synthetic qualified CLI, never executed')
        self.g = {'W':self.root, 'BUN':self.root/'bun', 'SPEC':'workspace-wiki-selected-backend.spec.ts',
                  'PLAYWRIGHT_CLI':self.cli,'before':{'external':{str(self.cli):self.helper.digest(self.cli)}}}

    def test_same_admitted_direct_args_and_sqlite_literal_fallback(self):
        expected = [str(self.g['BUN']),'--no-install',str(self.cli),'test','--config',
                    'e2e-pending/collab-playwright.config.ts','--reporter=line,json','--grep',
                    'selected normal main restart:',self.g['SPEC']]
        with patch.object(self.helper.subprocess,'run') as child:
            self.assertEqual(self.helper.restart_browser_args(self.g),expected)
            fallback = dict(self.g);fallback.pop('PLAYWRIGHT_CLI')
            self.assertEqual(self.helper.restart_browser_args(fallback),expected)
            child.assert_not_called()

    def test_missing_unadmitted_drift_symlink_and_foreign_cli_refuse(self):
        original = self.cli.read_text()
        with patch.object(self.helper.subprocess,'Popen') as child:
            self.g['before']['external'].clear()
            with self.assertRaises(KeyError):self.helper.restart_browser_args(self.g)
            self.g['before']['external'][str(self.cli)] = hashlib.sha256(original.encode()).hexdigest()
            self.cli.write_text('drift')
            with self.assertRaises(AssertionError):self.helper.restart_browser_args(self.g)
            self.cli.unlink()
            with self.assertRaises(AssertionError):self.helper.restart_browser_args(self.g)
            foreign = self.root/'foreign';foreign.write_text(original);self.cli.symlink_to(foreign)
            with self.assertRaises(AssertionError):self.helper.restart_browser_args(self.g)
            self.g['PLAYWRIGHT_CLI'] = foreign
            with self.assertRaises(AssertionError):self.helper.restart_browser_args(self.g)
            child.assert_not_called()

    def failure_probe(self, *, report='structured', before_browser=False, cleanup_failure=False, cli_missing=False):
        h = self.helper;run=self.root/'run';run.mkdir()
        storage=self.root/'storage';storage.mkdir();dist=self.root/'dist';dist.mkdir()
        parent=self.root/'parent.py';parent.write_text('synthetic parent')
        grant=self.root/'grant.json';grant.write_text('{}');(run/'source-inputs-before.json').write_text('{}')
        self.g['BUN'].write_text('synthetic bun');chromium=self.root/'chromium';chromium.write_text('synthetic browser')
        binaries={}
        for label in ('server','migrate','engine'):
            path=self.root/label;path.write_text(label);self.g[label]=str(path);binaries[str(path)]={'sha256':h.digest(path)}
        facts={'user':'pure_app','superuser':False,'bypassrls':False,'owns_schema':False,'owns_tables':False,
               'rls':{name:{'enabled':True} for name in ('documents','document_states','document_collab_updates','wiki_create_commands','revisions')}}
        old={'pid':11,'start_ticks':'1','namespace_pid':11,'uid':1000,'gid':1000,'args':'/fvoci/bin/fvoci-server'}
        new=dict(old,pid=22,start_ticks='2',namespace_pid=22)
        restarted=Mock();restarted.wait.return_value=0;restarted.poll.return_value=None
        if cleanup_failure:restarted.wait.side_effect=subprocess.TimeoutExpired('synthetic-owned-exec',10)
        secret='SYNTHETIC_PRIVATE_SECRET http://private.invalid/x token=PRIVATE'
        response=Mock(status=200);response.read.return_value=b'{"needed":false}'
        context=Mock();context.__enter__=Mock(return_value=response);context.__exit__=Mock(return_value=False)
        self.g.update(run=run,name='pure-container',browser_env={'FVOCI_E2E_SELECTED_BACKEND':'postgres',
             'PATH':'/usr/bin','LANG':'C.UTF-8','PRIVATE_TOKEN':secret},code=0,HEAD='a'*40,COMPILED_HEAD='a'*40,TREE='b'*40,
             current={'grant':{'runId':'123','runAttempt':'1'}},__file__=str(parent),binaries=binaries,
             assets={'dist_files':{}},abi={'host_runtime_files':{}},browser_inputs={
                 'bun':{'path':str(self.g['BUN']),'sha256':h.digest(self.g['BUN'])},
                 'chromium':{'path':str(chromium),'sha256':h.digest(chromium)},'chromium_directory_files':{}},
             source_before={},input_check=Mock(return_value={}),tree_hashes=Mock(return_value={}),dist=dist,storage=storage,
             server_row=old,server_process=Mock(),base='http://127.0.0.1:123',role='pure_app',master_key='different synthetic owner key',
             receipt={'actual_restricted_role_schema_rls':facts},identity_gone=Mock(return_value=True),
             owned_rows=Mock(side_effect=[[old],[new],[new]]),known_browser_checkpoint=self.project)
        self.g['server_process'].wait.return_value=0
        observed=[]
        def command(args, log=None, **kwargs):
            observed.append(args)
            text=''
            if args[0]==str(self.g['BUN']):
                self.assertEqual(args,self.helper.restart_browser_args(self.g))
                self.assertNotIn(secret,json.dumps(kwargs['env']))
                log.write_text(secret)
                if report=='structured':
                    title='selected normal main restart: fresh actor reads persisted native history and manual revision'
                    data={'config':{'workers':1},'suites':[{'specs':[{'title':title,
                        'file':'e2e-pending/workspace-wiki-selected-backend.spec.ts','tests':[{'results':[{
                        'status':'failed','errorLocation':{'file':'/fixed/apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts','line':701},
                        'errors':[{'message':secret,'stack':secret}]}]}]}]}]}
                    (run/'restart-playwright-result.private.json').write_text(json.dumps(data))
                elif report=='malformed':(run/'restart-playwright-result.private.json').write_text(secret)
                return subprocess.CompletedProcess(args,7)
            if 'sha256sum' in args:text='\n'.join(binaries[self.g[n]]['sha256']+'  fixture' for n in ('server','migrate','engine'))
            elif 'inspect' in args:text='pure-owner'
            elif 'stat' in args:text='0 1000 640'
            elif 'cat' in args:text='synthetic_scoped_key_long_enough'
            return subprocess.CompletedProcess(args,0,text,'')
        self.g['command']=Mock(side_effect=command)
        actual=copy.deepcopy(facts)
        if before_browser:actual['superuser']=True
        self.g['pg_sql']=Mock(return_value=actual)
        seed={'selected':'postgres','firstAck':'first','finalAck':'final','creatorId':'creator','freshActorId':'fresh',
              'canonicalEmojiOracleControls':list(range(6)),'nativeHistoryOracleControls':list(range(2))}
        (run/'playwright-result.private.json').write_text('{}')
        def popen(*args, **kwargs):
            kwargs['stdout'].write('fvoci-server listening on http://127.0.0.1:456\nmeilisearch enabled\noutbox dispatcher started\n')
            kwargs['stdout'].flush();return restarted
        if cli_missing:self.cli.unlink()
        output=io.StringIO()
        with patch.dict(os.environ,{'FVOCI_ROOT_RUN_OWNER':'pure-owner','FVOCI_ROOT_RESTART_GRANT':str(grant)}), \
             patch.object(h.os,'getuid',return_value=1000),patch.object(h.os,'getgid',return_value=1000), \
             patch.object(h,'validate_allocation'),patch.object(h,'single_attachment',return_value=seed), \
             patch.object(h,'port_closed',return_value=True),patch.object(h,'build_opener') as opener, \
             patch.object(h.subprocess,'Popen',side_effect=popen) as child,contextlib.redirect_stdout(output):
            opener.return_value.open.return_value=context
            with self.assertRaises(AssertionError) as caught:h.restart_same_app(self.g)
        receipt=json.loads((run/'restart-receipt.private.json').read_text())
        self.assertEqual(receipt['originalFailure']['message'],str(caught.exception))
        safe=json.loads(output.getvalue());self.assertNotIn(secret,output.getvalue())
        self.assertRegex(safe['restart_helper_checkpoint'],r'^scripts/selected-backend-ci/restart_checkpoint\.py:[1-9][0-9]*$')
        self.assertEqual((run/'restart-receipt.private.json').stat().st_mode & 0o777,0o600)
        if cli_missing:
            child.assert_not_called();self.g['command'].assert_not_called()
        else:
            restarted.wait.assert_called_once_with(timeout=10)
            self.assertTrue(any('/bin/kill' in args and '22' in args for args in observed))
            if cleanup_failure:self.assertNotIn('restartServerExit',receipt)
            else:self.assertEqual(receipt['restartServerExit'],0)
        return safe,receipt

    def test_browser_first_failure_structured_checkpoint_and_normal_cleanup(self):
        safe,receipt=self.failure_probe()
        self.assertEqual(safe['restart_browser_exit'],7)
        self.assertEqual(safe['known_browser_status'],'failed')
        self.assertEqual(safe['known_browser_checkpoint'],'e2e-pending/workspace-wiki-selected-backend.spec.ts:701')
        self.assertEqual(receipt['originalFailure']['message'],'preserve original restart browser failure')
        self.assertEqual(receipt['cleanupErrors'],[])

    def test_absent_report_stays_missing_and_normal_cleanup(self):
        safe,_=self.failure_probe(report='missing')
        self.assertEqual(safe['browser_report_state'],'report-missing');self.assertIsNone(safe['known_browser_checkpoint'])

    def test_malformed_report_preserves_first_failure_and_normal_cleanup(self):
        safe,receipt=self.failure_probe(report='malformed')
        self.assertEqual(safe['browser_report_state'],'report-unreadable')
        self.assertEqual(receipt['originalFailure']['message'],'preserve original restart browser failure')
        self.assertEqual(receipt['cleanupErrors'],[])

    def test_failure_before_browser_is_actual_helper_checkpoint_not_old_cause(self):
        safe,_=self.failure_probe(before_browser=True)
        self.assertIsNone(safe['restart_browser_exit']);self.assertIsNone(safe['known_browser_checkpoint'])
        self.assertEqual(safe['restart_stage'],'validated')

    def test_cli_refusal_precedes_any_restart_child(self):self.failure_probe(cli_missing=True)

    def test_cleanup_failure_does_not_replace_original_browser_failure(self):
        safe,receipt=self.failure_probe(cleanup_failure=True)
        self.assertEqual(receipt['originalFailure']['message'],'preserve original restart browser failure')
        self.assertEqual(len(receipt['cleanupErrors']),1);self.assertEqual(safe['restart_browser_exit'],7)


if __name__ == '__main__':unittest.main()
