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
from current_binding import load_current, validate_off_report
current = load_current('sqlite', __file__)
HEAD = COMPILED_HEAD = current['manifest']['source']
TREE = COMPILED_TREE = current['manifest']['tree']
IMAGE = 'ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7'
# Native origin/hash are recorded separately in the current bundle qualification.
OWNER = os.environ['FVOCI_CI_OWNER']
FLOW = current['manifest'].get('flow', 'on')
assert FLOW in ('on', 'off')
SPEC = 'workspace-off-selected-backend.spec.ts' if FLOW == 'off' else 'workspace-wiki-selected-backend.spec.ts'
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


def failure_checkpoint(receipt, directory, observed_exit, error=None, body_log=None, packet_name='original-failure.private.json'):
    """Persist this driver's first actual outcome before any risky cleanup."""
    if 'original_driver_failure' not in receipt:
        receipt['failed_phase'] = receipt['phase']
        receipt['observed_failed_exit'] = observed_exit
        receipt['original_driver_failure'] = ({'type': type(error).__name__, 'message': str(error)[:4096]}
            if error is not None else {'type': 'ReturnedNonzero', 'phase': receipt['phase'], 'observedExit': observed_exit})
        receipt['failure_code'] = 'SELECTED_DRIVER_EXCEPTION' if error is not None else 'SELECTED_BODY_NONZERO'
        receipt['original_body_log_sha256'] = None
        if body_log is not None:
            try:
                receipt['original_body_log_sha256'] = sha(body_log)
            except BaseException:
                receipt.setdefault('diagnostic_errors', []).append('original-body-log-hash-failed')
    path = directory / packet_name
    try:
        if not path.exists():
            with path.open('x') as output:
                os.fchmod(output.fileno(), 0o600)
                json.dump({key: receipt.get(key) for key in ('failed_phase', 'observed_failed_exit',
                    'failure_code', 'original_driver_failure', 'original_body_log_sha256')}, output)
        receipt['original_failure_checkpoint_sha256'] = sha(path)
    except BaseException:
        receipt.setdefault('diagnostic_errors', []).append('original-failure-checkpoint-write-failed')


def cleanup_attempt(receipt, errors, label, operation):
    try:
        return operation()
    except BaseException as error:
        errors.append(label)
        receipt.setdefault('secondary_cleanup_errors', []).append({
            'phase': label, 'type': type(error).__name__, 'message': str(error)[:4096]})
        return None


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
    'FVOCI_REALTIME_MODE': FLOW,
}
for file, body in [(run / 'environment.private.json', json.dumps(server_env)),
                   (run / 'environment.private.sh', ''.join(f'export {key}={shlex.quote(value)}\n' for key, value in server_env.items()))]:
    with file.open('x') as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(body)
receipt = {'source': HEAD, 'tree': TREE, 'compiled_source': COMPILED_HEAD, 'current_binding': str(current['manifest_path']), 'current_binding_sha256': sha(current['manifest_path']), 'root_owner': OWNER, 'started_utc': now(),
           'driver_sha256': sha(__file__), 'selected_flow': FLOW, 'container': name, 'image': IMAGE,
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
receipt.update(phase='container-prepare', owned_container_absent=None,
               owned_loopback_port_closed=None, loopback_port_observation='not-observed',
               recorded_process_identities_retired=None)
write(run / 'start.json', receipt)
created = False
server_process = None
server_log = None
base = None
rows = []
server_row = None
browser_inputs = {}
code = 1
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
    assert [line.split()[0] for line in hashes.splitlines()] == [binaries[path]['sha256'] for path in (server, migrate, engine)]
    command(['docker', 'inspect', '--format', '{{.HostConfig.NetworkMode}}', name], run / 'actual-network-mode.log')
    assert (run / 'actual-network-mode.log').read_text().strip() == 'host'
    receipt['phase'] = 'server-startup'
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
    receipt.update(phase='server-ready', loopback_port_observation='observed')
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
    definitions = sorted((W / 'migrations/sqlite/060').glob('[0-9][0-9]_*.sql'))
    expected = [(i + 1, 'fvoci-sqlite-060', sha(file)) for i, file in enumerate(definitions)]
    # The compiled registry (name, text, digest) is the authority: files, registry and receipts agree.
    registry = (W / 'src/db/migrate.rs').read_text().split('const SQLITE_STEPS:', 1)[1].split('];', 1)[0]
    registry_steps = re.findall(r'"([0-9]{2}_[a-z_]+)",\s*include_str!\("\.\./\.\./migrations/sqlite/060/([0-9]{2}_[a-z_]+)\.sql"\),\s*"([0-9a-f]{64})"', registry)
    assert [name for name, _, _ in registry_steps] == [file.stem for file in definitions]
    assert all(name == file for name, file, _ in registry_steps)
    assert [digest for _, _, digest in registry_steps] == [digest for _, _, digest in expected]
    assert applied == expected and len(applied) == len(registry_steps) == 12
    receipt.update(baseURL=base, actual_server=server_row, actual_process_rows_at_ready=rows,
                   database_inode=[meta.st_dev, meta.st_ino], actual_setup_needed=True,
                   actual_migration_rows=applied,
                   runtime_pin_oracle='actual SQLx server/fixture connect path refuses version/source/FK mismatch; successful real fixture later executes its own exact3.53.4/source/FK1/current-schema reads; observer does not qualify its own runtime')
    write(run / 'normal-main-ready.json', receipt)
    browser_env = {'TMPDIR':os.environ['TMPDIR'], **({'CI':'true'} if current['grant'].get('executionMode') != 'orca-local' else {}), 'BUN_RUNTIME_TRANSPILER_CACHE_PATH':os.environ['BUN_RUNTIME_TRANSPILER_CACHE_PATH'], 'PATH': os.environ['PATH'], 'LANG': os.environ.get('LANG', 'C.UTF-8'),
                   'PLAYWRIGHT_BASE_URL': base, 'FVOCI_E2E_SELECTED_BACKEND': 'sqlite',
                   'FVOCI_E2E_SELECTED_FLOW': FLOW,
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
    receipt['phase'] = 'browser'
    receipt.update(browser_command=args, browser_environment_names=sorted(browser_env), browser_start_utc=now())
    write(run / 'browser-start.json', receipt)
    started = time.monotonic()
    result = command(args, run / 'browser.log', required=False, env=browser_env, cwd=W / 'apps/web')
    code = result.returncode
    if code != 0:
        failure_checkpoint(receipt, run, code, body_log=run / 'browser.log')
    if (run / 'playwright-result.private.json').exists():
        os.chmod(run / 'playwright-result.private.json', 0o600)
        receipt['actual_json_report_sha256'] = sha(run / 'playwright-result.private.json')
    receipt.update(browser_exit=code, browser_seconds=time.monotonic()-started,
                   browser_end_utc=now(), browser_log_sha256=sha(run / 'browser.log'))
    actors = sorted(dbroot.glob('actor-*.json'))
    receipt['actual_actor_receipts'] = {file.name: json.loads(file.read_text()) for file in actors}
    if code == 0:
        assert len(actors) == 1, 'exactly one fresh selected actor fixture'
        actor = json.loads(actors[0].read_text())
        assert actor['backend'] == 'sqlite' and actor['commit'] == 'confirmed'
        assert actor['poolClosed'] and actor['connectionClose'] == 'confirmed' and actor['operationSucceeded']
        if FLOW == 'on':
            assert re.search(r'\b1 passed\b', (run / 'browser.log').read_text()), 'exact one selected browser test'
        else:
            receipt['actual_off_titles'] = validate_off_report(json.loads((run / 'playwright-result.private.json').read_text()), 'sqlite')
        assert (db.stat().st_dev, db.stat().st_ino) == (meta.st_dev, meta.st_ino), 'no replacement/reset DB'
        receipt.update(actual_browser_tests=7 if FLOW == 'off' else 1, retries=0, ignored=0,
                       actual_fixture_pin_checks_completed=True,
                       tested_product_flow=('immutable OFF7 actual Vue CAS/replay/native history/task/note/owner-transition/current revoke' if FLOW == 'off' else 'actual currentVue setup/login/stable wiki create/nonempty nativeON/matching durableACK/manualrevision/fresh cookie actor body-native-ID-permission-history readback'))
    if code == 0 and FLOW == 'on':
        receipt['phase'] = 'restart'
        receipt['current_schema_server_restart'] = restart_same_app(globals())
except BaseException as error:
    failure_checkpoint(receipt, run, receipt.get('browser_exit'), error)
    code = code or 1
finally:
    # The original packet exists before any observation/removal/hash can fail.
    cleanup_errors = list(receipt.get('diagnostic_errors', []))
    rows = None
    if created:
        rows = cleanup_attempt(receipt, cleanup_errors, 'process-observation-failed', lambda: owned_rows(name))
        receipt['actual_process_rows_before_cleanup'] = rows
        if server_row is not None:
            gone = cleanup_attempt(receipt, cleanup_errors, 'server-identity-observation-failed', lambda: identity_gone(server_row))
            if gone is False:
                stopped = cleanup_attempt(receipt, cleanup_errors, 'server-sigterm-failed', lambda: command(
                    ['docker', 'exec', '--user', '0', name, '/bin/kill', '-TERM', str(server_row['namespace_pid'])], required=False))
                if stopped is not None:
                    receipt['owned_server_sigterm_exit'] = stopped.returncode
                    if stopped.returncode: cleanup_errors.append('owned-server-sigterm-nonzero')
        if server_process is not None:
            receipt['normal_server_exit'] = cleanup_attempt(receipt, cleanup_errors, 'normal-server-wait-failed', lambda: server_process.wait(timeout=10))
            if receipt['normal_server_exit'] != 0: cleanup_errors.append('normal-server-finish-unconfirmed')
        removed = cleanup_attempt(receipt, cleanup_errors, 'owned-container-removal-failed', lambda: command(['docker','rm','-f','-v',name], required=False))
        if removed is None:
            removed = cleanup_attempt(receipt, cleanup_errors, 'exceptional-owned-removal-failed', lambda: command(['docker','rm','-f','-v',name], required=False))
        receipt['owned_container_cleanup_exit'] = removed.returncode if removed is not None else None
        absent = cleanup_attempt(receipt, cleanup_errors, 'owned-container-absence-failed', lambda: command(['docker', 'inspect', name], required=False))
        if absent is not None:
            receipt['owned_container_absent'] = absent.returncode != 0 and any(
                marker in absent.stderr.lower() for marker in ('no such object', 'no such container'))
        if removed is None or removed.returncode or receipt['owned_container_absent'] is not True:
            cleanup_errors.append('owned-container-cleanup-unconfirmed')
        if rows is not None:
            try:
                receipt['recorded_process_identities_retired'] = bool(rows) and all(identity_gone(row) for row in rows)
            except BaseException as error:
                cleanup_errors.append('pid-retirement-observation-failed')
                receipt.setdefault('secondary_cleanup_errors', []).append({'phase': 'pid-retirement-observation-failed', 'type': type(error).__name__, 'message': str(error)[:4096]})
        if receipt['recorded_process_identities_retired'] is not True:
            cleanup_errors.append('owned-pid-retirement-unconfirmed')
    if server_process is not None:
        running = cleanup_attempt(receipt, cleanup_errors, 'docker-exec-poll-failed', lambda: server_process.poll() is None)
        if running is True:
            waited = cleanup_attempt(receipt, cleanup_errors, 'docker-exec-wait-failed', lambda: server_process.wait(timeout=10))
            if waited is None:
                cleanup_attempt(receipt, cleanup_errors, 'docker-exec-kill-failed', server_process.kill)
                cleanup_attempt(receipt, cleanup_errors, 'docker-exec-force-wait-failed', lambda: server_process.wait(timeout=10))
                cleanup_errors.append('docker-exec-force-reap-not-normal-close')
    if server_log is not None:
        cleanup_attempt(receipt, cleanup_errors, 'server-log-close-failed', server_log.close)
        receipt['normal_server_log_sha256'] = cleanup_attempt(receipt, cleanup_errors, 'server-log-hash-failed', lambda: sha(run / 'normal-server.log'))
    if base is not None:
        def observe_port():
            with socket.socket() as probe:
                probe.settimeout(1)
                return probe.connect_ex(('127.0.0.1', int(base.rsplit(':',1)[1]))) != 0
        receipt['owned_loopback_port_closed'] = cleanup_attempt(receipt, cleanup_errors, 'loopback-port-observation-failed', observe_port)
        if receipt['owned_loopback_port_closed'] is not True:
            cleanup_errors.append('owned-loopback-port-closure-unconfirmed')
    if base is None:
        cleanup_errors.append('owned loopback port never observed; retirement remains unqualified')
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
        cleanup_errors.append('post-input-check-failed')
        receipt.setdefault('secondary_cleanup_errors', []).append({'phase':'post-input-check-failed', 'type':type(error).__name__, 'message':str(error)[:4096]})
    receipt['ended_utc'] = cleanup_attempt(receipt, cleanup_errors, 'end-clock-observation-failed', now)
    if cleanup_errors: code = code or 1
    receipt.update(cleanup_errors=cleanup_errors, final_exit_code=code,
                   retained_private_evidence=str(run), retained_dbroot=str(dbroot), retained_storage=str(storage))
    cleanup_attempt(receipt, cleanup_errors, 'final-receipt-write-failed', lambda: write(run / 'receipt.json', receipt))
    if cleanup_errors: code = code or 1
    summary={key:receipt.get(key) for key in ('source','final_exit_code','actual_browser_tests','browser_exit',
             'owned_container_absent','owned_loopback_port_closed','recorded_process_identities_retired',
             'loopback_port_observation','failed_phase','observed_failed_exit','original_body_log_sha256',
             'original_failure_checkpoint_sha256','exact_source_artifact_inputs_unchanged')}
    original=receipt.get('original_driver_failure')
    summary.update(failure_code='SELECTED_DRIVER_FAILED' if code else None, final_exit_code=code,
        cleanup_failure_codes=cleanup_errors,
        original_driver_failure_sha256=hashlib.sha256(json.dumps(original,sort_keys=True).encode()).hexdigest() if original is not None else None)
    try:
        print(json.dumps(summary),flush=True)
    except BaseException:
        code = code or 1

sys.exit(code)
