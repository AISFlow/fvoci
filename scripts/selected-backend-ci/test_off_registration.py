"""Pure registration/control tests; no driver import executes lifecycle code.
Actual runtime and DB/backend acceptance remain separate ROOT allocations.
"""
import ast
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import unittest
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]


def module(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


def functions(path, names):
    tree = ast.parse(path.read_text())
    tree.body = [node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name in names]
    scope = {'W': ROOT, 'Path': Path, 're': re,
             'sha': lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()}
    exec(compile(tree, str(path), 'exec'), scope)
    return scope


class OnSpecPin(unittest.TestCase):
    spec = 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'

    def guard(self, digest):
        # Execute the actual admission assertion, without running its resource loader.
        tree = ast.parse((HERE / 'current_binding.py').read_text())
        loader = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'load_current')
        guards = [node for node in loader.body if isinstance(node, ast.Assert) and self.spec in ast.unparse(node.test)]
        self.assertEqual(len(guards), 1)
        exec(compile(ast.Module(body=guards, type_ignores=[]), 'current_binding.py', 'exec'),
             {'before': {'tracked': {self.spec: digest}}})

    def test_declared_on_pin_accepts_actual_fixed_git_spec(self):
        source = subprocess.check_output(['git', 'show', 'HEAD:' + self.spec], cwd=ROOT)
        self.assertEqual(source, (ROOT / self.spec).read_bytes())
        self.guard(hashlib.sha256(source).hexdigest())

    def test_actual_on_guard_refuses_stale_and_mutated_spec(self):
        stale = subprocess.check_output(['git', 'show', '086129e0f95a00cf93703ae117bd102fd9944a0e^:' + self.spec], cwd=ROOT)
        current = (ROOT / self.spec).read_bytes()
        self.assertNotEqual(stale, current)
        for source in (stale, current + b'\n// unapproved spec mutation\n'):
            with self.subTest(digest=hashlib.sha256(source).hexdigest()), self.assertRaises(AssertionError):
                self.guard(hashlib.sha256(source).hexdigest())


class OffReports(unittest.TestCase):
    def setUp(self):
        self.names = re.findall(r'  test\("([^"\n]+)"', (ROOT / 'apps/web/e2e-pending/workspace-off-selected-backend.spec.ts').read_text())
        self.report = {'config': {'workers': 1, 'metadata': {'selectedBackend': 'sqlite', 'selectedFlow': 'off'}},
                       'errors': [], 'stats': {'expected': 7, 'unexpected': 0, 'flaky': 0, 'skipped': 0},
                       'suites': [{'suites': [{'specs': [
                           {'title': name, 'file': 'workspace-off-selected-backend.spec.ts', 'ok': True,
                            'tests': [{'expectedStatus': 'passed', 'results': [
                                {'status': 'passed', 'retry': 0, 'errors': []}]}]} for name in self.names]}]}]}

    def validate(self, report, backend):
        return functions(HERE / 'current_binding.py', {'validate_off_report'})['validate_off_report'](report, backend)

    def test_exact_immutable_seven_both_backends(self):
        self.assertEqual(len(self.names), 7)
        for backend in ('sqlite', 'postgres'):
            self.report['config']['metadata']['selectedBackend'] = backend
            self.assertEqual(self.validate(self.report, backend), self.names)

    def test_incomplete_wrong_flow_retry_skip_failure_duplicate_and_foreign_spec_are_refused(self):
        for backend in ('sqlite', 'postgres'):
            self.report['config']['metadata']['selectedBackend'] = backend
            for defect in ('missing', 'duplicate', 'reordered', 'foreign-spec', 'wrong-flow', 'wrong-backend',
                           'retry', 'extra-attempt', 'skipped', 'failed', 'unexpected', 'flaky', 'errors', 'workers'):
                with self.subTest(backend=backend, defect=defect):
                    report = copy.deepcopy(self.report); cases = report['suites'][0]['suites'][0]['specs']
                    actual = cases[0]['tests'][0]['results'][0]
                    if defect == 'missing': cases.pop()
                    elif defect == 'duplicate': cases[-1] = copy.deepcopy(cases[0])
                    elif defect == 'reordered': cases.reverse()
                    elif defect == 'foreign-spec': cases[0]['file'] = 'workspace-wiki-selected-backend.spec.ts'
                    elif defect == 'wrong-flow': report['config']['metadata']['selectedFlow'] = 'on'
                    elif defect == 'wrong-backend': report['config']['metadata']['selectedBackend'] = 'foreign'
                    elif defect == 'retry': actual['retry'] = 1
                    elif defect == 'extra-attempt': cases[0]['tests'][0]['results'].append(copy.deepcopy(actual))
                    elif defect == 'skipped': actual['status'] = 'skipped'
                    elif defect == 'failed': actual['status'] = 'failed'
                    elif defect in ('unexpected', 'flaky'): report['stats'][defect] = 1
                    elif defect == 'errors': report['errors'] = [{'message': 'beforeAll failed'}]
                    elif defect == 'workers': report['config']['workers'] = 2
                    with self.assertRaises(AssertionError): self.validate(report, backend)

    def test_mandatory_order_retains_install_and_on_both_before_off_both(self):
        runner = module(ROOT / 'scripts/run-selected-backend-e2e.py', 'selected_runner')
        self.assertEqual(runner.selected_runs(), (('install','on'), ('postgres','on'), ('sqlite','on'), ('postgres','off'), ('sqlite','off')))


class CompanionExecutionPlan(unittest.TestCase):
    """Run the actual companion control function with only fixture receipts.
    No binary/browser/container/DB command is dispatched by these pure controls.
    """
    def run_plan(self, failed=None, cleanup_failed=None):
        runner = module(ROOT / 'scripts/run-selected-backend-e2e.py', 'plan_control')
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory); source = {'head':'a'*40,'tree':'b'*40,'status':'','tracked':{},'external':{},'untracked':{}}
            for name,value in [('before',source),('after',source),('bundle',{'binaries':{}}),('web-receipt',{'dist_files':{'asset':'hash'}}),('abi-receipt',{'host_runtime_files':{}})]:
                (output/(name+'.json')).write_text(json.dumps(value))
            seen=[]; bindings=[]
            def child(args,env,cwd,stdout,stderr):
                allocation=json.loads(Path(env['FVOCI_ROOT_CURRENT_ALLOCATION']).read_text())
                manifest=json.loads(Path(env['FVOCI_ROOT_CURRENT_BINDING']).read_text())
                lane,flow=allocation['lane'],manifest['flow'];key=(lane,flow);seen.append(key);bindings.append((allocation,manifest))
                run=Path(allocation['runRoot']);run.mkdir()
                receipt={'actual_browser_tests':7 if flow=='off' else 1,'retries':0,
                         'owned_container_absent':key!=cleanup_failed,'owned_loopback_port_closed':True,'recorded_process_identities_retired':True}
                if lane=='install':receipt.update(actual_tests=4,actual_owned_process_receipts=15)
                elif flow=='on':receipt['current_schema_server_restart']={'restartBrowserExit':0}
                (run/'receipt.json').write_text(json.dumps(receipt))
                if lane=='postgres':(run/'parent-receipt.json').write_text(json.dumps({'all_owned_fixtures_closed':True}))
                return subprocess.CompletedProcess(args,23 if key==failed else 0)
            environment={'GITHUB_RUN_ID':'123','GITHUB_RUN_ATTEMPT':'1'}
            with patch.dict(os.environ,environment,clear=True), patch.object(os,'getuid',return_value=1000),patch.object(os,'getgid',return_value=1000),patch.object(os,'access',return_value=True),patch.object(runner,'identity',return_value='pure-fixture-owner'),patch.object(runner,'runtime_access'),patch.object(runner,'call',return_value=str(output/'chromium')),patch.object(runner,'sha',return_value='0'*64),patch.object(runner.shutil,'which',return_value='/pure-fixture/bun'),patch.object(runner.subprocess,'run',side_effect=child):
                if cleanup_failed:
                    with self.assertRaises(AssertionError):runner.run(output)
                    return seen,bindings,None
                code=runner.run(output);receipt=json.loads((output/'selected-ci-receipt.json').read_text())
                return seen,bindings,(code,receipt)

    def test_actual_control_function_registers_five_serial_runs_with_both_offs_required(self):
        seen,bindings,result=self.run_plan()
        self.assertEqual(seen,[('install','on'),('postgres','on'),('sqlite','on'),('postgres','off'),('sqlite','off')])
        code,receipt=result;self.assertEqual(code,0)
        self.assertTrue(receipt['normalBothAndRestartRequired'] and receipt['offBothRequired'])
        self.assertEqual(receipt['offTestsPerBackend'],7)
        for allocation,manifest in bindings:
            self.assertEqual(allocation['flow'],manifest['flow'])
            self.assertEqual('restartAllocation' in manifest, allocation['lane']!='install' and allocation['flow']=='on')
            self.assertTrue(allocation['exclusiveCIJob'] and allocation['currentCIJobConfirmed'])

    def test_each_off_failure_remains_nonzero_without_omitting_other_backend(self):
        for key in [('postgres','off'),('sqlite','off')]:
            with self.subTest(key=key):
                seen,bindings,(code,receipt)=self.run_plan(failed=key)
                self.assertEqual(code,23);self.assertEqual(receipt['exit'],23)
                self.assertEqual(seen[-2:],[('postgres','off'),('sqlite','off')])

    def test_unclosed_sqlite_on_blocks_next_pg_off_allocation(self):
        seen,_,_=self.run_plan(cleanup_failed=('sqlite','on'))
        self.assertEqual(seen,[('install','on'),('postgres','on'),('sqlite','on')])


class LocalAllocation(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'allocation.json'; self.output = Path(self.tmp.name) / 'output'
        self.env = {'FVOCI_CI_OWNER':'orca:fixture-root', 'FVOCI_ROOT_RUN_OWNER':'orca:fixture-root',
                    'FVOCI_CI_SELECTED_RUNS':str(self.output/'runtime'), 'FVOCI_SELECTED_EXECUTION_MODE':'orca-local',
                    'FVOCI_SELECTED_LOCAL_ALLOCATION':str(self.path), 'FVOCI_LOCAL_RUN_ID':'run_ab12',
                    'FVOCI_LOCAL_DISPATCH_ID':'ctx_cd34', 'FVOCI_LOCAL_TASK_ID':'task_ef56',
                    'ORCA_TERMINAL_HANDLE':'term_worker_fixture', 'FVOCI_LOCAL_ROOT_TERMINAL':'term_root_fixture'}
        self.grant = {'schema':1, 'status':'GRANTED', 'executionMode':'orca-local', 'exclusiveLocalBatch':True,
                      'currentDispatchConfirmed':True, 'owner':self.env['FVOCI_CI_OWNER'], 'runId':'run_ab12',
                      'dispatchId':'ctx_cd34', 'taskId':'task_ef56', 'workerTerminal':self.env['ORCA_TERMINAL_HANDLE'],
                      'rootTerminal':self.env['FVOCI_LOCAL_ROOT_TERMINAL'], 'worktree':str(ROOT), 'uid':1000, 'gid':1000,
                      'source':'a'*40, 'tree':'b'*40, 'allowedModes':['run'], 'expiresUtc':'2099-01-01T00:00:00+00:00',
                      'outputRoot':str(self.output), 'registrationHashes':{}}
        for name in ('run-selected-backend-e2e.py','selected-backend-ci/current_binding.py','selected-backend-ci/restart_checkpoint.py',
                     'selected-backend-ci/current-install-driver.py','selected-backend-ci/current-postgres-driver.py','selected-backend-ci/current-sqlite-driver.py'):
            self.grant['registrationHashes'][name] = hashlib.sha256((ROOT/'scripts'/name).read_bytes()).hexdigest()

    def check(self, grant=None, env=None, mode='run'):
        self.path.write_text(json.dumps(self.grant if grant is None else grant));self.path.chmod(0o600)
        current = dict(self.env if env is None else env)
        current['FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256'] = hashlib.sha256(self.path.read_bytes()).hexdigest()
        with patch.dict(os.environ,current,clear=True), patch.object(os,'getuid',return_value=1000), patch.object(os,'getgid',return_value=1000):
            binding = module(HERE/'current_binding.py', 'binding_control')
            with patch.object(binding.subprocess,'check_output',side_effect=['a'*40+'\n','b'*40+'\n']), patch.object(binding.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
                return binding.load_local_allocation(mode)

    def test_explicit_local_root_grant_only(self):
        self.assertEqual(self.check(), self.grant)

    def test_stale_foreign_absent_and_unallocated_mode_fail_closed(self):
        for field,value in [('source','c'*40),('tree','c'*40),('worktree','/foreign'),('dispatchId','ctx_other'),
                            ('owner','foreign'),('status','NOTRUN'),('exclusiveLocalBatch',False),('currentDispatchConfirmed',False),
                            ('expiresUtc','2000-01-01T00:00:00+00:00'),('uid',0),('workerTerminal','foreign')]:
            with self.subTest(field=field):
                grant=copy.deepcopy(self.grant);grant[field]=value
                with self.assertRaises((AssertionError,KeyError)):self.check(grant)
        with self.assertRaises(AssertionError):self.check(mode='stage')
        env=dict(self.env);del env['FVOCI_SELECTED_LOCAL_ALLOCATION']
        with self.assertRaises(KeyError):self.check(env=env)
        grant=copy.deepcopy(self.grant);grant['registrationHashes']['selected-backend-ci/current_binding.py']='0'*64
        with self.assertRaises(AssertionError):self.check(grant)

    def test_ci_cannot_fall_back_to_a_valid_local_grant(self):
        for key,value in [('CI','true'),('GITHUB_ACTIONS','true'),('GITHUB_SHA','a'*40),('GITHUB_RUN_ID','123')]:
            env=dict(self.env);env[key]=value
            with self.subTest(key=key), self.assertRaises(AssertionError):self.check(env=env)
        env=dict(self.env);env['FVOCI_SELECTED_EXECUTION_MODE']='github-ci'
        with self.assertRaises(AssertionError):self.check(env=env)

    def test_runner_without_ci_does_not_silently_consume_a_local_grant(self):
        runner=module(ROOT/'scripts/run-selected-backend-e2e.py','default_ci_control')
        env=dict(self.env);del env['FVOCI_SELECTED_EXECUTION_MODE']
        with patch.dict(os.environ,env,clear=True),patch.object(runner,'call') as command:
            with self.assertRaises(KeyError):runner.identity('run',self.output)
            command.assert_not_called()

    def test_local_lease_file_hash_and_symlink_are_rejected_before_source_or_resources(self):
        self.path.write_text(json.dumps(self.grant));self.path.chmod(0o600)
        env=dict(self.env);env['FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256']='0'*64
        with patch.dict(os.environ,env,clear=True):
            binding=module(HERE/'current_binding.py','bad_file_control')
            with patch.object(binding.subprocess,'check_output') as command:
                with self.assertRaises(AssertionError):binding.load_local_allocation('run')
                command.assert_not_called()
            link=self.path.parent/'link.json';link.symlink_to(self.path)
            os.environ['FVOCI_SELECTED_LOCAL_ALLOCATION']=str(link)
            with self.assertRaises(AssertionError):binding.load_local_allocation('run')

    def test_original_github_guards_remain_in_shared_binding_and_runner(self):
        for name in ('scripts/selected-backend-ci/current_binding.py','scripts/selected-backend-ci/restart_checkpoint.py','scripts/run-selected-backend-e2e.py'):
            before=subprocess.check_output(['git','show','6c18289e0e97d25c4a3c6b208fa91a1a33af3029:'+name],cwd=ROOT,text=True)
            after=(ROOT/name).read_text()
            protected = lambda source: {ast.dump(n.test) for n in ast.walk(ast.parse(source)) if isinstance(n,ast.Assert)
                                        and any(token in ast.unparse(n.test) for token in ('GITHUB_','exclusiveCIJob','currentCIJobConfirmed'))}
            self.assertTrue(protected(before))
            self.assertLessEqual(protected(before),protected(after), 'all original CI refusal expressions remain')


if __name__ == '__main__': unittest.main()
