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
    assert os.environ['GITHUB_ACTIONS'] == 'true' and os.environ['CI'] == 'true'
    assert m['source'] == os.environ['GITHUB_SHA']
    assert os.getuid() == os.getgid() == 1000
    assert os.environ.get('FVOCI_ROOT_RUN_OWNER') == OWNER
    grant_path = Path(os.environ['FVOCI_ROOT_CURRENT_ALLOCATION'])
    assert grant_path.is_absolute() and not grant_path.is_symlink()
    grant = json.loads(grant_path.read_text())
    assert grant['status'] == 'GRANTED' and grant['owner'] == OWNER
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
        assert binaries[engine]['features'] == ['worker']
        assert any(a['executable'] == engine and a['features'] == ['worker'] for a in artifacts)
    assets = referenced(m['webReceipt'])
    assert assets['source'] == m['source'] and assets['tree'] == m['tree']
    assert assets['exit_code'] == 0 and assets['full_inputs_unchanged'] and assets['dist_files']
    dist = W/'apps/web/dist'
    assert {str(p.relative_to(dist)):sha(p) for p in dist.rglob('*') if p.is_file()} == assets['dist_files']
    abi = referenced(m['abiReceipt'])
    assert abi['currentSource'] == m['source'] and abi['currentServerSha256'] == binaries[next(p for p in binaries if p.endswith('/fvoci-server'))]['sha256']
    assert abi['currentELFDependenciesVerified'] is True
    for p,h in abi['exact_copied_runtime_files'].items(): assert sha(p) == h
    if lane != 'install':
        closed = referenced(m['closedInstallReceipt'])
        assert closed['source'] == m['source'] and closed['tree'] == m['tree'] and closed['final_exit_code'] == 0
        assert closed['actual_tests'] == 4 and closed['actual_owned_process_receipts'] == 15 and closed['owned_container_absent']
        assert closed['actual_binary_inputs'] == binaries
        assert grant['backend'] == lane
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
                    'abiHashes':abi['exact_copied_runtime_files']}
        validate_allocation(restart, expected)
        browser = m['browserInputs']
        for key in ('bun','chromium'): assert sha(browser[key]['path']) == browser[key]['sha256']
        browser_root=Path(browser['chromium']['path']).parent
        assert {str(p.relative_to(browser_root)):sha(p) for p in browser_root.rglob('*') if p.is_file()} == browser['chromium_directory_files']
        assert restart['binding']['parentDriverSha256'] == sha(driver)
        assert restart['binding']['restartHelperSha256'] == sha(Path(driver).parent/'restart_checkpoint.py')
        assert restart['binding']['artifactHashes'] == {p:r['sha256'] for p,r in binaries.items()}
        assert restart['binding']['assetHashes'] == assets['dist_files']
        assert restart['binding']['abiHashes'] == abi['exact_copied_runtime_files']
        assert os.environ['FVOCI_ROOT_RESTART_GRANT'] == m['restartAllocation']['path']
    return dict(manifest=m, manifest_path=manifest_path, grant=grant, run=run,
                before=before, build=build, compile_receipt=compile_receipt, assets=assets, abi=abi)
