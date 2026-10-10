#!/usr/bin/env python3
"""Pure receipt/control faults; never import or execute a runtime driver."""
import ast
import contextlib
import copy
import hashlib
import importlib.util
import io
import json
import os
import subprocess
import types
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('selected_failure_control', ROOT/'scripts/run-selected-backend-e2e.py')
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
SOURCE, TREE, OWNER = 'a'*40, 'b'*40, 'pure-owned-control'
PRIVATE = {'type':'AssertionError', 'message':'SYNTHETIC_PRIVATE_SECRET'}


class EarlyDriverFailure(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.run = Path(self.tmp.name)
        self.receipt = {'source':SOURCE,'tree':TREE,'root_owner':OWNER,'final_exit_code':7,
            'selected_flow':'on','owned_container_absent':True,'owned_loopback_port_closed':True,
            'recorded_process_identities_retired':True,'cleanup_errors':[],
            'failed_phase':'server-startup','original_driver_failure':PRIVATE}
        self.parent = {'source':SOURCE,'tree':TREE,'root_owner':OWNER,'selected_flow':'on',
                       'all_owned_fixtures_closed':True}

    def admit(self, receipt=None, parent=None, code=7):
        (self.run/'receipt.json').write_text(json.dumps(self.receipt if receipt is None else receipt))
        (self.run/'parent-receipt.json').write_text(json.dumps(self.parent if parent is None else parent))
        return runner.lane_retirement(self.run,'postgres','on',SOURCE,TREE,OWNER,code)

    def test_known_positive_retirement_can_preserve_a_failed_product_exit(self):
        facts = self.admit()
        self.assertTrue(facts['qualified'])
        self.assertEqual(facts['failedPhase'],'server-startup')
        self.assertEqual(facts['originalFailureSha256'],hashlib.sha256(json.dumps(PRIVATE,sort_keys=True).encode()).hexdigest())
        self.assertNotIn(PRIVATE['message'],json.dumps(facts))

    def test_missing_unobserved_or_false_port_never_becomes_a_positive_fact(self):
        for value in ('missing',None,False,1,'true'):
            with self.subTest(value=value):
                receipt = copy.deepcopy(self.receipt)
                if value == 'missing':receipt.pop('owned_loopback_port_closed')
                else:receipt['owned_loopback_port_closed']=value
                facts = self.admit(receipt)
                self.assertFalse(facts['qualified'])
                self.assertEqual(facts['refusalCodes'],['SELECTED_DRIVER_RETIREMENT_UNCONFIRMED'])
                self.assertIsNotNone(facts['originalFailureSha256'])

    def test_missing_changed_source_owner_exit_pid_fixture_and_cleanup_refuse(self):
        for field in ('source','tree','root_owner','final_exit_code','recorded_process_identities_retired',
                      'owned_container_absent','selected_flow','cleanup_errors'):
            receipt = copy.deepcopy(self.receipt)
            receipt.pop(field)
            with self.subTest(field=field):self.assertFalse(self.admit(receipt)['qualified'])
        for field in ('source','tree','root_owner','selected_flow','all_owned_fixtures_closed'):
            parent = copy.deepcopy(self.parent)
            parent[field]=False
            with self.subTest(parent=field):self.assertFalse(self.admit(parent=parent)['qualified'])
        self.assertFalse(self.admit(code=0)['qualified'])

    def test_unreadable_malformed_or_nonobject_receipt_does_not_escape_as_keyerror(self):
        for raw in ('not-json','[]','null','{}'):
            (self.run/'receipt.json').write_text(raw)
            facts=runner.lane_retirement(self.run,'postgres','on',SOURCE,TREE,OWNER,7)
            self.assertFalse(facts['qualified'])
            self.assertEqual(facts['receiptSha256'],hashlib.sha256(raw.encode()).hexdigest())
        (self.run/'receipt.json').unlink()
        self.assertFalse(runner.lane_retirement(self.run,'postgres','on',SOURCE,TREE,OWNER,7)['qualified'])

    def test_partial_actual_failure_has_diagnostic_but_cannot_return_ownership(self):
        output=self.run/'output';output.mkdir()
        runtime=output/'runtime';runtime.mkdir()
        runroot=runtime/'root-current-postgres-0123456789ab';runroot.mkdir()
        receipt=dict(self.receipt,owned_loopback_port_closed=None)
        (runroot/'receipt.json').write_text(json.dumps(receipt))
        (output/'before.json').write_text(json.dumps({'head':SOURCE,'tree':TREE}))
        (output/'postgres-on-allocation.json').write_text('{}')
        (output/'selected-ci-receipt.json').write_text(json.dumps({'owner':OWNER,'source':SOURCE,'tree':TREE,
            'exit':7,'allRequestedRunsExecuted':False,'runs':[{'lane':'postgres','flow':'on','exit':7,
                'actualSource':SOURCE,'runRoot':str(runroot)}]}))
        stdout=io.StringIO()
        with patch.dict(runner.os.environ,{'GITHUB_SHA':SOURCE,'GITHUB_JOB':'collaboration-flow'},clear=True), \
             patch.object(runner,'identity',return_value=OWNER),patch.object(runner.os,'getuid',return_value=1000), \
             patch.object(runner.os,'getgid',return_value=1000),contextlib.redirect_stdout(stdout):
            with self.assertRaises(AssertionError):runner.runtime_ownership_return(output)
        diagnostic=json.loads(stdout.getvalue())
        self.assertFalse(diagnostic['ownership_return_qualified'])
        self.assertEqual(diagnostic['selected_exit'],7)
        self.assertEqual(diagnostic['lanes'][0]['invalid_required_fields'],['owned_loopback_port_closed'])
        self.assertIsNotNone(diagnostic['lanes'][0]['original_driver_failure_sha256'])
        self.assertEqual(diagnostic['lanes'][0]['failed_phase'],'server-startup')
        self.assertEqual(diagnostic['lanes'][0]['original_driver_failure_type'],'AssertionError')
        self.assertIsNone(diagnostic['lanes'][0]['original_driver_failure_code'])
        self.assertNotIn(PRIVATE['message'],stdout.getvalue())
        self.assertFalse((output/'runtime-close-stage.json').exists())

    def _refused_ownership(self, receipt, message, parent=False):
        output = self.run / ('own-' + hashlib.sha256(json.dumps(receipt,sort_keys=True).encode()).hexdigest()[:12])
        output.mkdir()
        runtime = output/'runtime'; runtime.mkdir()
        runroot = runtime/'root-current-postgres-0123456789ab'; runroot.mkdir()
        (runroot/'receipt.json').write_text(json.dumps(receipt))
        if parent:(runroot/'parent-receipt.json').write_text(json.dumps(self.parent))
        (output/'before.json').write_text(json.dumps({'head':SOURCE,'tree':TREE}))
        (output/'postgres-on-allocation.json').write_text('{}')
        (output/'selected-ci-receipt.json').write_text(json.dumps({'owner':OWNER,'source':SOURCE,'tree':TREE,
            'exit':receipt['final_exit_code'],'allRequestedRunsExecuted':False,'runs':[{'lane':'postgres','flow':'on',
                'exit':receipt['final_exit_code'],'actualSource':SOURCE,'runRoot':str(runroot)}]}))
        stdout=io.StringIO()
        with patch.dict(runner.os.environ,{'GITHUB_SHA':SOURCE,'GITHUB_JOB':'collaboration-flow'},clear=True), \
             patch.object(runner,'identity',return_value=OWNER),patch.object(runner.os,'getuid',return_value=1000), \
             patch.object(runner.os,'getgid',return_value=1000),contextlib.redirect_stdout(stdout):
            with self.assertRaisesRegex(AssertionError, message):
                runner.runtime_ownership_return(output)
        raw=stdout.getvalue()
        self.assertFalse((output/'runtime-close-stage.json').exists())
        return json.loads(raw), raw

    def test_public_lane_publishes_whitelisted_phase_type_and_code_only(self):
        secret='http://secret.example/a cookie=PRIVATE_LANE_SECRET argv=/tmp/owned'
        self.assertEqual(runner.public_failure_fields({
            'failed_phase':'browser','failure_code':secret,'original_driver_failure':secret}),
            {'failed_phase':'browser','known_driver_checkpoint':None,'preparation_command_exit':None,
             'original_driver_failure_type':None,'original_driver_failure_code':None,
             'browser_report_state':None,'known_browser_test':None,'known_browser_status':None,'known_browser_checkpoint':None})
        cases=(
            ('server-ready','AssertionError','SELECTED_DRIVER_EXCEPTION','server-ready','AssertionError','SELECTED_DRIVER_EXCEPTION'),
            ('browser','ReturnedNonzero','SELECTED_BODY_NONZERO','browser','ReturnedNonzero','SELECTED_BODY_NONZERO'),
            ('install-body','RuntimeError','SELECTED_DRIVER_EXCEPTION','install-body','RuntimeError','SELECTED_DRIVER_EXCEPTION'),
            ('owned-fixture-wrapper','OSError','SELECTED_DRIVER_FAILED',None,None,None),
            (secret,secret,secret,None,None,None),
        )
        for phase, kind, code, expect_phase, expect_kind, expect_code in cases:
            with self.subTest(phase=phase, kind=kind, code=code):
                failure={'type':kind,'message':secret,'phase':phase,'observedExit':7,'code':code}
                receipt=dict(self.receipt, owned_loopback_port_closed=None, failed_phase=phase,
                             failure_code=code, original_driver_failure=failure)
                diagnostic, raw=self._refused_ownership(receipt, 'missing or invalid current retirement proof')
                lane=diagnostic['lanes'][0]
                self.assertFalse(diagnostic['ownership_return_qualified'])
                self.assertEqual(lane['failed_phase'], expect_phase)
                self.assertEqual(lane['original_driver_failure_type'], expect_kind)
                self.assertEqual(lane['original_driver_failure_code'], expect_code)
                self.assertEqual(lane['original_driver_failure_sha256'],
                    hashlib.sha256(json.dumps(failure,sort_keys=True).encode()).hexdigest())
                self.assertNotIn(secret, raw)
                self.assertNotIn('original_driver_failure', lane)
        complete=dict(self.receipt, failed_phase='server-ready', failure_code='SELECTED_DRIVER_EXCEPTION',
                      original_driver_failure={'type':'AssertionError','message':secret})
        diagnostic, raw=self._refused_ownership(complete, 'all mandatory lanes remain required', parent=True)
        lane=diagnostic['lanes'][0]
        self.assertEqual([(item['lane'], item['flow']) for item in diagnostic['lanes']], [('postgres','on')])
        self.assertEqual(lane['failed_phase'],'server-ready')
        self.assertEqual(lane['original_driver_failure_type'],'AssertionError')
        self.assertEqual(lane['original_driver_failure_code'],'SELECTED_DRIVER_EXCEPTION')
        self.assertEqual(lane['closure_facts']['owned_loopback_port_closed'], True)
        self.assertEqual(lane['cleanup_error_count'], 0)
        self.assertNotIn(secret, raw)
        self.assertIsNone(lane['known_browser_test'])
        self.assertIsNone(lane['known_browser_status'])
        self.assertIsNone(lane['known_browser_checkpoint'])
        self.assertIsNone(lane['browser_report_state'])
        browser = dict(complete, failed_phase='browser', failure_code='SELECTED_BODY_NONZERO',
                       original_driver_failure={'type':'ReturnedNonzero','phase':'browser','observedExit':1},
                       browser_report_state='matched', known_browser_test=runner.KNOWN_ON_BROWSER_TEST,
                       known_browser_status='failed',
                       known_browser_checkpoint='e2e-pending/workspace-wiki-selected-backend.spec.ts:413')
        diagnostic, raw = self._refused_ownership(browser, 'all mandatory lanes remain required', parent=True)
        lane = diagnostic['lanes'][0]
        self.assertFalse(diagnostic['ownership_return_qualified'])
        self.assertEqual(diagnostic['proof_error_type'], 'AssertionError')
        self.assertEqual(lane['browser_report_state'], 'matched')
        self.assertEqual(lane['known_browser_status'], 'failed')
        self.assertEqual(lane['known_browser_checkpoint'], 'e2e-pending/workspace-wiki-selected-backend.spec.ts:413')
        self.assertNotIn(secret, raw)
        missing = dict(browser, browser_report_state='report-missing', known_browser_test=None,
                       known_browser_status=None, known_browser_checkpoint=None)
        diagnostic, raw = self._refused_ownership(missing, 'all mandatory lanes remain required', parent=True)
        self.assertEqual(diagnostic['lanes'][0]['browser_report_state'], 'report-missing')
        self.assertIsNone(diagnostic['lanes'][0]['known_browser_status'])
        self.assertNotIn(secret, raw)

    def test_checkpoint_reads_only_emitted_location_fields(self):
        secret = 'http://secret.example/a cookie=PRIVATE_BROWSER_SECRET'
        title = runner.KNOWN_ON_BROWSER_TEST
        driver = (HERE / 'current-postgres-driver.py').read_text()
        tree = ast.parse(driver)
        kept = [node for node in tree.body if (isinstance(node, ast.Assign) and any(
                    isinstance(target, ast.Name) and target.id in (
                        'KNOWN_ON_BROWSER_TEST', 'KNOWN_BROWSER_SUFFIXES', 'KNOWN_BROWSER_STATUSES')
                    for target in node.targets)) or (
                    isinstance(node, ast.FunctionDef) and node.name == 'known_browser_checkpoint')]
        state = {}
        exec(compile(ast.fix_missing_locations(ast.Module(body=kept, type_ignores=[])), 'known-browser', 'exec'), state)
        extract = state['known_browser_checkpoint']
        spec_file = '/opt/fvoci/apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'

        def report(result):
            return {'config': {'workers': 1}, 'suites': [{'specs': [{
                'title': title, 'file': spec_file, 'tests': [{'results': [result]}]}]}]}

        found = extract(report({'status': 'failed', 'errorLocation': {'file': spec_file, 'line': 413, 'column': 5},
                                'error': {'message': secret, 'stack': secret, 'snippet': secret},
                                'errors': [{'message': secret, 'location': {'file': spec_file, 'line': 9, 'column': 1}}]}))
        self.assertEqual(found['known_browser_checkpoint'], 'e2e-pending/workspace-wiki-selected-backend.spec.ts:413')
        self.assertNotIn(secret, json.dumps(found))
        missing = extract(report({'status': 'failed', 'errors': [{'message': secret}]}))
        self.assertEqual(missing['known_browser_status'], 'failed')
        self.assertIsNone(missing['known_browser_checkpoint'])
        self.assertNotIn(secret, json.dumps(missing))
        zero = extract(report({'status': 'failed', 'errorLocation': {'file': spec_file, 'line': 0, 'column': 0}}))
        self.assertIsNone(zero['known_browser_checkpoint'])
        timed = extract(report({'status': 'timedOut', 'errorLocation': {'file': spec_file, 'line': 323, 'column': 1}}))
        self.assertEqual(timed['known_browser_status'], 'timedOut')
        self.assertEqual(timed['known_browser_checkpoint'], 'e2e-pending/workspace-wiki-selected-backend.spec.ts:323')
        canonical = {'type': 'ReturnedNonzero', 'phase': 'browser', 'observedExit': 1}
        fields = runner.public_failure_fields({
            'failed_phase': 'browser', 'failure_code': 'SELECTED_BODY_NONZERO',
            'original_driver_failure': canonical, 'known_browser_test': title,
            'known_browser_status': 'failed',
            'known_browser_checkpoint': 'e2e-pending/workspace-wiki-selected-backend.spec.ts:413'})
        self.assertEqual(fields['known_browser_checkpoint'], 'e2e-pending/workspace-wiki-selected-backend.spec.ts:413')
        self.assertEqual(hashlib.sha256(json.dumps(canonical, sort_keys=True).encode()).hexdigest(),
                         hashlib.sha256(json.dumps({'observedExit': 1, 'phase': 'browser', 'type': 'ReturnedNonzero'}, sort_keys=True).encode()).hexdigest())




class SQLitePreparationFailureControls(unittest.TestCase):
    def test_host_network_proof_uses_attached_driver_not_mode_rendering(self):
        path = HERE / 'current-sqlite-driver.py'
        tree = ast.parse(path.read_text())
        main = next(n for n in tree.body if isinstance(n, ast.Try) and any(
            isinstance(a, ast.Assign) and any(isinstance(x, ast.Name) and x.id == 'created' for x in a.targets) for a in n.body))
        start = next(i for i, n in enumerate(main.body) if 'actual-network-mode.log' in ast.unparse(n))
        end = next(i for i, n in enumerate(main.body) if isinstance(n, ast.Assign) and ast.unparse(n.targets[0]) == "receipt['phase']")
        proof = compile(ast.fix_missing_locations(ast.Module(body=main.body[start:end], type_ignores=[])), str(path), 'exec')
        network_id = 'c' * 64
        # Alternate mode strings are synthetic compatibility cases, not a hosted reproduction.
        cases = [('host\n', {'host': {'NetworkID': network_id}}, network_id + ' host\n', True),
                 (network_id + '\n', {'host': {'NetworkID': network_id}}, network_id + ' host\n', True),
                 ('WARNING: synthetic stderr\nhost\n', {'host': {'NetworkID': network_id}}, network_id + ' host\n', True),
                 ('host\n', {'bridge': {'NetworkID': network_id}}, network_id + ' bridge\n', False),
                 ('host\n', {'host': {'NetworkID': network_id}}, network_id + ' Host\n', False),
                 ('host\n', {'host': {'NetworkID': network_id}}, 'd' * 64 + ' host\n', False),
                 ('host\n', {}, '', False),
                 ('host\n', {'host': {'NetworkID': network_id}, 'bridge': {'NetworkID': 'd' * 64}}, '', False)]
        for mode, attachments, actual_driver, accepted in cases:
            with self.subTest(mode=mode, attachments=attachments, driver=actual_driver), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                receipt = {'phase': 'container-prepare'}
                def command(args, log=None, preparation_receipt=None):
                    self.assertIs(preparation_receipt, receipt)
                    preparation_receipt['last_preparation_command_exit'] = 0
                    if args[1] == 'network':
                        self.assertEqual(args, ['docker', 'network', 'inspect', '--format', '{{.Id}} {{.Driver}}', network_id])
                        output = actual_driver
                    elif args[3] == '{{json .NetworkSettings.Networks}}':
                        output = json.dumps(attachments)
                    else:
                        self.assertEqual(args, ['docker', 'inspect', '--format', '{{.HostConfig.NetworkMode}}', 'synthetic-owned'])
                        output = mode
                    if log is not None:
                        log.write_text(output)
                    return types.SimpleNamespace(returncode=0, stdout=output)
                state = {'command': command, 'run': root, 'name': 'synthetic-owned', 'receipt': receipt, 'json': json}
                if accepted:
                    exec(proof, state)
                else:
                    with self.assertRaises(AssertionError):
                        exec(proof, state)
                self.assertEqual((root / 'actual-network-mode.log').read_text(), mode)
                self.assertEqual(receipt['last_preparation_command_exit'], 0)

    def test_actual_preparation_assertions_and_command_failure_record_before_cleanup(self):
        path = HERE / 'current-sqlite-driver.py'
        tree = ast.parse(path.read_text())
        helpers = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in ('command', 'failure_checkpoint')]
        main = next(n for n in tree.body if isinstance(n, ast.Try) and any(
            isinstance(a, ast.Assign) and any(isinstance(x, ast.Name) and x.id == 'created' for x in a.targets) for a in n.body))
        end = next(i for i, n in enumerate(main.body) if isinstance(n, ast.Assign) and ast.unparse(n.targets[0]) == "receipt['phase']")
        preparation = compile(ast.fix_missing_locations(ast.Module(body=main.body[:end], type_ignores=[])), str(path), 'exec')
        assertion_lines = [n.lineno for n in main.body[:end] if isinstance(n, ast.Assert)]
        for fault, expected_exit, expected_line in [('ldd', 0, assertion_lines[0]), ('hash', 0, assertion_lines[1]),
                ('network', 0, assertion_lines[-1]), ('query', 7, None), ('attachment-query', 7, None),
                ('driver-query', 7, None), ('spawn', None, None)]:
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                receipt = {'phase':'container-prepare'}
                def run(args, **kwargs):
                    if ((args[1] == 'inspect' and fault == 'query') or
                            (args[1] == 'inspect' and args[3] == '{{json .NetworkSettings.Networks}}' and fault == 'attachment-query') or
                            (args[1] == 'network' and fault == 'driver-query')):
                        return types.SimpleNamespace(returncode=7, stdout='', stderr='SYNTHETIC_PRIVATE_QUERY')
                    if args[1] == 'inspect' and fault == 'spawn':
                        raise OSError('SYNTHETIC_PRIVATE_SPAWN')
                    output = ''
                    if 'fvoci-runtime-abi' in args:
                        output = 'dependency not found\n' if fault == 'ldd' else 'qualified dependency\n'
                    elif 'sha256sum' in args:
                        output = '\n'.join(('wrong' if fault == 'hash' else 'h') + ' file' for _ in range(3))
                    elif args[1] == 'network':
                        output = 'c' * 64 + (' bridge\n' if fault == 'network' else ' host\n')
                    elif args[1] == 'inspect':
                        output = json.dumps({'host': {'NetworkID': 'c' * 64}}) if args[3] == '{{json .NetworkSettings.Networks}}' else 'host\n'
                    if 'stdout' in kwargs:
                        kwargs['stdout'].write(output)
                    return types.SimpleNamespace(returncode=0, stdout=output, stderr='')
                state = {'subprocess':types.SimpleNamespace(run=run, STDOUT=subprocess.STDOUT), 'os':os, 'json':json,
                    'sha':lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest(), 'W':ROOT, 'receipt':receipt,
                    'name':'synthetic-owned', 'OWNER':OWNER, 'IMAGE':'synthetic-image', 'run':root,
                    'dbroot':root/'db', 'storage':root/'storage', 'dist':root/'dist',
                    'server':'server', 'migrate':'migrate', 'engine':'engine',
                    'binaries':{p:{'sha256':'h'} for p in ('server','migrate','engine')}}
                exec(compile(ast.fix_missing_locations(ast.Module(body=helpers,type_ignores=[])), str(path), 'exec'), state)
                try:
                    exec(preparation, state)
                except (AssertionError, RuntimeError, OSError) as error:
                    state['failure_checkpoint'](receipt, root, None, error)
                else:
                    self.fail('actual preparation refusal was not reached')
                packet = json.loads((root/'original-failure.private.json').read_text())
                self.assertEqual((root/'original-failure.private.json').stat().st_mode & 0o777, 0o600)
                self.assertEqual(packet['preparation_command_exit'], expected_exit)
                checkpoint = packet['known_driver_checkpoint']
                self.assertTrue(checkpoint.startswith('scripts/selected-backend-ci/current-sqlite-driver.py:'))
                if expected_line is not None:
                    self.assertEqual(checkpoint.rsplit(':',1)[1], str(expected_line))
                published = runner.public_failure_fields(receipt)
                self.assertEqual(published['preparation_command_exit'], expected_exit)
                self.assertNotIn('SYNTHETIC_PRIVATE', json.dumps(published))
                receipt['last_preparation_command_exit'] = 99
                state['failure_checkpoint'](receipt, root, 99, RuntimeError('SECOND_PRIVATE_CLEANUP'))
                self.assertEqual(json.loads((root/'original-failure.private.json').read_text()), packet)
                self.assertEqual(receipt['preparation_command_exit'], expected_exit)
                self.assertIsNone(receipt['observed_failed_exit'])

    def test_foreign_error_frame_never_becomes_owned_checkpoint(self):
        path = HERE / 'current-sqlite-driver.py'
        helper = next(n for n in ast.parse(path.read_text()).body if isinstance(n, ast.FunctionDef) and n.name == 'failure_checkpoint')
        with tempfile.TemporaryDirectory() as tmp:
            state = {'os':os, 'json':json, 'W':ROOT, 'sha':lambda p:hashlib.sha256(Path(p).read_bytes()).hexdigest()}
            exec(compile(ast.fix_missing_locations(ast.Module(body=[helper],type_ignores=[])), str(path), 'exec'),state)
            receipt = {'phase':'container-prepare', 'last_preparation_command_exit':0}
            try:
                exec(compile("raise RuntimeError('PRIVATE_URL_cookie')", '/foreign/current-sqlite-driver.py', 'exec'))
            except RuntimeError as error:
                state['failure_checkpoint'](receipt,Path(tmp),None,error)
            self.assertIsNone(receipt['known_driver_checkpoint'])
            fields = runner.public_failure_fields(receipt)
            self.assertIsNone(fields['preparation_command_exit'])
            self.assertNotIn('PRIVATE_URL_cookie', json.dumps(fields))

    def test_public_projection_refuses_malformed_unmatched_and_noninteger_fields(self):
        facts = {'failed_phase':'container-prepare','original_driver_failure':{'type':'AssertionError','message':'PRIVATE'},
                 'failure_code':'SELECTED_DRIVER_EXCEPTION', 'known_driver_checkpoint':'scripts/selected-backend-ci/current-sqlite-driver.py:244',
                 'preparation_command_exit':0}
        self.assertEqual(runner.public_failure_fields(facts)['preparation_command_exit'],0)
        for checkpoint in (None, 244, 'https://private.example/a', '/foreign/current-sqlite-driver.py:244',
                'scripts/selected-backend-ci/current-postgres-driver.py:244',
                'scripts/selected-backend-ci/current-sqlite-driver.py:0',
                'scripts/selected-backend-ci/current-sqlite-driver.py:244\nPRIVATE'):
            fields = runner.public_failure_fields(dict(facts,known_driver_checkpoint=checkpoint))
            self.assertIsNone(fields['known_driver_checkpoint'])
            self.assertIsNone(fields['preparation_command_exit'])
        for value in (None, True, '0', 256, -256, {'secret':'PRIVATE'}):
            self.assertIsNone(runner.public_failure_fields(dict(facts,preparation_command_exit=value))['preparation_command_exit'])
        for phase in ('browser','server-ready','restart','PRIVATE'):
            fields = runner.public_failure_fields(dict(facts,failed_phase=phase))
            self.assertIsNone(fields['known_driver_checkpoint'])
            self.assertIsNone(fields['preparation_command_exit'])
        self.assertIsNone(runner.public_failure_fields(dict(facts,failure_code='PRIVATE'))['known_driver_checkpoint'])


class ActualDriverSourceControls(unittest.TestCase):
    def test_postgres_sqlite_direct_cli_keeps_on_off_selection_and_no_install(self):
        for name in ('postgres','sqlite'):
            tree = ast.parse((HERE/('current-'+name+'-driver.py')).read_text())
            args = next(n for n in ast.walk(tree) if isinstance(n,ast.Assign)
                        and any(isinstance(v,ast.Name) and v.id == 'args' for v in n.targets)
                        and isinstance(n.value,ast.List) and any(isinstance(v,ast.Name) and v.id == 'SPEC' for v in n.value.elts))
            for spec in ('workspace-wiki-selected-backend.spec.ts','workspace-off-selected-backend.spec.ts'):
                with self.subTest(driver=name,spec=spec):
                    state = {'BUN':Path('/qualified/bun'),'W':Path('/qualified'),
                             'PLAYWRIGHT_CLI':Path('/qualified/node_modules/playwright/cli.js'),'SPEC':spec}
                    exec(compile(ast.fix_missing_locations(ast.Module(body=[args],type_ignores=[])),'browser-cli','exec'),state)
                    self.assertEqual(state['args'],['/qualified/bun','--no-install','/qualified/node_modules/playwright/cli.js',
                        'test','--config','e2e-pending/collab-playwright.config.ts','--reporter=line,json',spec])

    def test_postgres_direct_cli_requires_pinned_bin_regular_file_and_admitted_hash(self):
        tree = ast.parse((HERE/'current-postgres-driver.py').read_text())
        start = next(i for i,n in enumerate(tree.body) if isinstance(n,ast.Assign)
                     and any(isinstance(v,ast.Name) and v.id == 'PLAYWRIGHT_CLI' for v in n.targets))
        qualification = compile(ast.fix_missing_locations(ast.Module(body=tree.body[start:start+5],type_ignores=[])),
                                'cli-qualification','exec')
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory);cli = root/'node_modules/playwright/cli.js';cli.parent.mkdir(parents=True)
            cli.write_text('synthetic official CLI');package = cli.parent/'package.json'
            package.write_text(json.dumps({'version':'1.63.0','bin':{'playwright':'cli.js'}}))
            bun = root/'bun';bun.write_text('synthetic qualified Bun')
            state = {'W':root,'BUN':bun,'json':json,'before':{'external':{str(cli):runner.sha(cli)}},'sha':runner.sha}
            exec(qualification,state)
            for fault in ('hash','missing','bin','version','symlink'):
                with self.subTest(fault=fault):
                    package.write_text(json.dumps({'version':'1.63.0','bin':{'playwright':'cli.js'}}))
                    state['before']['external'] = {str(cli):runner.sha(cli)}
                    if fault == 'hash':state['before']['external'][str(cli)] = '0'*64
                    if fault == 'missing':state['before']['external'] = {}
                    if fault == 'bin':package.write_text(json.dumps({'version':'1.63.0','bin':{'playwright':'other.js'}}))
                    if fault == 'version':package.write_text(json.dumps({'version':'0.0.0','bin':{'playwright':'cli.js'}}))
                    if fault == 'symlink':
                        target = root/'foreign-cli';cli.rename(target);cli.symlink_to(target)
                    with self.assertRaises((AssertionError,KeyError)):exec(qualification,state)
                    if fault == 'symlink':cli.unlink();target.rename(cli)

    def test_actual_container_absence_requires_positive_docker_absence(self):
        for name in ('sqlite','install'):
            tree=ast.parse((HERE/('current-'+name+'-driver.py')).read_text())
            assignment=next(n for n in ast.walk(tree) if isinstance(n,ast.Assign)
                and any(isinstance(t,ast.Subscript) and ast.unparse(t)=="receipt['owned_container_absent']" for t in n.targets)
                and isinstance(n.value,ast.BoolOp))
            for status,message,expected in ((1,'No such container: owned',True),(1,'permission denied',False),
                                             (1,'daemon unreachable',False),(0,'No such container: owned',False)):
                result=type('PureDockerResult',(),{'returncode':status,'stderr':message})()
                state={'receipt':{},'absence':result,'absent':result}
                exec(compile(ast.fix_missing_locations(ast.Module(body=[assignment],type_ignores=[])),name,'exec'),state)
                self.assertIs(state['receipt']['owned_container_absent'],expected)

    def test_empty_process_observation_cannot_claim_retired_identities(self):
        for name in ('postgres','sqlite'):
            tree=ast.parse((HERE/('current-'+name+'-driver.py')).read_text())
            assignment=next(n for n in ast.walk(tree) if isinstance(n,ast.Assign)
                and any(isinstance(t,ast.Subscript) and ast.unparse(t)=="receipt['recorded_process_identities_retired']" for t in n.targets))
            for rows,gone,expected in (([],True,False),([{}],False,False),([{}],True,True)):
                state={'receipt':{},'rows':rows,'identity_gone':lambda _:gone}
                exec(compile(ast.fix_missing_locations(ast.Module(body=[assignment],type_ignores=[])),name,'exec'),state)
                self.assertIs(state['receipt']['recorded_process_identities_retired'],expected)

    def test_actual_unknown_port_and_original_phase_records_without_runtime_import(self):
        for name in ('postgres','sqlite'):
            with self.subTest(backend=name):
                tree=ast.parse((HERE/('current-'+name+'-driver.py')).read_text())
                initial=next(n for n in tree.body if isinstance(n,ast.Expr) and isinstance(n.value,ast.Call)
                    and isinstance(n.value.func,ast.Attribute) and n.value.func.attr=='update'
                    and any(k.arg=='owned_loopback_port_closed' for k in n.value.keywords))
                main=next(n for n in tree.body if isinstance(n,ast.Try) and n.finalbody)
                unknown=next(n for n in main.finalbody if isinstance(n,ast.If)
                    and ast.unparse(n.test)=='base is None')
                state={'receipt':{},'cleanup_errors':[],'base':None,'code':7,'error':AssertionError(PRIVATE['message']),
                       'run':Path(tempfile.mkdtemp(prefix='fvoci-early-pure-')), 'json':json, 'os':os,
                       'sha':lambda path:hashlib.sha256(Path(path).read_bytes()).hexdigest()}
                helpers=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in ('failure_checkpoint','cleanup_attempt')]
                self.addCleanup(__import__('shutil').rmtree,state['run'])
                body=[*helpers,initial,*main.handlers[0].body,unknown]
                exec(compile(ast.fix_missing_locations(ast.Module(body=body,type_ignores=[])),str(HERE/name),'exec'),state)
                self.assertIsNone(state['receipt']['owned_loopback_port_closed'])
                self.assertEqual(state['receipt']['failed_phase'],'container-prepare')
                self.assertEqual(state['receipt']['original_driver_failure'],PRIVATE)
                self.assertEqual(state['code'],7)
                self.assertEqual(len(state['cleanup_errors']),1)

    def test_actual_driver_public_failure_summaries_disclose_hashes_only(self):
        for name in ('postgres','sqlite','install'):
            with self.subTest(backend=name):
                tree=ast.parse((HERE/('current-'+name+'-driver.py')).read_text())
                main=next(n for n in tree.body if isinstance(n,ast.Try) and n.finalbody)
                start=next(i for i,n in enumerate(main.finalbody) if isinstance(n,ast.Assign)
                           and any(isinstance(t,ast.Name) and t.id=='summary' for t in n.targets))
                receipt={'source':SOURCE,'original_driver_failure':PRIVATE,'driver_error':PRIVATE['message'],
                         'final_exit_code':7,'failed_phase':'server-startup','retained_private_evidence':PRIVATE['message']}
                stdout=io.StringIO()
                with contextlib.redirect_stdout(stdout):
                    exec(compile(ast.fix_missing_locations(ast.Module(body=main.finalbody[start:],type_ignores=[])),str(HERE/name),'exec'),
                         {'receipt':receipt,'code':7,'json':json,'hashlib':hashlib,'cleanup_errors':[]})
                summary=json.loads(stdout.getvalue())
                self.assertNotIn(PRIVATE['message'],stdout.getvalue())
                self.assertEqual(len(summary['original_driver_failure_sha256']),64)
                self.assertEqual(summary['final_exit_code'],7)


class WholeFinalizationFaults(unittest.TestCase):
    def exercise(self, backend, fault=None, ordinary=False):
        path = HERE / ('current-' + backend + '-driver.py')
        tree = ast.parse(path.read_text())
        main = next(n for n in tree.body if isinstance(n, ast.Try) and n.finalbody)
        helpers = [n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name in ('failure_checkpoint', 'cleanup_attempt')]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ('browser.log','test.log','normal-server.log'): (root/name).write_text('private synthetic body log')
            receipt = {'source':SOURCE,'tree':TREE,'root_owner':OWNER,'selected_flow':'on','phase':'browser' if backend!='install' else 'install-body',
                       'browser_exit':7, 'exit_code':7, 'owned_container_absent':None, 'owned_loopback_port_closed':None,
                       'recorded_process_identities_retired':None}
            attempted = []; writes = []
            def command(args, **kwargs):
                attempted.append(tuple(args[:2]))
                self.assertTrue((root/'original-failure.private.json').exists(), 'original packet must precede cleanup')
                if fault == 'remove' and args[1]=='rm': raise OSError('SECOND_PRIVATE_CLEANUP')
                if fault == 'inspect' and args[1]=='inspect': raise OSError('SECOND_PRIVATE_CLEANUP')
                if fault == 'copy' and args[1]=='cp': raise OSError('SECOND_PRIVATE_CLEANUP')
                return types.SimpleNamespace(returncode=1 if args[1]=='inspect' else 0,stderr='No such container: owned')
            def observe_rows(name):
                if fault == 'rows': raise OSError('SECOND_PRIVATE_CLEANUP')
                return [{'pid':22}]
            def write(path, value):
                writes.append(path.name)
                if fault == 'receipt' and path.name=='receipt.json': raise OSError('SECOND_PRIVATE_CLEANUP')
                path.write_text(json.dumps(value))
            def sha(path):
                if fault == 'log-hash' and Path(path).name=='normal-server.log': raise OSError('SECOND_PRIVATE_CLEANUP')
                if backend=='install' and fault=='inputs' and Path(path).name=='test.log' and (root/'original-failure.private.json').exists(): raise OSError('SECOND_PRIVATE_CLEANUP')
                if Path(path).is_file(): return hashlib.sha256(Path(path).read_bytes()).hexdigest()
                return 's'*64
            def post():
                if fault == 'inputs': raise OSError('SECOND_PRIVATE_CLEANUP')
                return {}
            process = types.SimpleNamespace(poll=lambda:None if fault=='wait' else 0,
                wait=lambda **kwargs: (_ for _ in ()).throw(OSError('SECOND_PRIVATE_CLEANUP')) if fault=='wait' else 0,
                kill=lambda: (_ for _ in ()).throw(OSError('SECOND_PRIVATE_CLEANUP')) if fault=='wait' else None)
            log = types.SimpleNamespace(close=lambda: (_ for _ in ()).throw(OSError('SECOND_PRIVATE_CLEANUP')) if fault=='log-close' else None)
            class Socket:
                def __enter__(self): return self
                def __exit__(self,*args): pass
                def settimeout(self,*args): pass
                def connect_ex(self,*args):
                    if fault=='port': raise OSError('SECOND_PRIVATE_CLEANUP')
                    return 1
            state={'receipt':receipt, 'run':root,'P':root,'W':root,'NAME':'owned','name':'owned','code':7,'created':True,
                   'server_row':{'namespace_pid':22},'server_process':process,'server_log':log,
                   'base':'http://127.0.0.1:12345','owned_rows':observe_rows,'identity_gone':lambda row:row.get('pid')==22,
                   'owned_object_absent':lambda args:command(args).returncode!=0,'command':command,
                   'pg_sql':lambda sql:{},'write':write,'sha':sha,'post_inputs':post,'input_check':lambda before:post(),
                   'source_before':{},'before':{'tracked':{'test.log':hashlib.sha256(b'private synthetic body log').hexdigest()},'external':{}},'browser_inputs':{},'assets':{'dist_files':{}},
                   'dist':root,'tree_hashes':lambda path:{},'binaries':{n:{'sha256':'s'*64} for n in ('server','migrate','fixture','engine')},
                   'server':'server','migrate':'migrate','fixture':'fixture','engine':'engine','bins':{},'abi':{'host_runtime_files':{}},
                   'storage':root/'storage','dbroot':root/'db','now':lambda:'pure-clock',
                   'socket':types.SimpleNamespace(socket=Socket),'subprocess':types.SimpleNamespace(check_output=lambda *args,**kw:SOURCE+'\n'),
                   'shutil':types.SimpleNamespace(disk_usage=lambda path:types.SimpleNamespace(free=123)),
                   'json':json,'hashlib':hashlib,'os':os,'result':types.SimpleNamespace(returncode=7)}
            if ordinary:
                index=next(i for i,n in enumerate(main.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='code' for t in n.targets) and ast.unparse(n.value)=='result.returncode')
                body=[*helpers,*main.body[index:index+2],*main.finalbody]
            else:
                main.body=ast.parse("raise RuntimeError('FIRST_PRIVATE_ORIGINAL')").body
                body=[*helpers,main]
            stdout=io.StringIO()
            with contextlib.redirect_stdout(stdout): exec(compile(ast.fix_missing_locations(ast.Module(body=body,type_ignores=[])),str(path),'exec'),state)
            self.assertEqual(state['code'],7)
            self.assertIn('receipt.json',writes)
            packet=json.loads((root/'original-failure.private.json').read_text())
            summary=json.loads(stdout.getvalue())
            self.assertEqual(summary['final_exit_code'],7)
            self.assertEqual(summary['failed_phase'],receipt['phase'])
            self.assertEqual(packet['observed_failed_exit'],7)
            self.assertNotIn('FIRST_PRIVATE_ORIGINAL',stdout.getvalue())
            self.assertNotIn('SECOND_PRIVATE_CLEANUP',stdout.getvalue())
            if ordinary:
                self.assertEqual(packet['original_driver_failure']['type'],'ReturnedNonzero')
                self.assertEqual(packet['failure_code'],'SELECTED_BODY_NONZERO')
                self.assertEqual(packet['original_body_log_sha256'],hashlib.sha256(b'private synthetic body log').hexdigest())
            else:
                self.assertEqual(packet['original_driver_failure'],{'type':'RuntimeError','message':'FIRST_PRIVATE_ORIGINAL'})
            if fault:
                self.assertTrue(summary['cleanup_failure_codes'])
            if fault=='rows': self.assertIsNone(receipt['recorded_process_identities_retired'])
            if fault=='port': self.assertIsNone(receipt['owned_loopback_port_closed'])
            if fault=='inspect': self.assertIsNone(receipt['owned_container_absent'])
            self.assertIn(('docker','inspect'),attempted)
            if fault!='receipt': self.assertTrue((root/'receipt.json').exists())
            return summary

    def test_entire_exception_handler_and_finalization_survives_each_secondary_fault(self):
        for backend in ('postgres','sqlite','install'):
            faults=('remove','inspect','inputs','receipt','copy') if backend=='install' else ('rows','remove','inspect','wait','log-close','log-hash','port','inputs','receipt')
            self.exercise(backend)
            for fault in faults:
                with self.subTest(backend=backend,fault=fault): self.exercise(backend,fault)

    def test_ordinary_nonzero_body_phase_exit_and_original_log_digest_survive_cleanup(self):
        for backend in ('postgres','sqlite','install'):
            for fault in (None,'remove','inputs'):
                with self.subTest(backend=backend,fault=fault): self.exercise(backend,fault,ordinary=True)

    def test_postgres_nonzero_browser_records_first_error_before_cleanup(self):
        path = HERE / 'current-postgres-driver.py'
        tree = ast.parse(path.read_text())
        main = next(n for n in tree.body if isinstance(n, ast.Try) and n.finalbody)
        index = next(i for i, n in enumerate(main.body) if isinstance(n, ast.Assign)
                     and any(isinstance(t, ast.Name) and t.id == 'code' for t in n.targets)
                     and ast.unparse(n.value) == 'result.returncode')
        self.assertIsInstance(main.body[index + 1], ast.If)
        helpers = [n for n in tree.body if (isinstance(n, ast.Assign) and any(
                       isinstance(t, ast.Name) and t.id in (
                           'KNOWN_ON_BROWSER_TEST', 'KNOWN_BROWSER_SUFFIXES', 'KNOWN_BROWSER_STATUSES')
                       for t in n.targets)) or (isinstance(n, ast.FunctionDef)
                       and n.name in ('failure_checkpoint', 'known_browser_checkpoint'))]
        secret = 'SECRET_COOKIE=synthetic-not-a-cause'
        spec_file = '/opt/fvoci/apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'browser.log').write_text('1 failed\n' + secret + '\n')
            report = {'config': {'workers': 1}, 'suites': [{'specs': [{
                'title': 'selected normal main: Vue setup, stable wiki create, native persist, manual revision and fresh actor readback',
                'file': spec_file,
                'tests': [{'results': [{'status': 'failed', 'error': {'message': secret, 'stack': secret},
                    'errorLocation': {'file': spec_file, 'line': 42, 'column': 1}}]}]}]}]}
            (root / 'playwright-result.private.json').write_text(json.dumps(report))
            attempted = []
            def command(args, **kwargs):
                attempted.append(args)
                raise AssertionError('cleanup must not start')
            receipt = {'phase': 'browser'}
            state = {'receipt': receipt, 'run': root, 'result': types.SimpleNamespace(returncode=7),
                     'os': os, 'json': json, 'command': command,
                     'sha': lambda item: hashlib.sha256(Path(item).read_bytes()).hexdigest(),
                     'time': types.SimpleNamespace(monotonic=lambda: 0), 'now': lambda: 'pure-clock'}
            exec(compile(ast.fix_missing_locations(ast.Module(body=[*helpers, *main.body[index:index + 2]], type_ignores=[])), str(path), 'exec'), state)
            packet = json.loads((root / 'original-failure.private.json').read_text())
            self.assertEqual(packet['browser_report_state'], 'matched')
            self.assertEqual(packet['known_browser_status'], 'failed')
            self.assertEqual(packet['known_browser_checkpoint'], 'e2e-pending/workspace-wiki-selected-backend.spec.ts:42')
            self.assertEqual(packet['failure_code'], 'SELECTED_BODY_NONZERO')
            self.assertNotIn(secret, json.dumps(packet))
            for name in ('browser.log', 'playwright-result.private.json'):
                self.assertEqual((root / name).stat().st_mode & 0o777, 0o600)
            self.assertFalse((root / 'browser-first-error.private.log').exists())
            self.assertFalse((root / 'playwright-first-error.private.json').exists())
            self.assertEqual(attempted, [])
            self.assertFalse((root / 'receipt.json').exists())

    def test_pg_parent_fixture_and_post_input_exceptions_still_emit_final_receipt(self):
        path=HERE/'current-postgres-driver.py';tree=ast.parse(path.read_text())
        branch=next(n for n in tree.body if isinstance(n,ast.If) and ast.unparse(n.test)=='len(sys.argv) == 1')
        start=next(i for i,n in enumerate(branch.body) if isinstance(n,ast.Assign) and any(isinstance(t,ast.Name) and t.id=='summary' for t in n.targets))
        helpers=[n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name in ('failure_checkpoint','cleanup_attempt')]
        for fault in ('fixture','post','both','write'):
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as directory:
                root=Path(directory);(root/'owned-fixtures.log').write_text('synthetic wrapper log')
                (root/'receipt.json').write_text(json.dumps({'final_exit_code':7}))
                writes=[]
                def write(path,value):
                    writes.append(path.name)
                    if fault=='write' and path.name=='parent-receipt.json':raise OSError('SECOND_PARENT_PRIVATE')
                    path.write_text(json.dumps(value))
                def fixtures(run):
                    if fault in ('fixture','both'):raise OSError('SECOND_PARENT_PRIVATE')
                    return [dict(containerAbsent=True,recordedPIDIdentitiesRetired=True,portClosed=True,ownedVolumesAbsent={'owned':True}) for _ in range(2)]
                def post():
                    if fault in ('post','both'):raise OSError('SECOND_PARENT_PRIVATE')
                    return {}
                state={'HEAD':SOURCE,'TREE':TREE,'COMPILED_HEAD':SOURCE,'OWNER':OWNER,'FLOW':'on','run':root,
                       'now':lambda:'pure-clock','command':lambda *args,**kwargs:types.SimpleNamespace(returncode=7),
                       'PG_SCRIPT':'not-executed','sys':types.SimpleNamespace(executable='not-executed',exit=lambda code:code),
                       'Path':Path,'__file__':str(path),'environment':{},'verify_fixtures_closed':fixtures,'source_before':{},
                       'post_inputs':post,'write':write,'sha':lambda path:hashlib.sha256(Path(path).read_bytes()).hexdigest(),
                       'json':json,'os':os,'hashlib':hashlib}
                stdout=io.StringIO()
                with contextlib.redirect_stdout(stdout):exec(compile(ast.fix_missing_locations(ast.Module(body=[*helpers,*branch.body[start:]],type_ignores=[])),str(path),'exec'),state)
                self.assertEqual(state['code'],7)
                self.assertIn('parent-receipt.json',writes)
                self.assertTrue((root/'parent-original-failure.private.json').exists())
                self.assertNotIn('SECOND_PARENT_PRIVATE',stdout.getvalue())
                self.assertEqual(json.loads(stdout.getvalue())['final_exit_code'],7)
                if fault!='write':self.assertTrue((root/'parent-receipt.json').exists())


    def test_aggregate_original_and_final_receipt_faults_preserve_first_exit_and_attempt_both(self):
        path=ROOT/'scripts/run-selected-backend-e2e.py';tree=ast.parse(path.read_text())
        function=next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='run')
        main=next(n for n in function.body if isinstance(n,ast.Try) and n.finalbody)
        main.body=ast.parse("raise RuntimeError('FIRST_AGGREGATE_PRIVATE')").body
        for fault in ('original','aggregate','both'):
            with self.subTest(fault=fault),tempfile.TemporaryDirectory() as directory:
                output=Path(directory);writes=[]
                def write(path,value):
                    writes.append(path.name)
                    if fault=='both' or (fault=='original' and path.name.endswith('.private.json')) or (fault=='aggregate' and path.name=='selected-ci-receipt.json'):
                        raise OSError('SECOND_AGGREGATE_PRIVATE')
                    path.write_text(json.dumps(value))
                state={'output':output,'code':7,'before':{'head':SOURCE,'tree':TREE},'owner':OWNER,
                       'results':[{'lane':'postgres','flow':'on','exit':7}], 'launcher_failure':None,
                       'selected_runs':runner.selected_runs,'write':write,'sha':lambda path:hashlib.sha256(Path(path).read_bytes()).hexdigest(),
                       'hashlib':hashlib,'json':json}
                stdout=io.StringIO()
                with contextlib.redirect_stdout(stdout):exec(compile(ast.fix_missing_locations(ast.Module(body=[main],type_ignores=[])),str(path),'exec'),state)
                self.assertEqual(state['code'],7)
                self.assertEqual(writes,['selected-launcher-failure.private.json','selected-ci-receipt.json'])
                self.assertEqual(state['launcher_failure']['receiptWrite'],'failed' if fault in ('original','both') else 'confirmed')
                self.assertEqual(len(state['launcher_failure']['originalOutcomeSha256']),64)
                self.assertNotIn('FIRST_AGGREGATE_PRIVATE',stdout.getvalue())
                self.assertNotIn('SECOND_AGGREGATE_PRIVATE',stdout.getvalue())
                if fault in ('aggregate','both'):self.assertEqual(json.loads(stdout.getvalue())['exit'],7)


class ConfigListPreflight(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.fixture_owner = (os.getuid(), os.getgid())
        self.output = self.root/'output';self.output.mkdir(mode=0o700)
        self.bun = self.root/'bun';self.bun.write_text('synthetic pinned executable')
        self.bun.chmod(0o555)
        self.chromium = self.root/'chromium';self.chromium.write_text('synthetic admitted browser')
        self.before = {'head':SOURCE,'tree':TREE}
        self.browser = {'bun':{'path':str(self.bun),'sha256':runner.sha(self.bun)},
                        'chromium':{'path':str(self.chromium),'sha256':runner.sha(self.chromium)}}
        self.env = {'PATH':'/usr/bin','LANG':'C.UTF-8','PLAYWRIGHT_BROWSERS_PATH':'/qualified/browser',
                    'HOME':'PRIVATE_HOME','GITHUB_TOKEN':'SYNTHETIC_TOKEN','FVOCI_E2E_ADMIN_DATABASE_URL':'SYNTHETIC_DB_SECRET',
                    'PASSWORD_PEPPER_KEYS':'SYNTHETIC_PEPPER','DATABASE_URL':'SYNTHETIC_DB_SECRET',
                    'FVOCI_TEST_TURSO_URL':'SYNTHETIC_REMOTE_SECRET'}

    def listing(self):
        return {'config':{'workers':1,'metadata':{'selectedBackend':'postgres','selectedFlow':'on'}},
                'errors':[],'stats':{'expected':0,'unexpected':0,'flaky':0,'skipped':1},
                'suites':[{'specs':[{'title':runner.KNOWN_ON_BROWSER_TEST,'tests':[{'results':[]}]}]}]}

    def execute(self, exit_code=0, report=True, expected_uid=None):
        def child(args, **kw):
            self.assertEqual(args,[str(self.bun),'--no-install',str(runner.ROOT/'node_modules/playwright/cli.js'),'test','--config',
                                  'e2e-pending/collab-playwright.config.ts','--reporter=line,json','--list',
                                  'workspace-wiki-selected-backend.spec.ts'])
            self.assertEqual(kw['cwd'],runner.ROOT/'apps/web')
            self.assertEqual(kw['stdin'],subprocess.DEVNULL)
            self.assertNotIn('stdout',kw);self.assertNotIn('stderr',kw)
            allowed = {'PATH','LANG','CI','TMPDIR','BUN_RUNTIME_TRANSPILER_CACHE_PATH','PLAYWRIGHT_BROWSERS_PATH',
                       'FVOCI_E2E_SELECTED_BACKEND','FVOCI_E2E_SELECTED_FLOW','FVOCI_E2E_SELECTED_AUXILIARY',
                       'FVOCI_E2E_SELECTED_SOURCE','FVOCI_E2E_SELECTED_COMPILED_SOURCE','FVOCI_E2E_RESULT_DIR',
                       'PLAYWRIGHT_JSON_OUTPUT_FILE'}
            self.assertEqual(set(kw['env']),allowed)
            self.assertEqual(kw['env']['FVOCI_E2E_SELECTED_SOURCE'],SOURCE)
            self.assertEqual(kw['env']['FVOCI_E2E_SELECTED_COMPILED_SOURCE'],SOURCE)
            self.assertEqual(kw['env']['CI'],'true')
            self.assertNotIn('SYNTHETIC_',json.dumps(kw['env']))
            if report:Path(kw['env']['PLAYWRIGHT_JSON_OUTPUT_FILE']).write_text(json.dumps(self.listing()))
            if exit_code:print('ORIGINAL_CONFIG_LOAD_FAILURE',file=runner.sys.stderr)
            return types.SimpleNamespace(returncode=exit_code)
        stdout, stderr = io.StringIO(), io.StringIO()
        with patch.dict(runner.os.environ,self.env,clear=True), \
             patch.object(runner,'config_list_inputs',return_value=(self.before,self.browser,{})) as admission, \
             patch.object(runner.subprocess,'run',side_effect=child) as launch, \
             contextlib.redirect_stdout(stdout),contextlib.redirect_stderr(stderr):
            code = runner.config_list(self.output, self.fixture_owner[0] if expected_uid is None else expected_uid)
        return code, stdout.getvalue(), stderr.getvalue(), admission.call_count, launch.call_count

    def test_filtered_child_env_and_pinned_list_success_is_zero_body_only(self):
        code, stdout, stderr, admissions, launches = self.execute()
        self.assertEqual((code,admissions,launches),(0,2,1));self.assertEqual(stderr,'')
        receipt = json.loads((self.output/'config-list/qualified.json').read_text())
        self.assertEqual(receipt['actual_browser_tests'],0);self.assertEqual(receipt['actual_db_tests'],0)
        self.assertTrue(receipt['list_only']);self.assertNotIn('SYNTHETIC_',stdout)
        self.assertFalse((self.output/'selected-ci-receipt.json').exists())
        self.assertFalse((self.output/'runtime').exists())
        for path in (self.output/'config-list').rglob('*'):
            self.assertEqual(path.stat().st_mode & 0o777,0o700 if path.is_dir() else 0o600)

    def test_original_nonzero_error_and_exit_precede_any_postcheck_or_resource(self):
        code, stdout, stderr, admissions, launches = self.execute(7,report=False)
        self.assertEqual((code,admissions,launches),(7,1,1))
        self.assertEqual(stderr,'ORIGINAL_CONFIG_LOAD_FAILURE\n')
        self.assertEqual(json.loads((self.output/'config-list/result.json').read_text())['exit'],7)
        self.assertFalse((self.output/'config-list/qualified.json').exists())
        self.assertFalse((self.output/'runtime').exists())

    def test_zero_exit_missing_json_is_not_configuration_load_acceptance(self):
        with self.assertRaises(AssertionError):self.execute(0,report=False)
        self.assertEqual(json.loads((self.output/'config-list/result.json').read_text())['exit'],0)
        self.assertFalse((self.output/'config-list/qualified.json').exists())

    def test_foreign_report_owner_never_qualifies_a_zero_exit(self):
        with self.assertRaises(AssertionError):self.execute(expected_uid=self.fixture_owner[0] + 1)
        self.assertEqual(json.loads((self.output/'config-list/result.json').read_text())['exit'],0)
        self.assertFalse((self.output/'config-list/qualified.json').exists())

    def test_occupied_or_symlink_output_is_preserved_without_launch(self):
        foreign = self.root/'foreign';foreign.mkdir();(foreign/'old').write_text('retained')
        destination = self.output/'config-list'
        for symlink in (False,True):
            if symlink:destination.symlink_to(foreign,target_is_directory=True)
            else:destination.mkdir();(destination/'old').write_text('retained')
            with patch.object(runner,'config_list_inputs',return_value=(self.before,self.browser,{})), \
                 patch.object(runner.subprocess,'run') as launch:
                with self.assertRaises(FileExistsError):runner.config_list(self.output)
                launch.assert_not_called()
            self.assertEqual((destination/'old').read_text(),'retained')
            if symlink:destination.unlink()
            else:(destination/'old').unlink();destination.rmdir()

    def prepare_cohort(self):
        source = self.root/'source';source.mkdir()
        config = source/'config.ts';config.write_text('fixed selected config')
        (source/'apps/web/dist').mkdir(parents=True)
        (source/'apps/web/dist/asset.js').write_text('fixed dist')
        external = {str(self.bun):runner.sha(self.bun)}
        for name in ('@playwright/test','playwright','playwright-core'):
            path = source/'node_modules'/name/'package.json';path.parent.mkdir(parents=True)
            path.write_text(json.dumps({'version':'1.63.0','bin':{'playwright':'cli.js'}}));external[str(path)]=runner.sha(path)
        cli = source/'node_modules/playwright/cli.js';cli.write_text('synthetic official CLI');external[str(cli)]=runner.sha(cli)
        before = {'head':SOURCE,'tree':TREE,'status':'','tracked':{'config.ts':runner.sha(config)},
                  'external':external,'untracked':{}}
        binaries = {}
        for name in ('fvoci-server','fvoci-migrate','fvoci-e2e-fixture','fvoci_server','selected_install_lifetime','collab-engine'):
            path = self.root/name;path.write_text('synthetic coherent artifact');path.chmod(0o555)
            binaries[str(path)] = {'sha256':runner.sha(path)}
        core = self.root/'libfvoci.rlib';core.write_text('synthetic emitted core');core.chmod(0o444)
        bundle = {'source':SOURCE,'tree':TREE,'binaries':binaries,'compiler_artifacts':[{
            'target':{'name':'fvoci_server'},'profile':{'test':False},'features':['api-schema','db-tests'],
            'filenames':[str(core)]}]}
        receipts = {'before.json':before,'after.json':before,'bundle.json':bundle,
                    'build-environment.json':{'bun':'1.4.2'},'build-env-inputs.json':{},'compile-receipt.json':{},
                    'web-receipt.json':{'source':SOURCE,'tree':TREE,'dist_files':{'asset.js':runner.sha(source/'apps/web/dist/asset.js')}},
                    'abi-receipt.json':{'currentSource':SOURCE,'host_runtime_files':{}}}
        for name,data in receipts.items():runner.write(self.output/name,data)
        for stage in ('main','lib','install','engine'):
            for suffix in ('-stage.json','-compiler.jsonl'):runner.write(self.output/(stage+suffix),{})
        received = {}
        for path in [*(self.output/name for name in receipts),*(self.output/(n+s) for n in ('main','lib','install','engine')
                      for s in ('-stage.json','-compiler.jsonl')),*(Path(p) for p in binaries),core]:
            received[str(path)] = {'sha256':runner.sha(path),'inode':path.stat().st_ino,'mode':path.stat().st_mode & 0o777}
        runner.write(self.output/'handoff-consumed.json',{'source':SOURCE,'tree':TREE,'repository':'owned/repo',
                     'run':'123','attempt':'1','full_current_physical_inputs_equal':True,'fresh_dist_equal':True,'received':received})
        groups = sorted(os.getgroups())
        runner.write(self.output/'runtime-access-stage.json',{'source':SOURCE,'tree':TREE,'owner':OWNER,
                     'runtime_uid':1000,'runtime_gid':1000,'groups':groups,'preflight_exit':0,'preflight':{'missing':0}})
        browser = self.output/'browser';browser.mkdir(mode=0o700)
        component = browser/'chromium-123';component.mkdir(mode=0o700)
        chromium = component/'chrome';chromium.write_text('synthetic private executable');chromium.chmod(0o700)
        runner.write(self.output/'runtime-browser-stage.json',{'source':SOURCE,'cache':str(browser),'chromium':str(chromium),
                     'files':{component.name:runner.browser_inventory(component,self.fixture_owner)},
                     'metadata':{component.name:runner.browser_inventory(component,self.fixture_owner,metadata=True)}})
        env = {'CI':'true','GITHUB_ACTIONS':'true','FVOCI_WEB_BUILD_PHASE':'consume','GITHUB_JOB':'collaboration-flow',
               'GITHUB_SHA':SOURCE,'GITHUB_REPOSITORY':'owned/repo','GITHUB_RUN_ID':'123','GITHUB_RUN_ATTEMPT':'1',
               'PLAYWRIGHT_BROWSERS_PATH':str(browser)}
        def git(args):
            return TREE if args[-1]=='HEAD^{tree}' else 'config.ts\0' if args[-1]=='-z' else ''
        return source,config,env,git

    def test_browser_fixture_owner_is_exact_and_symlink_assets_refuse(self):
        _, _, env, _ = self.prepare_cohort()
        with patch.dict(runner.os.environ,env,clear=True):
            self.assertEqual(runner.admitted_browser(self.output,self.fixture_owner),str(self.output/'browser/chromium-123/chrome'))
            for owner in ((self.fixture_owner[0] + 1,self.fixture_owner[1]),
                          (self.fixture_owner[0],self.fixture_owner[1] + 1),
                          (self.fixture_owner[0],0)):
                with self.subTest(owner=owner),self.assertRaises(AssertionError):runner.admitted_browser(self.output,owner)
            (self.output/'browser/chromium-123/linked-asset').symlink_to(self.bun)
            with self.assertRaises(AssertionError):runner.admitted_browser(self.output,self.fixture_owner)

    def wrapper(self, leaf=0, owner=0, occupied=False):
        safe = self.root/'safe';safe.mkdir(mode=0o700)
        if occupied:
            name = 'config-list.stderr.log' if occupied == 'stderr' else 'config-list.stdout.log'
            (safe/name).write_text('OLD_CAPTURE')
        source = (ROOT/'scripts/run-web-e2e.sh').read_text()
        start = source.index('  config_list_exit=not-run\n')
        end = source.index('\nfi\nif [[ "$pending_status"',start)
        script = self.root/'wrapper-fragment.sh'
        script.write_text('''set -euo pipefail
sudo() {
  for arg in "$@"; do
    case "$arg" in
      config-list) printf 'list\n' >> "$MARKS"; printf 'CREDENTIAL_FREE_LIST\n'; printf 'ORIGINAL_LOAD_ERROR\n' >&2; return "$LEAF" ;;
      run) printf 'run\n' >> "$MARKS"; return 0 ;;
      owner-return) printf 'owner\n' >> "$MARKS"; printf '{"ownership_return_qualified":false}\n'; return "$OWNER_EXIT" ;;
    esac
  done
  return 0
}
''' + source[start:end] + '\nexit "$selected_status"\n')
        env = {'PATH':os.environ['PATH'],'SELECTED_PHASE':'consume','ROOT':str(ROOT),'safe_diagnostics':str(safe),
               'FVOCI_SELECTED_CI_OUTPUT':str(self.output),'FVOCI_SELECTED_CI_SQLITE_PARENT':str(self.root/'sqlite'),
               'runtime_groups':'1000','runner_uid':'1000','runner_gid':'1000','selected_status':'0','pending_status':'0',
               'LEAF':str(leaf),'OWNER_EXIT':str(owner),'MARKS':str(self.root/'marks')}
        result = subprocess.run(['bash',str(script)],env=env,capture_output=True,text=True)
        marks = (self.root/'marks').read_text().splitlines() if (self.root/'marks').exists() else []
        return result,safe,marks

    def test_wrapper_leaf_failure_skips_real_launcher_preserves_error_and_no_start_owner_gate(self):
        result,safe,marks = self.wrapper(leaf=7)
        self.assertEqual(result.returncode,7);self.assertEqual(marks,['list','owner'])
        receipt = json.loads((safe/'launcher-stage.json').read_text())
        self.assertIsNone(receipt['actual_launcher_exit']);self.assertEqual(receipt['config_list_exit'],7)
        self.assertEqual(receipt['selected_final_exit'],7)
        self.assertEqual((safe/'config-list.stderr.log').read_text(),'ORIGINAL_LOAD_ERROR\n')
        for name in ('config-list.stdout.log','config-list.stderr.log'):
            self.assertEqual((safe/name).stat().st_mode & 0o777,0o600)

    def test_wrapper_list_success_still_runs_mandatory_launcher_and_owner_failure_keeps_captures(self):
        result,safe,marks = self.wrapper(owner=1)
        self.assertEqual(result.returncode,1);self.assertEqual(marks,['list','run','owner'])
        receipt = json.loads((safe/'launcher-stage.json').read_text())
        self.assertEqual(receipt['actual_launcher_exit'],0);self.assertEqual(receipt['config_list_exit'],0)
        self.assertTrue((safe/'config-list.stdout.log').is_file());self.assertTrue((safe/'config-list.stderr.log').is_file())

    def test_wrapper_occupied_capture_refuses_without_clobber_or_launcher(self):
        result,safe,marks = self.wrapper(occupied=True)
        self.assertNotEqual(result.returncode,0);self.assertEqual(marks,['owner'])
        self.assertEqual((safe/'config-list.stdout.log').read_text(),'OLD_CAPTURE')

    def test_wrapper_occupied_stderr_refuses_without_clobber_or_launcher(self):
        result,safe,marks = self.wrapper(occupied='stderr')
        self.assertNotEqual(result.returncode,0);self.assertEqual(marks,['owner'])
        self.assertEqual((safe/'config-list.stderr.log').read_text(),'OLD_CAPTURE')


if __name__=='__main__':unittest.main()
