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
            {'failed_phase':'browser','original_driver_failure_type':None,'original_driver_failure_code':None,
             'known_browser_test':None,'known_browser_status':None,'known_browser_checkpoint':None})
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



class ActualDriverSourceControls(unittest.TestCase):
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


if __name__=='__main__':unittest.main()
