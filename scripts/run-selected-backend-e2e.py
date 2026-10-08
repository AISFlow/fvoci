#!/usr/bin/env python3
"""Fixed same-job CI companion for the three maintained selected drivers.
Record actual current build inputs/artifacts; then execute install, PG and SQLite
serially. No product fixture, SQL writer, protocol parser or build fallback here.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import stat
import shutil
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[1]
TEMPLATES=ROOT/'scripts/selected-backend-ci'


def sha(path):
    h=hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda:f.read(1048576),b''):h.update(chunk)
    return h.hexdigest()


def write(path,data):
    path=Path(path)
    with path.open('x') as f:
        os.fchmod(f.fileno(),0o600)
        f.write(json.dumps(data,indent=2)+'\n')


def read(path):return json.loads(Path(path).read_text())


def call(args):
    if args[0]=='git':args=['git','-c','safe.directory='+str(ROOT),*args[1:]]
    return subprocess.check_output(args,cwd=ROOT,text=True).strip()


def identity(mode="handoff", output=None):
    execution = os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci')
    assert execution in ('github-ci', 'orca-local')
    if execution == 'orca-local':
        sys.path.insert(0, str(TEMPLATES))
        from current_binding import load_local_allocation
        grant = load_local_allocation(mode)
        assert str(output) == grant['outputRoot']
        assert subprocess.run(['git','-c','safe.directory='+str(ROOT),'diff','--quiet','HEAD'],cwd=ROOT).returncode == 0
        return grant['owner']
    assert os.environ['CI']=='true' and os.environ['GITHUB_ACTIONS']=='true', 'allocated GitHub CI job only'
    assert call(['git','rev-parse','HEAD'])==os.environ['GITHUB_SHA']
    assert subprocess.run(['git','-c','safe.directory='+str(ROOT),'diff','--quiet','HEAD'],cwd=ROOT).returncode==0, 'current tracked source must equal the tested SHA'
    assert re.fullmatch('[0-9]+',os.environ['GITHUB_RUN_ID'])
    assert re.fullmatch('[0-9]+',os.environ['GITHUB_RUN_ATTEMPT'])
    if os.environ['GITHUB_JOB'] == 'collaboration-build':
        assert mode in ('handoff', 'record-before', 'stage', 'record-after')
        assert os.environ.get('FVOCI_WEB_BUILD_PHASE') == 'prepare', 'wrong producer phase'
    else:
        assert os.environ['GITHUB_JOB']=='collaboration-flow'
        assert os.environ.get('FVOCI_WEB_BUILD_PHASE') in (None, 'consume'), 'wrong runtime phase'
    part = os.environ.get('FVOCI_COLLAB_FLOW_PART', 'whole')
    assert part in ('whole', 'pending', 'restart'), 'unallocated collaboration part'
    return 'github:'+':'.join(os.environ[k] for k in ('GITHUB_REPOSITORY','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','GITHUB_JOB')) + (':'+part if os.environ['GITHUB_JOB'] == 'collaboration-flow' and part != 'whole' else '')


def inputs():
    # One fresh-host closure before/after compilation; no historical/local receipt reuse.
    tracked={p:sha(ROOT/p) for p in call(['git','ls-files','-z']).split('\0') if p}
    untracked={p:sha(ROOT/p) for p in call(['git','ls-files','--others','--exclude-standard','-z']).split('\0') if p and (ROOT/p).is_file()}
    external={}
    def add(path):
        path=Path(path)
        if path.is_dir():
            for f in path.rglob('*'):
                if f.is_file():external[str(f)]=sha(f)
        elif path.is_file():external[str(path)]=sha(path)
        else:raise RuntimeError('missing qualified build input: '+str(path))
    cargo_home=Path(os.environ.get('CARGO_HOME',str(Path.home()/'.cargo'))).resolve()
    for subdirectory in ('registry','git'):
        if (cargo_home/subdirectory).exists():add(cargo_home/subdirectory)
    for config in (cargo_home/'config',cargo_home/'config.toml',Path(os.environ.get('FVOCI_SELECTED_CI_OUTPUT','/nonexistent'))/'build-env-inputs.json'):
        if config.exists():add(config)
    assert not (cargo_home/'credentials').exists() and not (cargo_home/'credentials.toml').exists(), 'public offline CI cannot borrow account credentials'
    for directory in [ROOT/'node_modules',Path(call(['rustc','--print','sysroot'])),Path(os.environ['LIBCLANG_PATH']),Path('/usr/include'),Path(os.environ['SQLITE3_LIB_DIR']).parent]:add(directory)
    add(Path(call(['cc','-print-file-name=include'])).parent)
    for name in ('cargo','rustc','cc','ar','ld','bun'):
        tool=shutil.which(name);assert tool, name;add(Path(tool).resolve())
        result=subprocess.run(['ldd',str(Path(tool).resolve())],capture_output=True,text=True)
        for library in elf_dependencies(result.returncode,result.stdout,result.stderr):add(Path(library).resolve())
    add(Path('/etc/os-release'))
    for path in abi_files():add(path)
    return {'head':call(['git','rev-parse','HEAD']),'tree':call(['git','rev-parse','HEAD^{tree}']),
            'status':subprocess.check_output(['git','-c','safe.directory='+str(ROOT),'status','--short'],cwd=ROOT,text=True),
            'tracked':tracked,'external':external,'untracked':untracked}


def elf_dependencies(code,stdout,stderr):
    text=stdout+'\n'+stderr
    if code==0:
        return [str(Path(x).resolve()) for x in re.findall(r'(?:=>\s*)?(/[^\s]+)\s+\(',text)]
    if code==1 and ('not a dynamic executable' in text or 'statically linked' in text):return []
    raise RuntimeError('ldd prerequisite failed; exit='+str(code))


def build_env():
    names=[k for k in os.environ if re.match(r'^(CARGO_|RUST|CC(?:_|$)|CXX(?:_|$)|HOST_CC$|TARGET_CC$|CFLAGS|CPPFLAGS|CXXFLAGS|LDFLAGS|LIBCLANG|BINDGEN_|SQLITE3_|PKG_CONFIG|CPATH$|C_INCLUDE_PATH$|CPLUS_INCLUDE_PATH$|LIBRARY_PATH$|PATH$)',k)]
    assert not any(re.search(r'TOKEN|PASSWORD|SECRET|CREDENTIAL',k) for k in names), 'no compiler account credentials'
    assert not os.environ.get('RUSTC_WRAPPER') and not os.environ.get('RUSTC_WORKSPACE_WRAPPER'), 'unqualified compiler wrapper'
    return {k:hashlib.sha256(os.environ[k].encode()).hexdigest() for k in sorted(names)}


def browser_inventory(directory, owner=None, metadata=False):
    """The regular-file closure used by staging, admission and runtime hashing."""
    directory = Path(directory)
    assert not directory.is_symlink() and directory.is_dir(), 'invalid browser directory'
    files = {}
    for path in [directory, *sorted(directory.rglob('*'))]:
        facts = path.lstat()
        assert stat.S_ISREG(facts.st_mode) or stat.S_ISDIR(facts.st_mode), 'nonregular browser asset'
        if owner is not None:
            assert (facts.st_uid, facts.st_gid) == owner and owner[1] >= 1000, 'foreign or privileged browser asset'
        key = str(path.relative_to(directory))
        if stat.S_ISREG(facts.st_mode):
            digest = sha(path)
            files[key] = {'sha256':digest, 'mode':stat.S_IMODE(facts.st_mode)} if metadata else digest
        elif metadata:
            files[key] = {'directory':True, 'mode':stat.S_IMODE(facts.st_mode)}
    assert files, 'empty browser closure'
    return files


def prepare_browser(output, chromium):
    # Never repair a shared cache's ownership/modes. Copy only the installed
    # Chromium runtime components into this exclusively owned output prefix.
    runner = os.getuid()
    assert runner >= 1000, 'nonprivileged browser preparation owner required'
    chromium = Path(chromium)
    cache = chromium.parent.parent.parent
    assert re.fullmatch(r'chromium-[0-9]+', chromium.parent.parent.name)
    assert not cache.is_symlink() and (cache.stat().st_uid, cache.stat().st_gid) == (runner, os.getgid()) and os.getgid() >= 1000
    components = sorted(p for p in cache.iterdir()
                        if re.fullmatch(r'(chromium|chromium_headless_shell|ffmpeg)-[0-9]+', p.name))
    assert chromium.parent.parent in components
    before = {p.name: browser_inventory(p, (runner, os.getgid())) for p in components}
    source_metadata = {p.name: browser_inventory(p, (runner, os.getgid()), metadata=True) for p in components}
    destination = output/'browser'
    destination.mkdir(mode=0o700)  # exclusive; no reuse of a prior run
    for component in components:
        shutil.copytree(component, destination/component.name)
    # Restrict only positively created copies, preserving all bytes and execute
    # bits. The later transfer gives1000 ownership, including owner-only assets.
    for path in destination.rglob('*'):
        facts = path.lstat()
        assert facts.st_uid == runner and (stat.S_ISREG(facts.st_mode) or stat.S_ISDIR(facts.st_mode))
        path.chmod(0o700 if path.is_dir() or facts.st_mode & 0o111 else 0o600)
    assert {p.name: browser_inventory(p, (runner, os.getgid()), metadata=True) for p in components} == source_metadata, 'browser source changed during copy'
    assert {p.name: browser_inventory(destination/p.name) for p in components} == before, 'browser copy differs'
    write(output/'runtime-browser-stage.json', {'source':os.environ['GITHUB_SHA'],
          'cache':str(destination), 'chromium':str(destination/chromium.relative_to(cache)), 'files':before,
          'metadata':{p.name:browser_inventory(destination/p.name, (runner, os.getgid()), metadata=True) for p in components}})
    return destination/chromium.relative_to(cache)


def admitted_browser(output, expected_owner=(1000, 1000)):
    # Runtime callers keep1000:1000; pure fixtures supply their real file owner.
    receipt = read(output/'runtime-browser-stage.json')
    assert receipt['source'] == os.environ['GITHUB_SHA']
    directory = output/'browser'
    assert receipt['cache'] == str(directory) and os.environ.get('PLAYWRIGHT_BROWSERS_PATH') == str(directory)
    assert directory.stat().st_uid == expected_owner[0] and directory.stat().st_mode & 0o777 == 0o700
    actual = {p.name: browser_inventory(p, expected_owner) for p in directory.iterdir()}
    assert actual == receipt['files'], 'current private browser assets differ'
    assert {p.name:browser_inventory(p,expected_owner,metadata=True) for p in directory.iterdir()} == receipt['metadata'], 'current private browser metadata differs'
    chromium = Path(receipt['chromium'])
    assert chromium.is_relative_to(directory) and chromium.is_file()
    return str(chromium)


def runtime_access(files,browser):
    # Real mode/ownership/access checks occur as the actual1000 runtime process.
    assert os.getuid()==os.getgid()==1000
    for path in files:
        assert Path(path).is_file() and os.access(path,os.R_OK), ('input is inaccessible to runtime1000',path)
    chromium = Path(browser['chromium']['path'])
    assert browser_inventory(chromium.parent) == browser['chromium_directory_files'], 'current browser assets differ'
    for path in (browser['bun']['path'],browser['chromium']['path']):
        assert os.access(path,os.R_OK|os.X_OK), ('browser executable is inaccessible to runtime1000',path)


def runtime_permissions(output, sqlite_parent, docker_gid):
    """Qualify the existing runner read group before the owned runtime transfer."""
    assert os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci') == 'github-ci'
    owner = identity('run', output)
    assert os.environ['GITHUB_JOB'] == 'collaboration-flow'
    runner_uid, runner_gid = os.getuid(), os.getgid()
    temp = Path(os.environ['RUNNER_TEMP']).resolve()
    sqlite_parent = Path(sqlite_parent)
    assert output == temp/'fvoci-selected-current' and sqlite_parent == temp/'fvoci-sqlite'
    for prefix in (output, sqlite_parent):
        assert not prefix.is_symlink() and prefix.is_dir() and prefix.stat().st_uid == runner_uid
        for path in prefix.rglob('*'):
            assert not path.is_symlink() and path.stat().st_uid == runner_uid, 'foreign runtime transfer input'
    assert Path(os.environ['SQLITE3_LIB_DIR']).resolve().is_relative_to(sqlite_parent)
    before = read(output/'before.json')
    assert before['head'] == os.environ['GITHUB_SHA']
    assert read(output/'after.json') == before
    bun = str(Path(shutil.which('bun')).resolve())
    chromium = call([bun, '--eval', "console.log(require('@playwright/test').chromium.executablePath())"])
    # Hash and admit the complete consumed asset closure before ownership moves.
    chromium = str(prepare_browser(output, chromium))
    files = {str(ROOT/p): os.R_OK for p in before['tracked']}
    files.update({p: os.R_OK for p in before['external']})
    files.update({p: os.R_OK|os.X_OK for p in read(output/'bundle.json')['binaries']})
    files.update({str(Path(chromium).parent/p): os.R_OK for p in browser_inventory(Path(chromium).parent)})
    files.update({bun: os.R_OK|os.X_OK, chromium: os.R_OK|os.X_OK})
    # These two private prefixes will change owner; their outside ancestors will not.
    accessible = {p: mask for p, mask in files.items()
                  if not any(Path(p).is_relative_to(prefix) for prefix in (output, sqlite_parent))}
    accessible.update({str(output.parent): os.X_OK, str(sqlite_parent.parent): os.X_OK})
    groups = {docker_gid}
    needed = {}
    for name, mask in accessible.items():
        path = Path(name)
        for entry, required in [(path, mask), *((ancestor, os.X_OK) for ancestor in path.parents)]:
            facts = entry.stat()
            permitted = (facts.st_mode >> 6) if facts.st_uid == 1000 else (
                (facts.st_mode >> 3) if facts.st_gid in (1000, docker_gid) else facts.st_mode)
            if permitted & required != required and facts.st_gid == runner_gid and (facts.st_mode >> 3) & required == required:
                # Never inherit sudo/admin/root or the runner's full supplementary list.
                assert runner_gid >= 1000, 'privileged preparation group is not runtime authority'
                groups.add(runner_gid)
                needed[str(entry)] = {'path_sha256':hashlib.sha256(str(entry).encode()).hexdigest(),
                                      'uid':facts.st_uid,'gid':facts.st_gid,'mode':facts.st_mode & 0o777}
    groups = sorted(groups)
    check = """import hashlib,json,os,sys
request=json.load(sys.stdin)
assert os.getuid()==os.getgid()==1000 and sorted(os.getgroups())==request['groups']
missing=[p for p,m in request['files'].items() if not os.access(p,m)]
print(json.dumps({'uid':os.getuid(),'gid':os.getgid(),'groups':sorted(os.getgroups()),
 'checked':len(request['files']),'missing':len(missing),
 'missing_path_sha256':[hashlib.sha256(p.encode()).hexdigest() for p in missing[:16]]}))
sys.exit(bool(missing))
"""
    result = subprocess.run(['sudo','setpriv','--reuid=1000','--regid=1000',
                             '--groups='+','.join(map(str,groups)),sys.executable,'-c',check],
                            input=json.dumps({'groups':groups,'files':accessible}), capture_output=True, text=True)
    try: receipt = json.loads(result.stdout)
    except ValueError: receipt = {'invalid_receipt':True}
    write(output/'runtime-access-stage.json', {'source':before['head'],'tree':before['tree'],'owner':owner,
          'runner_uid':runner_uid,'runner_gid':runner_gid,'runtime_uid':1000,'runtime_gid':1000,
          'groups':groups,'required_group_paths':list(needed.values())[:16],
          'required_group_path_count':len(needed),'preflight_exit':result.returncode,'preflight':receipt})
    assert result.returncode == 0 and receipt.get('missing') == 0, 'runtime input access preflight failed'
    print(','.join(map(str,groups)))


# Same child phases lane_retirement already admits. Parent-only
# 'owned-fixture-wrapper' is not a child failed_phase.
PUBLIC_FAILURE_PHASES = ('container-prepare','server-startup','server-ready','browser','restart','install-body')
# failure_checkpoint writes ReturnedNonzero, or type(error).__name__ when the
# drivers raise AssertionError or RuntimeError by name. The code is the sibling
# receipt field failure_code, not a key inside original_driver_failure.
PUBLIC_FAILURE_TYPES = ('ReturnedNonzero','AssertionError','RuntimeError')
PUBLIC_FAILURE_CODES = ('SELECTED_DRIVER_EXCEPTION','SELECTED_BODY_NONZERO')
# Sibling of original_driver_failure. A checkpoint is only an allowlisted file:line.
KNOWN_ON_BROWSER_TEST = 'selected normal main: Vue setup, stable wiki create, native persist, manual revision and fresh actor readback'
KNOWN_BROWSER_STATUSES = ('failed', 'timedOut', 'interrupted')
KNOWN_BROWSER_CHECKPOINT = re.compile(r'^e2e-pending/workspace-wiki-selected-(?:backend\.spec|auxiliary)\.ts:[1-9][0-9]{0,4}$')
BROWSER_REPORT_STATES = ('matched', 'report-missing', 'report-unreadable', 'workers-not-one', 'spec-mismatch', 'status-not-known')


def public_failure_fields(facts):
    """Whitelisted phase, type, code, report state, and one allowlisted file:line. Never a message or path."""
    phase = facts.get('failed_phase')
    failure = facts.get('original_driver_failure')
    kind = failure.get('type') if isinstance(failure, dict) else None
    code = facts.get('failure_code')
    browser = phase == 'browser'
    state = facts.get('browser_report_state')
    if state not in BROWSER_REPORT_STATES:
        state = None
    # A later failed_phase must not hide a browser classification already on the receipt.
    show_browser = state is not None or browser
    test = facts.get('known_browser_test')
    status = facts.get('known_browser_status')
    checkpoint = facts.get('known_browser_checkpoint')
    published_checkpoint = None
    if show_browser and type(checkpoint) is str and KNOWN_BROWSER_CHECKPOINT.fullmatch(checkpoint):
        line = int(checkpoint.rsplit(':', 1)[1])
        if 1 <= line <= 10000:
            published_checkpoint = checkpoint
    matched = state == 'matched' or (state is None and browser and published_checkpoint is not None)
    driver_checkpoint = facts.get('known_driver_checkpoint')
    preparation_exit = facts.get('preparation_command_exit')
    preparation = phase == 'container-prepare' and kind in PUBLIC_FAILURE_TYPES and code in PUBLIC_FAILURE_CODES
    if (not preparation or type(driver_checkpoint) is not str or not re.fullmatch(
            r'scripts/selected-backend-ci/current-sqlite-driver\.py:[1-9][0-9]{0,3}', driver_checkpoint)):
        driver_checkpoint = None
    if driver_checkpoint is None or type(preparation_exit) is not int or not -255 <= preparation_exit <= 255:
        preparation_exit = None
    return {'failed_phase': phase if phase in PUBLIC_FAILURE_PHASES else None,
            'known_driver_checkpoint': driver_checkpoint, 'preparation_command_exit': preparation_exit,
            'original_driver_failure_type': kind if kind in PUBLIC_FAILURE_TYPES else None,
            'original_driver_failure_code': code if code in PUBLIC_FAILURE_CODES else None,
            'browser_report_state': state,
            'known_browser_test': test if show_browser and (matched or state is None) and test == KNOWN_ON_BROWSER_TEST else None,
            'known_browser_status': status if show_browser and (matched or state is None) and status in KNOWN_BROWSER_STATUSES else None,
            'known_browser_checkpoint': published_checkpoint if matched or state is None else None}


def runtime_ownership_return(output):
    """A waited launcher alone does not prove its product resources retired."""
    diagnostic = {'schema':1, 'phase':'identity', 'source':None, 'tree':None,
                  'ownership_return_qualified':False, 'lanes':[]}
    try:
        assert os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci') == 'github-ci'
        owner = identity('run', output)
        diagnostic['phase'] = 'current-source'
        assert os.getuid() == os.getgid() == 1000 and os.environ['GITHUB_JOB'] == 'collaboration-flow'
        before = read(output/'before.json')
        assert before['head'] == os.environ['GITHUB_SHA']
        diagnostic.update(source=before['head'],
            tree=before['tree'] if re.fullmatch('[0-9a-f]{40}', str(before['tree'])) else None)
        runtime = output/'runtime'
        allocated = list(output.glob('*-allocation.json')) + list(output.glob('*-binding.json'))
        no_start = not allocated and (not runtime.exists() or not any(runtime.iterdir()))
        if no_start:
            proof = {'no_runtime_started':True}
        else:
            diagnostic['phase'] = 'selected-receipt'
            result = read(output/'selected-ci-receipt.json')
            assert result['owner'] == owner and result['source'] == before['head'] and result['tree'] == before['tree']
            diagnostic['selected_exit']=result['exit'] if type(result.get('exit')) is int else None
            diagnostic['all_requested_runs_executed']=result.get('allRequestedRunsExecuted') is True
            assert len({r['runRoot'] for r in result['runs']}) == len(result['runs'])
            for run in result['runs']:
                diagnostic['phase'] = run['lane']
                root = Path(run['runRoot'])
                assert root.parent == runtime and root.name.startswith('root-current-'+run['lane']+'-')
                facts = read(root/'receipt.json')
                required = ['source', 'tree', 'root_owner', 'final_exit_code', 'owned_container_absent'] + (
                    ['actual_owned_process_receipts'] if run['lane'] == 'install' else
                    ['selected_flow', 'owned_loopback_port_closed', 'recorded_process_identities_retired', 'cleanup_errors'])
                expected = {'source':str, 'tree':str, 'root_owner':str, 'selected_flow':str, 'final_exit_code':int, 'owned_container_absent':bool,
                            'actual_owned_process_receipts':int, 'owned_loopback_port_closed':bool,
                            'recorded_process_identities_retired':bool, 'cleanup_errors':list}
                # A whitelist only: no paths, URLs, failure messages or raw logs.
                # Missing evidence remains missing and still refuses ownership return.
                diagnostic['lanes'].append({'lane':run['lane'], 'flow':run['flow'],
                    'launcher_observed_driver_exit':run['exit'] if type(run['exit']) is int else None,
                    'receipt_final_exit':facts.get('final_exit_code') if type(facts.get('final_exit_code')) is int else None,
                    'receipt_sha256':sha(root/'receipt.json'),
                    'missing_required_fields':[key for key in required if key not in facts],
                    'invalid_required_fields':[key for key in required if key in facts and type(facts[key]) is not expected[key]],
                    'source_matches_current':facts.get('source') == before['head'],
                    'closure_facts':{key:facts[key] if type(facts.get(key)) is bool else None
                        for key in ('owned_container_absent','owned_loopback_port_closed','recorded_process_identities_retired')},
                    'cleanup_error_count':len(facts['cleanup_errors']) if type(facts.get('cleanup_errors')) is list else None,
                    'original_driver_failure_sha256':hashlib.sha256(json.dumps(facts['original_driver_failure'],sort_keys=True).encode()).hexdigest()
                        if 'original_driver_failure' in facts else None,
                    **public_failure_fields(facts)})
                assert all(key in facts and type(facts[key]) is expected[key] for key in required), 'missing or invalid current retirement proof'
                assert run['actualSource'] == before['head']
                assert facts['source'] == before['head'] and facts['tree'] == before['tree'] and facts['root_owner'] == owner
                assert facts['final_exit_code'] == run['exit']
                assert facts['owned_container_absent'] is True
                if run['lane'] == 'install':
                    assert facts['actual_owned_process_receipts'] == 15
                    processes = list((root/'retained-run').rglob('*process.json'))
                    assert len(processes) == 15 and all(read(p)['status'] is not None for p in processes)
                else:
                    assert facts['selected_flow'] == run['flow']
                    assert facts['owned_loopback_port_closed'] is True and facts['recorded_process_identities_retired'] is True
                    assert facts['cleanup_errors'] == []
                    if run['lane'] == 'postgres':
                        parent = read(root/'parent-receipt.json')
                        assert parent['source'] == before['head'] and parent['tree'] == before['tree'] and parent['root_owner'] == owner
                        assert parent['selected_flow'] == run['flow'] and parent['all_owned_fixtures_closed'] is True
            assert [(r['lane'],r['flow']) for r in result['runs']] == list(selected_runs()), 'all mandatory lanes remain required'
            proof = {'closed_current_runs':[{'lane':lane,'flow':flow} for lane,flow in selected_runs()],
                     'installation_process_receipts':15}
        write(output/'runtime-close-stage.json', {'source':before['head'],'tree':before['tree'],
              'owner':owner,'ownership_return_qualified':True,**proof})

        diagnostic['ownership_return_qualified'] = True
    except Exception as error:
        diagnostic['proof_error_type'] = type(error).__name__
        raise
    finally:
        # The launcher captures this bounded summary into its own private prefix.
        # Product data stay owned by1000 when any closure assertion refuses.
        print(json.dumps(diagnostic), flush=True)


def abi_files():
    return [str(Path(p).resolve()) for p in ('/lib64/ld-linux-x86-64.so.2','/lib/x86_64-linux-gnu/libc.so.6','/lib/x86_64-linux-gnu/libm.so.6','/lib/x86_64-linux-gnu/libgcc_s.so.1')]


def reference(path):return {'path':str(Path(path).resolve()),'sha256':sha(path)}


def record_before(output):
    identity('record-before', output);assert not (output/'before.json').exists()
    os_release=dict(line.split('=',1) for line in Path('/etc/os-release').read_text().splitlines() if '=' in line)
    assert os_release['ID'].strip(chr(34))=='ubuntu' and os_release['VERSION_ID'].strip(chr(34))=='26.04'
    assert 'release: 1.98.1' in call(['rustc','-Vv']) and 'host: x86_64-unknown-linux-gnu' in call(['rustc','-Vv'])
    assert os.environ.get('CARGO_BUILD_TARGET','x86_64-unknown-linux-gnu')=='x86_64-unknown-linux-gnu'
    assert call(['bun','-v'])=='1.4.2'
    assert os.environ['SQLITE3_STATIC']==os.environ['SQLITE3_NO_PKG_CONFIG']=='1'
    assert shutil.disk_usage(output).free>=20_000_000_000
    write(output/'build-env-inputs.json',build_env())
    write(output/'before.json',inputs())
    write(output/'build-environment.json',{'sqlite':{k:os.environ[k] for k in ('SQLITE3_LIB_DIR','SQLITE3_INCLUDE_DIR','SQLITE3_STATIC','SQLITE3_NO_PKG_CONFIG')},
          'rustc':call(['rustc','-Vv']),'cargo':call(['cargo','-V']),'bun':call(['bun','-v']),
          'os_release':Path('/etc/os-release').read_text(),
          'target':str(Path(os.environ['CARGO_TARGET_DIR']).resolve()),'features':['api-schema','db-tests'],
          'nativeFeatures':['worker'],'profile':'debug','devDebug':os.environ.get('CARGO_PROFILE_DEV_DEBUG'),
          'testDebug':os.environ.get('CARGO_PROFILE_TEST_DEBUG'),'compilerBeforeRecorded':True})


def record_after(output):
    identity('record-after', output);before=read(output/'before.json');assert read(output/'build-env-inputs.json')==build_env(), 'compiler environment changed'
    after=inputs();write(output/'after.json',after)
    assert before==after, 'source/native/dependency/toolchain changed during current compile'
    stages=[];artifacts=[]
    for name in ('main','lib','install','engine'):
        log=output/(name+'-compiler.jsonl');stage=read(output/(name+'-stage.json'))
        assert stage['exit_code']==0 and stage['source']==before['head'] and stage['tree']==before['tree']
        stage['compilerMessages']=reference(log);stages.append(stage)
        for line in log.read_text().splitlines():
            r=json.loads(line)
            if r.get('reason')=='compiler-artifact':artifacts.append(r)
    bins={}
    for name in ('fvoci-server','fvoci-migrate','fvoci-e2e-fixture','fvoci_server','selected_install_lifetime','collab-engine'):
        matches=[a for a in artifacts if a['target']['name']==name and a['executable'] and
                 a['profile']['test']==(name in ('fvoci_server','selected_install_lifetime'))]
        assert matches,(name,'missing current emitted artifact')
        unique={a['executable'] for a in matches};assert len(unique)==1,(name,unique)
        a=matches[-1]
        assert all(m['target']==a['target'] and m['features']==a['features'] and m['profile']==a['profile'] for m in matches);path=a['executable'];metadata=Path(path).stat()
        # Cargo lists every activated feature, including a declared `default` (collab-engine declares `default = []`).
        expected=['default','worker'] if name=='collab-engine' else ['api-schema','db-tests']
        assert sorted(a['features'])==expected,(name,'actual emitted features',sorted(a['features']),'expected',expected)
        bins[path]={'sha256':sha(path),'bytes':metadata.st_size,'mode':oct(metadata.st_mode),'inode':metadata.st_ino,
                    'compiledSource':before['head'],'targetTriple':'x86_64-unknown-linux-gnu',
                    'target':a['target'],'features':a['features'],'profile':a['profile'],'cargo_fresh':a['fresh']}
    compile_receipt={'source':before['head'],'tree':before['tree'],'exit_code':0,'full_inputs_unchanged':True,'stages':stages}
    write(output/'compile-receipt.json',compile_receipt)
    write(output/'bundle.json',{'source':before['head'],'tree':before['tree'],'full_inputs_unchanged':True,'binaries':bins,'compiler_artifacts':artifacts})
    dist=ROOT/'apps/web/dist';assets={str(p.relative_to(dist)):sha(p) for p in dist.rglob('*') if p.is_file()};assert assets
    write(output/'web-receipt.json',{'source':before['head'],'tree':before['tree'],'exit_code':0,'full_inputs_unchanged':True,'dist_files':assets,'servedDist':str(dist),'scope':'maintained current build before current source/native snapshot; actual emitted assets'})
    libs={p:sha(p) for p in abi_files()};ldd={}
    for p in bins:
        text=call(['ldd',p]);ldd[p]=text
        actual=[str(Path(x).resolve()) for x in re.findall(r'(?:=>\s*)?(/[^\s]+)\s+\(',text)]
        assert actual and set(actual)<=set(libs), ('unqualified current ELF dependencies',p,text)
    server=next(p for p in bins if p.endswith('/fvoci-server'))
    write(output/'abi-receipt.json',{'currentSource':before['head'],'currentServerSha256':bins[server]['sha256'],
          'currentELFDependenciesVerified':True,'actualCurrentELFldd':ldd,'host_runtime_files':libs,
          'os_release':Path('/etc/os-release').read_text(),
          'scope':'current Ubuntu 26.04 build-host ABI evidence only; runtime uses its own image libraries'})


def stage(output,name,command):
    identity('stage', output);assert name in ('main','lib','install','engine') and command
    if os.environ.get('FVOCI_SELECTED_EXECUTION_MODE') == 'orca-local':
        from current_binding import load_local_allocation
        assert command == load_local_allocation('stage')['stageCommands'][name]
    before=read(output/'before.json');assert call(['git','rev-parse','HEAD'])==before['head']
    start=time.monotonic()
    with (output/(name+'-compiler.jsonl')).open('x') as out,(output/(name+'-stderr.log')).open('x') as err:
        r=subprocess.run(command,cwd=ROOT,stdout=out,stderr=err)
    write(output/(name+'-stage.json'),{'source':before['head'],'tree':before['tree'],'command':command,'exit_code':r.returncode,'seconds':time.monotonic()-start})
    return r.returncode


def selected_runs():
    # Mandatory serial companion, not an opt-in replacing the retained ON flows.
    return (('install','on'),('postgres','on'),('sqlite','on'),('postgres','off'),('sqlite','off'))


def lane_retirement(runroot, lane, flow, source, tree, owner, driver_exit):
    """Concrete current-driver receipt admission; never manufacture missing closure."""
    facts = {'qualified':False, 'receiptPresent':False, 'refusalCodes':[],
             'receiptSha256':None, 'originalFailureSha256':None, 'failedPhase':None}
    path = runroot/'receipt.json'
    try:
        facts['receiptSha256'] = sha(path)
        receipt = read(path)
        assert isinstance(receipt,dict)
        facts['receiptPresent'] = True
        original = receipt.get('original_driver_failure', receipt.get('driver_error'))
        if original is not None:
            facts['originalFailureSha256'] = hashlib.sha256(json.dumps(original,sort_keys=True).encode()).hexdigest()
        phase = receipt.get('failed_phase')
        if phase in PUBLIC_FAILURE_PHASES:
            facts['failedPhase'] = phase
        assert receipt['source'] == source and receipt['tree'] == tree and receipt['root_owner'] == owner
        assert type(receipt['final_exit_code']) is int and receipt['final_exit_code'] == driver_exit
        assert receipt['owned_container_absent'] is True
        if lane == 'install':
            assert receipt.get('actual_tests') == 4 and receipt.get('actual_owned_process_receipts') == 15
        else:
            assert receipt['selected_flow'] == flow and receipt.get('cleanup_errors') == []
            assert receipt.get('owned_loopback_port_closed') is True
            assert receipt.get('recorded_process_identities_retired') is True
            if lane == 'postgres':
                parent = read(runroot/'parent-receipt.json')
                assert parent['source'] == source and parent['tree'] == tree and parent['root_owner'] == owner
                assert parent['selected_flow'] == flow and parent['all_owned_fixtures_closed'] is True
            if driver_exit == 0:
                if flow == 'on': assert receipt['current_schema_server_restart']['restartBrowserExit'] == 0
                assert receipt['actual_browser_tests'] == (8 if flow == 'off' else 1) and receipt['retries'] == 0
        facts['qualified'] = True
    except (OSError, ValueError, KeyError, TypeError, AssertionError):
        facts['refusalCodes'].append('SELECTED_DRIVER_RETIREMENT_UNCONFIRMED')
    return facts


def config_list_inputs(output, browser_owner=(1000, 1000)):
    """Recheck the already consumed cohort as1000; never create a lane grant."""
    if sys.flags.optimize:
        raise RuntimeError('current input assertions require ordinary Python')
    assert os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci') == 'github-ci'
    assert os.environ.get('FVOCI_WEB_BUILD_PHASE') == 'consume', 'config list requires admitted consumer'
    assert os.getuid() == os.getgid() == 1000
    owner = identity('config-list', output)
    assert not (output/'runtime').exists() and not list(output.glob('*-allocation.json')) and not list(output.glob('*-binding.json')), 'config list must precede all selected allocations'
    consumed = read(output/'handoff-consumed.json')
    before = read(output/'before.json')
    assert consumed['source'] == before['head'] == os.environ['GITHUB_SHA']
    assert consumed['tree'] == before['tree'] == call(['git','rev-parse','HEAD^{tree}'])
    assert consumed['repository'] == os.environ['GITHUB_REPOSITORY']
    assert consumed['run'] == os.environ['GITHUB_RUN_ID'] and consumed['attempt'] == os.environ['GITHUB_RUN_ATTEMPT']
    assert consumed['full_current_physical_inputs_equal'] is True and consumed['fresh_dist_equal'] is True
    assert read(output/'after.json') == before
    assert call(['git','status','--short']) == before['status'].strip()
    assert set(call(['git','ls-files','-z']).split('\0')) - {''} == set(before['tracked'])
    for names, base in ((before['tracked'], ROOT), (before['external'], Path('/')), (before['untracked'], ROOT)):
        for name, digest in names.items():
            assert sha(base/name) == digest, 'current config-list input changed'
    # Reuse the handoff's exact destination catalog rather than invent a second
    # native/receipt schema. The producer admission already qualified all stages.
    sys.dont_write_bytecode = True
    spec = importlib.util.spec_from_file_location('config_list_handoff', TEMPLATES/'web-build-handoff.py')
    handoff = importlib.util.module_from_spec(spec);spec.loader.exec_module(handoff)
    bundle = read(output/'bundle.json')
    expected, _, _ = handoff.expected_files(bundle, output)
    assert set(consumed['received']) == {str(p) for p in expected}
    for name, recorded in consumed['received'].items():
        path = handoff.regular(name);facts = path.stat()
        assert sha(path) == recorded['sha256'] and facts.st_ino == recorded['inode']
        assert stat.S_IMODE(facts.st_mode) == recorded['mode'], 'consumed file mode changed'
    assert bundle['source'] == before['head'] and bundle['tree'] == before['tree']
    web = read(output/'web-receipt.json')
    assert web['source'] == before['head'] and web['tree'] == before['tree']
    assert web['dist_files'] == {str(p.relative_to(ROOT/'apps/web/dist')):sha(p)
                               for p in (ROOT/'apps/web/dist').rglob('*') if p.is_file()}
    abi = read(output/'abi-receipt.json')
    assert abi['currentSource'] == before['head']
    for name, digest in abi['host_runtime_files'].items():assert sha(name) == digest
    access = read(output/'runtime-access-stage.json')
    assert access['source'] == before['head'] and access['tree'] == before['tree'] and access['owner'] == owner
    assert access['runtime_uid'] == access['runtime_gid'] == 1000
    assert access['preflight_exit'] == 0 and access['preflight']['missing'] == 0
    assert sorted(os.getgroups()) == access['groups'], 'runtime read groups differ'
    bun = str(Path(shutil.which('bun')).resolve())
    assert read(output/'build-environment.json')['bun'] == '1.4.2'
    assert sha(bun) == before['external'][bun]
    chromium = admitted_browser(output, browser_owner)
    browser = {'bun':{'path':bun,'sha256':sha(bun)},'chromium':{'path':chromium,'sha256':sha(chromium)},
               'chromium_directory_files':browser_inventory(Path(chromium).parent)}
    runtime_access([*(str(ROOT/p) for p in before['tracked']), *before['external'], *bundle['binaries']], browser)
    for name in bundle['binaries']:assert os.access(name,os.R_OK|os.X_OK)
    modules = {}
    for name in ('@playwright/test', 'playwright', 'playwright-core'):
        path = ROOT/'node_modules'/name/'package.json'
        package = read(path)
        assert package['version'] == '1.63.0'
        if name == 'playwright':assert package['bin']['playwright'] == 'cli.js'
        modules[name] = sha(path)
    cli = ROOT/'node_modules/playwright/cli.js'
    assert not cli.is_symlink() and cli.is_file() and os.access(cli,os.R_OK)
    assert sha(cli) == before['external'][str(cli)], 'Playwright CLI must be in the admitted input closure'
    modules['playwright/cli.js'] = sha(cli)
    return before, browser, modules


def config_list(output, expected_uid=1000):
    """Credential-free load/list only. Original body errors remain private/MISSING."""
    # The CLI keeps UID1000; a pure fixture may supply its report creator's UID.
    before, browser, modules = config_list_inputs(output)
    directory = output/'config-list'
    directory.mkdir(mode=0o700)  # Occupied, symlink and retry destinations refuse.
    for name in ('tmp', 'bun-transpiler-cache'):(directory/name).mkdir(mode=0o700)
    env = {'PATH':os.environ['PATH'], 'LANG':os.environ.get('LANG','C.UTF-8'), 'CI':'true',
           'TMPDIR':str(directory/'tmp'), 'BUN_RUNTIME_TRANSPILER_CACHE_PATH':str(directory/'bun-transpiler-cache'),
           'PLAYWRIGHT_BROWSERS_PATH':os.environ['PLAYWRIGHT_BROWSERS_PATH'],
           'FVOCI_E2E_SELECTED_BACKEND':'postgres', 'FVOCI_E2E_SELECTED_FLOW':'on',
           'FVOCI_E2E_SELECTED_AUXILIARY':'normal-api',
           'FVOCI_E2E_SELECTED_SOURCE':before['head'], 'FVOCI_E2E_SELECTED_COMPILED_SOURCE':before['head'],
           'FVOCI_E2E_RESULT_DIR':str(directory), 'PLAYWRIGHT_JSON_OUTPUT_FILE':str(directory/'listing.json')}
    # Run the admitted official entrypoint directly; bunx --bun uses a node shim.
    args = [browser['bun']['path'],'--no-install',str(ROOT/'node_modules/playwright/cli.js'),'test',
            '--config','e2e-pending/collab-playwright.config.ts','--reporter=line,json',
            '--list','workspace-wiki-selected-backend.spec.ts']
    # Only qualified identity, metadata and env NAMES enter the safe capture.
    receipt = {'schema':1,'phase':'config-list','source':before['head'],'tree':before['tree'],
               'compiledSource':before['head'],'uid':os.getuid(),'gid':os.getgid(),'groups':sorted(os.getgroups()),
               'environment_names':sorted(env),'list_only':True,'actual_browser_tests':0,'actual_db_tests':0,
               'bun_version':'1.4.2','module_hashes':modules,
               'executables':{name:{'sha256':facts['sha256'], 'mode':stat.S_IMODE(Path(facts['path']).stat().st_mode),
                                   'uid':Path(facts['path']).stat().st_uid,'gid':Path(facts['path']).stat().st_gid}
                              for name,facts in browser.items() if name in ('bun','chromium')}}
    write(directory/'start.json',receipt)
    print(json.dumps(receipt),flush=True)
    mask = os.umask(0o077)
    try:
        # Inherit ONLY the runner-opened captures; no runtime browser log copy.
        result = subprocess.run(args,cwd=ROOT/'apps/web',env=env,stdin=subprocess.DEVNULL)
    finally:
        os.umask(mask)
    receipt['exit'] = result.returncode
    # Actual exit precedes report observations and post-admission checks.
    write(directory/'result.json',receipt)
    print(json.dumps(receipt),flush=True)
    if result.returncode:return result.returncode
    report = directory/'listing.json'
    assert not report.is_symlink() and report.is_file() and report.stat().st_uid == expected_uid
    os.chmod(report,0o600)
    listed = read(report)
    assert listed['config']['workers'] == 1 and listed['errors'] == []
    assert listed['config']['metadata'] == {'selectedBackend':'postgres','selectedFlow':'on'}
    assert listed['stats']['expected'] == listed['stats']['unexpected'] == listed['stats']['flaky'] == 0
    assert listed['stats']['skipped'] == 1  # list discovery, not an actual skipped body
    assert len(listed['suites']) == len(listed['suites'][0]['specs']) == 1
    case = listed['suites'][0]['specs'][0]
    assert case['title'] == KNOWN_ON_BROWSER_TEST and len(case['tests']) == 1 and case['tests'][0]['results'] == []
    config_list_inputs(output)
    receipt.update(json_report_sha256=sha(report),configuration_load_qualified=True)
    write(directory/'qualified.json',receipt)
    print(json.dumps(receipt),flush=True)
    return 0


def run(output):
    assert os.environ.get("GITHUB_JOB") != "collaboration-build", "build producer cannot start runtime"
    assert os.getuid()==os.getgid()==1000, 'normal SQLite browser/fixture/app file ownership must be1000:1000'
    owner=identity('run', output);before=read(output/'before.json');assert read(output/'after.json')==before
    runtime=output/'runtime';runtime.mkdir(mode=0o700)
    bun=str(Path(shutil.which('bun')).resolve());chromium=call([bun,'--eval',"console.log(require('@playwright/test').chromium.executablePath())"])
    if os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci') == 'github-ci':
        assert chromium == admitted_browser(output), 'Playwright did not resolve the admitted private browser'
    browser={'bun':{'path':bun,'sha256':sha(bun)},'chromium':{'path':chromium,'sha256':sha(chromium)},
             'chromium_directory_files':browser_inventory(Path(chromium).parent)}
    runtime_access(before['external'],browser)
    bundle=read(output/'bundle.json');web=read(output/'web-receipt.json');abi=read(output/'abi-receipt.json')
    for executable in bundle['binaries']:
        assert os.access(executable,os.R_OK|os.X_OK), ('current cohort executable inaccessible to1000',executable)
    source_written={k:before[k] for k in ('head','tree','status','tracked','external','untracked')}
    env=dict(os.environ);env.update(BUN_RUNTIME_TRANSPILER_CACHE_PATH=str(output/'bun-transpiler-cache'),FVOCI_CI_OWNER=owner,FVOCI_CI_BUN=bun,FVOCI_CI_SELECTED_RUNS=str(runtime),FVOCI_ROOT_RUN_OWNER=owner,PYTHONDONTWRITEBYTECODE='1')
    closed=None;results=[];code=0
    local = os.environ.get('FVOCI_SELECTED_EXECUTION_MODE') == 'orca-local'
    authority = ({'executionMode':'orca-local','localAuthorizationSha256':os.environ['FVOCI_SELECTED_LOCAL_ALLOCATION_SHA256'],
                  'runId':os.environ['FVOCI_LOCAL_RUN_ID'],'runAttempt':os.environ['FVOCI_LOCAL_DISPATCH_ID']} if local else
                 {'exclusiveCIJob':True,'currentCIJobConfirmed':True,'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT']})
    launcher_failure=None
    try:
        for lane, flow in selected_runs():
            runroot=runtime/('root-current-'+lane+'-'+secrets.token_hex(6));driver=TEMPLATES/('current-'+lane+'-driver.py')
            m={'schema':1,'ready':True,'flow':flow,'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],
               'sourceInputsBefore':reference(output/'before.json'),'sourceInputsAfter':reference(output/'after.json'),
               'bundle':reference(output/'bundle.json'),'compileReceipt':reference(output/'compile-receipt.json'),
               'webReceipt':reference(output/'web-receipt.json'),'abiReceipt':reference(output/'abi-receipt.json'),
               'nativeQualification':None,'browserInputs':browser,'closedInstallReceipt':closed}
            if lane!='install':
                assert closed, 'mandatory actual current installation4 failed; cannot qualify normal main'
            if lane!='install' and flow=='on':
                binding={'runId':authority['runId'],'runAttempt':authority['runAttempt'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'backend':lane,'runRoot':str(runroot),
                         'parentDriverSha256':sha(driver),'restartHelperSha256':sha(TEMPLATES/'restart_checkpoint.py'),
                         'sourceInputsSha256':hashlib.sha256((json.dumps(source_written,indent=2)+'\n').encode()).hexdigest(),
                         'artifactHashes':{p:r['sha256'] for p,r in bundle['binaries'].items()},'assetHashes':web['dist_files'],'browserInputs':browser,'abiHashes':abi['host_runtime_files']}
                restart=output/(lane+'-'+flow+'-restart-allocation.json');write(restart,{'schema':1,'status':'GRANTED','owner':owner,**authority,
                           'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'backend':lane,'binding':binding})
                m['restartAllocation']=reference(restart);env['FVOCI_ROOT_RESTART_GRANT']=str(restart)
            else:
                env.pop('FVOCI_ROOT_RESTART_GRANT', None)
            manifest=output/(lane+'-'+flow+'-binding.json');write(manifest,m)
            allocation=output/(lane+'-'+flow+'-allocation.json');write(allocation,{'schema':1,'status':'GRANTED','owner':owner,**authority,'flow':flow,
                       'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'lane':lane,'backend':None if lane=='install' else lane,
                       'runRoot':str(runroot),'driverSha256':sha(driver),'bindingSha256':sha(manifest),'bindingModuleSha256':sha(TEMPLATES/'current_binding.py')})
            env.update(FVOCI_ROOT_CURRENT_BINDING=str(manifest),FVOCI_ROOT_CURRENT_ALLOCATION=str(allocation))
            env['FVOCI_E2E_SELECTED_FLOW']=flow
            if lane=='postgres' and flow=='on':env['FVOCI_E2E_SELECTED_AUXILIARY']='normal-api'
            else:env.pop('FVOCI_E2E_SELECTED_AUXILIARY',None)
            # Public timing only: lane, flow, UTC time, elapsed seconds and exit.
            print(f"selected-driver lane={lane} flow={flow} started at={time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}",flush=True)
            started=time.monotonic()
            with (output/(lane+'-'+flow+'-driver.log')).open('x') as log:r=subprocess.run([sys.executable,str(driver)],env=env,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT)
            print(f"selected-driver lane={lane} flow={flow} finished at={time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())} elapsed_seconds={round(time.monotonic()-started)} exit={r.returncode}",flush=True)
            results.append({'lane':lane,'flow':flow,'exit':r.returncode,'actualSource':before['head'],'runRoot':str(runroot)})
            code=code or r.returncode
            retirement=lane_retirement(runroot,lane,flow,before['head'],before['tree'],owner,r.returncode)
            results[-1]['retirement']=retirement
            if not retirement['qualified']:code=code or 1
            # A failed driver or absent retirement proof stops the serial allocation.
            # Preserve its actual exit and raw private receipt/log; never start a lane
            # just to fill the expected count or turn a partial run into PASS.
            if r.returncode != 0 or not retirement['qualified']:break
            if lane=='install':closed=reference(runroot/'receipt.json')
    except BaseException as error:
        code=code or (130 if isinstance(error,KeyboardInterrupt) else 1)
        original=output/'selected-launcher-failure.private.json'
        packet={'type':type(error).__name__,'message':str(error)[:4096]}
        launcher_failure={'code':'SELECTED_LAUNCHER_FAILED',
            'originalOutcomeSha256':hashlib.sha256(json.dumps(packet,sort_keys=True).encode()).hexdigest(),
            'sha256':None,'receiptWrite':'not-attempted'}
        try:
            write(original,packet)
            launcher_failure.update(sha256=sha(original),receiptWrite='confirmed')
        except BaseException:
            launcher_failure['receiptWrite']='failed'
    finally:
        complete=[(r['lane'],r['flow']) for r in results] == list(selected_runs())
        if not complete:code=code or 1
        aggregate={'source':before['head'],'tree':before['tree'],'owner':owner,'runs':results,'exit':code,
              'allRequestedRunsExecuted':complete,'launcherFailure':launcher_failure,
              'normalBothAndRestartRequired':True,'offBothRequired':True,'offTestsPerBackend':8,
              'sqliteAuxiliary':'BLOCKED: normal writers unported','whole060Complete':False}
        try:
            write(output/'selected-ci-receipt.json',aggregate)
        except BaseException:
            code=code or 1
            try:
                print(json.dumps({'source':before['head'],'tree':before['tree'],'exit':code,
                    'launcherFailure':launcher_failure,'aggregateReceiptWrite':'failed'}),flush=True)
            except BaseException:
                pass
    return code


def main():
    parser=argparse.ArgumentParser();parser.add_argument('mode',choices=['record-before','stage','record-after','run','permissions','owner-return','config-list']);parser.add_argument('--output',required=True);parser.add_argument('--stage-name');parser.add_argument('--sqlite-parent');parser.add_argument('--docker-gid',type=int);args,command=parser.parse_known_args()
    assert args.mode=='stage' or not command, 'unexpected arguments outside compiler stage'
    assert args.mode=='permissions' or (args.sqlite_parent is None and args.docker_gid is None), 'unexpected runtime permission arguments'
    if args.mode=='config-list':
        assert Path(args.output).is_absolute() and Path(args.output).resolve()==Path(args.output), 'config list requires physical output'
    output=Path(args.output).resolve();assert output.is_dir() and output.stat().st_uid==os.getuid() and output.stat().st_mode&0o777==0o700
    if args.mode=='record-before':record_before(output)
    elif args.mode=='record-after':record_after(output)
    elif args.mode=='stage':return stage(output,args.stage_name,command[1:] if command[:1]==['--'] else command)
    elif args.mode=='permissions':
        assert args.sqlite_parent and args.docker_gid is not None
        runtime_permissions(output, args.sqlite_parent, args.docker_gid)
    elif args.mode=='owner-return':runtime_ownership_return(output)
    elif args.mode=='config-list':return config_list(output)
    else:return run(output)
    return 0


if __name__=='__main__':sys.exit(main())
