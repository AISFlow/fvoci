#!/usr/bin/env python3
"""One hosted current-primary UI consumer; credentials never enter browsers.

Local preparation is credential-free. Runtime admission belongs to the existing
guard and ROOT's exact current dataset binding. No reset/restore cleanup exists.
"""
import base64
import ctypes
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import secrets
import select
import shlex
import signal
import socket
import stat
import subprocess
import sys
import time
import threading
import uuid
from urllib.request import ProxyHandler, build_opener

W = Path(__file__).resolve().parents[2]
ON = 'workspace-wiki-selected-backend.spec.ts'
OFF = 'workspace-off-selected-backend.spec.ts'


class UiError(Exception):
    """Fixed codes only; no SDK, URL, token, actor password or raw trace."""


def require(condition, code):
    if not condition:
        raise UiError(code)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode()


def value_digest(value):
    return hashlib.sha256(canonical(value)).hexdigest()


def write(path, value):
    with Path(path).open('x') as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(json.dumps(value, ensure_ascii=False, indent=2) + '\n')


def private_read(path, cap=1024 * 1024):
    path = Path(path)
    info = path.lstat()
    require(path.is_absolute() and stat.S_ISREG(info.st_mode) and info.st_nlink == 1
            and info.st_uid == os.getuid() and stat.S_IMODE(info.st_mode) == 0o600
            and info.st_size <= cap, 'UI_PRIVATE_INPUT_REFUSED')
    return json.loads(path.read_text())


def root():
    path = Path(os.environ['RUNNER_TEMP']) / 'turso-ui'
    path.mkdir(mode=0o700, exist_ok=True)
    require(not path.is_symlink() and path.stat().st_uid == os.getuid()
            and stat.S_IMODE(path.stat().st_mode) == 0o700, 'UI_ROOT_REFUSED')
    return path


def call(args):
    return subprocess.check_output(args, cwd=W, text=True).strip()


def source_inputs():
    names = call(['git', 'ls-files', '-z']).split('\0')
    names = [name for name in names if name]
    require(call(['git', 'status', '--short']) == '', 'UI_DIRTY_SOURCE')
    return {'source': call(['git', 'rev-parse', 'HEAD']), 'tree': call(['git', 'rev-parse', 'HEAD^{tree}']),
            'files': {name: digest(W / name) for name in names}}


def hosted_identity():
    require(os.environ.get('GITHUB_ACTIONS') == 'true' and os.environ.get('CI') == 'true'
            and os.environ.get('GITHUB_JOB') == 'turso-ui', 'UI_HOSTED_ALLOCATION_REQUIRED')
    for name in ('GITHUB_RUN_ID', 'GITHUB_RUN_ATTEMPT'):
        require(re.fullmatch('[0-9]+', os.environ.get(name, '')), 'UI_HOSTED_ALLOCATION_REQUIRED')
    source = source_inputs()
    require(source['source'] == os.environ['GITHUB_SHA'], 'UI_SOURCE_MISMATCH')
    return source


def source_identity(mode):
    if execution_mode() == 'github-ci':
        return hosted_identity()
    require(execution_mode() == 'orca-local', 'UI_EXECUTION_MODE_REFUSED')
    grant = load_existing_local_lease(mode)
    base = source_inputs()
    require(base['source'] == grant['source'] and base['tree'] == grant['tree'], 'UI_LOCAL_SOURCE_REFUSED')
    return {'executionMode': 'orca-local', 'runId': grant['runId'], 'dispatchId': grant['dispatchId'],
            'source': base['source'], 'tree': base['tree'], 'files': base['files']}


def clean_env():
    return {key: os.environ[key] for key in ('PATH', 'LANG', 'LD_LIBRARY_PATH', 'SSL_CERT_FILE',
                'SSL_CERT_DIR', 'TZ', 'RUNNER_TEMP', 'PYTHONDONTWRITEBYTECODE',
                'PLAYWRIGHT_BROWSERS_PATH', 'BUN_RUNTIME_TRANSPILER_CACHE_PATH')
            if key in os.environ}


def physical_inputs():
    # Reuse the maintained selected-driver collector, including credentials
    # refusal, physical sysroot/registry/config/compiler/SQLite/LLVM and ABI.
    spec = importlib.util.spec_from_file_location('ui_build_inputs', W / 'scripts/run-selected-backend-e2e.py')
    collector = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(collector)
    return {'files': collector.inputs(), 'buildEnvironment': collector.build_env()}


def recheck_physical(record):
    require(record == physical_inputs(), 'UI_PHYSICAL_BUILD_INPUTS_CHANGED')


def freeze():
    before = private_read(root() / 'source-before.json', 4 * 1024 * 1024)
    require(before == source_identity('freeze'), 'UI_BUILD_INPUTS_CHANGED')
    physical = private_read(root() / 'physical-before.private.json', 64 * 1024 * 1024)
    recheck_physical(physical)
    artifacts = []
    for name in ('ui-compile.json', 'ui-engine-compile.json'):
        for line in (Path(os.environ['RUNNER_TEMP']) / name).read_text().splitlines():
            if line.startswith('{'):
                record = json.loads(line)
                if record.get('reason') == 'compiler-artifact' and record.get('executable'):
                    artifacts.append(record)
    binaries = {}
    for name in ('fvoci-server', 'fvoci-migrate', 'fvoci-e2e-fixture', 'collab-engine'):
        matches = [a for a in artifacts if a['target']['name'] == name and not a['profile']['test']]
        require(len(matches) == 1, 'UI_CURRENT_ARTIFACT_MISSING')
        a = matches[0]
        expected = ['default', 'worker'] if name == 'collab-engine' else ['api-schema', 'db-tests']
        require(sorted(a['features']) == expected and a['profile']['opt_level'] == '0'
                and a['profile']['debug_assertions'], 'UI_ARTIFACT_FEATURES_MISMATCH')
        p = Path(a['executable'])
        require(p.is_absolute() and not p.is_symlink() and p.read_bytes()[:4] == b'\x7fELF', 'UI_ARTIFACT_REFUSED')
        binaries[name] = {'path': str(p), 'sha256': digest(p), 'features': a['features'], 'profile': a['profile']}
    dist = W / 'apps/web/dist'
    assets = {str(p.relative_to(dist)): digest(p) for p in dist.rglob('*') if p.is_file()}
    require(bool(assets) and 'index.html' in assets, 'UI_FRESH_DIST_MISSING')
    abi = {}
    for artifact in binaries.values():
        output = call(['ldd', artifact['path']])
        require('not found' not in output, 'UI_RUNTIME_ABI_MISSING')
        for path in re.findall(r'(/[\w./+-]+)', output):
            p = Path(path).resolve()
            require(p.is_file(), 'UI_RUNTIME_ABI_MISSING')
            abi[str(p)] = digest(p)
    bun = call(['which', 'bun'])
    chromium = call([bun, '--eval', 'import {chromium} from "@playwright/test";console.log(chromium.executablePath())'])
    require(Path(chromium).is_file(), 'UI_PINNED_BROWSER_MISSING')
    browser_files = {str(p): digest(p) for p in Path(chromium).parent.rglob('*') if p.is_file()}
    manifest = {'schema': 1, 'sourceInputs': before, 'physicalInputs': {'path': str(root() / 'physical-before.private.json'),
                'sha256': digest(root() / 'physical-before.private.json')}, 'binaries': binaries, 'assets': assets, 'abi': abi,
                'bun': {'path': bun, 'sha256': digest(bun), 'version': call([bun, '--version'])},
                'chromium': chromium, 'browserFiles': browser_files,
                'sqliteInputs': {'path': str(Path(os.environ['RUNNER_TEMP']) / 'fvoci-sqlite/consumer-inputs.json'),
                                 'sha256': digest(Path(os.environ['RUNNER_TEMP']) / 'fvoci-sqlite/consumer-inputs.json')},
                'rustc': call(['rustc', '-vV'])}
    require(manifest['bun']['version'] == '1.4.2', 'UI_BUN_PIN_MISMATCH')
    write(root() / 'current-build.json', manifest)


def current_build():
    manifest = private_read(root() / 'current-build.json', 8 * 1024 * 1024)
    require(manifest['sourceInputs'] == source_identity('current-build'), 'UI_SOURCE_CHANGED')
    physical = manifest['physicalInputs']
    require(digest(physical['path']) == physical['sha256'], 'UI_PHYSICAL_RECEIPT_CHANGED')
    recheck_physical(private_read(physical['path'], 64 * 1024 * 1024))
    for r in manifest['binaries'].values():
        require(digest(r['path']) == r['sha256'], 'UI_BINARY_CHANGED')
    for p, sha in manifest['abi'].items():
        require(digest(p) == sha, 'UI_ABI_CHANGED')
    for p, sha in manifest['browserFiles'].items():
        require(digest(p) == sha, 'UI_BROWSER_CHANGED')
    require(digest(manifest['bun']['path']) == manifest['bun']['sha256'], 'UI_BUN_CHANGED')
    require(digest(manifest['sqliteInputs']['path']) == manifest['sqliteInputs']['sha256'], 'UI_SQLITE_INPUTS_CHANGED')
    dist = W / 'apps/web/dist'
    require({str(p.relative_to(dist)): digest(p) for p in dist.rglob('*') if p.is_file()} == manifest['assets'], 'UI_ASSETS_CHANGED')
    return manifest


QUALIFIED_CANONICAL_IMAGE = 'sha256:396a5f8e43e8de4b2e1567f2c8a8e841bf45037a4e4ff7cb76dc384951025f35'
QUALIFIED_SHELL = '/bin/sh'
DAEMON_CAPS = {'memory.max': '12884901888', 'cpu.max': 'max 100000',
               'pids.max': '128', 'memory.swap.max': '0'}
SECRET_ENV_KEYS = ('FVOCI_LIBSQL_URL', 'FVOCI_LIBSQL_AUTH_TOKEN')
NATIVE_CAPSULE_KEYS = SECRET_ENV_KEYS + (
    'PASSWORD_PEPPER_KEYS', 'PASSWORD_PEPPER_ACTIVE_KEY_ID', 'FVOCI_E2E_TURSO_NAMESPACE',
    'FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE', 'FVOCI_TEST_TURSO_DESTRUCTIVE', 'E2E_DATABASE_BACKEND',
    'FVOCI_E2E_TURSO_UI_SELECTED', 'FVOCI_DATABASE_BACKEND', 'FVOCI_REALTIME_MODE', 'FVOCI_BIND',
    'FVOCI_PUBLIC_ORIGIN', 'FVOCI_COOKIE_SECURE', 'STORAGE_DRIVER', 'FVOCI_STORAGE_DIR',
    'FVOCI_STATIC_DIR', 'FVOCI_COLLAB_ENGINE', 'FVOCI_COLLAB_FAMILY_LEASE_MS',
    'FVOCI_COLLAB_FAMILY_RENEW_MS', 'FVOCI_COLLAB_MAX_ROOMS', 'RUST_LOG',
    'FVOCI_MAINTENANCE_TICK_SECS', 'FVOCI_MAINTENANCE_INTERVAL_SECS',
    'FVOCI_UPLOAD_GC_INTERVAL_SECS', 'FVOCI_REVISION_SWEEP_INTERVAL_SECS')
FIXED_LAUNCHER = (
    '#!/bin/sh\n'
    'set -eu\n'
    'set -a\n'
    '. "$1"\n'
    'set +a\n'
    'shift\n'
    "newline='\n'\n"
    'read -r fvoci_stat < /proc/$$/stat || exit 78\n'
    'exec 3>/fvoci-private/stat.ready\n'
    '[ "${#fvoci_stat}" -le 511 ] && case $fvoci_stat in *"$newline"*) false ;; *) true ;; esac || exit 78\n'
    "printf '%s\\n' \"$fvoci_stat\" >&3\n"
    'exec 3>&-\n'
    'exec 3</fvoci-private/exec.go\n'
    'read -r fvoci_go <&3 || exit 78\n'
    'exec 3<&-\n'
    '[ "$fvoci_go" = GO ] || exit 78\n'
    'exec "$@"\n')
LAUNCHER_DST = '/fvoci-current/launcher.sh'
CAPSULE_DST = '/fvoci-private/native-env.sh'
BINARY_DST = '/fvoci-current/bin'
DIST_DST = '/fvoci-current/dist'
ONE_SHOT_LIVE = 'unsupported-before-execution'
BLOCKED = 'BLOCKED'
FIXTURE_BUDGET = 120
SERVER_BUDGET = 10
STAT_READ_BOUND = 512
STAT_BODY_MAX = 511
STAT_FIFO_NAME = 'stat.ready'
GO_FIFO_NAME = 'exec.go'
STAT_FIFO_DST = '/fvoci-private/stat.ready'
GO_FIFO_DST = '/fvoci-private/exec.go'
GO_MARKER = b'GO\n'


def execution_mode():
    mode = os.environ.get('FVOCI_SELECTED_EXECUTION_MODE', 'github-ci')
    require(mode in ('github-ci', 'orca-local'), 'UI_EXECUTION_MODE_REFUSED')
    return mode


def load_existing_local_lease(mode):
    # current_binding.load_local_allocation keeps GRANTED, currentDispatchConfirmed,
    # exclusiveLocalBatch, expiresUtc, clean git, and source/tree. Do not re-code them.
    spec = importlib.util.spec_from_file_location(
        'ui_current_binding', W / 'scripts/selected-backend-ci/current_binding.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    try:
        return module.load_local_allocation(mode, consumer='turso-ui')
    except AssertionError:
        raise UiError('UI_LOCAL_ALLOCATION_REFUSED')


def read_shell_proof(runtime):
    proof = runtime.get('shellProof') if isinstance(runtime, dict) else None
    require(isinstance(proof, dict), 'UI_CANONICAL_SHELL_UNAVAILABLE')
    raw_path, raw_hash = proof.get('path'), proof.get('sha256')
    require(isinstance(raw_path, str) and raw_path.startswith('/') and not raw_path.startswith('//'),
            'UI_CANONICAL_SHELL_UNAVAILABLE')
    path = Path(raw_path)
    require(path.is_absolute() and not path.is_symlink(), 'UI_CANONICAL_SHELL_UNAVAILABLE')
    require(re.fullmatch('[0-9a-f]{64}', raw_hash or ''), 'UI_CANONICAL_SHELL_UNAVAILABLE')
    require(digest(path) == raw_hash, 'UI_CANONICAL_SHELL_UNAVAILABLE')
    receipt = json.loads(path.read_text())
    config = receipt['Config']
    require(config.get('Image') == QUALIFIED_CANONICAL_IMAGE == runtime.get('imageId'),
            'UI_CANONICAL_SHELL_UNAVAILABLE')
    require(QUALIFIED_SHELL in (config.get('Cmd') or []), 'UI_CANONICAL_SHELL_UNAVAILABLE')
    require(not any(str(item).startswith('FVOCI_LIBSQL_') for item in (config.get('Env') or [])),
            'UI_DOCKER_ENV_SECRET_REFUSED')
    return QUALIFIED_SHELL


def open_referenced_grant(mode):
    require(execution_mode() == 'orca-local', 'UI_EXECUTION_MODE_REFUSED')
    grant = load_existing_local_lease(mode)
    runtime = grant.get('canonicalRuntime')
    require(isinstance(runtime, dict) and runtime.get('networkAuthorized') is True, 'UI_LOCAL_NETWORK_NOT_GRANTED')
    require(read_shell_proof(runtime) == QUALIFIED_SHELL, 'UI_CANONICAL_SHELL_UNAVAILABLE')
    return grant


def capsule_text(environment):
    require('LOCPATH' not in environment, 'UI_LOCPATH_REFUSED')
    leaked = [key for key in environment if key not in NATIVE_CAPSULE_KEYS and (
        key.startswith('FVOCI_LIBSQL_') or key.startswith('PASSWORD_') or 'TOKEN' in key
        or 'SECRET' in key or key == 'LOCPATH')]
    require(not leaked, 'UI_DOCKER_ENV_SECRET_REFUSED')
    lines = []
    for key in NATIVE_CAPSULE_KEYS:
        if key not in environment:
            continue
        value = environment[key]
        require(isinstance(value, str) and '\0' not in value and '\n' not in value, 'UI_DOCKER_ENV_SECRET_REFUSED')
        lines.append(key + '=' + shlex.quote(value) + '\n')
    require(all(any(line.startswith(key + '=') for line in lines) for key in SECRET_ENV_KEYS), 'UI_DOCKER_ENV_SECRET_REFUSED')
    return ''.join(lines)


def write_readonly_capsule(directory, environment):
    path = Path(directory) / 'native-env.sh'
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, 'w') as output:
        output.write(capsule_text(environment))
    require(stat.S_IMODE(path.stat().st_mode) == 0o600, 'UI_PRIVATE_INPUT_REFUSED')
    return path


def write_launcher(directory):
    path = Path(directory) / 'launcher.sh'
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700)
    with os.fdopen(fd, 'w') as output:
        output.write(FIXED_LAUNCHER)
    os.chmod(path, 0o500)
    require(FIXED_LAUNCHER == path.read_text() and "invented" not in FIXED_LAUNCHER, 'UI_CANONICAL_SHELL_UNAVAILABLE')
    return path


def container_argv(name, launcher, capsule, binary, mount_args, command):
    require(str(launcher).startswith('/') and str(capsule).startswith('/'), 'UI_CURRENT_ARTIFACT_MISSING')
    argv = ['docker', 'create', '--name', name, '--read-only', '--cap-drop', 'ALL',
            '--security-opt', 'no-new-privileges', '--user', '1000:1000',
            '--memory', DAEMON_CAPS['memory.max'], '--memory-swap', DAEMON_CAPS['memory.max'],
            '--pids-limit', DAEMON_CAPS['pids.max'],
            '--tmpfs', '/tmp:rw,noexec,nosuid,size=67108864,uid=1000,gid=1000',
            '--network', 'host', '--entrypoint', QUALIFIED_SHELL, *mount_args,
            QUALIFIED_CANONICAL_IMAGE, LAUNCHER_DST, CAPSULE_DST, binary, *command]
    require('--env-file' not in argv and '-e' not in argv, 'UI_DOCKER_ENV_SECRET_REFUSED')
    require(not any(flag in argv for flag in ('--cpus', '--cpu-quota', '--cpu-period', '--cpuset-cpus')),
            'UI_DAEMON_CAP_REFUSED')
    return argv


def admit_published_config(argv, env_items, secret_values):
    keys = []
    for item in env_items:
        key, sep, _value = str(item).partition('=')
        require(sep == '=', 'UI_DOCKER_ENV_SECRET_REFUSED')
        keys.append(key)
    require(not any(key in keys for key in SECRET_ENV_KEYS), 'UI_DOCKER_ENV_SECRET_REFUSED')
    published = list(argv) + [str(item) for item in env_items]
    for value in secret_values:
        require(value and not any(value in item for item in published), 'UI_DOCKER_ENV_SECRET_REFUSED')
    require('--env-file' not in argv and '-e' not in argv, 'UI_DOCKER_ENV_SECRET_REFUSED')


def creation_identity(inspect, argv, secret_values, kind):
    config, host = inspect['Config'], inspect['HostConfig']
    require(inspect['State']['Pid'] == 0 and inspect['State']['Running'] is False, 'UI_DAEMON_PID_REFUSED')
    require(config.get('Image') == QUALIFIED_CANONICAL_IMAGE, 'UI_CANONICAL_SHELL_UNAVAILABLE')
    require(list(config.get('Entrypoint') or []) == [QUALIFIED_SHELL], 'UI_CANONICAL_SHELL_UNAVAILABLE')
    admit_published_config(config.get('Cmd') or [], config.get('Env') or [], secret_values)
    admit_published_config(argv, [], secret_values)
    # HostConfig is a create-shape refusal only. It is not cgroup cap proof.
    require(host.get('ReadonlyRootfs') is True and list(host.get('CapDrop') or []) == ['ALL'], 'UI_DAEMON_CAP_REFUSED')
    require(config.get('User') == '1000:1000' and host.get('Privileged') is not True, 'UI_DAEMON_CAP_REFUSED')
    # Present CPU fields must be the Docker zero values. Missing fields are not proof.
    if 'CpuQuota' in host:
        quota = host.get('CpuQuota')
        require(isinstance(quota, int) and not isinstance(quota, bool) and quota == 0, 'UI_DAEMON_CAP_REFUSED')
    if 'NanoCpus' in host:
        nano = host.get('NanoCpus')
        require(isinstance(nano, int) and not isinstance(nano, bool) and nano == 0, 'UI_DAEMON_CAP_REFUSED')
    if 'CpusetCpus' in host:
        require(host.get('CpusetCpus') == '', 'UI_DAEMON_CAP_REFUSED')
    live = ONE_SHOT_LIVE if kind == 'fixture' else 'pending-listen'
    return {'phase': 'created', 'liveDaemon': live, 'qualification': BLOCKED,
            'cgroupCaps': 'not-observed', 'shell': QUALIFIED_SHELL, 'image': QUALIFIED_CANONICAL_IMAGE}


def finished_one_shot(creation, returncode):
    require(creation.get('qualification') == BLOCKED and creation.get('liveDaemon') == ONE_SHOT_LIVE,
            'UI_DAEMON_OBSERVATION_UNSUPPORTED')
    require(creation.get('cgroupCaps') == 'not-observed', 'UI_DAEMON_CAP_REFUSED')
    require(isinstance(returncode, int), 'UI_NATIVE_FIXTURE_OUTPUT_REFUSED')
    return {'productExit': returncode, 'liveDaemon': ONE_SHOT_LIVE, 'qualification': BLOCKED}


def running_daemon_sample(inspect, caps, proc_row, nspid, client_pid, binary, waited):
    require(waited is True, 'UI_DAEMON_WAIT_MISSING')
    state = inspect['State']
    pid = state.get('Pid')
    require(isinstance(pid, int) and pid > 0 and pid == proc_row['pid'] and pid != client_pid, 'UI_DAEMON_PID_REFUSED')
    require(isinstance(nspid, int) and 0 < nspid != pid, 'UI_NAMESPACE_PID_REFUSED')
    require(state.get('Running') is True and state.get('OOMKilled') is False, 'UI_DAEMON_STATE_REFUSED')
    require('HostConfig' not in caps and 'CapDrop' not in caps, 'UI_DAEMON_CAP_REFUSED')
    for key, expected in DAEMON_CAPS.items():
        require(caps.get(key) == expected, 'UI_DAEMON_CAP_REFUSED')
    require(proc_row.get('comm') == 'fvoci-server', 'UI_NORMAL_MAIN_IDENTITY_FAILED')
    require(re.fullmatch('[0-9]+', str(proc_row.get('startTicks') or '')), 'UI_DAEMON_PID_REFUSED')
    if proc_row.get('exeInspection') == 'observed':
        require(proc_row.get('exe') == binary, 'UI_CANONICAL_IMAGE_BINARY_REFUSED')
    else:
        require(proc_row.get('exeInspection') == 'UNAVAILABLE', 'UI_NORMAL_MAIN_IDENTITY_FAILED')
    return {'daemonPid': pid, 'pid': nspid, 'startTicks': proc_row['startTicks'], 'waited': True,
            'qualification': 'daemon-observed', 'caps': {key: caps[key] for key in DAEMON_CAPS}}


def fixture(manifest, mode, environment, input=None):
    if execution_mode() == 'orca-local':
        return local_fixture(manifest, mode, environment, input)
    env = clean_env()
    env.update({k: environment[k] for k in ('FVOCI_LIBSQL_URL', 'FVOCI_LIBSQL_AUTH_TOKEN',
        'PASSWORD_PEPPER_KEYS', 'PASSWORD_PEPPER_ACTIVE_KEY_ID', 'FVOCI_E2E_TURSO_NAMESPACE',
        'FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE', 'FVOCI_TEST_TURSO_DESTRUCTIVE') if k in environment})
    env.update(E2E_DATABASE_BACKEND='libsql-remote', FVOCI_E2E_TURSO_UI_SELECTED='1')
    require(_PROCESSES is not None, 'UI_OWNED_PROCESS_SCOPE_REQUIRED')
    process = _PROCESSES.spawn([manifest['binaries']['fvoci-e2e-fixture']['path'], mode], 'fixture-' + mode,
                env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    original = None
    failures = []
    try:
        stdout, _ = process.communicate(input=canonical(input) if input else b'', timeout=120)
        result = subprocess.CompletedProcess([], process.returncode, stdout)
        # Observe the bounded native outcome before process finalization can
        # fail. Missing/malformed output never supplies an invented cause.
        require(len(result.stdout) < 64 * 1024 * 1024, 'UI_NATIVE_FIXTURE_OUTPUT_REFUSED')
        value = json.loads(result.stdout)
        require(isinstance(value, dict), 'UI_NATIVE_FIXTURE_OUTPUT_REFUSED')
        if result.returncode != 0:
            code = value.get('originalFailure', 'UI_NATIVE_FIXTURE_FAILED')
            require(isinstance(code, str) and re.fullmatch('TURSO_UI_[A-Z_]+|UI_NATIVE_FIXTURE_FAILED', code), 'UI_NATIVE_FAILURE_CODE_REFUSED')
            original = UiError(code)
            try:
                write(root() / ('fixture-failure-' + secrets.token_hex(6) + '.private.json'),
                      {'mode': mode, 'exit': result.returncode, 'receipt': value})
            except BaseException:
                failures.append('UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED')
            raise original
        require(value['lifecycleDrain'] == 'confirmed' and value['leases'] == 0
                and value['serverCloseReceipt'] == 'not-exposed-by-sdk', 'UI_NATIVE_DRAIN_FAILED')
        return value
    except BaseException as error:
        original = error
        raise
    finally:
        try:
            _PROCESSES.finish(process)
        except BaseException as cleanup:
            failures.append(failure_code(cleanup))
        if failures:
            try:
                print(json.dumps({'originalFailure': failure_code(original) if original else None,
                                  'nativeCleanupErrors': failures}), file=sys.stderr)
            except BaseException:
                pass
            if original is None:
                raise UiError('UI_PROCESS_CLOSURE_FAILED')


AUDITED_COUNTERS = {'event_sequence', 'collab_fence_counter', 'maintenance_job_claims'}
# Admission is intentionally narrower than general product support. Normal
# startup must not consume preexisting jobs or deliver preexisting events.
BACKGROUND_TABLES = ('documents', 'tasks', 'attachments', 'attachment_object_cleanups',
    'revisions', 'events', 'outbox_consumers', 'outbox_failures', 'processed_events',
    'notifications', 'notification_prefs', 'ics_tokens', 'magic_tokens',
    'github_deliveries', 'github_install_states', 'github_installations', 'github_issue_links',
    'import_jobs', 'import_deferred_events', 'push_deliveries', 'push_subscriptions',
    'webhook_deliveries', 'webhooks')


def startup_blockers(baseline):
    blocked = [table for table in BACKGROUND_TABLES if baseline['fingerprints'][table]]
    if baseline['startupHazards'] != 0 or baseline['liveOutboxLeases'] != 0:
        blocked.append('existing-owner-or-deletion')
    return blocked


def integer(cell):
    require(isinstance(cell, list) and len(cell) == 2 and cell[0] == 'integer'
            and isinstance(cell[1], str) and re.fullmatch('0|[1-9][0-9]*', cell[1]), 'UI_COUNTER_TYPE_REFUSED')
    value = int(cell[1])
    require(value <= 9223372036854775807, 'UI_COUNTER_RANGE_REFUSED')
    return value


def singleton(rows):
    require(len(rows) == 1 and len(rows[0]) == 2 and integer(rows[0][0]) == 1,
            'UI_COUNTER_KEY_CHANGED')
    return integer(rows[0][1])


def uuid_hex(value):
    require(isinstance(value, str), 'UI_ALLOCATION_ID_REFUSED')
    parsed = uuid.UUID(value)
    require(str(parsed) == value, 'UI_ALLOCATION_ID_REFUSED')
    return parsed.hex


def remember_actor(audit, actor, namespace):
    require(actor['namespace'] == namespace and actor['commit'] == 'confirmed'
            and actor['freshPrimaryReadback'] is True and actor['lifecycleDrain'] == 'confirmed'
            and actor['leases'] == 0, 'UI_ACTOR_RECEIPT_FAILED')
    actor_id, workspace_id = uuid_hex(actor['userId']), uuid_hex(actor['workspaceId'])
    require(actor['email'] in (namespace + '-owner@example.invalid', namespace + '-member@example.invalid'),
            'UI_ACTOR_EMAIL_REFUSED')
    require(audit['actors'].get(actor_id, actor['email']) == actor['email']
            and audit['workspaces'].get(workspace_id, namespace) == namespace, 'UI_ALLOCATION_CHANGED')
    audit['actors'][actor_id] = actor['email']
    audit['workspaces'][workspace_id] = namespace


def allocated_rows(before, after, audit):
    old_users = {row[0][1] for row in before['operations']['users']}
    old_spaces = {row[0][1] for row in before['operations']['workspaces']}
    added_users = [row for row in after['operations']['users'] if row[0][1] not in old_users]
    require({row[0][1] for row in added_users} == set(audit['actors']), 'UI_FOREIGN_USER_ADDITION')
    personal = set()
    for row in added_users:
        require(len(row) == 3 and row[0][0] == 'blob'
                and row[1] == ['text', audit['actors'][row[0][1]]], 'UI_ACTOR_ROWS_CHANGED')
        if row[2] != ['null']:
            require(row[2][0] == 'blob' and re.fullmatch('[a-f0-9]{32}', row[2][1]), 'UI_PERSONAL_ALLOCATION_REFUSED')
            personal.add(row[2][1])
    workspaces = set(audit['workspaces']) | personal
    require(not workspaces.intersection(old_spaces), 'UI_PREEXISTING_WORKSPACE_ALLOCATION')
    added_spaces = [row for row in after['operations']['workspaces'] if row[0][1] not in old_spaces]
    require({row[0][1] for row in added_spaces} == workspaces, 'UI_FOREIGN_WORKSPACE_ADDITION')
    for row in added_spaces:
        require(len(row) == 3 and row[0][0] == 'blob', 'UI_WORKSPACE_ROWS_CHANGED')
        if row[0][1] in personal:
            require(row[2] == ['text', 'personal'], 'UI_PERSONAL_ALLOCATION_REFUSED')
        else:
            require(row[1] == ['text', audit['workspaces'][row[0][1]]] and row[2] == ['text', 'team'],
                    'UI_WORKSPACE_ROWS_CHANGED')
    actors = set(audit['actors'])
    events = set()
    for sha, refs in after['allocations']['events'].items():
        if sha not in before['fingerprints']['events']:
            require(refs['workspaces'] and set(refs['workspaces']) <= workspaces
                    and refs['actors'] and set(refs['actors']) <= actors, 'UI_FOREIGN_EVENT')
            events.add(refs['self'])
    for table, hashes in after['fingerprints'].items():
        if table in AUDITED_COUNTERS:
            continue
        for sha, count in hashes.items():
            old = before['fingerprints'][table].get(sha, 0)
            if old:
                require(count == old, 'UI_PREEXISTING_ROW_MULTIPLICITY_CHANGED')
                continue
            refs = after['allocations'][table][sha]
            require(set(refs['workspaces']) <= workspaces and set(refs['actors']) <= actors
                    and set(refs['events']) <= events, 'UI_FOREIGN_ROW_ADDITION')
            if table == 'users':
                require(refs['self'] in actors, 'UI_FOREIGN_USER_ADDITION')
            elif table == 'workspaces':
                require(refs['self'] in workspaces, 'UI_FOREIGN_WORKSPACE_ADDITION')
            elif table == 'outbox_consumers':
                require(refs['consumer'] in ('notifications', 'mail', 'push', 'webhooks', 'github'), 'UI_RELAY_REGISTRY_CHANGED')
            else:
                require(refs['workspaces'] or refs['actors'] or refs['events'], 'UI_UNSCOPED_ROW_ADDITION')
    return workspaces, actors


def maintenance_receipts(logpath, server_identity, target):
    records = []
    marker = 'FVOCI_E2E_MAINTENANCE_RECEIPT '
    require(logpath.stat().st_size <= 32 * 1024 * 1024, 'UI_SERVER_LOG_CAP_REFUSED')
    for line in logpath.read_text(errors='strict').splitlines():
        if marker in line:
            record, _ = json.JSONDecoder().raw_decode(line.split(marker, 1)[1])
            require(record.keys() == {'schema', 'pid', 'key', 'ownerSha256', 'generation', 'outcome'}
                    and record['schema'] == 1 and record['pid'] == server_identity['pid']
                    and record['key'] in (1, 8, 9) and re.fullmatch('[a-f0-9]{64}', record['ownerSha256']),
                    'UI_MAINTENANCE_RECEIPT_REFUSED')
            records.append(record)
    require(len(records) <= 30, 'UI_MAINTENANCE_RECEIPT_CAP_REFUSED')
    return {'identity': server_identity, 'targetSha256': target, 'receipts': records}


def audit_maintenance(before, after, audit):
    keys = list(range(1, 10))
    require([integer(row[0]) for row in before] == keys and [integer(row[0]) for row in after] == keys,
            'UI_MAINTENANCE_KEYS_CHANGED')
    current = {integer(row[0]): integer(row[2]) for row in before}
    require(len(audit['servers']) == audit['serverStarts'] and audit['serverStarts'] in (1, 2, 3), 'UI_START_COUNT_REFUSED')
    identities_seen, owners_seen = set(), set()
    for server in audit['servers']:
        identity_key = (server['identity']['pid'], server['identity']['startTicks'])
        require(identity_key not in identities_seen and re.fullmatch('[0-9]+', server['identity']['startTicks'])
                and server['targetSha256'] == audit['targetSha256'], 'UI_FOREIGN_MAINTENANCE_PROCESS')
        identities_seen.add(identity_key)
        for key in (8, 9, 1):
            records = [r for r in server['receipts'] if r['key'] == key]
            require(len(records) == 3 and [r['outcome'] for r in records] == ['prepared', 'acquired', 'released'],
                    'UI_MAINTENANCE_FINISH_UNCONFIRMED')
            owner = records[0]['ownerSha256']
            require(owner not in owners_seen and all(r['pid'] == server['identity']['pid'] and r['ownerSha256'] == owner for r in records)
                    and records[0]['generation'] is None, 'UI_FOREIGN_MAINTENANCE_OWNER')
            owners_seen.add(owner)
            expected = str(current[key] + 1)
            require(records[1]['generation'] == records[2]['generation'] == expected, 'UI_MAINTENANCE_GENERATION_UNEXPLAINED')
            current[key] += 1
    for old, new in zip(before, after):
        key = integer(old[0])
        require(len(old) == 4 and len(new) == 4 and old[1] == new[1] == ['null'] and old[3] == new[3] == ['null'],
                'UI_MAINTENANCE_OWNER_REMAINS')
        require(integer(new[2]) == current[key], 'UI_MAINTENANCE_GENERATION_UNEXPLAINED')


def audit_counters(before, after, audit):
    b, a = before['operations'], after['operations']
    require(before['startupHazards'] == 0 and after['startupHazards'] == 0
            and after['liveOutboxLeases'] == 0, 'UI_FOREIGN_OR_UNRELEASED_OWNER')
    workspaces, actors = allocated_rows(before, after, audit)
    first, last = singleton(b['event_sequence']), singleton(a['event_sequence'])
    require(first <= last <= first + 10000, 'UI_EVENT_COUNTER_RESET')
    added_events = [row for row in a['events'] if integer(row[0]) > first]
    require([integer(row[0]) for row in added_events] == list(range(first + 1, last + 1)), 'UI_EVENT_COUNTER_UNEXPLAINED')
    for row in added_events:
        require(len(row) == 3 and row[1][0] == 'blob' and row[1][1] in workspaces
                and row[2][0] == 'blob' and row[2][1] in actors, 'UI_FOREIGN_EVENT')
    first_fence, last_fence = singleton(b['collab_fence_counter']), singleton(a['collab_fence_counter'])
    require(first_fence <= last_fence <= first_fence + 10000, 'UI_FENCE_COUNTER_RESET')
    fences = set()
    observed = {canonical(row[:4]) for row in audit['observedFences']}
    for row in audit['observedFences']:
        require(len(row) == 5 and row[0] == ['blob', row[0][1]] and row[0][1] in workspaces
                and row[2][0] == 'blob' and re.fullmatch('[a-f0-9]{32}', row[2][1]), 'UI_FOREIGN_ROOM_FENCE')
        value = integer(row[3])
        require(first_fence <= value < last_fence, 'UI_FOREIGN_ROOM_FENCE')
        fences.add(value)
    for name in ('collab_room_fences', 'task_collab_room_fences'):
        for row in a[name]:
            if row not in b[name]:
                require(canonical(row[:4]) in observed, 'UI_UNOBSERVED_ROOM_OWNER')
    require(fences == set(range(first_fence, last_fence)), 'UI_FENCE_COUNTER_UNOBSERVED')
    audit_maintenance(b['maintenance_job_claims'], a['maintenance_job_claims'], audit)
    require(b['outbox_consumers'] == [], 'UI_PREEXISTING_RELAY_REFUSED')
    names = [row[0] for row in a['outbox_consumers']]
    require(sorted(names) == sorted([['text', name] for name in ('notifications', 'mail', 'push', 'webhooks', 'github')]), 'UI_RELAY_REGISTRY_CHANGED')
    for row in a['outbox_consumers']:
        require(len(row) == 4 and first <= integer(row[1]) <= last and row[2] == row[3] == ['null'], 'UI_RELAY_FINISH_UNCONFIRMED')


def assert_preserved(before, after, audit=None):
    require(before['ledger'] == after['ledger'] and before['schemaSha256'] == after['schemaSha256']
            and before['lineage'] == after['lineage'], 'UI_CURRENT_LEDGER_CHANGED')
    require(before['fingerprints'].keys() == after['fingerprints'].keys(), 'UI_CURRENT_TABLES_CHANGED')
    for table, rows in before['fingerprints'].items():
        if audit is not None and table in AUDITED_COUNTERS:
            continue
        for sha, count in rows.items():
            require(after['fingerprints'][table].get(sha, 0) == count, 'UI_PREEXISTING_ROW_CHANGED')
    if audit is not None:
        audit_counters(before, after, audit)


def report_cases(report, spec, titles):
    require(report['config']['workers'] == 1 and report['errors'] == []
            and report['config']['metadata']['selectedBackend'] == 'libsql-remote'
            and report['stats']['expected'] == len(titles)
            and all(report['stats'][k] == 0 for k in ('unexpected', 'flaky', 'skipped')), 'UI_BROWSER_REPORT_FAILED')
    cases = []
    def visit(suites):
        for suite in suites:
            for case in suite.get('specs', []):
                require(Path(case['file']).name == spec and case['ok'] is True and len(case['tests']) == 1, 'UI_BROWSER_CASE_FAILED')
                test = case['tests'][0]
                require(test['expectedStatus'] == 'passed' and len(test['results']) == 1, 'UI_BROWSER_CASE_FAILED')
                actual = test['results'][0]
                require(actual['status'] == 'passed' and actual['retry'] == 0 and actual['errors'] == [], 'UI_BROWSER_CASE_FAILED')
                cases.append((case['title'], actual))
            visit(suite.get('suites', []))
    visit(report['suites'])
    require([title for title, _ in cases] == titles, 'UI_BROWSER_COUNT_MISMATCH')
    return cases


def attachment(cases, name):
    entries = [a for _, actual in cases for a in actual['attachments'] if a['name'] == name]
    require(len(entries) == 1 and entries[0]['contentType'] == 'application/json', 'UI_ATTACHMENT_MISSING')
    return json.loads(base64.b64decode(entries[0]['body'], validate=True))


def actor():
    if execution_mode() == 'orca-local':
        load_existing_local_lease('actor')
    capsule = private_read(os.environ['FVOCI_E2E_TURSO_PRIVATE_INPUT'])
    namespace = os.environ.get('FVOCI_E2E_TURSO_NAMESPACE', '')
    require(namespace == capsule['namespace'] and re.fullmatch('tui-[a-f0-9]{20}', namespace), 'UI_ACTOR_NAMESPACE_REFUSED')
    expected = {'E2E_USER_EMAIL': namespace + '-member@example.invalid', 'E2E_USER_PASSWORD': 'memberpass1',
                'E2E_USER_GIVEN_NAME': '협업', 'E2E_USER_FAMILY_NAME': '멤버', 'E2E_WORKSPACE_SLUG': namespace,
                'E2E_MEMBERSHIP_ROLE': 'member', 'E2E_DATABASE_BACKEND': 'libsql-remote'}
    require(all(os.environ.get(k) == v for k, v in expected.items()), 'UI_ACTOR_INPUT_REFUSED')
    m = capsule['manifest']
    require(digest(m['binaries']['fvoci-e2e-fixture']['path']) == m['binaries']['fvoci-e2e-fixture']['sha256'], 'UI_ACTOR_BINARY_CHANGED')
    result = fixture(m, 'member', capsule['environment'])
    require(result['namespace'] == namespace and (capsule['workspaceId'] is None or result['workspaceId'] == capsule['workspaceId'])
            and result['commit'] == 'confirmed' and result['freshPrimaryReadback'], 'UI_ACTOR_RECEIPT_FAILED')
    write(Path(os.environ['FVOCI_E2E_TURSO_PRIVATE_INPUT']).parent / ('member-' + result['userId'] + '.json'), result)
    print(result['userId'])


def proc_identity(pid):
    raw = (Path('/proc') / str(pid) / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': pid, 'parentPid': int(raw[1]), 'startTicks': raw[19], 'state': raw[0]}


def identity(pid):
    row = proc_identity(pid)
    path = Path('/proc') / str(pid)
    row['comm'] = (path / 'comm').read_text().strip()
    try:
        row['exe'] = os.readlink(path / 'exe')
        row['exeInspection'] = 'observed'
    except PermissionError:
        row['exe'] = None
        row['exeInspection'] = 'UNAVAILABLE'
    return row


def retired(row):
    try:
        return proc_identity(row['pid'])['startTicks'] != row['startTicks']
    except (FileNotFoundError, ProcessLookupError):
        return True


_PROCESSES = None


class UiProcesses:
    """One hosted UI consumer's allocations, never a reusable process service.
    The subreaper attr is process-local. Signals use captured pidfds only.
    Neither /proc secrets nor unrelated process capabilities are inspected.
    """
    def __enter__(self):
        global _PROCESSES
        require(_PROCESSES is None and callable(getattr(os, 'pidfd_open', None))
                and callable(getattr(signal, 'pidfd_send_signal', None)), 'UI_PROCESS_CAPABILITY_REQUIRED')
        self.pid = os.getpid()
        require(not any(row['parentPid'] == self.pid for row in self.proc_rows()), 'UI_PREEXISTING_CHILD_REFUSED')
        self.libc = ctypes.CDLL(None, use_errno=True)
        self.libc.prctl.restype = ctypes.c_int
        # prctl is variadic: explicitly use unsigned-long arguments, including
        # the pointer value in GET, matching the Linux ABI on this fixed host.
        self.libc.prctl.argtypes = [ctypes.c_int, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong, ctypes.c_ulong]
        self.prior = ctypes.c_int()
        self.prctl(37, ctypes.addressof(self.prior))  # PR_GET_CHILD_SUBREAPER
        self.entries, self.allocations, self.errors = {}, [], []
        self.lock, self.halt = threading.RLock(), threading.Event()
        try:
            self.prctl(36, 1)  # PR_SET_CHILD_SUBREAPER
            check = ctypes.c_int()
            self.prctl(37, ctypes.addressof(check))
            require(check.value == 1, 'UI_SUBREAPER_NOT_CONFIRMED')
            self.thread = threading.Thread(target=self.watch, daemon=False)
            self.thread.start()
            _PROCESSES = self
        except BaseException as original:
            # No allocation is possible before this method returns. Restore
            # the observed prior attribute without masking setup failure.
            try:
                self.prctl(36, self.prior.value)
                check = ctypes.c_int()
                self.prctl(37, ctypes.addressof(check))
                require(check.value == self.prior.value, 'UI_SUBREAPER_RESTORE_FAILED')
            except BaseException:
                print(json.dumps({'originalFailure': failure_code(original), 'processCleanupErrors': ['UI_SUBREAPER_RESTORE_FAILED']}), file=sys.stderr)
            raise
        return self

    def prctl(self, option, value):
        if self.libc.prctl(option, value, 0, 0, 0) != 0:
            self.prctl_errno = ctypes.get_errno()
            raise UiError('UI_SUBREAPER_SETUP_FAILED')

    def proc_rows(self):
        paths = [p for p in Path('/proc').iterdir() if p.name.isdigit()]
        require(len(paths) <= 4096, 'UI_PROCESS_SNAPSHOT_CAP_REFUSED')
        rows = []
        for path in paths:
            try:
                rows.append(proc_identity(int(path.name)))
            except (FileNotFoundError, ProcessLookupError):
                pass
        return rows

    def capture(self, row, label, allocation=None):
        key = (row['pid'], row['startTicks'])
        if key in self.entries:
            return key
        require(len(self.entries) < 512, 'UI_PROCESS_HISTORY_CAP_REFUSED')
        fd = None
        try:
            fd = os.pidfd_open(row['pid'], 0)
            require(proc_identity(row['pid'])['startTicks'] == row['startTicks'], 'UI_PROCESS_IDENTITY_RACE')
        except (FileNotFoundError, ProcessLookupError):
            require(retired(row), 'UI_PROCESS_IDENTITY_UNCONFIRMED')
            if fd is not None:
                os.close(fd)
            fd = None
        except BaseException:
            if fd is not None:
                os.close(fd)
            raise
        self.entries[key] = {'identity': row, 'label': label, 'allocation': allocation, 'pidfd': fd, 'reaped': False}
        return key

    def spawn(self, args, label, **kwargs):
        with self.lock:
            process = subprocess.Popen(args, start_new_session=True, **kwargs)
            allocation = {'process': process, 'label': label, 'closed': False, 'forced': False}
            self.allocations.append(allocation)
            try:
                allocation['key'] = self.capture(proc_identity(process.pid), label, len(self.allocations) - 1)
            except BaseException:
                self.errors.append('UI_SPAWN_IDENTITY_UNCONFIRMED')
                # No PID-only kill is allowed when capture fails.
                raise
            self.snapshot()
            return process

    def snapshot(self):
        if not any(not a['closed'] for a in self.allocations):
            return
        rows = self.proc_rows()
        selected = {row['pid']: row for row in rows}
        parents = {pid for (pid, ticks) in self.entries if pid in selected and selected[pid]['startTicks'] == ticks}
        while True:
            found = [r for r in rows if r['parentPid'] in parents or r['parentPid'] == self.pid]
            new = [r for r in found if (r['pid'], r['startTicks']) not in self.entries]
            if not new:
                break
            for row in new:
                # With zero preexisting children, actual adoption by this
                # subreaper proves its own descendant ancestry, even detached.
                parent = selected.get(row['parentPid'])
                parent_entry = self.entries.get((parent['pid'], parent['startTicks'])) if parent else None
                self.capture(row, 'observed-or-adopted-descendant', parent_entry['allocation'] if parent_entry else None)
                parents.add(row['pid'])
        roots = {a['process'].pid for a in self.allocations}
        for entry in self.entries.values():
            row = entry['identity']
            actual = selected.get(row['pid'])
            if actual and actual['startTicks'] == row['startTicks'] and actual['state'] == 'Z' and actual['parentPid'] == self.pid and row['pid'] not in roots:
                child, _ = os.waitpid(row['pid'], os.WNOHANG)
                entry['reaped'] = child == row['pid']

    def watch(self):
        while not self.halt.wait(0.02):
            try:
                with self.lock:
                    self.snapshot()
            except BaseException:
                self.errors.append('UI_PROCESS_OBSERVATION_FAILED')
                return

    def send(self, entry, sig):
        if retired(entry['identity']):
            return
        require(entry['pidfd'] is not None, 'UI_PROCESS_SIGNAL_UNQUALIFIED')
        signal.pidfd_send_signal(entry['pidfd'], sig, None, 0)

    def finish(self, process, normal_signal=False):
        index = next(i for i, a in enumerate(self.allocations) if a['process'] is process)
        allocation = self.allocations[index]
        with self.lock:
            self.snapshot()
            entry = self.entries[allocation['key']]
            if normal_signal and process.poll() is None:
                self.send(entry, signal.SIGTERM)
        try:
            code = process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            allocation['forced'] = True
            with self.lock:
                self.send(entry, signal.SIGKILL)
            code = process.wait(timeout=10)
        deadline = time.monotonic() + 10
        forced_deadline = None
        while True:
            with self.lock:
                self.snapshot()
                own_alive = [e for e in self.entries.values() if e['allocation'] in (index, None) and not retired(e['identity'])]
                other_live = any(i != index and not a['closed'] for i, a in enumerate(self.allocations))
            if not own_alive:
                break
            if time.monotonic() >= deadline:
                allocation['forced'] = True
                with self.lock:
                    # Unknown adopted ancestry is still our own descendant,
                    # but cannot be assigned to a concurrent allocation.
                    for e in own_alive:
                        if e['allocation'] == index or not other_live:
                            self.send(e, signal.SIGKILL)
                if forced_deadline is None:
                    forced_deadline = time.monotonic() + 10
                if time.monotonic() >= forced_deadline:
                    break
            time.sleep(0.02)
        with self.lock:
            self.snapshot()
            allocation['closed'] = retired(entry['identity']) and all(retired(e['identity']) for e in self.entries.values() if e['allocation'] == index)
        require(not allocation['forced'] and allocation['closed'] and not self.errors, 'UI_PROCESS_CLOSURE_FAILED')
        return code

    def closure(self):
        with self.lock:
            self.snapshot()
            return not self.errors and all(a['closed'] for a in self.allocations) and all(retired(e['identity']) for e in self.entries.values())

    def __exit__(self, kind, original, trace):
        global _PROCESSES
        failures = []
        closed = False
        try:
            for allocation in self.allocations:
                if not allocation['closed']:
                    try:
                        self.finish(allocation['process'], True)
                    except BaseException:
                        failures.append('UI_PROCESS_FINAL_CLOSURE_FAILED')
            try:
                closed = self.closure()
            except BaseException:
                failures.append('UI_PROCESS_FINAL_OBSERVATION_FAILED')
            if closed:
                try:
                    self.prctl(36, self.prior.value)
                    check = ctypes.c_int()
                    self.prctl(37, ctypes.addressof(check))
                    require(check.value == self.prior.value, 'UI_SUBREAPER_RESTORE_FAILED')
                except BaseException:
                    failures.append('UI_SUBREAPER_RESTORE_FAILED')
        finally:
            try:
                self.halt.set()
            except BaseException:
                failures.append('UI_PROCESS_OBSERVER_STOP_FAILED')
            try:
                self.thread.join(timeout=1)
            except BaseException:
                failures.append('UI_PROCESS_OBSERVER_JOIN_FAILED')
            try:
                if self.thread.is_alive():
                    failures.append('UI_PROCESS_OBSERVER_NOT_STOPPED')
            except BaseException:
                failures.append('UI_PROCESS_OBSERVER_STATE_FAILED')
            for entry in self.entries.values():
                if entry['pidfd'] is not None:
                    try:
                        os.close(entry['pidfd'])
                    except BaseException:
                        failures.append('UI_PIDFD_CLOSE_FAILED')
            _PROCESSES = None
        try:
            write(root() / ('process-closure-' + secrets.token_hex(6) + '.private.json'), {
                'confirmed': closed, 'normalClosure': closed and not any(a['forced'] for a in self.allocations),
                'errors': self.errors + failures,
                'allocations': [{k: v for k, v in a.items() if k not in ('process', 'key')} for a in self.allocations],
                'identities': [{'identity': e['identity'], 'allocation': e['allocation'], 'reaped': e['reaped']} for e in self.entries.values()]})
        except BaseException:
            failures.append('UI_PROCESS_RECEIPT_WRITE_FAILED')
        if failures:
            try:
                print(json.dumps({'originalFailure': failure_code(original) if original else None,
                                  'processCleanupErrors': failures}), file=sys.stderr)
            except BaseException:
                pass
        if original is None:
            require(closed and not failures, 'UI_PROCESS_CLOSURE_FAILED')


def port_closed(base):
    with socket.socket() as probe:
        probe.settimeout(1)
        return probe.connect_ex(('127.0.0.1', int(base.rsplit(':', 1)[1]))) != 0


def stop(server, base, directory):
    if getattr(server, 'container_id', None):
        return stop_attached_daemon(server, base, directory)
    allocation = next(i for i, a in enumerate(_PROCESSES.allocations) if a['process'] is server)
    code = _PROCESSES.finish(server, True)
    rows = [e['identity'] for e in _PROCESSES.entries.values() if e['allocation'] in (allocation, None)]
    require(code == 0 and rows and all(retired(row) for row in rows) and port_closed(base), 'UI_SERVER_CLOSURE_FAILED')
    stopped = {'serverExit': code, 'portClosed': True, 'recordedIdentitiesRetired': True}
    write(directory / ('server-identities-' + secrets.token_hex(6) + '.private.json'),
          {'stopped': stopped, 'identities': rows})
    return stopped


def start(manifest, environment, directory, expected_setup=False):
    if execution_mode() == 'orca-local':
        return local_server_start(manifest, environment, directory, expected_setup)
    logpath = directory / 'server.private.log'
    log = logpath.open('xb')
    os.fchmod(log.fileno(), 0o600)
    server = _PROCESSES.spawn([manifest['binaries']['fvoci-migrate']['path'], '--start'], 'server',
                env=environment, stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 10
    try:
        while True:
            raw = logpath.read_bytes()
            match = re.search(rb'fvoci-server listening on (http://127\.0\.0\.1:\d+)', raw)
            if match:
                base = match[1].decode()
                break
            require(server.poll() is None and time.monotonic() < deadline, 'UI_SERVER_START_FAILED')
            time.sleep(0.02)
        observed = identity(server.pid)
        require(observed['comm'] == 'fvoci-server' and (observed['exeInspection'] == 'UNAVAILABLE'
                or observed['exe'] == manifest['binaries']['fvoci-server']['path']), 'UI_NORMAL_MAIN_IDENTITY_FAILED')
        write(directory / 'launch-identity.private.json', {'observed': observed,
              'launchSha256': manifest['binaries']['fvoci-migrate']['sha256'],
              'serverSha256': manifest['binaries']['fvoci-server']['sha256'],
              'execContract': 'maintained migrate adjacent-server same-PID exec', 'kernelExeProof': observed['exeInspection']})
        with build_opener(ProxyHandler({})).open(base + '/api/v1/setup', timeout=10) as response:
            require(response.status == 200 and json.loads(response.read())['needed'] is expected_setup, 'UI_INITIALIZED_SETUP_CHANGED')
        return server, base, log
    except BaseException as original:
        cleanup = []
        try:
            _PROCESSES.finish(server, True)
        except BaseException as error:
            cleanup.append(failure_code(error))
        try:
            log.close()
        except BaseException:
            cleanup.append('UI_SERVER_LOG_CLOSE_FAILED')
        try:
            write(directory / 'start-failure.private.json', {'originalFailure': failure_code(original), 'cleanupErrors': cleanup})
        except BaseException:
            print(json.dumps({'originalFailure': failure_code(original), 'cleanupErrors': cleanup, 'receiptWrite': 'failed'}), file=sys.stderr)
        raise


def browser(manifest, directory, environment, spec, grep=None):
    report = directory / 'playwright.private.json'
    env = clean_env()
    env.update(environment)
    env.update(CI='true', FVOCI_E2E_RESULT_DIR=str(directory), PLAYWRIGHT_JSON_OUTPUT_FILE=str(report))
    require(not any(k in env for k in ('FVOCI_LIBSQL_URL', 'FVOCI_LIBSQL_AUTH_TOKEN', 'DATABASE_URL',
                                     'DATABASE_APP_URL', 'PASSWORD_PEPPER_KEYS')), 'UI_BROWSER_SECRET_ENV_REFUSED')
    args = [manifest['bun']['path'], '--bun', 'x', 'playwright', 'test', '--config',
            'e2e-pending/collab-playwright.config.ts', '--reporter=line,json']
    if grep:
        args += ['--grep', grep]
    args += ['e2e-pending/' + spec]
    with (directory / 'browser.private.log').open('xb') as output:
        os.fchmod(output.fileno(), 0o600)
        process = _PROCESSES.spawn(args, 'browser', env=env, cwd=W / 'apps/web', stdout=output, stderr=subprocess.STDOUT)
        original = None
        try:
            code = process.wait(timeout=900)
            require(code == 0, 'UI_ACTUAL_BROWSER_FAILED')
        except BaseException as error:
            original = error
            raise
        finally:
            try:
                _PROCESSES.finish(process)
            except BaseException as cleanup:
                print(json.dumps({'originalFailure': failure_code(original) if original else None, 'browserProcessClosure': failure_code(cleanup)}), file=sys.stderr)
                if original is None:
                    raise
    if report.exists():
        os.chmod(report, 0o600)
    require(code == 0, 'UI_ACTUAL_BROWSER_FAILED')
    return json.loads(report.read_text())


def execute_ui(manifest, baseline, environment):
    source = manifest['sourceInputs']
    require(startup_blockers(baseline) == [], 'UI_EXISTING_BACKGROUND_WORK_REFUSED')
    audit = {'namespaces': [], 'actors': {}, 'workspaces': {}, 'servers': [],
             'targetSha256': hashlib.sha256(environment['FVOCI_LIBSQL_URL'].encode()).hexdigest(),
             'serverStarts': 0, 'observedFences': []}
    receipt = {'source': source['source'], 'tree': source['tree'], 'backend': 'libsql-remote',
               'counts': {'on': 0, 'restart': 0, 'off': 0}, 'restore': 'NOTRUN',
               'precisionWorkload': 'NOTRUN', 'matchedOnOffCost': 'NOTRUN', 'cleanupErrors': [],
               'originalFailure': None, 'preservation': {'result': 'NOTRUN'}, 'receiptWrite': 'not-attempted'}
    last_environment = environment
    server_allocations = []
    try:
        for flow in ('on', 'off'):
            namespace = 'tui-' + secrets.token_hex(10)
            audit['namespaces'].append(namespace)
            directory = root() / flow
            directory.mkdir(mode=0o700)
            storage = directory / 'storage'
            storage.mkdir(mode=0o700)
            env = {**clean_env(), **environment, 'FVOCI_E2E_TURSO_NAMESPACE': namespace,
                   'FVOCI_REALTIME_MODE': flow, 'FVOCI_BIND': '127.0.0.1:0',
                   'FVOCI_PUBLIC_ORIGIN': 'http://127.0.0.1:0', 'FVOCI_COOKIE_SECURE': '0',
                   'STORAGE_DRIVER': 'local', 'FVOCI_STORAGE_DIR': str(storage),
                   'FVOCI_STATIC_DIR': str(W / 'apps/web/dist'),
                   'FVOCI_COLLAB_ENGINE': manifest['binaries']['collab-engine']['path'],
                   'FVOCI_COLLAB_FAMILY_LEASE_MS': '30000', 'FVOCI_COLLAB_FAMILY_RENEW_MS': '5000',
                   'FVOCI_COLLAB_MAX_ROOMS': '2', 'RUST_LOG': 'info'}
            # Existing scheduler configuration: retain the immediate startup
            # sweep and account for each generation; avoid a second cadence
            # within this bounded UI allocation. No consumer is disabled.
            env.update({name: '86400' for name in ('FVOCI_MAINTENANCE_TICK_SECS',
                'FVOCI_MAINTENANCE_INTERVAL_SECS', 'FVOCI_UPLOAD_GC_INTERVAL_SECS',
                'FVOCI_REVISION_SWEEP_INTERVAL_SECS')})
            last_environment = env
            setup_needed = flow == 'on' and baseline['setupNeeded']
            owner = None if setup_needed else fixture(manifest, 'owner', env)
            if owner is not None:
                require(owner['commit'] == 'confirmed' and owner['freshPrimaryReadback'], 'UI_OWNER_READBACK_FAILED')
                remember_actor(audit, owner, namespace)
            binding = {'schema': 1, 'backend': 'libsql-remote', 'setupNeeded': setup_needed,
                       'namespace': namespace, 'ownerEmail': namespace + '-owner@example.invalid',
                       'memberEmail': namespace + '-member@example.invalid', 'workspaceSlug': namespace,
                       'source': source['source'], 'tree': source['tree'], 'schemaCurrent': True,
                       'commit': 'not-attempted' if setup_needed else 'confirmed', 'lifecycleDrain': 'confirmed', 'leases': 0,
                       'baselineSha256': value_digest(baseline), 'owner': owner}
            write(directory / 'actor-binding.json', binding)
            capsule = directory / 'actor-input.private.json'
            write(capsule, {'manifest': manifest, 'environment': env, 'namespace': namespace, 'workspaceId': owner['workspaceId'] if owner is not None else None})
            wrapper = directory / 'member-fixture'
            with wrapper.open('x') as output:
                os.fchmod(output.fileno(), 0o700)
                output.write('#!/bin/sh\nexec ' + shlex.quote(sys.executable) + ' ' + shlex.quote(str(Path(__file__).resolve())) + ' --actor\n')
            server = None
            base = None
            log = None
            try:
                server, base, log = start(manifest, env, directory, setup_needed)
                audit['serverStarts'] += 1
                server_allocations.append((server, directory, allocation_identity(server)))
                b_env = {'PLAYWRIGHT_BASE_URL': base, 'FVOCI_E2E_SELECTED_BACKEND': 'libsql-remote',
                    'FVOCI_E2E_SELECTED_FLOW': flow, 'FVOCI_E2E_TURSO_NAMESPACE': namespace,
                    'FVOCI_E2E_TURSO_SOURCE': source['source'], 'FVOCI_E2E_TURSO_TREE': source['tree'],
                    'FVOCI_E2E_SELECTED_FIXTURE_BIN': str(wrapper), 'FVOCI_E2E_TURSO_PRIVATE_INPUT': str(capsule),
                    'FVOCI_E2E_TURSO_ACTOR_BINDING': str(directory / 'actor-binding.json')}
                current = {'schema': 1, 'ready': True, 'flow': flow, 'source': source['source'], 'tree': source['tree'],
                           'compiledSource': source['source'], 'buildSha256': digest(root() / 'current-build.json'),
                           'baselineSha256': value_digest(baseline)}
                write(directory / 'binding.json', current)
                write(directory / 'normal-main-ready.json', {'baseURL': base, 'selected_flow': flow,
                    'source': source['source'], 'tree': source['tree'], 'compiled_source': source['source'],
                    'current_binding': str(directory / 'binding.json'), 'current_binding_sha256': digest(directory / 'binding.json')})
                spec = ON if flow == 'on' else OFF
                titles = re.findall(r'^(?:  )?test\("([^"\n]+)"', (W / 'apps/web/e2e-pending' / spec).read_text(), re.M)
                if flow == 'on':
                    titles = [title for title in titles if not title.startswith('selected normal main restart:')]
                require(len(titles) == (1 if flow == 'on' else 8), 'UI_EXPECTED_REGISTRATION_CHANGED')
                cases = report_cases(browser(manifest, directory, b_env, spec,
                                     '^selected normal main:' if flow == 'on' else None), spec, titles)
                receipt['counts'][flow] = len(cases)
                if flow == 'on':
                    seed = attachment(cases, 'selected-vue-native-readback.json')
                    if owner is None:
                        audit['actors'][uuid_hex(seed['creatorId'])] = namespace + '-owner@example.invalid'
                        audit['workspaces'][uuid_hex(seed['workspaceId'])] = namespace
                    require(seed['selected'] == 'libsql-remote' and (owner is None or seed['workspaceId'] == owner['workspaceId'])
                            and seed['firstAck'] != seed['finalAck'], 'UI_NATIVE_SEED_MISMATCH')
                    observed = fixture(manifest, 'observe', env, {'workspaceId': seed['workspaceId'], 'documentIds': [seed['document']['id']]})
                    row = observed['rows'][seed['document']['id']]
                    audit['observedFences'].extend(row['roomFences'])
                    # Persist ACK IDs must name actual operation receipts, with
                    # the observer already checking payload lengths/digests/tail.
                    for ack in (seed['firstAck'], seed['finalAck']):
                        require(any(r['op'] == ack for r in row['receipts']), 'UI_DURABLE_ACK_RECEIPT_MISSING')
                    require(row['content'] == seed['persisted']['contentJson']
                            and row['text'] == seed['persisted']['text'], 'UI_NATIVE_CURRENT_BODY_MISMATCH')
                    write(directory / 'native-before-restart.private.json', observed)
                    old = stop(server, base, directory)
                    server = None
                    log.close()
                    log = None
                    checkpoint = {'schema': 1, 'source': source['source'], 'tree': source['tree'],
                        'compiledSource': source['source'], 'selected': 'libsql-remote', 'stopped': old, 'seed': seed}
                    write(directory / 'restart-checkpoint.private.json', checkpoint)
                    restart_dir = directory / 'restart'
                    restart_dir.mkdir(mode=0o700)
                    server, base, log = start(manifest, env, restart_dir)
                    audit['serverStarts'] += 1
                    server_allocations.append((server, restart_dir, allocation_identity(server)))
                    restart_env = {k: v for k, v in b_env.items() if k not in ('FVOCI_E2E_SELECTED_FIXTURE_BIN', 'FVOCI_E2E_TURSO_PRIVATE_INPUT')}
                    restart_env.update(PLAYWRIGHT_BASE_URL=base, FVOCI_E2E_SELECTED_RESTART_SOURCE=source['source'],
                        FVOCI_E2E_SELECTED_RESTART_CHECKPOINT=str(directory / 'restart-checkpoint.private.json'))
                    title = 'selected normal main restart: fresh actor reads persisted native history and manual revision'
                    report_cases(browser(manifest, restart_dir, restart_env, ON, '^selected normal main restart:'), ON, [title])
                    receipt['counts']['restart'] = 1
                    after = fixture(manifest, 'observe', env, {'workspaceId': seed['workspaceId'], 'documentIds': [seed['document']['id']]})
                    # Restart may append session revisions/generation. The exact
                    # original current body and all acknowledged ops must survive.
                    fresh = after['rows'][seed['document']['id']]
                    audit['observedFences'].extend(fresh['roomFences'])
                    require(fresh['content'] == row['content'] and fresh['text'] == row['text']
                            and all(any(r['op'] == ack for r in fresh['receipts']) for ack in (seed['firstAck'], seed['finalAck'])), 'UI_FRESH_PRIMARY_RESTART_MISMATCH')
                    write(restart_dir / 'native-after-restart.private.json', after)
            except BaseException as error:
                if receipt['originalFailure'] is None:
                    receipt['originalFailure'] = failure_code(error)
                raise
            finally:
                if server is not None:
                    try:
                        stop(server, base, directory)
                    except BaseException:
                        receipt['cleanupErrors'].append('UI_SERVER_CLOSURE_FAILED')
                        try:
                            _PROCESSES.finish(server, True)
                        except BaseException:
                            receipt['cleanupErrors'].append('UI_SERVER_FORCE_RETIRE_FAILED')
                if log is not None:
                    try:
                        log.close()
                    except BaseException:
                        receipt['cleanupErrors'].append('UI_SERVER_LOG_CLOSE_FAILED')
            require(receipt['cleanupErrors'] == [], 'UI_RESOURCE_CLOSURE_FAILED')
        require(receipt['counts'] == {'on': 1, 'restart': 1, 'off': 8}, 'UI_ACTUAL_COUNTS_FAILED')
        receipt['uiResult'] = 'PASS'
    except BaseException as error:
        receipt['uiResult'] = 'FAIL'
        if receipt['originalFailure'] is None:
            receipt['originalFailure'] = failure_code(error)
        raise
    finally:
        audit_errors = []
        for namespace, flow in zip(audit['namespaces'], ('on', 'off')):
            try:
                directory = root() / flow
                for path in directory.glob('member-*.json'):
                    remember_actor(audit, private_read(path), namespace)
            except BaseException as error:
                audit_errors.append(failure_code(error))
        for server, directory, server_identity in server_allocations:
            try:
                audit['servers'].append(maintenance_receipts(directory / 'server.private.log', server_identity, audit['targetSha256']))
            except BaseException as error:
                audit_errors.append(failure_code(error))
        receipt['auditErrors'] = audit_errors
        try:
            require(_PROCESSES.closure(),
                    'UI_RESOURCE_CLOSURE_FAILED')
            current_build()
            final = fixture(manifest, 'baseline', last_environment)
            # Save the actual after-state before judging preservation; a failed
            # audit must never erase the bounded primary observation itself.
            write(root() / 'preservation.private.json', {'before': baseline, 'after': final, 'audit': audit})
            receipt['preservation'] = {'result': 'observed', 'afterSha256': value_digest(final)}
            require(audit_errors == [], 'UI_OWNERSHIP_AUDIT_FAILED')
            assert_preserved(baseline, final, audit)
            receipt['preservation']['result'] = 'PASS'
        except BaseException as error:
            receipt['preservation']['result'] = 'FAIL' if receipt['preservation'].get('afterSha256') else 'NOTRUN'
            receipt['preservation']['failure'] = failure_code(error)
            receipt['uiResult'] = 'FAIL'
            if receipt['originalFailure'] is None:
                receipt['originalFailure'] = failure_code(error)
        receipt['receiptWrite'] = 'attempted'
        try:
            write(root() / 'ui-result.private.json', receipt)
        except BaseException:
            receipt['receiptWrite'] = 'failed'
            print(json.dumps({'originalFailure': receipt['originalFailure'], 'receiptWrite': 'failed',
                              'preservation': receipt['preservation'], 'cleanupErrors': receipt['cleanupErrors']}), file=sys.stderr)
            if receipt['originalFailure'] is None:
                receipt['originalFailure'] = 'UI_RECEIPT_WRITE_FAILED'
    require(receipt['uiResult'] == 'PASS' and receipt['receiptWrite'] != 'failed', receipt['originalFailure'] or 'UI_RECEIPT_WRITE_FAILED')
    return receipt



def failure_code(error):
    value = str(error) if isinstance(error, UiError) else ''
    return value if re.fullmatch('(?:UI|TURSO_UI)_[A-Z_]+', value) else 'UI_CONSUMER_FAILED'

def _consume(phase, inputs):
    manifest = current_build()
    require(inputs.get('ui_source_sha') == manifest['sourceInputs']['source'], 'UI_REVIEWED_SOURCE_REQUIRED')
    environment = {k: os.environ[k] for k in ('FVOCI_LIBSQL_URL', 'FVOCI_LIBSQL_AUTH_TOKEN')}
    baseline = fixture(manifest, 'baseline', environment)
    write(root() / 'baseline.private.json', baseline)
    baseline_sha = value_digest(baseline)
    target_sha = hashlib.sha256(environment['FVOCI_LIBSQL_URL'].encode()).hexdigest()
    if phase == 'ui-baseline':
        print('TURSO_UI_BASELINE_PASS source=' + manifest['sourceInputs']['source'] + ' baseline_sha256=' + baseline_sha
              + ' target_sha256=' + target_sha + ' rows=' + str(baseline['rows']) + ' setup_needed=' + str(baseline['setupNeeded']).lower()
              + ' startup_admissible=' + str(startup_blockers(baseline) == []).lower())
        return
    require(phase == 'ui-ack' and inputs.get('ui_baseline_sha256') == baseline_sha, 'UI_CURRENT_DATASET_BINDING_REQUIRED')
    require(inputs.get('ui_target_sha256') == target_sha, 'UI_CURRENT_TARGET_BINDING_REQUIRED')
    environment.update(FVOCI_DATABASE_BACKEND='libsql-remote', FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE='true',
        FVOCI_TEST_TURSO_DESTRUCTIVE='true', PASSWORD_PEPPER_KEYS=json.dumps({'fixture': secrets.token_hex(32)}),
        PASSWORD_PEPPER_ACTIVE_KEY_ID='fixture')
    result = execute_ui(manifest, baseline, environment)
    print('TURSO_UI_ACK_PASS on=1 restart=1 off=8 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN')
    require(result['cleanupErrors'] == [], 'UI_RESOURCE_CLOSURE_FAILED')



def consume(phase, inputs):
    with UiProcesses():
        return _consume(phase, inputs)

def main():
    try:
        mode = sys.argv[1:]
        if mode == ['--record-before']:
            write(root() / 'source-before.json', source_identity('record-before'))
            write(root() / 'physical-before.private.json', physical_inputs())
        elif mode == ['--freeze']:
            freeze()
        elif mode == ['--actor']:
            with UiProcesses():
                actor()
        else:
            raise UiError('UI_EXPLICIT_MODE_REQUIRED')
        return 0
    except UiError as error:
        print(str(error), file=sys.stderr)
        return 78
    except BaseException:
        print('UI_CONSUMER_FAILED', file=sys.stderr)
        return 78


def local_fixture(manifest, mode, environment, input):
    directory = root() / ('local-fixture-' + mode)
    directory.mkdir(mode=0o700)
    container_id, creation = publish_container(directory, environment, manifest, 'fvoci-e2e-fixture', [mode], 'fixture')
    process = None
    go_fd = None
    original = None
    receipt_errors = []
    try:
        require(creation.get('qualification') == BLOCKED and creation.get('liveDaemon') == ONE_SHOT_LIVE
                and creation.get('cgroupCaps') == 'not-observed', 'UI_DAEMON_OBSERVATION_UNSUPPORTED')
        deadline = budget_deadline(FIXTURE_BUDGET)
        process = _PROCESSES.spawn(
            ['docker', 'start', '--attach', '--interactive', container_id], 'fixture-' + mode,
            env=clean_env(), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        sample, go_fd = release_attached(directory, container_id, process, deadline)
        stdout, _stderr = communicate_within_budget(process, canonical(input) if input else b'', deadline)
        state = read_daemon_exit(container_id, 10)
        require(len(stdout) < 64 * 1024 * 1024, 'UI_NATIVE_FIXTURE_OUTPUT_REFUSED')
        value = json.loads(stdout)
        require(isinstance(value, dict), 'UI_NATIVE_FIXTURE_OUTPUT_REFUSED')
        native_code = state.get('ExitCode') if isinstance(state, dict) else None
        failure = native_failure_code(value, native_code)
        if failure is not None:
            try:
                write(directory / ('fixture-failure-' + secrets.token_hex(6) + '.private.json'),
                      {'mode': mode, 'exit': native_code, 'receipt': value})
            except BaseException:
                receipt_errors.append('UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED')
            raise UiError(failure)
        record = prove_normal_daemon(process, sample, state, False)
        require(value['lifecycleDrain'] == 'confirmed' and value['leases'] == 0
                and value['serverCloseReceipt'] == 'not-exposed-by-sdk', 'UI_NATIVE_DRAIN_FAILED')
        record['body'] = value
        require(record['qualification'] == 'retired', 'UI_DAEMON_OBSERVATION_UNSUPPORTED')
        write(directory / 'daemon-completion.private.json', record)
        return value
    except BaseException as error:
        original = error
        raise
    finally:
        errors = cleanup_owned(container_id, process, directory, [go_fd])
        errors.extend(receipt_errors)
        report_cleanup(original, errors)
        if errors and original is None:
            raise UiError('UI_PROCESS_CLOSURE_FAILED')


def allocation_index(process):
    return next(i for i, a in enumerate(_PROCESSES.allocations) if a['process'] is process)


def budget_deadline(seconds):
    require(seconds == FIXTURE_BUDGET or seconds == SERVER_BUDGET, 'UI_SERVER_START_FAILED')
    return time.monotonic() + seconds


def budget_remain(deadline):
    remain = deadline - time.monotonic()
    require(remain > 0, 'UI_SERVER_START_FAILED')
    return remain


def bounded_client_timeout(deadline):
    remain = budget_remain(deadline)
    if remain > 10:
        return 10
    return remain


def communicate_within_budget(process, payload, deadline):
    return process.communicate(input=payload, timeout=budget_remain(deadline))


def accept_stat_payload(payload):
    require(payload.__class__ is bytes and payload.endswith(b'\n') and payload.count(b'\n') == 1
            and STAT_BODY_MAX >= len(payload) - 1 >= 1 and len(payload) <= STAT_READ_BOUND,
            'UI_DAEMON_PID_REFUSED')
    return payload[:-1].decode('utf-8')


def stat_fields(text):
    require(isinstance(text, str) and ' ' in text and ')' in text, 'UI_DAEMON_PID_REFUSED')
    token, _sep, _rest = text.partition(' ')
    tail = text.rsplit(')', 1)[1].split()
    require(token.isdigit() and len(tail) > 19 and tail[19].isdigit(), 'UI_DAEMON_PID_REFUSED')
    return token, tail[19]


def stat_open_flags():
    return os.O_RDONLY | os.O_NONBLOCK | os.O_CLOEXEC


def go_open_flags():
    return os.O_RDWR | os.O_NONBLOCK | os.O_CLOEXEC


def open_stat_reader(path):
    return os.open(path, stat_open_flags())


def open_go_holder(path):
    return os.open(path, go_open_flags())


def read_stat_once(fd, deadline):
    remain = budget_remain(deadline)
    readable, _writable, _except = select.select([fd], [], [], remain)
    require(readable == [fd], 'UI_SERVER_START_FAILED')
    return accept_stat_payload(os.read(fd, STAT_READ_BOUND))


def write_go_once(fd):
    written = os.write(fd, GO_MARKER)
    require(written == len(GO_MARKER), 'UI_DAEMON_OBSERVATION_UNSUPPORTED')


def prepare_private_fifos(directory):
    directory = Path(directory)
    made = []
    for name in (STAT_FIFO_NAME, GO_FIFO_NAME):
        path = directory / name
        os.mkfifo(path, 0o600)
        os.chmod(path, 0o600)
        info = path.stat()
        require(stat.S_ISFIFO(info.st_mode) and stat.S_IMODE(info.st_mode) == 0o600
                and info.st_uid == os.getuid(), 'UI_PRIVATE_INPUT_REFUSED')
        made.append(path)
    return made[0], made[1]


def unlink_private_fifos(directory):
    for name in (STAT_FIFO_NAME, GO_FIFO_NAME):
        try:
            (Path(directory) / name).unlink()
        except FileNotFoundError:
            pass


def parse_status_fields(text, label):
    fields = next((line.split()[1:] for line in text.splitlines() if line.startswith(label)), None)
    require(isinstance(fields, list) and len(fields) == 4 and all(item.isdigit() for item in fields),
            'UI_DAEMON_PID_REFUSED')
    return [int(item) for item in fields]


def parse_nspid(text):
    fields = next((line.split()[1:] for line in text.splitlines() if line.startswith('NSpid:')), None)
    require(isinstance(fields, list) and len(fields) == 2 and all(item.isdigit() for item in fields),
            'UI_NAMESPACE_PID_REFUSED')
    return [int(item) for item in fields]


def read_proc_status(pid):
    return (Path('/proc') / str(pid) / 'status').read_text()


def read_daemon_caps(pid):
    lines = (Path('/proc') / str(pid) / 'cgroup').read_text().splitlines()
    cgroup_line = next(line for line in lines if line.startswith('0:'))
    cgroup = Path('/sys/fs/cgroup' + cgroup_line.split(':', 2)[2])
    return {key: (cgroup / key).read_text().strip() for key in DAEMON_CAPS}


def bind_paused_shell(stat_text, inspect, proc_row, nspid_fields, caps, client_pid, credentials):
    token, ticks = stat_fields(stat_text)
    state = inspect['State']
    pid = state.get('Pid')
    require(isinstance(pid, int) and not isinstance(pid, bool) and pid > 0 and pid != client_pid,
            'UI_DAEMON_PID_REFUSED')
    require(isinstance(nspid_fields, list) and len(nspid_fields) == 2, 'UI_NAMESPACE_PID_REFUSED')
    host_ns, namespace = nspid_fields
    require(isinstance(host_ns, int) and isinstance(namespace, int) and pid == host_ns
            and token == str(namespace) and pid != namespace, 'UI_NAMESPACE_PID_REFUSED')
    require(proc_row.get('pid') == pid and proc_row.get('startTicks') == ticks, 'UI_DAEMON_PID_REFUSED')
    require(proc_row.get('comm') == 'sh', 'UI_NORMAL_MAIN_IDENTITY_FAILED')
    require(state.get('Running') is True and state.get('OOMKilled') is False, 'UI_DAEMON_STATE_REFUSED')
    require(isinstance(caps, dict) and 'HostConfig' not in caps and 'CapDrop' not in caps, 'UI_DAEMON_CAP_REFUSED')
    for key, expected in DAEMON_CAPS.items():
        require(caps.get(key) == expected, 'UI_DAEMON_CAP_REFUSED')
    uid, gid = credentials.get('uid'), credentials.get('gid')
    require(uid == [1000, 1000, 1000, 1000] and gid == [1000, 1000, 1000, 1000], 'UI_DAEMON_PID_REFUSED')
    return {'maintenance': {'pid': namespace, 'startTicks': ticks},
            'retirement': {'pid': pid, 'startTicks': ticks}, 'clientPid': client_pid, 'comm': 'sh',
            'caps': {key: caps[key] for key in DAEMON_CAPS}, 'uid': list(uid), 'gid': list(gid)}


def allocation_identity(server):
    maintenance = getattr(server, 'maintenance_identity', None)
    if maintenance is not None:
        return {'pid': maintenance['pid'], 'startTicks': maintenance['startTicks']}
    return identity(server.pid)


def qualify_daemon_exit(state, forced):
    if not isinstance(state, dict) or state.get('Pid') != 0:
        return False, None
    code = state.get('ExitCode')
    oom = state.get('OOMKilled')
    if isinstance(code, bool) or not isinstance(code, int) or not isinstance(oom, bool) or not isinstance(forced, bool):
        return False, None
    return (forced is False and code == 0 and oom is False), code


def capture_owned_daemon(client, row):
    index = allocation_index(client)
    _PROCESSES.capture(row, 'container-init', index)
    return index


def sample_paused_daemon(container_id, stat_text, client, deadline):
    raw = docker_client(['docker', 'inspect', container_id], bounded_client_timeout(deadline))
    inspected = json.loads(raw.decode())[0]
    pid = inspected['State']['Pid']
    status = read_proc_status(pid)
    bound = bind_paused_shell(stat_text, inspected, identity(pid), parse_nspid(status), read_daemon_caps(pid),
                               client.pid, {'uid': parse_status_fields(status, 'Uid:'),
                                            'gid': parse_status_fields(status, 'Gid:')})
    bound['allocation'] = capture_owned_daemon(client, proc_identity(pid))
    return bound


def release_attached(directory, container_id, client, deadline):
    stat_fd = open_stat_reader(Path(directory) / STAT_FIFO_NAME)
    go_fd = None
    try:
        go_fd = open_go_holder(Path(directory) / GO_FIFO_NAME)
        sample = sample_paused_daemon(container_id, read_stat_once(stat_fd, deadline), client, deadline)
        write_go_once(go_fd)
        return sample, go_fd
    except BaseException:
        if go_fd is not None:
            os.close(go_fd)
        raise
    finally:
        os.close(stat_fd)


def read_daemon_exit(container_id, timeout):
    require(isinstance(container_id, str) and re.fullmatch('[0-9a-f]{64}', container_id), 'UI_DOCKER_CLIENT_FAILED')
    require(isinstance(timeout, (int, float)) and not isinstance(timeout, bool) and 0 < timeout <= 10,
            'UI_SERVER_START_FAILED')
    process = _PROCESSES.spawn(['docker', 'wait', container_id], 'docker-wait', env=clean_env(),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    timed_out = False
    try:
        try:
            process.communicate(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            process.kill()
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                raise UiError('UI_DOCKER_CLIENT_FAILED')
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=10)
        try:
            _PROCESSES.finish(process)
        except UiError:
            if not timed_out:
                raise
    if timed_out or process.returncode != 0:
        return None
    state = json.loads(docker_client(['docker', 'inspect', container_id], timeout).decode())[0]['State']
    return {'Pid': state.get('Pid'), 'ExitCode': state.get('ExitCode'), 'OOMKilled': state.get('OOMKilled')}


def native_rows(process):
    index = allocation_index(process)
    return index, [entry for entry in _PROCESSES.entries.values() if entry['allocation'] == index]


def require_native_retired(process, retirement):
    require(retired(retirement), 'UI_PROCESS_CLOSURE_FAILED')
    _index, rows = native_rows(process)
    native = [entry for entry in rows if entry['identity']['pid'] != process.pid]
    require(native and all(retired(entry['identity']) for entry in native), 'UI_PROCESS_CLOSURE_FAILED')


def allocation_retired(process):
    index, rows = native_rows(process)
    require(rows and _PROCESSES.allocations[index]['closed'] and not _PROCESSES.allocations[index]['forced']
            and all(retired(entry['identity']) for entry in rows), 'UI_PROCESS_CLOSURE_FAILED')
    return index


def native_failure_code(value, exit_code):
    if not isinstance(exit_code, int) or isinstance(exit_code, bool) or exit_code == 0:
        return None
    code = value.get('originalFailure', 'UI_NATIVE_FIXTURE_FAILED')
    require(isinstance(code, str) and re.fullmatch('TURSO_UI_[A-Z_]+|UI_NATIVE_FIXTURE_FAILED', code),
            'UI_NATIVE_FAILURE_CODE_REFUSED')
    return code


def prove_normal_daemon(process, sample, state, forced):
    qualified, product_exit = qualify_daemon_exit(state or {}, forced)
    require(qualified, 'UI_DAEMON_OBSERVATION_UNSUPPORTED')
    require_native_retired(process, sample['retirement'])
    client_code = _PROCESSES.finish(process)
    require(client_code == 0, 'UI_PROCESS_CLOSURE_FAILED')
    record = {'productExit': product_exit, 'clientExit': client_code, 'qualification': 'retired',
              'liveDaemon': 'retired', 'caps': sample['caps'], 'uid': sample['uid'], 'gid': sample['gid'],
              'maintenance': sample['maintenance'], 'retirement': sample['retirement'],
              'allocation': allocation_retired(process)}
    require(record['qualification'] != BLOCKED and record['liveDaemon'] != ONE_SHOT_LIVE,
            'UI_DAEMON_OBSERVATION_UNSUPPORTED')
    return record


def verify_own_closure(process, directory):
    require(all(not (Path(directory) / name).exists() for name in (STAT_FIFO_NAME, GO_FIFO_NAME)),
            'UI_PROCESS_CLOSURE_FAILED')
    if process is None:
        return
    require(process.poll() is not None, 'UI_PROCESS_CLOSURE_FAILED')
    index, rows = native_rows(process)
    require(_PROCESSES.allocations[index]['closed'] and all(retired(entry['identity']) for entry in rows),
            'UI_PROCESS_CLOSURE_FAILED')


def cleanup_owned(container_id, process, directory, fds):
    errors = []
    seen = set()
    for fd in fds:
        if fd is None or fd in seen:
            continue
        seen.add(fd)
        try:
            os.close(fd)
        except BaseException:
            errors.append('UI_PROCESS_CLOSURE_FAILED')
    try:
        if process is not None and process.poll() is None:
            docker_client(['docker', 'stop', '-t', '10', container_id], 10)
    except BaseException:
        errors.append('UI_DOCKER_CLIENT_FAILED')
    try:
        if process is not None:
            _PROCESSES.finish(process)
    except BaseException:
        errors.append('UI_PROCESS_CLOSURE_FAILED')
    try:
        if container_id is not None:
            docker_client(['docker', 'rm', container_id], 10)
    except BaseException:
        errors.append('UI_DOCKER_CLIENT_FAILED')
    try:
        unlink_private_fifos(directory)
    except BaseException:
        errors.append('UI_PROCESS_CLOSURE_FAILED')
    try:
        verify_own_closure(process, directory)
    except BaseException:
        errors.append('UI_PROCESS_CLOSURE_FAILED')
    return errors


def report_cleanup(original, errors):
    if errors:
        print(json.dumps({'originalFailure': failure_code(original) if original is not None else None,
                          'cleanupErrors': errors}), file=sys.stderr)
    return original


def release_failed_publish(directory, container_id, original):
    errors = []
    if container_id is not None:
        try:
            docker_client(['docker', 'rm', container_id], 10)
        except BaseException:
            errors.append('UI_DOCKER_CLIENT_FAILED')
    try:
        unlink_private_fifos(directory)
    except BaseException:
        errors.append('UI_PROCESS_CLOSURE_FAILED')
    report_cleanup(original, errors)
    return original


def stop_attached_daemon(server, base, directory):
    original = None
    try:
        host = server.host_identity
        require(isinstance(host, dict) and host.get('pid') != server.pid, 'UI_DAEMON_PID_REFUSED')
        entry = _PROCESSES.entries[(host['pid'], host['startTicks'])]
        forced = False
        if not retired(host):
            _PROCESSES.send(entry, signal.SIGTERM)
        state = read_daemon_exit(server.container_id, 10)
        if state is None or state.get('Pid') != 0:
            forced = True
            if not retired(host):
                _PROCESSES.send(entry, signal.SIGKILL)
            state = read_daemon_exit(server.container_id, 10)
            if state is None or state.get('Pid') != 0:
                docker_client(['docker', 'stop', '-t', '10', server.container_id], 10)
                inspected = json.loads(docker_client(['docker', 'inspect', server.container_id], 10).decode())[0]
                inspected = inspected['State']
                state = {'Pid': inspected.get('Pid'), 'ExitCode': inspected.get('ExitCode'),
                         'OOMKilled': inspected.get('OOMKilled')}
        record = prove_normal_daemon(server, server.daemon_sample, state, forced)
        require(port_closed(base), 'UI_SERVER_CLOSURE_FAILED')
        stopped = {'serverExit': record['productExit'], 'clientExit': record['clientExit'],
                   'qualification': record['qualification'], 'portClosed': True,
                   'recordedIdentitiesRetired': True, 'forced': False, 'caps': record['caps'],
                   'uid': record['uid'], 'gid': record['gid']}
        rows = [item['identity'] for item in _PROCESSES.entries.values() if item['allocation'] == record['allocation']]
        write(directory / ('server-identities-' + secrets.token_hex(6) + '.private.json'),
              {'stopped': stopped, 'identities': rows})
        return stopped
    except BaseException as error:
        original = error
        raise
    finally:
        errors = cleanup_owned(getattr(server, 'container_id', None), server, directory, [getattr(server, 'go_fd', None)])
        server.go_fd = None
        report_cleanup(original, errors)
        if errors and original is None:
            raise UiError('UI_PROCESS_CLOSURE_FAILED')


def local_server_start(manifest, environment, directory, expected_setup=False):
    container_id, creation = publish_container(directory, environment, manifest, 'fvoci-migrate', ['--start'], 'server')
    log = None
    server = None
    go_fd = None
    try:
        require(creation['qualification'] == BLOCKED and creation['cgroupCaps'] == 'not-observed',
                'UI_DAEMON_OBSERVATION_UNSUPPORTED')
        logpath = directory / 'server.private.log'
        log = logpath.open('xb')
        os.fchmod(log.fileno(), 0o600)
        deadline = budget_deadline(SERVER_BUDGET)
        server = _PROCESSES.spawn(['docker', 'start', '--attach', container_id], 'server',
                                  env=clean_env(), stdin=subprocess.DEVNULL, stdout=log, stderr=subprocess.STDOUT)
        server.container_id = container_id
        sample, go_fd = release_attached(directory, container_id, server, deadline)
        server.go_fd = go_fd
        go_fd = None
        server.daemon_sample = sample
        server.maintenance_identity = sample['maintenance']
        server.host_identity = sample['retirement']
        while True:
            raw = logpath.read_bytes()
            match = re.search(rb'fvoci-server listening on (http://127\.0\.0\.1:\d+)', raw)
            if match:
                base = match[1].decode()
                break
            require(server.poll() is None and time.monotonic() < deadline, 'UI_SERVER_START_FAILED')
            time.sleep(0.02)
        with build_opener(ProxyHandler({})).open(base + '/api/v1/setup', timeout=10) as response:
            require(response.status == 200 and json.loads(response.read())['needed'] is expected_setup,
                    'UI_INITIALIZED_SETUP_CHANGED')
        return server, base, log
    except BaseException as original:
        held = [go_fd]
        if server is not None:
            held.append(getattr(server, 'go_fd', None))
        errors = cleanup_owned(container_id, server, directory, held)
        if server is not None:
            server.go_fd = None
        try:
            if log is not None:
                log.close()
        except BaseException:
            errors.append('UI_SERVER_LOG_CLOSE_FAILED')
        report_cleanup(original, errors)
        raise original


def publish_container(directory, environment, manifest, binary_name, command, kind):
    grant = open_referenced_grant(kind)
    secret_values = [environment[key] for key in SECRET_ENV_KEYS]
    storage = environment.get('FVOCI_STORAGE_DIR')
    require(isinstance(storage, str) and storage.startswith('/') and ',' not in storage, 'UI_CURRENT_ARTIFACT_MISSING')
    prepared = {key: environment[key] for key in NATIVE_CAPSULE_KEYS if key in environment}
    prepared['FVOCI_COLLAB_ENGINE'] = BINARY_DST + '/collab-engine'
    prepared['FVOCI_STATIC_DIR'] = DIST_DST
    prepared['FVOCI_STORAGE_DIR'] = storage
    capsule = write_readonly_capsule(directory, prepared)
    launcher = write_launcher(directory)
    container_id = None
    try:
        stat_fifo, go_fifo = prepare_private_fifos(directory)
        parent = str(Path(manifest['binaries'][binary_name]['path']).resolve().parent)
        mounts = [mount_spec(launcher, LAUNCHER_DST, True), mount_spec(capsule, CAPSULE_DST, True),
                  mount_spec(stat_fifo, STAT_FIFO_DST, False), mount_spec(go_fifo, GO_FIFO_DST, False),
                  mount_spec(parent, BINARY_DST, True), mount_spec(W / 'apps/web/dist', DIST_DST, True),
                  mount_spec(storage, storage, False)]
        argv = container_argv('fvoci-tui-' + secrets.token_hex(6), str(launcher), str(capsule),
                              BINARY_DST + '/' + binary_name, mounts, command)
        admit_published_config(argv, [], secret_values)
        require(_PROCESSES is not None, 'UI_OWNED_PROCESS_SCOPE_REQUIRED')
        container_id = docker_client(argv, 10).decode().strip()
        require(re.fullmatch('[0-9a-f]{64}', container_id), 'UI_DOCKER_CLIENT_FAILED')
        inspect = json.loads(docker_client(['docker', 'inspect', container_id], 10).decode())[0]
        creation = creation_identity(inspect, argv, secret_values, kind)
        require(creation['qualification'] == BLOCKED and grant['canonicalRuntime']['imageId'] == creation['image'],
                'UI_DAEMON_OBSERVATION_UNSUPPORTED')
        write(directory / 'creation-identity.private.json', creation)
        return container_id, creation
    except BaseException as original:
        release_failed_publish(directory, container_id, original)
        raise


def mount_spec(src, dst, readonly):
    src, dst = str(src), str(dst)
    require(src.startswith('/') and dst.startswith('/') and not any(char in src + dst for char in ',\n'),
            'UI_CURRENT_ARTIFACT_MISSING')
    return 'type=bind,src=' + src + ',dst=' + dst + (',readonly' if readonly else '')


def docker_client(args, timeout):
    require(args and args[0] == 'docker' and '--env-file' not in args and '-e' not in args, 'UI_DOCKER_CLIENT_FAILED')
    process = _PROCESSES.spawn(args, 'docker-client', env=clean_env(),
                               stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        stdout, _stderr = process.communicate(timeout=timeout)
        if process.returncode != 0:
            raise UiError('UI_DOCKER_CLIENT_FAILED')
        return stdout
    finally:
        if process.poll() is None:
            process.kill()
        try:
            _PROCESSES.finish(process)
        except BaseException:
            if process.returncode == 0:
                raise


if __name__ == '__main__':
    sys.exit(main())
