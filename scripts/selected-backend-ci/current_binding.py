"""Only artifact/allocation binding for the three maintained, literal drivers.
No build, lifecycle, DB, native or browser operations occur in this module.
Root supplies actual compiler/web/ABI receipts after an exclusive allocation.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys

OWNER = os.environ['FVOCI_CI_OWNER']
E = Path(os.environ['FVOCI_CI_SELECTED_RUNS'])
W = Path(__file__).resolve().parents[2]

# The feature list cargo actually emits for the engine stage: crates/collab-engine
# declares `default = []`, and a compiler-artifact lists every activated feature
# including a declared default. Must equal the record-after guard in
# scripts/run-selected-backend-e2e.py. Exact: a missing default or an extra
# feature (test-hang) is a different, non-current engine.
ENGINE_FEATURES = ['default', 'worker']


def engine_features_match(features):
    return sorted(features) == ENGINE_FEATURES


def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as file:
        for chunk in iter(lambda: file.read(1048576), b''):
            h.update(chunk)
    return h.hexdigest()


def referenced(record):
    path = Path(record['path'])
    assert path.is_absolute() and not path.is_symlink()
    assert sha(path) == record['sha256']
    return json.loads(path.read_text())


def validate_off_report(report, backend):
    spec = W / 'apps/web/e2e-pending/workspace-off-selected-backend.spec.ts'
    assert sha(spec) == 'bb702397a262b183fcb35ad76279bd2057e1e296f72c72c2c5f5812985148814'
    expected = re.findall(r'  test\("([^"\n]+)"', spec.read_text())
    assert len(expected) == len(set(expected)) == 7
    assert report['config']['workers'] == 1 and report['errors'] == []
    assert report['config']['metadata']['selectedBackend'] == backend
    assert report['config']['metadata']['selectedFlow'] == 'off'
    assert report['stats']['expected'] == 7
    assert all(report['stats'][k] == 0 for k in ('unexpected', 'flaky', 'skipped'))
    cases = []
    def visit(suites):
        for suite in suites:
            for case in suite.get('specs', []):
                assert Path(case['file']).name == spec.name and case['ok'] is True
                assert len(case['tests']) == 1
                test = case['tests'][0]
                assert test['expectedStatus'] == 'passed' and len(test['results']) == 1
                actual = test['results'][0]
                assert actual['status'] == 'passed' and actual['retry'] == 0 and actual['errors'] == []
                cases.append(case['title'])
            visit(suite.get('suites', []))
    visit(report['suites'])
    assert cases == expected, 'all seven original cases must pass once in declared order'
    return cases


def load_local_allocation(mode):
    # Explicit same-user procedural ROOT lease, not authentication or a sandbox.
    # CI never falls back to this path, even when a local lease is also supplied.
    assert os.environ.get('FVOCI_SELECTED_EXECUTION_MODE') == 'orca-local'
    assert not os.environ.get('CI') and not any(k.startswith('GITHUB_') for k in os.environ)
    if sys.flags.optimize:
        raise RuntimeError('root driver assertions require ordinary Python without optimization')
    path = Path(os.environ['FVOCI_SELECTED_LOCAL_ALLOCATION'])
    assert path.is_absolute() and not path.is_symlink()
    assert path.stat().st_uid == os.getuid() and path.stat().st_mode & 0o777 == 0o600
    assert sha(path) == os.environ['FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256']
    grant = json.loads(path.read_text())
    assert grant['schema'] == 1 and grant['status'] == 'GRANTED' and grant['executionMode'] == 'orca-local'
    assert grant['exclusiveLocalBatch'] is True and grant['currentDispatchConfirmed'] is True
    assert grant['owner'] == OWNER == os.environ['FVOCI_ROOT_RUN_OWNER']
    assert grant['runId'] == os.environ['FVOCI_LOCAL_RUN_ID'] and re.fullmatch('run_[0-9a-f]+', grant['runId'])
    assert grant['dispatchId'] == os.environ['FVOCI_LOCAL_DISPATCH_ID'] and re.fullmatch('ctx_[0-9a-f]+', grant['dispatchId'])
    assert grant['taskId'] == os.environ['FVOCI_LOCAL_TASK_ID'] and re.fullmatch('task_[0-9a-f]+', grant['taskId'])
    assert grant['workerTerminal'] == os.environ['ORCA_TERMINAL_HANDLE']
    assert grant['rootTerminal'] == os.environ['FVOCI_LOCAL_ROOT_TERMINAL']
    assert grant['workerTerminal'] != grant['rootTerminal']
    assert grant['worktree'] == str(W.resolve()) and grant['uid'] == os.getuid() == 1000
    assert grant['gid'] == os.getgid() == 1000
    assert re.fullmatch('[0-9a-f]{40}', grant['source']) and re.fullmatch('[0-9a-f]{40}', grant['tree'])
    assert subprocess.check_output(['git','-C',str(W),'rev-parse','HEAD'], text=True).strip() == grant['source']
    assert subprocess.check_output(['git','-C',str(W),'rev-parse','HEAD^{tree}'], text=True).strip() == grant['tree']
    assert subprocess.run(['git','-C',str(W),'diff','--quiet','HEAD']).returncode == 0
    assert mode in grant['allowedModes'] and set(grant['allowedModes']) <= {'record-before','stage','record-after','run'}
    from datetime import datetime, timezone
    assert datetime.now(timezone.utc) < datetime.fromisoformat(grant['expiresUtc'])
    assert Path(grant['outputRoot']).is_absolute() and Path(grant['outputRoot']) == E.parent
    assert Path(grant['outputRoot']) / 'runtime' == E
    for name in ('run-selected-backend-e2e.py','selected-backend-ci/current_binding.py','selected-backend-ci/restart_checkpoint.py',
                 'selected-backend-ci/current-install-driver.py','selected-backend-ci/current-postgres-driver.py','selected-backend-ci/current-sqlite-driver.py'):
        assert grant['registrationHashes'][name] == sha(W / 'scripts' / name)
    return grant


def load_current(lane, driver):
    # Reject absent/false allocation before hashing large inputs or any resource mutation.
    if sys.flags.optimize:
        raise RuntimeError('root driver assertions require ordinary Python without optimization')
    path = os.environ.get('FVOCI_ROOT_CURRENT_BINDING')
    assert path, 'NOT GRANTED: root must supply exact current compile and allocation binding'
    manifest_path = Path(path)
    assert manifest_path.is_absolute() and not manifest_path.is_symlink()
    m = json.loads(manifest_path.read_text())
    assert m['schema'] == 1 and m['ready'] is True, 'NOT GRANTED: preparation template is false'
    execution = os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci')
    assert execution in ('github-ci', 'orca-local')
    local = load_local_allocation('run') if execution == 'orca-local' else None
    if local is not None:
        assert m['source'] == local['source'] and m['tree'] == local['tree']
    else:
        assert os.environ['GITHUB_ACTIONS'] == 'true' and os.environ['CI'] == 'true'
        assert m['source'] == os.environ['GITHUB_SHA']
    assert os.getuid() == os.getgid() == 1000
    assert os.environ.get('FVOCI_ROOT_RUN_OWNER') == OWNER
    grant_path = Path(os.environ['FVOCI_ROOT_CURRENT_ALLOCATION'])
    assert grant_path.is_absolute() and not grant_path.is_symlink()
    grant = json.loads(grant_path.read_text())
    assert grant['status'] == 'GRANTED' and grant['owner'] == OWNER
    assert grant.get('executionMode', 'github-ci') == execution
    if local is not None:
        assert grant['executionMode'] == 'orca-local' and grant['localAuthorizationSha256'] == os.environ['FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256']
        assert grant['runId'] == local['runId'] and grant['runAttempt'] == local['dispatchId']
    else:
        assert grant['exclusiveCIJob'] is True and grant['currentCIJobConfirmed'] is True
        assert grant['runId'] == os.environ['GITHUB_RUN_ID']
        assert grant['runAttempt'] == os.environ['GITHUB_RUN_ATTEMPT']
        assert re.fullmatch('[0-9]+', grant['runId']) and re.fullmatch('[0-9]+', grant['runAttempt'])
    assert m['source'] == m['compiledSource'] == grant['source'] == grant['compiledSource']
    assert m['tree'] == grant['tree'] and grant['lane'] == lane
    assert re.fullmatch('[0-9a-f]{40}', m['source']) and re.fullmatch('[0-9a-f]{40}', m['tree'])
    assert m['source'] != 'd49f1842f612561c318b70bcc22f8227b5ed85a1', 'historical main cohort cannot qualify changed current Rust'
    assert subprocess.check_output(['git','-c','safe.directory='+str(W),'-C',str(W),'rev-parse','HEAD'], text=True).strip() == m['source']
    assert subprocess.check_output(['git','-c','safe.directory='+str(W),'-C',str(W),'rev-parse','HEAD^{tree}'], text=True).strip() == m['tree']
    assert grant['driverSha256'] == sha(driver) and grant['bindingSha256'] == sha(manifest_path)
    assert grant['bindingModuleSha256'] == sha(__file__)
    run = Path(grant['runRoot'])
    assert run.is_absolute() and run.parent == E and re.fullmatch('root-current-(install|postgres|sqlite)-[0-9a-f]{12}', run.name)
    assert run.name.startswith('root-current-' + lane + '-')
    assert shutil.disk_usage(E).free >= 20_000_000_000, 'root heavy start floor is20GB; source preparation grants no cleanup'
    before = referenced(m['sourceInputsBefore'])
    after = referenced(m['sourceInputsAfter'])
    assert before == after and before['head'] == m['source'] and before['tree'] == m['tree']
    assert before['tracked'] and before['external']
    assert before['tracked']['apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'] == '871ec11473d6ba4d0019475fcef48d41fff96991b364a85ca51635f262f0943d'
    flow = m.get('flow', 'on')
    assert flow in ('on', 'off') and grant.get('flow', 'on') == flow
    assert os.environ.get('FVOCI_E2E_SELECTED_FLOW', 'on') == flow
    if lane == 'install': assert flow == 'on'
    if flow == 'off':
        assert before['tracked']['apps/web/e2e-pending/workspace-off-selected-backend.spec.ts'] == 'bb702397a262b183fcb35ad76279bd2057e1e296f72c72c2c5f5812985148814'
    if lane == 'postgres':
        assert before['tracked']['apps/web/e2e-pending/workspace-wiki-selected-auxiliary.ts'] == 'c38b23f590e08f68f4a7abf64d71976e9b088f66631982e73e8f26aa8d606f57'
    assert subprocess.check_output(['git','-c','safe.directory='+str(W),'-C',str(W),'status','--short'], text=True) == before['status']
    tracked = subprocess.check_output(['git','-c','safe.directory='+str(W),'-C',str(W),'ls-files','-z']).decode().split('\0')[:-1]
    assert set(tracked) == set(before['tracked']), 'full current closure must include newly integrated helper/tests'
    for name, digest in before['tracked'].items(): assert sha(W/name) == digest
    for name, digest in before['external'].items(): assert sha(name) == digest
    for name, digest in before['untracked'].items(): assert sha(W/name) == digest
    build = referenced(m['bundle'])
    compile_receipt = referenced(m['compileReceipt'])
    assert build['source'] == compile_receipt['source'] == m['source']
    assert build['tree'] == compile_receipt['tree'] == m['tree']
    assert build['full_inputs_unchanged'] and compile_receipt['full_inputs_unchanged'] and compile_receipt['exit_code'] == 0
    artifacts = []
    for stage in compile_receipt['stages']:
        assert stage['exit_code'] == 0
        raw = Path(stage['compilerMessages']['path'])
        assert sha(raw) == stage['compilerMessages']['sha256']
        for line in raw.read_text().splitlines():
            if line.startswith('{'):
                record = json.loads(line)
                if record.get('reason') == 'compiler-artifact': artifacts.append(record)
    binaries = build['binaries']
    core = ('fvoci-server','fvoci-migrate','fvoci-e2e-fixture','selected_install_lifetime','fvoci_server')
    for name in core:
        matches = [(p,r) for p,r in binaries.items() if r['target']['name'] == name]
        assert len(matches) == 1, ('missing coherent current cohort target', name)
        p,r = matches[0]
        assert r['compiledSource'] == m['source'] and r['targetTriple'] == 'x86_64-unknown-linux-gnu'
        assert sorted(r['features']) == ['api-schema','db-tests']
        assert r['profile']['opt_level'] == '0' and r['profile']['debug_assertions'] is True
        assert r['profile']['test'] is (name in ('selected_install_lifetime','fvoci_server'))
        assert sha(p) == r['sha256']
        assert any(a['target'] == r['target'] and a['profile'] == r['profile'] and sorted(a['features']) == sorted(r['features'])
                   and p in a['filenames'] for a in artifacts), ('not emitted by current recorded Cargo command', p)
    engine = next(p for p,r in binaries.items() if r['target']['name'] == 'collab-engine')
    assert sha(engine) == binaries[engine]['sha256']
    if binaries[engine]['compiledSource'] != m['source']:
        q = referenced(m['nativeQualification'])
        assert q['currentSource'] == m['source'] and q['currentTree'] == m['tree']
        assert q['originalSource'] == binaries[engine]['compiledSource']
        assert q['executableSha256'] == binaries[engine]['sha256']
        assert all(q[k] is True for k in ('fullNativeInputsEqual','featuresEqual','toolchainEqual','abiEqual','actualExecutableHashChecked'))
        assert q['originalFeature'] == 'worker' and q['originalTarget'] == 'x86_64-unknown-linux-gnu'
        assert q['originalNativeInputs'] == q['currentNativeInputs'] and q['currentNativeInputs']
        assert all(before['tracked'][n] == h for n,h in q['currentNativeInputs'].items())
        assert q['originalToolchainInputs'] == q['currentToolchainInputs'] and q['currentToolchainInputs']
        assert all(before['external'][n] == h for n,h in q['currentToolchainInputs'].items())
        assert q['originalAbiInputs'] == q['currentAbiInputs'] and q['currentAbiInputs']
        assert all(sha(n) == h for n,h in q['currentAbiInputs'].items())
    else:
        assert engine_features_match(binaries[engine]['features']), ('engine features', binaries[engine]['features'], ENGINE_FEATURES)
        assert any(a['executable'] == engine and engine_features_match(a['features']) for a in artifacts)
    assets = referenced(m['webReceipt'])
    assert assets['source'] == m['source'] and assets['tree'] == m['tree']
    assert assets['exit_code'] == 0 and assets['full_inputs_unchanged'] and assets['dist_files']
    dist = W/'apps/web/dist'
    assert {str(p.relative_to(dist)):sha(p) for p in dist.rglob('*') if p.is_file()} == assets['dist_files']
    abi = referenced(m['abiReceipt'])
    assert abi['currentSource'] == m['source'] and abi['currentServerSha256'] == binaries[next(p for p in binaries if p.endswith('/fvoci-server'))]['sha256']
    assert abi['currentELFDependenciesVerified'] is True
    for p,h in abi['host_runtime_files'].items(): assert sha(p) == h
    if lane != 'install':
        closed = referenced(m['closedInstallReceipt'])
        assert closed['source'] == m['source'] and closed['tree'] == m['tree'] and closed['final_exit_code'] == 0
        assert closed['actual_tests'] == 4 and closed['actual_owned_process_receipts'] == 15 and closed['owned_container_absent']
        assert closed['actual_binary_inputs'] == binaries
        assert grant['backend'] == lane
        if flow == 'on':
            restart = referenced(m['restartAllocation'])
            from restart_checkpoint import validate_allocation
            source_written = {k:before[k] for k in ('head','tree','status','tracked','external','untracked')}
            expected = {'runId':grant['runId'], 'runAttempt':grant['runAttempt'],
                        'source':m['source'], 'tree':m['tree'], 'compiledSource':m['source'],
                        'backend':lane, 'runRoot':str(run), 'parentDriverSha256':sha(driver),
                        'restartHelperSha256':sha(Path(driver).parent/'restart_checkpoint.py'),
                        'sourceInputsSha256':hashlib.sha256((json.dumps(source_written,indent=2)+'\n').encode()).hexdigest(),
                        'artifactHashes':{p:r['sha256'] for p,r in binaries.items()},
                        'assetHashes':assets['dist_files'], 'browserInputs':m['browserInputs'],
                        'abiHashes':abi['host_runtime_files']}
            validate_allocation(restart, expected)
        browser = m['browserInputs']
        for key in ('bun','chromium'): assert sha(browser[key]['path']) == browser[key]['sha256']
        browser_root=Path(browser['chromium']['path']).parent
        assert {str(p.relative_to(browser_root)):sha(p) for p in browser_root.rglob('*') if p.is_file()} == browser['chromium_directory_files']
        if flow == 'on':
            assert restart['binding']['parentDriverSha256'] == sha(driver)
            assert restart['binding']['restartHelperSha256'] == sha(Path(driver).parent/'restart_checkpoint.py')
            assert restart['binding']['artifactHashes'] == {p:r['sha256'] for p,r in binaries.items()}
            assert restart['binding']['assetHashes'] == assets['dist_files']
            assert restart['binding']['abiHashes'] == abi['host_runtime_files']
            assert os.environ['FVOCI_ROOT_RESTART_GRANT'] == m['restartAllocation']['path']
    return dict(manifest=m, manifest_path=manifest_path, grant=grant, run=run,
                before=before, build=build, compile_receipt=compile_receipt, assets=assets, abi=abi)
