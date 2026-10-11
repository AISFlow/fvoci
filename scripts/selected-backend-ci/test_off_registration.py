"""Controls of the two Python files kept for turso-ui.py: the local lease loader in
current_binding.py and the CI guards of run-selected-backend-e2e.py. The selected
backend runs the Bun lane drivers; their controls are in
tools/selected-backend-ci/lane-controls.test.ts. This file goes with turso-ui.py.
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


def github_guard_expressions(source):
    return {ast.dump(node.test) for node in ast.walk(ast.parse(source)) if isinstance(node, ast.Assert)
            and any(token in ast.unparse(node.test) for token in ('GITHUB_', 'exclusiveCIJob', 'currentCIJobConfirmed'))}


def web_collaboration_lane_ids():
    # collaboration-flow, then the web.yml lane jobs in file order. The build producer is not a lane.
    lanes = re.findall(r'(?m)^  (collaboration-(?!build\b)[A-Za-z0-9-]+):$',
                       (ROOT / '.github/workflows/web.yml').read_text())
    return ('collaboration-flow', *lanes)


def original_github_guards(name, before):
    """One substitution: the runner's `== 'collaboration-flow'` stands for the six-id membership."""
    found = github_guard_expressions(before)
    equality = ast.dump(ast.parse("os.environ['GITHUB_JOB'] == 'collaboration-flow'", mode='eval').body)
    if name != 'scripts/run-selected-backend-e2e.py' or equality not in found:
        return found
    ids = web_collaboration_lane_ids()
    if len(ids) != 6 or len(set(ids)) != 6 or ids[0] != 'collaboration-flow' or 'collaboration-build' in ids:
        raise AssertionError('web.yml collaboration lane jobs')
    expression = "os.environ['GITHUB_JOB'] in (" + ', '.join(repr(job) for job in ids) + ')'
    widened = ast.dump(ast.parse(expression, mode='eval').body)
    return (found - {equality}) | {widened}


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
                      'source':'a'*40, 'tree':'b'*40, 'allowedModes':['fixture', 'freeze', 'record-before', 'current-build', 'actor', 'server'], 'expiresUtc':'2099-01-01T00:00:00+00:00',
                      'outputRoot':str(self.output), 'registrationHashes':{}}
        name = 'selected-backend-ci/turso-ui.py'
        self.grant['registrationHashes'][name] = hashlib.sha256((ROOT/'scripts'/name).read_bytes()).hexdigest()

    ACTOR = 1000  # the only runtime actor the product accepts; the real host uid running this file may differ

    def check(self, grant=None, env=None, mode='fixture', file_mode=0o600, file_owner=None, consumer='turso-ui'):
        """Model the lease file's owner as the simulated actor for the exact allocation path only.
        The real host uid is irrelevant to the product contract (owner == actor), so stat of that one
        path reports file_owner (default: the actor); mode, hash, symlink and every other check stay real.
        """
        self.path.write_text(json.dumps(self.grant if grant is None else grant));self.path.chmod(file_mode)
        current = dict(self.env if env is None else env)
        current['FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256'] = hashlib.sha256(self.path.read_bytes()).hexdigest()
        lease, owner = self.path.resolve(), self.ACTOR if file_owner is None else file_owner
        class LeasePath(type(Path())):
            def stat(self, *args, **kwargs):
                facts = super().stat(*args, **kwargs)
                if Path(self).resolve() != lease: return facts
                return os.stat_result((facts.st_mode, facts.st_ino, facts.st_dev, facts.st_nlink, owner, facts.st_gid,
                                       facts.st_size, facts.st_atime, facts.st_mtime, facts.st_ctime))
        with patch.dict(os.environ,current,clear=True), patch.object(os,'getuid',return_value=self.ACTOR), patch.object(os,'getgid',return_value=self.ACTOR):
            binding = module(HERE/'current_binding.py', 'binding_control')
            with patch.object(binding,'Path',LeasePath), patch.object(binding.subprocess,'check_output',side_effect=['a'*40+'\n','b'*40+'\n']), patch.object(binding.subprocess,'run',return_value=subprocess.CompletedProcess([],0)):
                return binding.load_local_allocation(mode, consumer=consumer)

    def refused_at(self, **kwargs):
        """The source line of the actual failing assertion, so a refusal is attributed to its own precondition."""
        try: self.check(**kwargs)
        except AssertionError as error:
            frame = error.__traceback__
            while frame.tb_next: frame = frame.tb_next
            return open(frame.tb_frame.f_code.co_filename).read().splitlines()[frame.tb_lineno-1].strip()
        self.fail('accepted')

    def test_valid_baseline_then_mismatched_owner_and_private_mode_are_refused_at_the_lease_guard(self):
        self.assertEqual(self.check(), self.grant)
        self.assertEqual(self.check(file_owner=self.ACTOR), self.grant)
        for kwargs in ({'file_owner':1001}, {'file_owner':0}, {'file_mode':0o640}, {'file_mode':0o644}, {'file_mode':0o700}):
            with self.subTest(**kwargs):
                self.assertIn('st_uid', self.refused_at(**kwargs))

    def test_explicit_local_root_grant_only(self):
        self.assertEqual(self.check(), self.grant)

    def test_stale_foreign_absent_and_unallocated_mode_fail_closed(self):
        self.assertEqual(self.check(), self.grant)  # valid baseline first: a negative must fail on its own field
        for field,value in [('source','c'*40),('tree','c'*40),('worktree','/foreign'),('dispatchId','ctx_other'),
                            ('owner','foreign'),('status','NOTRUN'),('exclusiveLocalBatch',False),('currentDispatchConfirmed',False),
                            ('expiresUtc','2000-01-01T00:00:00+00:00'),('uid',0),('workerTerminal','foreign')]:
            with self.subTest(field=field):
                grant=copy.deepcopy(self.grant);grant[field]=value
                with self.assertRaises((AssertionError,KeyError)):self.check(grant)
                self.assertNotIn('st_uid', self.refused_at(grant=grant), 'refused by the lease owner/mode guard instead of the mutated field')
        self.assertNotIn('st_uid', self.refused_at(mode='stage'))
        env=dict(self.env);del env['FVOCI_SELECTED_LOCAL_ALLOCATION']
        with self.assertRaises(KeyError):self.check(env=env)
        grant=copy.deepcopy(self.grant);grant['registrationHashes']['selected-backend-ci/turso-ui.py']='0'*64
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

    def test_turso_ui_consumer_registration_is_fixed_and_selected_backend_fails_closed(self):
        # The selected-backend branch hashes the removed Python drivers, so it admits no grant.
        legacy = copy.deepcopy(self.grant); legacy['allowedModes'] = ['run']
        legacy['registrationHashes'] = {name: hashlib.sha256((ROOT/'scripts'/name).read_bytes()).hexdigest()
                                        for name in ('run-selected-backend-e2e.py', 'selected-backend-ci/current_binding.py')}
        for name in ('restart_checkpoint.py', 'current-install-driver.py', 'current-postgres-driver.py', 'current-sqlite-driver.py'):
            legacy['registrationHashes']['selected-backend-ci/' + name] = '0'*64
        with self.assertRaises(FileNotFoundError) as refused:
            self.check(grant=legacy, mode='run', consumer='selected-backend')
        self.assertEqual(Path(refused.exception.filename).name, 'restart_checkpoint.py')
        self.assertEqual(self.check(), self.grant)
        grant = copy.deepcopy(self.grant)
        grant['allowedModes'] = ['fixture', 'freeze', 'record-before', 'current-build', 'actor', 'server']
        name = 'selected-backend-ci/turso-ui.py'
        grant['registrationHashes'] = {name: hashlib.sha256((ROOT/'scripts'/name).read_bytes()).hexdigest()}
        with self.assertRaises(AssertionError):
            self.check(grant=grant, mode='fixture', consumer='selected-backend')
        self.assertEqual(self.check(grant=grant, mode='fixture', consumer='turso-ui'), grant)
        with self.assertRaises(AssertionError):
            self.check(grant=grant, mode='run', consumer='turso-ui')
        with self.assertRaises(AssertionError):
            self.check(grant=grant, mode='fixture', consumer='other')
        for field, value in (('source', 'c'*40), ('expiresUtc', '2000-01-01T00:00:00+00:00'), ('owner', 'foreign')):
            mutated = copy.deepcopy(grant)
            mutated[field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                self.check(grant=mutated, mode='fixture', consumer='turso-ui')
        wrong = copy.deepcopy(grant)
        wrong['registrationHashes'][name] = '0'*64
        with self.assertRaises(AssertionError):
            self.check(grant=wrong, mode='fixture', consumer='turso-ui')
        enlarged = copy.deepcopy(grant)
        enlarged['registrationHashes']['selected-backend-ci/current_binding.py'] = 'ab'*32
        with self.assertRaises(AssertionError):
            self.check(grant=enlarged, mode='fixture', consumer='turso-ui')

    def test_original_github_guards_remain_in_shared_binding_and_runner(self):
        for name in ('scripts/selected-backend-ci/current_binding.py','scripts/run-selected-backend-e2e.py'):
            before=subprocess.check_output(['git','show','6c18289e0e97d25c4a3c6b208fa91a1a33af3029:'+name],cwd=ROOT,text=True)
            after=(ROOT/name).read_text()
            self.assertTrue(github_guard_expressions(before))
            self.assertLessEqual(original_github_guards(name, before), github_guard_expressions(after),
                                 'all original CI refusal expressions remain')

    def test_runner_job_widening_rejects_an_extra_id_or_getenv(self):
        name = 'scripts/run-selected-backend-e2e.py'
        before = subprocess.check_output(['git', 'show', '6c18289e0e97d25c4a3c6b208fa91a1a33af3029:' + name], cwd=ROOT, text=True)
        after = (ROOT / name).read_text()
        required = original_github_guards(name, before)
        extra = after.replace("'collaboration-sqlite-off')", "'collaboration-sqlite-off', 'not-a-lane')")
        self.assertEqual(extra.count("'not-a-lane'"), 3)
        with self.assertRaises(AssertionError):
            self.assertLessEqual(required, github_guard_expressions(extra), 'all original CI refusal expressions remain')
        switched = after.replace("os.environ['GITHUB_JOB']", "os.environ.get('GITHUB_JOB')")
        self.assertGreater(switched.count("os.environ.get('GITHUB_JOB')"), after.count("os.environ.get('GITHUB_JOB')"))
        with self.assertRaises(AssertionError):
            self.assertLessEqual(required, github_guard_expressions(switched), 'all original CI refusal expressions remain')


if __name__ == '__main__': unittest.main()
