"""Root-owned, one-shot current development SQLite normal-main/current Vue tracer.
Authored by W1; do not execute before root's current heavy lane is closed.
No compilation, package install, product patch, migration reset or PG credential.
"""
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import socket
import sqlite3
import subprocess
import sys
import time
from urllib.parse import quote
from urllib.request import ProxyHandler, Request, build_opener

E = Path(os.environ['FVOCI_CI_SELECTED_RUNS'])
W = Path(__file__).resolve().parents[2]
from current_binding import load_current
current = load_current('sqlite', __file__)
HEAD = COMPILED_HEAD = current['manifest']['source']
TREE = COMPILED_TREE = current['manifest']['tree']
IMAGE = 'ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7'
# Native origin/hash are recorded separately in the current bundle qualification.
OWNER = os.environ['FVOCI_CI_OWNER']
SPEC = 'workspace-wiki-selected-backend.spec.ts'
BUN = Path(os.environ['FVOCI_CI_BUN'])

# Rebind the WHOLE existing preparation/artifact proof first, not just labels.
# The historical bfb/D49 script MUST refuse this new restart extension.
assert HEAD == COMPILED_HEAD, 'restart requires exact current qualified compiled source'
assert os.environ.get('FVOCI_ROOT_RUN_OWNER') == OWNER
from restart_checkpoint import restart_same_app


def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()


def sha(path):
    digest = hashlib.sha256()
    with Path(path).open('rb') as file:
        for chunk in iter(lambda: file.read(1048576), b''):
            digest.update(chunk)
    return digest.hexdigest()


def write(path, value):
    path.write_text(json.dumps(value, indent=2) + '\n')


def command(args, log=None, required=True, env=None, cwd=None):
    if log is None:
        result = subprocess.run(args, capture_output=True, text=True, env=env, cwd=cwd)
    else:
        with log.open('w') as file:
            result = subprocess.run(args, stdout=file, stderr=subprocess.STDOUT, env=env, cwd=cwd)
    if required and result.returncode:
        raise RuntimeError(f'owned command failed exit={result.returncode}; executable={args[0]}')
    return result


def tree_hashes(root):
    return {str(file.relative_to(root)): sha(file) for file in root.rglob('*') if file.is_file()}


def input_check(before):
    head = command(['git', '-c', 'safe.directory=' + str(W), '-C', str(W), 'rev-parse', 'HEAD']).stdout.strip()
    tree = command(['git', '-c', 'safe.directory=' + str(W), '-C', str(W), 'rev-parse', 'HEAD^{tree}']).stdout.strip()
    status = command(['git', '-c', 'safe.directory=' + str(W), '-C', str(W), 'status', '--short']).stdout
    assert head == HEAD and tree == TREE and status == before['status']
    assert before['tracked'] and before['external']
    assert set(command(['git', '-c', 'safe.directory=' + str(W), '-C', str(W), 'ls-files', '-z']).stdout.split('\0')[:-1]) == set(before['tracked'])
    tracked = {name: sha(W / name) for name in before['tracked']}
    external = {name: sha(name) for name in before['external']}
    untracked = {name: sha(W / name) for name in before['untracked']}
    assert tracked == before['tracked'] and external == before['external'] and untracked == before['untracked']
    return {'head': head, 'tree': tree, 'status': status,
            'tracked': tracked, 'external': external, 'untracked': untracked}


def owned_rows(name):
    result = command(['docker', 'top', name, '-eo', 'pid,ppid,uid,gid,args'])
    rows = []
    for line in result.stdout.splitlines()[1:]:
        pid, parent, uid, gid, args = line.split(None, 4)
        try:
            status = Path(f'/proc/{pid}/status').read_text()
            stat = Path(f'/proc/{pid}/stat').read_text()
        except FileNotFoundError:
            # The daemon observed this owned short-lived child before it reaped.
            rows.append({'pid': int(pid), 'parent': int(parent), 'uid': int(uid), 'gid': int(gid),
                         'args': args, 'already_retired_at_observation': True})
            continue
        rows.append({'pid': int(pid), 'parent': int(parent), 'uid': int(uid), 'gid': int(gid),
                     'args': args, 'namespace_pid': int(re.search(r'^NSpid:\s+(.+)$', status, re.M)[1].split()[-1]),
                     'start_ticks': stat.rsplit(')', 1)[1].split()[19]})
    return rows


def identity_gone(row):
    if row.get('already_retired_at_observation'):
        return True
    try:
        actual = Path(f"/proc/{row['pid']}/stat").read_text().rsplit(')', 1)[1].split()[19]
    except FileNotFoundError:
        return True
    return actual != row['start_ticks']


# Root's literal current binding replaces the ENTIRE historical BFB/D49 proof.
# Original preparation, restricted role, native/browser assertions and cleanup follow.
before = current['before']
source_before = input_check(before)
build = current['build']
binaries = build['binaries']
assets, abi = current['assets'], current['abi']
server = next(p for p in binaries if p.endswith('/fvoci-server'))
migrate = next(p for p in binaries if p.endswith('/fvoci-migrate'))
fixture = next(p for p in binaries if p.endswith('/fvoci-e2e-fixture'))
engine = next(p for p in binaries if p.endswith('/collab-engine'))
dist = W / 'apps/web/dist'
actual_bundle_hashes = {p:r['sha256'] for p,r in binaries.items()}
assert BUN.is_file() and (W / 'node_modules/.bin/playwright').is_file()


assert os.environ.get('FVOCI_E2E_SELECTED_AUXILIARY') is None, 'BLOCKED: SQLite auxiliary normal writers are not ready'
run = current['run']
run.mkdir(mode=0o700)
write(run / 'source-inputs-before.json', source_before)
dbroot = run / 'database'
storage = run / 'storage'
dbroot.mkdir(mode=0o700)
storage.mkdir(mode=0o700)
db = dbroot / 'app.sqlite'
assert not db.exists()
name = 'fvoci-v060-vue-sqlite-current-' + secrets.token_hex(6)
server_env = {
    'PATH': '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
    'PASSWORD_PEPPER_KEYS': json.dumps({'fixture': secrets.token_hex(32)}),
    'PASSWORD_PEPPER_ACTIVE_KEY_ID': 'fixture',
    'ENCRYPTION_KEYS': json.dumps({'fixture': secrets.token_hex(32)}),
    'ENCRYPTION_ACTIVE_KEY_ID': 'fixture',
    'FVOCI_DATABASE_BACKEND': 'sqlite', 'FVOCI_SQLITE_PATH': '/fvoci/database/app.sqlite',
    'FVOCI_BIND': '127.0.0.1:0', 'FVOCI_PUBLIC_ORIGIN': 'http://127.0.0.1:0',
    'FVOCI_COOKIE_SECURE': '0', 'STORAGE_DRIVER': 'local', 'FVOCI_STORAGE_DIR': '/fvoci/storage',
    'FVOCI_COLLAB_ENGINE': '/fvoci/bin/collab-engine', 'FVOCI_STATIC_DIR': '/srv/fvoci-web',
    'FVOCI_COLLAB_FAMILY_LEASE_MS': '30000', 'FVOCI_COLLAB_FAMILY_RENEW_MS': '5000',
    'FVOCI_COLLAB_MAX_ROOMS': '2', 'RUST_LOG': 'info',
}
for file, body in [(run / 'environment.private.json', json.dumps(server_env)),
                   (run / 'environment.private.sh', ''.join(f'export {key}={shlex.quote(value)}\n' for key, value in server_env.items()))]:
    with file.open('x') as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(body)
receipt = {'source': HEAD, 'tree': TREE, 'compiled_source': COMPILED_HEAD, 'current_binding': str(current['manifest_path']), 'current_binding_sha256': sha(current['manifest_path']), 'root_owner': OWNER, 'started_utc': now(),
           'driver_sha256': sha(__file__), 'container': name, 'image': IMAGE,
           'scope': 'one real SQLite normal migrate--start/current Vue/native ON tracer; PG/Turso/search/OFF/restore/fullCI/shipping image pending',
           'network': 'host network, app bind127.0.0.1:0 only; no network namespace isolation',
           'runtime_abi': abi, 'binary_inputs': {path: binaries[path] for path in (server, migrate, fixture, engine)},
           'source_count': len(before['tracked']), 'external_count': len(before['external']), 'static_count': len(assets['dist_files']),
           'static_build_source': assets['source'], 'environment_names': sorted(server_env),
           'private_environment_file': str(run / 'environment.private.json'),
           'new_owned_dbroot': str(dbroot), 'new_owned_storage': str(storage),
           'host_uid': os.getuid(), 'host_gid': os.getgid(), 'browser_retries': 0, 'workers': 1,
           'original_failure_policy': 'retain raw/traces/database/actor receipts; no reset or relaxed assertions',
           'restart_constraint': 'exact67c checkpoint preserves actor receipt privately and restarts SAME DB/storage; no reset/reseed'}
write(run / 'start.json', receipt)
created = False
server_process = None
server_log = None
base = None
rows = []
server_row = None
browser_inputs = {}
code = 1
driver_phase = 'preparation'
try:
    command(['docker', 'create', '--name', name, '--network', 'host', '--user', '0',
             '--label', 'fvoci.owner=' + OWNER, '--label', 'fvoci.test-run=v060-current-normal-vue-sqlite',
             '--mount', f'type=bind,src={dbroot},dst=/fvoci/database',
             '--mount', f'type=bind,src={storage},dst=/fvoci/storage',
             '--entrypoint', '/bin/sleep', IMAGE, '1800'], run / 'container-create.log')
    created = True
    command(['docker', 'start', name], run / 'container-start.log')
    command(['docker', 'exec', name, '/bin/sh', '-ec',
             'mkdir -p /fvoci/bin /fvoci/inputs /srv/fvoci-web; chmod 0700 /fvoci/inputs; ldd --version | head -1'], run / 'runtime-abi.log')
    copies = [(server, '/fvoci/bin/fvoci-server'), (migrate, '/fvoci/bin/fvoci-migrate'),
              (engine, '/fvoci/bin/collab-engine'), (str(run / 'environment.private.sh'), '/fvoci/inputs/environment.sh')]
    for source, destination in copies:
        command(['docker', 'cp', source, name + ':' + destination])
    command(['docker', 'exec', name, 'chown', '0:0', *[dest for _, dest in copies]])
    command(['docker', 'exec', name, 'chmod', '0755', *[dest for _, dest in copies[:-1]]])
    runtime_ldd = command(['docker', 'exec', name, '/bin/sh', '-ec',
                           '. /etc/os-release; test "$ID" = ubuntu; test "$VERSION_ID" = 26.04; for binary do ldd "$binary"; done',
                           'fvoci-runtime-abi', *[dest for _, dest in copies[:-1]]]).stdout
    (run / 'native-runtime-abi.log').write_text(runtime_ldd)
    assert 'not found' not in runtime_ldd, 'Ubuntu26 runtime ELF dependencies missing'
    command(['docker', 'exec', name, 'chmod', '0600', copies[-1][1]])
    command(['docker', 'cp', str(dist) + '/.', name + ':/srv/fvoci-web'])
    command(['docker', 'exec', name, 'stat', '-c', '%n %u %g %a', *[dest for _, dest in copies]], run / 'copied-owned-files.log')
    hashes = command(['docker', 'exec', name, 'sha256sum', *[dest for _, dest in copies[:-1]]]).stdout
    (run / 'copied-executable-hashes.log').write_text(hashes)
    driver_phase = 'copied-native-hashes'
    assert [line.split()[0] for line in hashes.splitlines()] == [binaries[path]['sha256'] for path in (server, migrate, engine)]
    driver_phase = 'owned-network-mode'
    command(['docker', 'inspect', '--format', '{{.HostConfig.NetworkMode}}', name], run / 'actual-network-mode.log')
    assert (run / 'actual-network-mode.log').read_text().strip() == 'host'
    driver_phase = 'normal-runtime'
    server_log = (run / 'normal-server.log').open('w')
    server_process = subprocess.Popen(['docker', 'exec', name, '/bin/sh', '-ec',
                                      '. /fvoci/inputs/environment.sh; exec /fvoci/bin/fvoci-migrate --start'],
                                     stdin=subprocess.DEVNULL, stdout=server_log, stderr=subprocess.STDOUT)
    deadline = time.monotonic() + 10
    while base is None:
        raw = (run / 'normal-server.log').read_text()
        matched = re.search(r'fvoci-server listening on (http://127\.0\.0\.1:(\d+))', raw)
        if matched:
            base = matched[1]
            break
        assert server_process.poll() is None, 'normal entrypoint exited before listen; see actual raw log'
        assert time.monotonic() < deadline, 'normal entrypoint did not listen within unchanged10s process observation'
        time.sleep(0.02)
    rows = owned_rows(name)
    candidates = [row for row in rows if row['args'] == '/fvoci/bin/fvoci-server' and not row.get('already_retired_at_observation')]
    assert len(candidates) == 1, 'one actual normal server required'
    server_row = candidates[0]
    assert server_row['uid'] == server_row['gid'] == 1000
    meta = db.stat()
    parent = dbroot.stat()
    assert (meta.st_uid, meta.st_gid, meta.st_mode & 0o777, meta.st_nlink) == (1000, 1000, 0o600, 1)
    assert (parent.st_uid, parent.st_gid, parent.st_mode & 0o777) == (1000, 1000, 0o700)
    opener = build_opener(ProxyHandler({}))
    with opener.open(Request(base + '/api/v1/setup'), timeout=10) as response:
        assert response.status == 200 and json.loads(response.read())['needed'] is True
    # Independent read-only observer of actual committed migration receipts.
    # Python's SQLite version/connection PRAGMA are NOT the app runtime/FK proof.
    with sqlite3.connect('file:' + quote(str(db)) + '?mode=ro', uri=True) as observer:
        applied = observer.execute('SELECT version,lineage,sql_sha256 FROM schema_migrations ORDER BY version').fetchall()
    definitions = sorted((W / 'migrations/sqlite').glob('[0-9][0-9][0-9]_*.sql'))
    expected = [(i + 1, 'fvoci-sqlite-current-v1', sha(file)) for i, file in enumerate(definitions)]
    assert applied == expected and len(applied) == 4
    receipt.update(baseURL=base, actual_server=server_row, actual_process_rows_at_ready=rows,
                   database_inode=[meta.st_dev, meta.st_ino], actual_setup_needed=True,
                   actual_migration_rows=applied,
                   runtime_pin_oracle='actual SQLx server/fixture connect path refuses version/source/FK mismatch; successful real fixture later executes its own exact3.53.4/source/FK1/current-schema reads; observer does not qualify its own runtime')
    write(run / 'normal-main-ready.json', receipt)
    browser_env = {'TMPDIR':os.environ['TMPDIR'], 'CI':'true', 'BUN_RUNTIME_TRANSPILER_CACHE_PATH':os.environ['BUN_RUNTIME_TRANSPILER_CACHE_PATH'], 'PATH': os.environ['PATH'], 'LANG': os.environ.get('LANG', 'C.UTF-8'),
                   'PLAYWRIGHT_BASE_URL': base, 'FVOCI_E2E_SELECTED_BACKEND': 'sqlite',
                   'FVOCI_E2E_RESULT_DIR': str(run),
                   'PLAYWRIGHT_JSON_OUTPUT_FILE': str(run / 'playwright-result.private.json'),
                   'FVOCI_E2E_SELECTED_FIXTURE_BIN': fixture,
                   'FVOCI_E2E_SQLITE_RUN_ROOT': str(dbroot), 'FVOCI_E2E_SQLITE_PATH': str(db),
                   'PASSWORD_PEPPER_KEYS': server_env['PASSWORD_PEPPER_KEYS'],
                   'PASSWORD_PEPPER_ACTIVE_KEY_ID': server_env['PASSWORD_PEPPER_ACTIVE_KEY_ID']}
    if os.environ.get('PLAYWRIGHT_BROWSERS_PATH'):
        browser_env['PLAYWRIGHT_BROWSERS_PATH'] = os.environ['PLAYWRIGHT_BROWSERS_PATH']
    chromium = command([str(BUN), '--eval', "import { chromium } from '@playwright/test'; console.log(chromium.executablePath());"],
                       env=browser_env, cwd=W / 'apps/web').stdout.strip()
    assert Path(chromium).is_absolute() and Path(chromium).is_file()
    browser_inputs = {'bun': {'path': str(BUN), 'sha256': sha(BUN)},
                      'chromium': {'path': chromium, 'sha256': sha(chromium)},
                      'chromium_directory_files': tree_hashes(Path(chromium).parent)}
    write(run / 'actual-browser-inputs.json', browser_inputs)
    args = [str(BUN), '--bun', 'x', 'playwright', 'test', '--config', 'e2e-pending/collab-playwright.config.ts', '--reporter=line,json', SPEC]
    receipt.update(browser_command=args, browser_environment_names=sorted(browser_env), browser_start_utc=now())
    write(run / 'browser-start.json', receipt)
    started = time.monotonic()
    result = command(args, run / 'browser.log', required=False, env=browser_env, cwd=W / 'apps/web')
    code = result.returncode
    receipt.update(browser_exit=code, browser_seconds=time.monotonic()-started,
                   browser_end_utc=now(), browser_log_sha256=sha(run / 'browser.log'))
    actors = sorted(dbroot.glob('actor-*.json'))
    receipt['actual_actor_receipts'] = {file.name: json.loads(file.read_text()) for file in actors}
    if code == 0:
        assert len(actors) == 1, 'exactly one fresh selected actor fixture'
        actor = json.loads(actors[0].read_text())
        assert actor['backend'] == 'sqlite' and actor['commit'] == 'confirmed'
        assert actor['poolClosed'] and actor['connectionClose'] == 'confirmed' and actor['operationSucceeded']
        assert re.search(r'\b1 passed\b', (run / 'browser.log').read_text()), 'exact one selected browser test'
        assert (db.stat().st_dev, db.stat().st_ino) == (meta.st_dev, meta.st_ino), 'no replacement/reset DB'
        receipt.update(actual_browser_tests=1, retries=0, ignored=0,
                       actual_fixture_pin_checks_completed=True,
                       tested_product_flow='actual currentVue setup/login/stable wiki create/nonempty nativeON/matching durableACK/manualrevision/fresh cookie actor body-native-ID-permission-history readback')
    if code == 0:
        receipt['current_schema_server_restart'] = restart_same_app(globals())
except BaseException as error:
    receipt['original_driver_failure'] = {'type': type(error).__name__, 'message': str(error)}
    origin = None
    traceback = error.__traceback__
    while traceback is not None:
        if traceback.tb_frame.f_code.co_filename == __file__:
            origin = traceback.tb_lineno
        traceback = traceback.tb_next
    receipt['original_driver_failure_origin'] = {
        'phase': driver_phase, 'driver_sha256': sha(__file__), 'line': origin,
        'type': type(error).__name__ if type(error).__name__ in
            ('AssertionError', 'RuntimeError', 'PermissionError', 'OSError', 'TimeoutExpired') else 'OtherError'}
    code = code or 1
finally:
    # Capture/reap only this named container/server; retain DB/storage and all raw evidence.
    cleanup_errors = []
    if created:
        try:
            rows = owned_rows(name)
            receipt['actual_process_rows_before_cleanup'] = rows
            if server_row is not None and not identity_gone(server_row):
                stopped = command(['docker', 'exec', '--user', '0', name, '/bin/kill', '-TERM', str(server_row['namespace_pid'])], required=False)
                receipt['owned_server_sigterm_exit'] = stopped.returncode
                if stopped.returncode:
                    cleanup_errors.append('owned server SIGTERM failed')
            if server_process is not None:
                try:
                    receipt['normal_server_exit'] = server_process.wait(timeout=10)
                    if receipt['normal_server_exit'] != 0:
                        cleanup_errors.append('normal server exit nonzero; see raw original log')
                except subprocess.TimeoutExpired:
                    cleanup_errors.append('owned server did not finish within unchanged10s process observation; forced container cleanup is not graceful PASS')
            removed = command(['docker', 'rm', '-f', '-v', name], required=False)
            receipt['owned_container_cleanup_exit'] = removed.returncode
            receipt['owned_container_absent'] = command(['docker', 'inspect', name], required=False).returncode != 0
            if removed.returncode or not receipt['owned_container_absent']:
                cleanup_errors.append('owned container cleanup/absence failed')
            receipt['recorded_process_identities_retired'] = all(identity_gone(row) for row in rows)
            if not receipt['recorded_process_identities_retired']:
                cleanup_errors.append('owned recorded process identity remains')
        except BaseException as error:
            cleanup_errors.append(f'cleanup {type(error).__name__}: {error}')
            # Even if inspection failed, do not leak the uniquely owned container.
            removed = command(['docker', 'rm', '-f', '-v', name], required=False)
            receipt['exceptional_force_container_cleanup_exit'] = removed.returncode
            receipt['owned_container_absent'] = command(['docker', 'inspect', name], required=False).returncode != 0
            if removed.returncode or not receipt['owned_container_absent']:
                cleanup_errors.append('exceptional owned container cleanup/absence failed')
    if server_process is not None and server_process.poll() is None:
        try:
            server_process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            server_process.kill()
            server_process.wait()
            cleanup_errors.append('owned docker-exec CLI force-reaped; cannot qualify graceful finish')
    if server_log is not None:
        server_log.close()
        receipt['normal_server_log_sha256'] = sha(run / 'normal-server.log')
    if base is not None:
        port = int(base.rsplit(':', 1)[1])
        with socket.socket() as probe:
            probe.settimeout(1)
            receipt['owned_loopback_port_closed'] = probe.connect_ex(('127.0.0.1', port)) != 0
        if not receipt['owned_loopback_port_closed']:
            cleanup_errors.append('owned loopback port remains open')
    try:
        source_after = input_check(before)
        write(run / 'source-inputs-after.json', source_after)
        assert source_before == source_after
        assert tree_hashes(dist) == assets['dist_files']
        for path in (server, migrate, fixture, engine):
            assert sha(path) == binaries[path]['sha256']
        for path, expected_hash in abi['host_runtime_files'].items():
            assert sha(path) == expected_hash
        if browser_inputs:
            assert sha(BUN) == browser_inputs['bun']['sha256']
            assert sha(browser_inputs['chromium']['path']) == browser_inputs['chromium']['sha256']
            assert tree_hashes(Path(browser_inputs['chromium']['path']).parent) == browser_inputs['chromium_directory_files']
        receipt['exact_source_artifact_inputs_unchanged'] = True
    except BaseException as error:
        receipt['exact_source_artifact_inputs_unchanged'] = False
        cleanup_errors.append(f'post-input check {type(error).__name__}: {error}')
    if cleanup_errors:
        code = code or 1
    receipt.update(cleanup_errors=cleanup_errors, final_exit_code=code, ended_utc=now(),
                   retained_private_evidence=str(run), retained_dbroot=str(dbroot), retained_storage=str(storage))
    write(run / 'receipt.json', receipt)
    print(json.dumps({key: receipt.get(key) for key in ['source', 'final_exit_code', 'actual_browser_tests',
          'browser_exit', 'original_driver_failure', 'owned_container_absent', 'owned_loopback_port_closed',
          'exact_source_artifact_inputs_unchanged', 'retained_private_evidence']}), flush=True)
sys.exit(code)
