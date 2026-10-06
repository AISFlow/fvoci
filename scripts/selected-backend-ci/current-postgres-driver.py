"""Root-owned, one-shot current development PostgreSQL normal-main/current Vue tracer.
Authored by W1; do not execute before root's current heavy lane is closed.
No compilation, package install, product patch, migration reset or external account.
Worker authored only; execute exclusively in root allocated foreground batch.
"""
import datetime
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import shutil
import socket
import subprocess
import sys
import time
import uuid
from urllib.parse import quote, urlsplit, unquote
from urllib.request import ProxyHandler, Request, build_opener

E = Path(os.environ['FVOCI_CI_SELECTED_RUNS'])
W = Path(__file__).resolve().parents[2]
from current_binding import load_current
current = load_current('postgres', __file__)
HEAD = COMPILED_HEAD = current['manifest']['source']
TREE = COMPILED_TREE = current['manifest']['tree']
IMAGE = 'ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7'
# Native origin/hash are recorded separately in the current bundle qualification.
OWNER = os.environ['FVOCI_CI_OWNER']
SPEC = 'workspace-wiki-selected-backend.spec.ts'
BUN = Path(os.environ['FVOCI_CI_BUN'])

# Root must rebind the WHOLE maintained artifact/preparation proof first.
assert HEAD == COMPILED_HEAD, 'auxiliary flow requires exact fresh current compiled source'
assert os.environ.get('FVOCI_E2E_SELECTED_AUXILIARY') == 'normal-api'
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


def browser_failure_diagnostic(run, code, browser_env, browser_inputs):
    """Optional scalar-only evidence; never replace browser exit or retirement proof."""
    report = run / 'playwright-result.private.json'
    log = run / 'browser.log'
    observation = {'schema': 1, 'driver_sha256': sha(__file__),
                   'line': sys._getframe().f_lineno, 'exit': code,
                   'error_kind': 'UNKNOWN', 'source_location': None,
                   'report_exists': report.exists() or report.is_symlink(),
                   'report_state': 'MISSING', 'report_sha256': None,
                   'log_sha256': sha(log) if log.is_file() and not log.is_symlink() else None,
                   'input_observation': {'explicit_cache_path_present': bool(browser_env.get('PLAYWRIGHT_BROWSERS_PATH')),
                       'cache_owned_by_runtime': None, 'cache_matches_preflight': None,
                       'chromium_file_exists': False, 'chromium_owned_by_runtime': None,
                       'bun_sha256': browser_inputs.get('bun', {}).get('sha256'),
                       'chromium_sha256': browser_inputs.get('chromium', {}).get('sha256')}}
    for key in ('bun_sha256', 'chromium_sha256'):
        digest = observation['input_observation'][key]
        if not (type(digest) is str and re.fullmatch('[0-9a-f]{64}', digest)):
            observation['input_observation'][key] = None
    try:
        chromium = Path(browser_inputs['chromium']['path'])
        inputs = observation['input_observation']
        inputs['chromium_file_exists'] = chromium.is_file() and not chromium.is_symlink()
        if inputs['chromium_file_exists']:
            facts = chromium.stat()
            inputs['chromium_owned_by_runtime'] = (facts.st_uid, facts.st_gid) == (1000, 1000)
        cache = browser_env.get('PLAYWRIGHT_BROWSERS_PATH')
        if cache:
            directory = Path(cache)
            inputs['cache_matches_preflight'] = directory == chromium.parent.parent.parent
            if inputs['cache_matches_preflight'] and directory.is_dir() and not directory.is_symlink():
                facts = directory.stat()
                inputs['cache_owned_by_runtime'] = (facts.st_uid, facts.st_gid) == (1000, 1000)
    except (KeyError, TypeError, ValueError, OSError):
        pass
    try:
        first = None
        failed = None
        if observation['report_exists']:
            observation['report_state'] = 'UNREADABLE'
            if report.is_file() and not report.is_symlink() and report.stat().st_size <= 4 * 1024 * 1024:
                observation['report_sha256'] = sha(report)
                observation['report_state'] = 'MALFORMED'
                body = json.loads(report.read_text())
                errors = body.get('errors', [])
                if errors:
                    first = errors[0]
                else:
                    failed = body['suites'][0]['specs'][0]['tests'][0]['results'][0]
                    if failed['status'] in ('failed', 'timedOut') and type(failed['retry']) is int and failed['retry'] == 0:
                        errors = failed.get('errors', [])
                        first = errors[0] if errors else failed.get('error')
                observation['report_state'] = 'AVAILABLE'
        if type(first) is dict:
            message = first.get('message')
            if message == 'Playwright must run under Bun in CI (bun --bun x playwright)':
                observation['error_kind'] = 'BUN_CI_CONFIG_GUARD'
            elif type(message) is str and message.startswith("browserType.launch: Executable doesn't exist at "):
                observation['error_kind'] = 'BROWSER_EXECUTABLE_MISSING'
            location = first.get('location')
            spec = W / 'apps/web/e2e-pending' / SPEC
            if (failed is not None and type(location) is dict and location.get('file') == str(spec) and
                    type(location.get('line')) is int and 1 <= location['line'] <= len(spec.read_text().splitlines()) and
                    sha(spec) == before['tracked'].get('apps/web/e2e-pending/' + SPEC)):
                observation['source_location'] = {'spec_sha256': sha(spec), 'line': location['line']}
                if observation['error_kind'] == 'UNKNOWN':
                    observation['error_kind'] = 'SELECTED_SPEC_TIMEOUT' if failed['status'] == 'timedOut' else 'SELECTED_SPEC_FAILURE'
        elif (observation['report_state'] == 'MISSING' and log.is_file() and not log.is_symlink() and
                log.stat().st_size <= 1024 * 1024 and
                any(line.strip() == 'Error: Playwright must run under Bun in CI (bun --bun x playwright)' for line in log.read_text().splitlines())):
            observation['error_kind'] = 'BUN_CI_CONFIG_GUARD'
    except (KeyError, IndexError, TypeError, AttributeError, ValueError, OSError):
        pass  # UNKNOWN/partial metadata is not a successful browser execution.
    return observation


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


PG_SCRIPT = W / 'scripts/start-test-postgres.sh'
MEILI_SCRIPT = W / 'scripts/start-test-meili.sh'
PG_IMAGE = 'postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db'
MEILI_IMAGE = 'getmeili/meilisearch:v1.53.2@sha256:c94e58ca09662dd6e65e8f1b0fd145767be3da7d5422a863a27b8d2b68e090c9'
assert PG_IMAGE in PG_SCRIPT.read_text() and MEILI_IMAGE in MEILI_SCRIPT.read_text()
assert sha(PG_SCRIPT) == before['tracked']['scripts/start-test-postgres.sh']
assert sha(MEILI_SCRIPT) == before['tracked']['scripts/start-test-meili.sh']
REPORTER_SOURCE = W / 'node_modules/playwright/lib/runner/index.js'
REPORTER_SHA = sha(REPORTER_SOURCE)
assert 'PLAYWRIGHT_${name}_OUTPUT_FILE' in REPORTER_SOURCE.read_text()
assert 'body: a.body?.toString("base64")' in REPORTER_SOURCE.read_text()


def fixture_info(kind):
    name = os.environ['FVOCI_TEST_' + kind.upper() + '_CONTAINER']
    prefix = 'fvoci-rust-test-' + kind + '-'
    assert name.startswith(prefix) and re.fullmatch('[0-9a-f]{32}', name[len(prefix):])
    data = json.loads(command(['docker', 'inspect', name]).stdout)[0]
    assert data['Config']['Image'] == (PG_IMAGE if kind == 'pg' else MEILI_IMAGE)
    assert data['Config']['Labels']['fvoci.test-run'] == name[len(prefix):]
    container_port = '5432/tcp' if kind == 'pg' else '7700/tcp'
    binding = data['NetworkSettings']['Ports'][container_port]
    assert len(binding) == 1 and binding[0]['HostIp'] == '127.0.0.1'
    return {'kind': kind, 'name': name, 'image': data['Config']['Image'],
            'port': int(binding[0]['HostPort']), 'rows': owned_rows(name),
            'volumes': [mount['Name'] for mount in data['Mounts'] if mount['Type'] == 'volume']}


def verify_fixtures_closed(run):
    result = []
    for file in sorted(run.glob('fixture-*-ready.json')):
        data = json.loads(file.read_text())
        absent = owned_object_absent(['docker', 'inspect', data['name']])
        volumes = {name: owned_object_absent(['docker', 'volume', 'inspect', name])
                   for name in data['volumes']}
        retired = all(identity_gone(row) for row in data['rows'])
        with socket.socket() as probe:
            probe.settimeout(1)
            closed = probe.connect_ex(('127.0.0.1', data['port'])) != 0
        result.append({'kind': data['kind'], 'name': data['name'], 'containerAbsent': absent,
                       'recordedPIDIdentitiesRetired': retired, 'portClosed': closed,
                       'ownedVolumesAbsent': volumes})
    return result


def owned_object_absent(args):
    checked = command(args, required=False)
    return checked.returncode != 0 and any(message in checked.stderr.lower()
           for message in ('no such object', 'no such container', 'no such volume'))


def post_inputs():
    checked = input_check(before)
    assert tree_hashes(dist) == assets['dist_files']
    for path, expected in actual_bundle_hashes.items():
        assert sha(path) == expected
    for path, expected in abi['host_runtime_files'].items():
        assert sha(path) == expected
    assert sha(REPORTER_SOURCE) == REPORTER_SHA
    return checked


assert os.environ.get('FVOCI_ROOT_RUN_OWNER') == OWNER, 'explicit root-owned foreground allocation marker required'
if len(sys.argv) == 1:
    run = current['run']
    run.mkdir(mode=0o700)
    write(run / 'source-inputs-before.json', source_before)
    # The owned fixture scripts re-run this driver (--pg-ready, then --inside) with this
    # filtered environment; keep bytecode suppression explicit so the child never writes
    # scripts/selected-backend-ci/__pycache__ into the checkout before its own status check.
    environment = {'PATH': os.environ['PATH'], 'LANG': os.environ.get('LANG', 'C.UTF-8'),
                   'PYTHONDONTWRITEBYTECODE': '1',
                   'FVOCI_ROOT_RUN_OWNER': OWNER, 'FVOCI_TEST_PG_MAJOR': '18',
                   'FVOCI_E2E_SELECTED_AUXILIARY': 'normal-api',
                   **{k: os.environ[k] for k in ('FVOCI_ROOT_CURRENT_BINDING','FVOCI_ROOT_CURRENT_ALLOCATION','FVOCI_ROOT_RESTART_GRANT','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','CI','GITHUB_ACTIONS','GITHUB_SHA','GITHUB_REPOSITORY','GITHUB_JOB','FVOCI_CI_OWNER','FVOCI_CI_SELECTED_RUNS','FVOCI_CI_BUN','BUN_RUNTIME_TRANSPILER_CACHE_PATH','TMPDIR')}}
    if os.environ.get('PLAYWRIGHT_BROWSERS_PATH'):
        environment['PLAYWRIGHT_BROWSERS_PATH'] = os.environ['PLAYWRIGHT_BROWSERS_PATH']
    started = now()
    result = command(['bash', str(PG_SCRIPT), sys.executable, str(Path(__file__).resolve()),
                      '--pg-ready', str(run)], run / 'owned-fixtures.log', required=False, env=environment)
    code = result.returncode
    checks = verify_fixtures_closed(run)
    complete = len(checks) == 2 and all(row['containerAbsent'] and row['recordedPIDIdentitiesRetired']
               and row['portClosed'] and all(row['ownedVolumesAbsent'].values()) for row in checks)
    child = json.loads((run / 'receipt.json').read_text()) if (run / 'receipt.json').exists() else None
    if not complete or child is None or child['final_exit_code'] != 0:
        code = code or 1
    try:
        after = post_inputs()
        write(run / 'parent-source-inputs-after.json', after)
        inputs_equal = source_before == after
    except BaseException as error:
        inputs_equal = False
        write(run / 'parent-input-failure.json', {'type': type(error).__name__, 'message': str(error)})
        code = code or 1
    summary = {'source': HEAD, 'tree': TREE, 'compiled_source': COMPILED_HEAD,
               'root_owner': OWNER, 'driver_sha256': sha(__file__), 'started_utc': started,
               'wrapper_exit': result.returncode, 'actual_child_receipt_present': child is not None,
               'fixture_closure': checks, 'all_owned_fixtures_closed': complete,
               'exact_source_artifact_inputs_unchanged': inputs_equal, 'final_exit_code': code,
               'ended_utc': now(), 'retained_private_evidence': str(run),
               'scope': 'one normal restricted PG18/current Vue tracer; no full0.6/Turso/restore/CI/shipping acceptance'}
    write(run / 'parent-receipt.json', summary)
    print(json.dumps(summary), flush=True)
    sys.exit(code)

assert len(sys.argv) == 3 and sys.argv[1] in ('--pg-ready', '--inside')
run = Path(sys.argv[2]).resolve()
assert run == current['run'] and run.parent == E and re.fullmatch('root-current-postgres-[0-9a-f]{12}', run.name)
assert run.is_dir() and run.stat().st_uid == 1000 and run.stat().st_mode & 0o777 == 0o700
if sys.argv[1] == '--pg-ready':
    write(run / 'fixture-pg-ready.json', fixture_info('pg'))
    result = command(['bash', str(MEILI_SCRIPT), sys.executable, str(Path(__file__).resolve()),
                      '--inside', str(run)], run / 'owned-meili-and-tracer.log', required=False)
    sys.exit(result.returncode)

pg = fixture_info('pg')
meili = fixture_info('meili')
write(run / 'fixture-meili-ready.json', meili)
url = urlsplit(os.environ['TEST_DATABASE_URL'])
assert url.scheme == 'postgres' and url.hostname == '127.0.0.1' and url.port == pg['port']
assert url.username == 'postgres' and url.path == '/postgres' and not url.query and not url.fragment
owner_password = unquote(url.password or '')
assert len(owner_password) >= 16
owner_url = os.environ['TEST_DATABASE_URL']
assert os.environ['FVOCI_MEILI_URL'] == 'http://127.0.0.1:' + str(meili['port'])
master_key = os.environ['MEILI_MASTER_KEY']
assert master_key == os.environ['FVOCI_MEILI_KEY'] and len(master_key) >= 16
role = 'fvoci_v060_app_' + secrets.token_hex(8)
app_password = secrets.token_hex(24)
assert app_password != owner_password
app_url = f'postgres://{role}:{app_password}@127.0.0.1:{pg["port"]}/postgres'


def pg_sql(sql, app=False):
    # Password stays in the owned subprocess environment, never command text/logs.
    environment = dict(os.environ)
    environment['PGPASSWORD'] = app_password if app else owner_password
    result = subprocess.run(['docker', 'exec', '-i', '-e', 'PGPASSWORD', pg['name'],
                             'psql', '-h', '127.0.0.1', '-U', role if app else 'postgres',
                             '-d', 'postgres', '-X', '-qAt', '-v', 'ON_ERROR_STOP=1'],
                            input=sql, capture_output=True, text=True, env=environment)
    if result.returncode:
        # Preserve actual diagnostics privately; no credential/environment dump.
        with (run / 'postgres-query-errors.log').open('a') as output:
            output.write(result.stderr)
        raise RuntimeError(f'owned PostgreSQL diagnostic failed exit={result.returncode}')
    return json.loads(result.stdout.strip())


storage = run / 'storage'
storage.mkdir(mode=0o700)
name = 'fvoci-v060-vue-pg-current-' + secrets.token_hex(6)
server_env = {
    'PATH': '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
    'PASSWORD_PEPPER_KEYS': json.dumps({'fixture': secrets.token_hex(32)}),
    'PASSWORD_PEPPER_ACTIVE_KEY_ID': 'fixture',
    'ENCRYPTION_KEYS': json.dumps({'fixture': secrets.token_hex(32)}),
    'ENCRYPTION_ACTIVE_KEY_ID': 'fixture', 'FVOCI_DATABASE_BACKEND': 'postgres',
    'POSTGRES_USER': 'postgres', 'POSTGRES_DB': 'postgres', 'POSTGRES_PASSWORD': owner_password,
    'FVOCI_DB_HOST': '127.0.0.1:' + str(pg['port']), 'FVOCI_APP_ROLE': role,
    'FVOCI_APP_PASSWORD': app_password, 'DATABASE_APP_URL': app_url,
    'MEILI_MASTER_KEY': master_key, 'FVOCI_MEILI_URL': os.environ['FVOCI_MEILI_URL'],
    'FVOCI_MEILI_KEY_FILE': '/run/fvoci/meili/api_key',
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
receipt = {'source': HEAD, 'tree': TREE, 'compiled_source': COMPILED_HEAD,
           'root_owner': OWNER, 'started_utc': now(), 'driver_sha256': sha(__file__),
           'current_binding': str(current['manifest_path']),
           'current_binding_sha256': sha(current['manifest_path']),
           'container': name, 'image': IMAGE, 'postgres_image': PG_IMAGE, 'meili_image': MEILI_IMAGE,
           'network': 'app host network with loopback port0; PG/Meili maintained isolated container loopback port0 mappings; no app network namespace isolation',
           'scope': 'one actual PG18 normal migrate--start/current Vue/native ON tracer, restricted app role and real scoped search/startup consumers; full0.6/Turso/search correctness/OFF/restore/fullCI/shipping pending',
           'runtime_abi': abi, 'actual_cohort_ELF_hashes': actual_bundle_hashes,
           'source_count': len(before['tracked']), 'external_count': len(before['external']), 'static_count': len(assets['dist_files']),
           'actual_json_reporter_source': {'path': str(REPORTER_SOURCE), 'sha256': REPORTER_SHA},
           'static_build_source': assets['source'], 'environment_names': sorted(server_env),
           'new_owned_storage': str(storage), 'host_uid': os.getuid(), 'host_gid': os.getgid(),
           'browser_retries': 0, 'workers': 1, 'application_role': role,
           'credential_separation': 'normal prepare creates NOSUPERUSER/NOBYPASSRLS role; existing exec_server removes preparation-only credentials, app role pool is independently observed; owner URL only in private provisioning fixture process',
           'original_failure_policy': 'retain raw logs/traces and bounded native/ID/receipt diagnostic without auth secret values; no reset/relaxed assertions'}
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
             '--label', 'fvoci.owner=' + OWNER, '--label', 'fvoci.test-run=v060-current-normal-vue-pg',
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
    hashes = command(['docker', 'exec', name, 'sha256sum', *[dest for _, dest in copies[:-1]]]).stdout
    driver_phase = 'copied-native-hashes'
    assert [line.split()[0] for line in hashes.splitlines()] == [binaries[path]['sha256'] for path in (server, migrate, engine)]
    (run / 'copied-executable-hashes.log').write_text(hashes)
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
    assert len(candidates) == 1
    server_row = candidates[0]
    assert server_row['uid'] == server_row['gid'] == 1000
    opener = build_opener(ProxyHandler({}))
    with opener.open(Request(base + '/api/v1/setup'), timeout=10) as response:
        assert response.status == 200 and json.loads(response.read())['needed'] is True
    flags = pg_sql("""SELECT jsonb_build_object('user',current_user,'version',current_setting('server_version_num'),
      'superuser',r.rolsuper,'bypassrls',r.rolbypassrls,
      'owns_schema',EXISTS(SELECT 1 FROM pg_namespace WHERE nspname='fvoci' AND pg_get_userbyid(nspowner)=current_user),
      'owns_tables',EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci' AND pg_get_userbyid(c.relowner)=current_user),
      'versions',(SELECT jsonb_agg(version ORDER BY version) FROM fvoci.schema_migrations),
      'rls',(SELECT jsonb_object_agg(c.relname,jsonb_build_object('enabled',c.relrowsecurity,'forced',c.relforcerowsecurity))
             FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci'
             AND c.relname IN('documents','document_states','document_collab_updates','wiki_create_commands','revisions')))
      FROM pg_roles r WHERE r.rolname=current_user""", app=True)
    registry = (W / 'src/db/migrate.rs').read_text().split('const MIGRATIONS:', 1)[1].split('];', 1)[0]
    expected_versions = [int(match) for match in re.findall(r'include_str!\("\.\./\.\./migrations/([0-9]{3})_', registry)]
    assert flags['user'] == role and flags['version'] == '180003'
    assert not flags['superuser'] and not flags['bypassrls'] and not flags['owns_schema'] and not flags['owns_tables']
    assert flags['versions'] == expected_versions and expected_versions == list(range(1, 56))
    assert len(flags['rls']) == 5 and all(row['enabled'] for row in flags['rls'].values())
    assert flags['rls']['revisions']['forced'] and flags['rls']['wiki_create_commands']['forced']
    key_metadata = command(['docker', 'exec', name, 'stat', '-c', '%u %g %a', '/run/fvoci/meili/api_key']).stdout.strip()
    assert key_metadata == '0 1000 640'
    scoped_key = command(['docker', 'exec', '--user', '1000', name, 'cat', '/run/fvoci/meili/api_key']).stdout.strip()
    assert len(scoped_key) >= 16 and scoped_key != master_key
    assert 'meilisearch enabled' in (run / 'normal-server.log').read_text()
    assert 'outbox dispatcher started' in (run / 'normal-server.log').read_text()
    receipt.update(baseURL=base, actual_server=server_row, actual_process_rows_at_ready=rows,
                   actual_setup_needed=True, actual_restricted_role_schema_rls=flags,
                   actual_scoped_search_key_metadata=key_metadata,
                   actual_scoped_search_key_distinct_from_master=True, actual_search_and_outbox_started=True)
    write(run / 'normal-main-ready.json', receipt)
    browser_env = {'TMPDIR':os.environ['TMPDIR'], 'CI':'true', 'BUN_RUNTIME_TRANSPILER_CACHE_PATH':os.environ['BUN_RUNTIME_TRANSPILER_CACHE_PATH'], 'PATH': os.environ['PATH'], 'LANG': os.environ.get('LANG', 'C.UTF-8'),
                   'PLAYWRIGHT_BASE_URL': base, 'FVOCI_E2E_SELECTED_BACKEND': 'postgres',
                   'FVOCI_E2E_SELECTED_AUXILIARY': 'normal-api',
                   'FVOCI_E2E_SELECTED_SOURCE': HEAD, 'FVOCI_E2E_SELECTED_COMPILED_SOURCE': COMPILED_HEAD,
                   'FVOCI_E2E_RESULT_DIR': str(run), 'FVOCI_E2E_ADMIN_DATABASE_URL': owner_url,
                   'PLAYWRIGHT_JSON_OUTPUT_FILE': str(run / 'playwright-result.private.json'),
                   'CARGO_TARGET_DIR': str(Path(fixture).parent.parent),
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
    args = [str(BUN), '--bun', 'x', 'playwright', 'test', '--config', 'e2e-pending/collab-playwright.config.ts',
            '--reporter=line,json', SPEC]
    receipt.update(browser_command=args, browser_environment_names=sorted(browser_env), browser_start_utc=now())
    write(run / 'browser-start.json', receipt)
    started = time.monotonic()
    driver_phase = 'browser-run'
    result = command(args, run / 'browser.log', required=False, env=browser_env, cwd=W / 'apps/web')
    code = result.returncode
    if code != 0:
        receipt['browser_failure_diagnostic'] = browser_failure_diagnostic(run, code, browser_env, browser_inputs)
    if code != 0:
        receipt['original_browser_failure'] = {
            'phase': driver_phase, 'driver_sha256': sha(__file__), 'line': sys._getframe().f_lineno,
            'exit': code, 'log_sha256': sha(run / 'browser.log'),
            'report_sha256': sha(run / 'playwright-result.private.json') if (run / 'playwright-result.private.json').exists() else None,
            'assertion_location': None}
        # Only the first actual JSON error at the exact committed selected spec.
        # Missing/foreign locations stay unknown; never derive one from a stack/message.
        try:
            failed = json.loads((run / 'playwright-result.private.json').read_text())['suites'][0]['specs'][0]['tests'][0]['results'][0]
            errors = failed.get('errors', [])
            first = errors[0] if errors else failed.get('error')
            location = first.get('location') if type(first) is dict else None
            spec = W / 'apps/web/e2e-pending' / SPEC
            if (failed['status'] in ('failed', 'timedOut') and type(failed['retry']) is int and failed['retry'] == 0 and
                    type(location) is dict and location.get('file') == str(spec) and
                    type(location.get('line')) is int and 1 <= location['line'] <= len(spec.read_text().splitlines()) and
                    sha(spec) == before['tracked'].get('apps/web/e2e-pending/' + SPEC)):
                receipt['original_browser_failure']['assertion_location'] = {'spec_sha256': sha(spec), 'line': location['line']}
        except (KeyError, IndexError, TypeError, AttributeError, ValueError, OSError):
            pass  # Diagnostic unavailable; the original nonzero browser status remains.
    receipt.update(browser_exit=code, browser_seconds=time.monotonic()-started,
                   browser_end_utc=now(), browser_log_sha256=sha(run / 'browser.log'))
    if (run / 'playwright-result.private.json').exists():
        os.chmod(run / 'playwright-result.private.json', 0o600)
        receipt['actual_json_report_sha256'] = sha(run / 'playwright-result.private.json')
    driver_phase = 'durable-readback'
    if code == 0:
        assert re.search(r'\b1 passed\b', (run / 'browser.log').read_text())
        report = json.loads((run / 'playwright-result.private.json').read_text())
        assert report['config']['workers'] == 1 and report['errors'] == []
        assert report['stats']['expected'] == 1 and all(report['stats'][field] == 0 for field in ('unexpected','flaky','skipped'))
        assert len(report['suites']) == 1 and len(report['suites'][0]['specs']) == 1
        cases = report['suites'][0]['specs'][0]['tests']
        assert len(cases) == 1 and len(cases[0]['results']) == 1
        actual = cases[0]['results'][0]
        assert actual['status'] == 'passed' and actual['retry'] == 0
        attachments = [entry for entry in actual['attachments'] if entry['name'] == 'selected-vue-native-readback.json']
        assert len(attachments) == 1 and attachments[0]['contentType'] == 'application/json'
        tracer = json.loads(base64.b64decode(attachments[0]['body'], validate=True))
        assert tracer['selected'] == 'postgres' and len(tracer['canonicalEmojiOracleControls']) == 6
        assert len(tracer['nativeHistoryOracleControls']) == 2
        assert tracer['firstAck'] != tracer['finalAck'] and tracer['creatorId'] != tracer['freshActorId']
        write(run / 'selected-vue-native-readback.private.json', tracer)
        os.chmod(run / 'selected-vue-native-readback.private.json', 0o600)
        receipt['typed_tracer_receipt_sha256'] = sha(run / 'selected-vue-native-readback.private.json')
        aux_entries = [a for a in actual['attachments'] if a['name'] == 'selected-wiki-auxiliary-mounted.json']
        assert len(aux_entries) == 1 and aux_entries[0]['contentType'] == 'application/json'
        aux = json.loads(base64.b64decode(aux_entries[0]['body'], validate=True))
        assert aux['source'] == aux['compiledSource'] == HEAD
        assert aux['workspaceId'] == tracer['workspaceId'] and aux['documentId'] == tracer['document']['id']
        assert aux['readerId'] == tracer['freshActorId'] and aux['nativeBodyAndManualRevisionUnchanged'] is True
        for phase in ('ownerMounted', 'freshMounted', 'reloadedMounted', 'afterDenialMounted'):
            observed = aux[phase]
            assert [r['consumer'] for r in observed['responses']] == ['tags','task-origins','task-projects','comments']
            assert all(r['status'] == 200 for r in observed['responses'])
            assert all(observed[k]['items'] for k in ('tags','origins','projects','comments'))
        assert len(aux['denials']) == 8 and all(r['status'] == 404 for r in aux['denials'])
        write(run / 'selected-wiki-auxiliary-mounted.private.json', aux)
        os.chmod(run / 'selected-wiki-auxiliary-mounted.private.json', 0o600)
        receipt['typed_auxiliary_receipt_sha256'] = sha(run / 'selected-wiki-auxiliary-mounted.private.json')
        facts = pg_sql("""SELECT jsonb_build_object('workspace',(SELECT id FROM fvoci.workspaces WHERE slug='acme'),
          'actors',(SELECT jsonb_agg(jsonb_build_object('id',u.id,'email',u.email,'role',m.role))
                    FROM fvoci.users u JOIN fvoci.memberships m ON m.user_id=u.id
                    JOIN fvoci.workspaces w ON w.id=m.workspace_id WHERE w.slug='acme'),
          'open_client_roles',(SELECT jsonb_agg(DISTINCT usename) FROM pg_stat_activity
                               WHERE backend_type='client backend' AND pid<>pg_backend_pid()))""")
        tenant = str(uuid.UUID(facts['workspace']))
        assert tenant == tracer['workspaceId'] and tracer['document']['workspaceId'] == tenant
        assert {a['id'] for a in facts['actors']} == {tracer['creatorId'], tracer['freshActorId']}
        assert sorted((a['email'], a['role']) for a in facts['actors']) == [('admin@example.com','owner'),('collab-member@example.com','member')]
        assert facts['open_client_roles'] == [role], 'preparation/actor provisioning owner connections must be closed'
        wrong = str(uuid.uuid4())
        hidden = pg_sql(f"BEGIN READ ONLY; SET LOCAL app.tenant_id='{wrong}'; SELECT jsonb_build_object('documents',(SELECT count(*) FROM fvoci.documents),'commands',(SELECT count(*) FROM fvoci.wiki_create_commands),'revisions',(SELECT count(*) FROM fvoci.revisions)); COMMIT;", app=True)
        assert hidden == {'documents':0,'commands':0,'revisions':0}
        durable = pg_sql(f"""BEGIN READ ONLY; SET LOCAL app.tenant_id='{tenant}';
          SELECT jsonb_build_object('documents',(SELECT count(*) FROM fvoci.documents),
            'commands',(SELECT count(*) FROM fvoci.wiki_create_commands),
            'bound_receipts',(SELECT count(*) FROM fvoci.wiki_create_commands c JOIN fvoci.documents d ON d.id=c.document_id AND d.workspace_id=c.workspace_id),
            'manual_revisions',(SELECT count(*) FROM fvoci.revisions WHERE reason='manual'),
            'native_bytes',(SELECT coalesce(sum(octet_length(state)),0) FROM fvoci.document_states)+(SELECT coalesce(sum(octet_length(payload)),0) FROM fvoci.document_collab_updates),
            'tail',(SELECT max(tail_seq) FROM fvoci.document_states),
            'native_op_receipts',(SELECT count(*) FROM fvoci.document_collab_op_receipts)); COMMIT;""", app=True)
        if os.environ.get('FVOCI_E2E_SELECTED_AUXILIARY') == 'normal-api':
            project_id = str(uuid.UUID(aux['fixture']['project']['id']))
            root_id = str(uuid.UUID(aux['fixture']['project']['rootDocumentId']))
            assert root_id != tracer['document']['id']
            extra = pg_sql(f"BEGIN READ ONLY; SET LOCAL app.tenant_id='{tenant}'; SELECT jsonb_build_object('root',(SELECT id FROM fvoci.documents WHERE workspace_id='{tenant}' AND project_id='{project_id}' AND id='{root_id}' AND parent_id IS NULL AND deleted_at IS NULL),'wiki_count',(SELECT count(*) FROM fvoci.documents WHERE project_id IS NULL),'project_count',(SELECT count(*) FROM fvoci.documents WHERE project_id='{project_id}')); COMMIT;", app=True)
            assert extra == {'root':root_id,'wiki_count':1,'project_count':1}
            assert durable['documents'] == 2
            assert durable['commands'] == durable['bound_receipts'] == durable['manual_revisions'] == 1
            receipt['actual_auxiliary_project_root'] = extra
        else:
            assert durable['documents'] == durable['commands'] == durable['bound_receipts'] == durable['manual_revisions'] == 1
        assert durable['native_bytes'] > 2 and durable['tail'] >= 1 and durable['native_op_receipts'] >= 1
        receipt.update(actual_browser_tests=1, retries=0, ignored=0, actual_actor_and_closed_provisioning=facts,
                       actual_wrong_tenant_hidden=hidden, actual_restricted_role_durable_commit=durable,
                       tested_product_flow='identical actual currentVue setup/login/stable wiki create/nonempty nativeON/matching durableACK/manual DSSV reconstruction/fresh cookie actor and new connection native-body-ID-permission-history readback')
    if code == 0:
        driver_phase = 'server-restart'
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
    # Preserve bounded synthetic tracer state before maintained wrappers remove
    # their private fixture. Do not export password hashes/session tokens/keys.
    try:
        preserved = pg_sql("""SELECT jsonb_build_object('versions',(SELECT jsonb_agg(version ORDER BY version) FROM fvoci.schema_migrations),
          'documents',(SELECT jsonb_agg(to_jsonb(d)) FROM fvoci.documents d),
          'states',(SELECT jsonb_agg(jsonb_build_object('workspace',workspace_id,'document',document_id,'encoding',encoding,'state',encode(state,'base64'),'tail',tail_seq,'cutoff',snapshot_cutoff_seq)) FROM fvoci.document_states),
          'updates',(SELECT jsonb_agg(jsonb_build_object('workspace',workspace_id,'document',document_id,'seq',seq,'op',op_id,'payload',encode(payload,'base64'))) FROM fvoci.document_collab_updates),
          'commands',(SELECT jsonb_agg(to_jsonb(c)) FROM fvoci.wiki_create_commands c),
          'revisions',(SELECT jsonb_agg(jsonb_build_object('id',id,'workspace',workspace_id,'target',target_id,'reason',reason,'creator',created_by,'snapshot',encode(y_snapshot,'base64'),'content',content_json)) FROM fvoci.revisions))""")
        write(run / 'retained-native-tracer-state.json', preserved)
        receipt['retained_native_state_sha256'] = sha(run / 'retained-native-tracer-state.json')
    except BaseException as error:
        receipt['native_evidence_preservation_error'] = {'type': type(error).__name__, 'message': str(error)}
        origin = None
        traceback = error.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code.co_filename == __file__:
                origin = traceback.tb_lineno
            traceback = traceback.tb_next
        receipt['native_evidence_preservation_origin'] = {
            'phase': 'native-preservation', 'driver_sha256': sha(__file__), 'line': origin,
            'type': type(error).__name__ if type(error).__name__ in
                ('AssertionError', 'RuntimeError', 'PermissionError', 'OSError', 'TimeoutExpired') else 'OtherError'}
        code = code or 1
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
                    cleanup_errors.append('owned server did not finish within unchanged10s; forced removal is not graceful PASS')
            removed = command(['docker', 'rm', '-f', '-v', name], required=False)
            receipt['owned_container_cleanup_exit'] = removed.returncode
            receipt['owned_container_absent'] = owned_object_absent(['docker', 'inspect', name])
            if removed.returncode or not receipt['owned_container_absent']:
                cleanup_errors.append('owned app container cleanup/absence failed')
            receipt['recorded_process_identities_retired'] = all(identity_gone(row) for row in rows)
            if not receipt['recorded_process_identities_retired']:
                cleanup_errors.append('owned recorded app PID identity remains')
        except BaseException as error:
            cleanup_errors.append(f'cleanup {type(error).__name__}: {error}')
            removed = command(['docker', 'rm', '-f', '-v', name], required=False)
            receipt['exceptional_force_container_cleanup_exit'] = removed.returncode
            receipt['owned_container_absent'] = owned_object_absent(['docker', 'inspect', name])
            if removed.returncode or not receipt['owned_container_absent']:
                cleanup_errors.append('exceptional owned app container cleanup/absence failed')
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
        with socket.socket() as probe:
            probe.settimeout(1)
            receipt['owned_loopback_port_closed'] = probe.connect_ex(('127.0.0.1', int(base.rsplit(':',1)[1]))) != 0
        if not receipt['owned_loopback_port_closed']:
            cleanup_errors.append('owned app loopback port remains open')
    try:
        after = post_inputs()
        write(run / 'source-inputs-after.json', after)
        assert after == source_before
        if browser_inputs:
            assert sha(BUN) == browser_inputs['bun']['sha256']
            assert sha(browser_inputs['chromium']['path']) == browser_inputs['chromium']['sha256']
            assert tree_hashes(Path(browser_inputs['chromium']['path']).parent) == browser_inputs['chromium_directory_files']
        receipt['exact_source_artifact_inputs_unchanged'] = True
    except BaseException as error:
        receipt['exact_source_artifact_inputs_unchanged'] = False
        origin = None
        traceback = error.__traceback__
        while traceback is not None:
            if traceback.tb_frame.f_code.co_filename == __file__:
                origin = traceback.tb_lineno
            traceback = traceback.tb_next
        receipt['post_input_failure_origin'] = {
            'phase': 'post-input-check', 'driver_sha256': sha(__file__), 'line': origin,
            'type': type(error).__name__ if type(error).__name__ in
                ('AssertionError', 'RuntimeError', 'PermissionError', 'OSError', 'TimeoutExpired') else 'OtherError'}
        cleanup_errors.append(f'post-input check {type(error).__name__}: {error}')
    if cleanup_errors:
        code = code or 1
    receipt.update(cleanup_errors=cleanup_errors, final_exit_code=code, ended_utc=now(),
                   retained_private_evidence=str(run), retained_storage=str(storage))
    write(run / 'receipt.json', receipt)
    print(json.dumps({key: receipt.get(key) for key in ['source','final_exit_code','actual_browser_tests',
          'browser_exit','original_driver_failure','owned_container_absent','owned_loopback_port_closed',
          'exact_source_artifact_inputs_unchanged','retained_private_evidence']}), flush=True)
sys.exit(code)
