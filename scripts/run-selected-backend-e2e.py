#!/usr/bin/env python3
"""Fixed same-job CI companion for the three maintained selected drivers.
Record actual current build inputs/artifacts; then execute install, PG and SQLite
serially. No product fixture, SQL writer, protocol parser or build fallback here.
"""
import argparse
import hashlib
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


def identity():
    assert os.environ['CI']=='true' and os.environ['GITHUB_ACTIONS']=='true', 'allocated GitHub CI job only'
    assert call(['git','rev-parse','HEAD'])==os.environ['GITHUB_SHA']
    assert subprocess.run(['git','-c','safe.directory='+str(ROOT),'diff','--quiet','HEAD'],cwd=ROOT).returncode==0, 'current tracked source must equal the tested SHA'
    assert re.fullmatch('[0-9]+',os.environ['GITHUB_RUN_ID'])
    assert re.fullmatch('[0-9]+',os.environ['GITHUB_RUN_ATTEMPT'])
    assert ((os.environ['GITHUB_JOB']=='collaboration-flow' and os.environ.get('FVOCI_WEB_BUILD_PHASE') in (None,'consume')) or
            (os.environ['GITHUB_JOB']=='collaboration-build' and os.environ.get('FVOCI_WEB_BUILD_PHASE')=='prepare')), 'wrong current build/runtime phase'
    return 'github:'+':'.join(os.environ[k] for k in ('GITHUB_REPOSITORY','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','GITHUB_JOB'))


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


def admitted_browser(output):
    receipt = read(output/'runtime-browser-stage.json')
    assert receipt['source'] == os.environ['GITHUB_SHA']
    directory = output/'browser'
    assert receipt['cache'] == str(directory) and os.environ.get('PLAYWRIGHT_BROWSERS_PATH') == str(directory)
    assert directory.stat().st_uid == 1000 and directory.stat().st_mode & 0o777 == 0o700
    actual = {p.name: browser_inventory(p, (1000, 1000)) for p in directory.iterdir()}
    assert actual == receipt['files'], 'current private browser assets differ'
    assert {p.name:browser_inventory(p,(1000,1000),metadata=True) for p in directory.iterdir()} == receipt['metadata'], 'current private browser metadata differs'
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
    owner = identity()
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


def installation_failure_diagnostic(output, before, owner):
    """Installation observation only, independently of the all-lane close proof."""
    unknown = {'launcher_observed_driver_exit':None, 'origin':None}
    try:
        stage_path = output/'install-launcher-stage.json'
        if stage_path.is_symlink():
            return unknown
        stage = read(stage_path)
        required = {'schema', 'source', 'tree', 'owner', 'runId', 'runAttempt',
                    'runRoot', 'driver_sha256', 'allocation_sha256', 'binding_sha256', 'exit'}
        if type(stage) is not dict or set(stage) != required:
            return unknown
        driver = TEMPLATES/'current-install-driver.py'
        digest = sha(driver)
        allocation = output/'install-allocation.json'
        binding = output/'install-binding.json'
        if any(path.is_symlink() for path in (allocation, binding, driver, TEMPLATES/'current_binding.py', output/'runtime')):
            return unknown
        grant = read(allocation)
        manifest = read(binding)
        expected = {'source':before['head'], 'tree':before['tree'], 'owner':owner,
                    'runId':os.environ['GITHUB_RUN_ID'], 'runAttempt':os.environ['GITHUB_RUN_ATTEMPT']}
        if (type(stage['schema']) is not int or stage['schema'] != 1 or type(stage['exit']) is not int or not -255 <= stage['exit'] <= 255 or
                not all(stage[key] == value == grant.get(key) for key, value in expected.items()) or
                stage['allocation_sha256'] != sha(allocation) or stage['binding_sha256'] != sha(binding) or
                stage['driver_sha256'] != digest or grant.get('driverSha256') != digest or
                before.get('tracked', {}).get('scripts/selected-backend-ci/current-install-driver.py') != digest or
                before.get('tracked', {}).get('scripts/selected-backend-ci/current_binding.py') != sha(TEMPLATES/'current_binding.py') or
                type(grant.get('schema')) is not int or grant.get('schema') != 1 or grant.get('status') != 'GRANTED' or
                grant.get('exclusiveCIJob') is not True or grant.get('currentCIJobConfirmed') is not True or
                grant.get('lane') != 'install' or grant.get('backend') is not None or
                grant.get('compiledSource') != before['head'] or grant.get('bindingSha256') != sha(binding) or
                grant.get('bindingModuleSha256') != sha(TEMPLATES/'current_binding.py') or
                type(manifest.get('schema')) is not int or manifest.get('schema') != 1 or manifest.get('ready') is not True or
                manifest.get('source') != before['head'] or manifest.get('compiledSource') != before['head'] or
                manifest.get('tree') != before['tree']):
            return unknown
        root = Path(stage['runRoot'])
        if (type(stage['runRoot']) is not str or stage['runRoot'] != grant.get('runRoot') or
                not root.is_absolute() or root.parent != output/'runtime' or root.is_symlink() or
                not re.fullmatch(r'root-current-install-[0-9a-f]{12}', root.name)):
            return unknown
        observed = {'launcher_observed_driver_exit':stage['exit'], 'origin':None}
        origin_path = output/'install-failure-origin.json'
        if origin_path.is_symlink():
            return observed
        origin = read(origin_path)
        if (type(origin) is dict and set(origin) == {'schema', 'source', 'driver_sha256',
                'allocation_sha256', 'binding_sha256', 'phase', 'line', 'type'} and
                type(origin['schema']) is int and origin['schema'] == 1 and
                origin['source'] == before['head'] and origin['driver_sha256'] == digest and
                origin['allocation_sha256'] == stage['allocation_sha256'] and
                origin['binding_sha256'] == stage['binding_sha256'] and
                type(origin['phase']) is str and origin['phase'] in
                    ('before-binding', 'preparation', 'test-body', 'test-results') and
                type(origin['type']) is str and origin['type'] in
                    ('AssertionError', 'RuntimeError', 'PermissionError', 'OSError', 'TimeoutExpired', 'OtherError', 'ProcessExit') and
                type(origin['line']) is int and 1 <= origin['line'] <= len(driver.read_text().splitlines()) and
                stage['exit'] != 0 and (origin['type'] != 'ProcessExit' or origin['phase'] == 'test-body')):
            observed['origin'] = {key:origin[key] for key in ('driver_sha256', 'phase', 'line', 'type')}
        return observed
    except BaseException:
        # Preserve a qualified waited exit even when the optional origin is absent.
        return locals().get('observed', unknown)


def runtime_ownership_return(output):
    """A waited launcher alone does not prove its product resources retired."""
    diagnostic = {'schema':1, 'phase':'identity', 'source':None, 'tree':None,
                  'ownership_return_qualified':False, 'lanes':[]}
    try:
        owner = identity()
        diagnostic['phase'] = 'current-source'
        assert os.getuid() == os.getgid() == 1000 and os.environ['GITHUB_JOB'] == 'collaboration-flow'
        before = read(output/'before.json')
        assert before['head'] == os.environ['GITHUB_SHA']
        diagnostic.update(source=before['head'],
            tree=before['tree'] if re.fullmatch('[0-9a-f]{40}', str(before['tree'])) else None)
        diagnostic['installation_failure'] = installation_failure_diagnostic(output, before, owner)
        runtime = output/'runtime'
        allocated = list(output.glob('*-allocation.json')) + list(output.glob('*-binding.json'))
        no_start = not allocated and (not runtime.exists() or not any(runtime.iterdir()))
        if no_start:
            proof = {'no_runtime_started':True}
        else:
            diagnostic['phase'] = 'selected-receipt'
            result = read(output/'selected-ci-receipt.json')
            assert result['owner'] == owner and result['source'] == before['head'] and result['tree'] == before['tree']
            assert [r['lane'] for r in result['runs']] == ['install','postgres','sqlite']
            for run in result['runs']:
                diagnostic['phase'] = run['lane']
                root = Path(run['runRoot'])
                assert root.parent == runtime and root.name.startswith('root-current-'+run['lane']+'-')
                facts = read(root/'receipt.json')
                required = ['source', 'final_exit_code', 'owned_container_absent'] + (
                    ['actual_owned_process_receipts'] if run['lane'] == 'install' else
                    ['owned_loopback_port_closed', 'recorded_process_identities_retired', 'cleanup_errors'])
                expected = {'source':str, 'final_exit_code':int, 'owned_container_absent':bool,
                            'actual_owned_process_receipts':int, 'owned_loopback_port_closed':bool,
                            'recorded_process_identities_retired':bool, 'cleanup_errors':list}
                origin = facts.get('original_driver_failure_origin')
                driver = TEMPLATES / ('current-' + run['lane'] + '-driver.py')
                # Legacy receipts omit optional diagnostics and their driver binding.
                # A declared valid binding still requires the real template's hash.
                bound_driver = (run['lane'] in ('postgres', 'sqlite') and facts.get('source') == before['head'] and
                                type(facts.get('driver_sha256')) is str and re.fullmatch('[0-9a-f]{64}', facts['driver_sha256']) and
                                sha(driver) == facts.get('driver_sha256'))
                phases = ('preparation', 'copied-native-hashes', 'owned-network-mode', 'normal-runtime') if run['lane'] == 'sqlite' else (
                    'preparation', 'copied-native-hashes', 'normal-runtime', 'browser-run', 'durable-readback', 'server-restart')
                if not (bound_driver and type(origin) is dict and
                        set(origin) == {'phase', 'driver_sha256', 'line', 'type'} and
                        origin['phase'] in phases and
                        origin['type'] in ('AssertionError', 'RuntimeError', 'PermissionError', 'OSError', 'TimeoutExpired', 'OtherError') and
                        origin['driver_sha256'] == sha(driver) == facts.get('driver_sha256') and
                        type(origin['line']) is int and 1 <= origin['line'] <= len(driver.read_text().splitlines())):
                    origin = None
                network = facts.get('network_mode_observation')
                if not (bound_driver and run['lane'] == 'sqlite' and type(network) is dict and
                        set(network) == {'phase', 'driver_sha256', 'line', 'exit', 'stdout_mode', 'stdout_sha256', 'stderr_present', 'stderr_sha256'} and
                        network['phase'] == 'owned-network-mode' and network['driver_sha256'] == sha(driver) and
                        type(network['line']) is int and 1 <= network['line'] <= len(driver.read_text().splitlines()) and
                        type(network['exit']) is int and -255 <= network['exit'] <= 255 and
                        network['stdout_mode'] in ('host', 'bridge', 'none', 'default', 'UNKNOWN') and
                        type(network['stderr_present']) is bool and
                        all(type(network[key]) is str and re.fullmatch('[0-9a-f]{64}', network[key])
                            for key in ('stdout_sha256', 'stderr_sha256'))):
                    network = None
                browser = facts.get('original_browser_failure')
                if not (bound_driver and run['lane'] == 'postgres' and type(browser) is dict and
                        set(browser) == {'phase', 'driver_sha256', 'line', 'exit', 'log_sha256', 'report_sha256', 'assertion_location'} and
                        browser['phase'] == 'browser-run' and browser['driver_sha256'] == sha(driver) and
                        type(browser['line']) is int and 1 <= browser['line'] <= len(driver.read_text().splitlines()) and
                        type(browser['exit']) is int and -255 <= browser['exit'] <= 255 and browser['exit'] != 0 and
                        type(browser['log_sha256']) is str and re.fullmatch('[0-9a-f]{64}', browser['log_sha256']) and
                        (browser['report_sha256'] is None or type(browser['report_sha256']) is str and
                         re.fullmatch('[0-9a-f]{64}', browser['report_sha256']))):
                    browser = None
                if browser is not None:
                    location = browser['assertion_location']
                    spec = ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
                    if not (type(location) is dict and set(location) == {'spec_sha256', 'line'} and
                            location['spec_sha256'] == sha(spec) == before.get('tracked', {}).get(str(spec.relative_to(ROOT))) and
                            type(location['line']) is int and 1 <= location['line'] <= len(spec.read_text().splitlines())):
                        browser = {**browser, 'assertion_location': None}
                def qualified_browser_diagnostic(observed, expected_exit, restart=False):
                    observation_keys = {'schema', 'driver_sha256', 'line', 'exit', 'error_kind', 'source_location',
                                        'report_exists', 'report_state', 'report_sha256', 'log_sha256', 'input_observation'}
                    if restart:
                        observation_keys = observation_keys | {'restart_verdict'}
                    input_keys = {'explicit_cache_path_present', 'cache_owned_by_runtime', 'cache_matches_preflight',
                                  'chromium_file_exists', 'chromium_owned_by_runtime', 'bun_sha256', 'chromium_sha256'}
                    if not (bound_driver and type(observed) is dict and set(observed) == observation_keys and
                            type(observed['schema']) is int and observed['schema'] == 1 and
                            observed['driver_sha256'] == sha(driver) and
                            type(observed['line']) is int and 1 <= observed['line'] <= len(driver.read_text().splitlines()) and
                            type(observed['exit']) is int and -255 <= observed['exit'] <= 255 and (restart or observed['exit'] != 0) and
                            observed['exit'] == expected_exit and type(expected_exit) is int and
                            observed['error_kind'] in ('UNKNOWN', 'BUN_CI_CONFIG_GUARD', 'BROWSER_EXECUTABLE_MISSING',
                                                       'SELECTED_SPEC_FAILURE', 'SELECTED_SPEC_TIMEOUT') and
                            type(observed['report_exists']) is bool and
                            observed['report_state'] in ('MISSING', 'UNREADABLE', 'MALFORMED', 'AVAILABLE') and
                            observed['report_exists'] == (observed['report_state'] != 'MISSING') and
                            all(observed[key] is None or type(observed[key]) is str and re.fullmatch('[0-9a-f]{64}', observed[key])
                                for key in ('report_sha256', 'log_sha256')) and
                            type(observed['input_observation']) is dict and set(observed['input_observation']) == input_keys and
                            all(type(observed['input_observation'][key]) is bool for key in ('explicit_cache_path_present', 'chromium_file_exists')) and
                            all(observed['input_observation'][key] is None or type(observed['input_observation'][key]) is bool
                                for key in ('cache_owned_by_runtime', 'cache_matches_preflight', 'chromium_owned_by_runtime')) and
                            all(observed['input_observation'][key] is None or type(observed['input_observation'][key]) is str and
                                re.fullmatch('[0-9a-f]{64}', observed['input_observation'][key]) for key in ('bun_sha256', 'chromium_sha256'))):
                        observed = None
                    if observed is not None:
                        location = observed['source_location']
                        spec = ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
                        if not (type(location) is dict and set(location) == {'spec_sha256', 'line'} and
                                location['spec_sha256'] == sha(spec) == before.get('tracked', {}).get(str(spec.relative_to(ROOT))) and
                                type(location['line']) is int and 1 <= location['line'] <= len(spec.read_text().splitlines())):
                            observed = {**observed, 'source_location': None,
                                        'error_kind': 'UNKNOWN' if observed['error_kind'] in
                                            ('SELECTED_SPEC_FAILURE', 'SELECTED_SPEC_TIMEOUT') else observed['error_kind']}
                    if observed is not None and restart:
                        producer_lines = [index + 1 for index, text in enumerate(driver.read_text().splitlines())
                                          if "'line': sys._getframe().f_lineno, 'exit': code," in text]
                        if producer_lines != [observed['line']]:
                            return None
                        verdict = observed['restart_verdict']
                        spec = ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
                        title = 'selected normal main restart: fresh actor reads persisted native history and manual revision'
                        registration = [index + 1 for index, text in enumerate(spec.read_text().splitlines())
                                        if ('test("' + title + '"') in text]
                        status_count = {'passed': 'expected', 'failed': 'unexpected', 'timedOut': 'unexpected',
                                        'skipped': 'skipped', 'interrupted': 'unexpected'}
                        if not (type(verdict) is dict and
                                set(verdict) == {'registered_test', 'spec_sha256', 'registration_line', 'status', 'counts'} and
                                verdict['registered_test'] == 'SELECTED_NORMAL_RESTART' and
                                verdict['spec_sha256'] == sha(spec) == before.get('tracked', {}).get(str(spec.relative_to(ROOT))) and
                                len(registration) == 1 and type(verdict['registration_line']) is int and
                                verdict['registration_line'] == registration[0] and type(verdict['status']) is str and
                                verdict['status'] in status_count and type(verdict['counts']) is dict and
                                set(verdict['counts']) == {'expected', 'unexpected', 'flaky', 'skipped'} and
                                all(type(value) is int and 0 <= value <= 1 for value in verdict['counts'].values()) and
                                sum(verdict['counts'].values()) == 1 and verdict['counts'][status_count[verdict['status']]] == 1):
                            observed = {**observed, 'restart_verdict': None}
                    return observed
                observed = qualified_browser_diagnostic(facts.get('browser_failure_diagnostic'), facts.get('browser_exit'))
                restart = facts.get('restart_browser_diagnostic')
                helper = TEMPLATES / 'restart_checkpoint.py'
                if (bound_driver and type(restart) is dict and
                        set(restart) == {'helper_sha256', 'helper_line', 'observation'} and
                        helper.is_file() and not helper.is_symlink() and
                        restart['helper_sha256'] == sha(helper) == before.get('tracked', {}).get('scripts/selected-backend-ci/restart_checkpoint.py') and
                        type(restart['helper_line']) is int and
                        [index + 1 for index, text in enumerate(helper.read_text().splitlines())
                         if "'helper_line': g['sys']._getframe().f_lineno," in text] == [restart['helper_line']]):
                    restart_observed = qualified_browser_diagnostic(restart['observation'], facts.get('restart_browser_exit'), restart=True)
                    restart = {**restart, 'observation': restart_observed} if restart_observed is not None else None
                else:
                    restart = None
                secondary = {}
                for key, phase in (('native_evidence_preservation_origin', 'native-preservation'),
                                   ('post_input_failure_origin', 'post-input-check')):
                    record = facts.get(key)
                    secondary[key] = record if (bound_driver and run['lane'] == 'postgres' and type(record) is dict and
                        set(record) == {'phase', 'driver_sha256', 'line', 'type'} and record['phase'] == phase and
                        record['driver_sha256'] == sha(driver) and type(record['line']) is int and
                        1 <= record['line'] <= len(driver.read_text().splitlines()) and record['type'] in
                        ('AssertionError', 'RuntimeError', 'PermissionError', 'OSError', 'TimeoutExpired', 'OtherError')) else None
                # A whitelist only: no paths, URLs, failure messages or raw logs.
                # Missing evidence remains missing and still refuses ownership return.
                diagnostic['lanes'].append({'lane':run['lane'],
                    'launcher_observed_driver_exit':run['exit'] if type(run['exit']) is int else None,
                    'receipt_final_exit':facts.get('final_exit_code') if type(facts.get('final_exit_code')) is int else None,
                    'receipt_sha256':sha(root/'receipt.json'),
                    'original_driver_failure_origin':origin,
                    'network_mode_observation':network,
                    'original_browser_failure':browser,
                    'browser_failure_diagnostic':observed,
                    'restart_browser_diagnostic':restart,
                    'secondary_failure_origins':secondary,
                    'native_preservation_error_sha256':hashlib.sha256(json.dumps(facts['native_evidence_preservation_error'],sort_keys=True).encode()).hexdigest()
                        if 'native_evidence_preservation_error' in facts else None,
                    'exact_source_artifact_inputs_unchanged':facts.get('exact_source_artifact_inputs_unchanged')
                        if type(facts.get('exact_source_artifact_inputs_unchanged')) is bool else None,
                    'missing_required_fields':[key for key in required if key not in facts],
                    'invalid_required_fields':[key for key in required if key in facts and type(facts[key]) is not expected[key]],
                    'source_matches_current':facts.get('source') == before['head'],
                    'closure_facts':{key:facts[key] if type(facts.get(key)) is bool else None
                        for key in ('owned_container_absent','owned_loopback_port_closed','recorded_process_identities_retired')},
                    'cleanup_error_count':len(facts['cleanup_errors']) if type(facts.get('cleanup_errors')) is list else None,
                    'original_driver_failure_sha256':hashlib.sha256(json.dumps(facts['original_driver_failure'],sort_keys=True).encode()).hexdigest()
                        if 'original_driver_failure' in facts else None})
                assert all(key in facts and type(facts[key]) is expected[key] for key in required), 'missing or invalid current retirement proof'
                assert facts['source'] == before['head'] and facts['final_exit_code'] == run['exit']
                assert facts['owned_container_absent'] is True
                if run['lane'] == 'install':
                    assert facts['actual_owned_process_receipts'] == 15
                    processes = list((root/'retained-run').rglob('*process.json'))
                    assert len(processes) == 15 and all(read(p)['status'] is not None for p in processes)
                else:
                    assert facts['owned_loopback_port_closed'] is True and facts['recorded_process_identities_retired'] is True
                    assert facts['cleanup_errors'] == []
                    if run['lane'] == 'postgres':
                        assert read(root/'parent-receipt.json')['all_owned_fixtures_closed'] is True
            proof = {'closed_current_lanes':['install','postgres','sqlite'],'installation_process_receipts':15}
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
    identity();assert not (output/'before.json').exists()
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
    identity();before=read(output/'before.json');assert read(output/'build-env-inputs.json')==build_env(), 'compiler environment changed'
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
    identity();assert name in ('main','lib','install','engine') and command
    before=read(output/'before.json');assert call(['git','rev-parse','HEAD'])==before['head']
    start=time.monotonic()
    with (output/(name+'-compiler.jsonl')).open('x') as out,(output/(name+'-stderr.log')).open('x') as err:
        r=subprocess.run(command,cwd=ROOT,stdout=out,stderr=err)
    write(output/(name+'-stage.json'),{'source':before['head'],'tree':before['tree'],'command':command,'exit_code':r.returncode,'seconds':time.monotonic()-start})
    return r.returncode


def run(output):
    assert os.environ['GITHUB_JOB']=='collaboration-flow', 'build producer cannot start runtime'
    assert os.getuid()==os.getgid()==1000, 'normal SQLite browser/fixture/app file ownership must be1000:1000'
    owner=identity();before=read(output/'before.json');assert read(output/'after.json')==before
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
    for lane in ('install','postgres','sqlite'):
        runroot=runtime/('root-current-'+lane+'-'+secrets.token_hex(6));driver=TEMPLATES/('current-'+lane+'-driver.py')
        m={'schema':1,'ready':True,'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],
           'sourceInputsBefore':reference(output/'before.json'),'sourceInputsAfter':reference(output/'after.json'),
           'bundle':reference(output/'bundle.json'),'compileReceipt':reference(output/'compile-receipt.json'),
           'webReceipt':reference(output/'web-receipt.json'),'abiReceipt':reference(output/'abi-receipt.json'),
           'nativeQualification':None,'browserInputs':browser,'closedInstallReceipt':closed}
        if lane!='install':
            assert closed, 'mandatory actual current installation4 failed; cannot qualify normal main'
            binding={'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'backend':lane,'runRoot':str(runroot),
                     'parentDriverSha256':sha(driver),'restartHelperSha256':sha(TEMPLATES/'restart_checkpoint.py'),
                     'sourceInputsSha256':hashlib.sha256((json.dumps(source_written,indent=2)+'\n').encode()).hexdigest(),
                     'artifactHashes':{p:r['sha256'] for p,r in bundle['binaries'].items()},'assetHashes':web['dist_files'],'browserInputs':browser,'abiHashes':abi['host_runtime_files']}
            restart=output/(lane+'-restart-allocation.json');write(restart,{'schema':1,'status':'GRANTED','owner':owner,'exclusiveCIJob':True,'currentCIJobConfirmed':True,
                       'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'backend':lane,'binding':binding})
            m['restartAllocation']=reference(restart);env['FVOCI_ROOT_RESTART_GRANT']=str(restart)
        manifest=output/(lane+'-binding.json');write(manifest,m)
        allocation=output/(lane+'-allocation.json');write(allocation,{'schema':1,'status':'GRANTED','owner':owner,'exclusiveCIJob':True,'currentCIJobConfirmed':True,
                   'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'lane':lane,'backend':None if lane=='install' else lane,
                   'runRoot':str(runroot),'driverSha256':sha(driver),'bindingSha256':sha(manifest),'bindingModuleSha256':sha(TEMPLATES/'current_binding.py')})
        env.update(FVOCI_ROOT_CURRENT_BINDING=str(manifest),FVOCI_ROOT_CURRENT_ALLOCATION=str(allocation))
        if lane=='postgres':env['FVOCI_E2E_SELECTED_AUXILIARY']='normal-api'
        else:env.pop('FVOCI_E2E_SELECTED_AUXILIARY',None)
        with (output/(lane+'-driver.log')).open('x') as log:r=subprocess.run([sys.executable,str(driver)],env=env,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT)
        if lane == 'install':
            try:
                write(output/'install-launcher-stage.json', {'schema':1, 'source':before['head'], 'tree':before['tree'],
                    'owner':owner, 'runId':env['GITHUB_RUN_ID'], 'runAttempt':env['GITHUB_RUN_ATTEMPT'],
                    'runRoot':str(runroot), 'driver_sha256':sha(driver), 'allocation_sha256':sha(allocation),
                    'binding_sha256':sha(manifest), 'exit':r.returncode})
            except BaseException:
                pass  # Optional diagnostic must not replace the original fail-fast exit.
        results.append({'lane':lane,'exit':r.returncode,'actualSource':before['head'],'runRoot':str(runroot)})
        code=code or r.returncode
        if lane=='postgres':
            parent=read(runroot/'parent-receipt.json')
            assert parent['all_owned_fixtures_closed'], 'PG/Meili closure failed: preserve failure, no overlapping SQLite start'
            app=read(runroot/'receipt.json')
            assert app['owned_container_absent'] and app['owned_loopback_port_closed'] and app['recorded_process_identities_retired']
        if lane=='install':
            assert r.returncode==0,'current installation4 failed; preserve original log/15process receipts'
            closed=reference(runroot/'receipt.json')
        elif r.returncode==0:
            record=read(runroot/'receipt.json');assert record['current_schema_server_restart']['restartBrowserExit']==0
            assert record['actual_browser_tests']==1 and record['retries']==0
    write(output/'selected-ci-receipt.json',{'source':before['head'],'tree':before['tree'],'owner':owner,'runs':results,'exit':code,'normalBothAndRestartRequired':True,'sqliteAuxiliary':'BLOCKED: normal writers unported','whole060Complete':False})
    return code


def main():
    parser=argparse.ArgumentParser();parser.add_argument('mode',choices=['record-before','stage','record-after','run','permissions','owner-return']);parser.add_argument('--output',required=True);parser.add_argument('--stage-name');parser.add_argument('--sqlite-parent');parser.add_argument('--docker-gid',type=int);args,command=parser.parse_known_args()
    assert args.mode=='stage' or not command, 'unexpected arguments outside compiler stage'
    assert args.mode=='permissions' or (args.sqlite_parent is None and args.docker_gid is None), 'unexpected runtime permission arguments'
    output=Path(args.output).resolve();assert output.is_dir() and output.stat().st_uid==os.getuid() and output.stat().st_mode&0o777==0o700
    if args.mode=='record-before':record_before(output)
    elif args.mode=='record-after':record_after(output)
    elif args.mode=='stage':return stage(output,args.stage_name,command[1:] if command[:1]==['--'] else command)
    elif args.mode=='permissions':
        assert args.sqlite_parent and args.docker_gid is not None
        runtime_permissions(output, args.sqlite_parent, args.docker_gid)
    elif args.mode=='owner-return':runtime_ownership_return(output)
    else:return run(output)
    return 0


if __name__=='__main__':sys.exit(main())
