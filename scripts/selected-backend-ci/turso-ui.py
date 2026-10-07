#!/usr/bin/env python3
"""One hosted current-primary UI consumer; credentials never enter browsers.

Local preparation is credential-free. Runtime admission belongs to the existing
guard and ROOT's exact current dataset binding. No reset/restore cleanup exists.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shlex
import signal
import socket
import stat
import subprocess
import sys
import time
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


def clean_env():
    return {key: os.environ[key] for key in ('PATH', 'LANG', 'LD_LIBRARY_PATH', 'SSL_CERT_FILE',
                'SSL_CERT_DIR', 'TZ', 'PLAYWRIGHT_BROWSERS_PATH', 'BUN_RUNTIME_TRANSPILER_CACHE_PATH')
            if key in os.environ}


def freeze():
    before = private_read(root() / 'source-before.json', 4 * 1024 * 1024)
    require(before == hosted_identity(), 'UI_BUILD_INPUTS_CHANGED')
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
    manifest = {'schema': 1, 'sourceInputs': before, 'binaries': binaries, 'assets': assets, 'abi': abi,
                'bun': {'path': bun, 'sha256': digest(bun), 'version': call([bun, '--version'])},
                'chromium': chromium, 'browserFiles': browser_files,
                'sqliteInputs': {'path': str(Path(os.environ['RUNNER_TEMP']) / 'fvoci-sqlite/consumer-inputs.json'),
                                 'sha256': digest(Path(os.environ['RUNNER_TEMP']) / 'fvoci-sqlite/consumer-inputs.json')},
                'rustc': call(['rustc', '-vV'])}
    require(manifest['bun']['version'] == '1.4.2', 'UI_BUN_PIN_MISMATCH')
    write(root() / 'current-build.json', manifest)


def current_build():
    manifest = private_read(root() / 'current-build.json', 8 * 1024 * 1024)
    require(manifest['sourceInputs'] == hosted_identity(), 'UI_SOURCE_CHANGED')
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


def fixture(manifest, mode, environment, input=None):
    env = clean_env()
    env.update({k: environment[k] for k in ('FVOCI_LIBSQL_URL', 'FVOCI_LIBSQL_AUTH_TOKEN',
        'PASSWORD_PEPPER_KEYS', 'PASSWORD_PEPPER_ACTIVE_KEY_ID', 'FVOCI_E2E_TURSO_NAMESPACE',
        'FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE', 'FVOCI_TEST_TURSO_DESTRUCTIVE') if k in environment})
    env.update(E2E_DATABASE_BACKEND='libsql-remote', FVOCI_E2E_TURSO_UI_SELECTED='1')
    result = subprocess.run([manifest['binaries']['fvoci-e2e-fixture']['path'], mode], env=env,
                input=canonical(input) if input else b'', stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=120)
    # Never reflect returned SDK errors or malformed output into hosted logs.
    require(result.returncode == 0 and len(result.stdout) < 64 * 1024 * 1024, 'UI_NATIVE_FIXTURE_FAILED')
    value = json.loads(result.stdout)
    require(value['lifecycleDrain'] == 'confirmed' and value['leases'] == 0
            and value['serverCloseReceipt'] == 'not-exposed-by-sdk', 'UI_NATIVE_DRAIN_FAILED')
    return value


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


def audit_counters(before, after, audit):
    b, a = before['operations'], after['operations']
    require(before['startupHazards'] == 0 and after['startupHazards'] == 0
            and after['liveOutboxLeases'] == 0, 'UI_FOREIGN_OR_UNRELEASED_OWNER')
    # Namespace identifiers come from exact native rows, rather than accepting
    # every added workspace/user as one of the synthetic test allocations.
    workspaces = {row[0][1] for row in a['workspaces'] if row[0][0] == 'blob'
                  and row[1] in [['text', ns] for ns in audit['namespaces']]}
    actors = {row[0][1] for row in a['users'] if row[0][0] == 'blob'
              and row[1] in [['text', ns + suffix + '@example.invalid']
                             for ns in audit['namespaces'] for suffix in ('-owner', '-member')]}
    require(len(workspaces) == len(audit['namespaces']) and len(actors) >= len(workspaces),
            'UI_COUNTER_ALLOCATION_MISSING')
    first, last = singleton(b['event_sequence']), singleton(a['event_sequence'])
    require(first <= last <= first + 10000, 'UI_EVENT_COUNTER_RESET')
    added_events = [row for row in a['events'] if integer(row[0]) > first]
    require([integer(row[0]) for row in added_events] == list(range(first + 1, last + 1)),
            'UI_EVENT_COUNTER_UNEXPLAINED')
    for row in added_events:
        require(len(row) == 3 and row[1][0] == 'blob' and row[1][1] in workspaces
                and row[2][0] == 'blob' and row[2][1] in actors, 'UI_FOREIGN_EVENT')
    first_fence, last_fence = singleton(b['collab_fence_counter']), singleton(a['collab_fence_counter'])
    require(first_fence <= last_fence <= first_fence + 10000, 'UI_FENCE_COUNTER_RESET')
    fences = set()
    for row in audit['observedFences'] + a['collab_room_fences'] + a['task_collab_room_fences']:
        value = integer(row[3])
        if value >= first_fence:
            require(len(row) == 5 and row[0][0] == 'blob' and row[0][1] in workspaces
                    and row[2][0] == 'blob', 'UI_FOREIGN_ROOM_FENCE')
            fences.add(value)
    # Missing transient allocations fail; monotonic movement alone is no proof.
    require(fences == set(range(first_fence, last_fence)), 'UI_FENCE_COUNTER_UNOBSERVED')
    expected_keys = list(range(1, 10))
    require([integer(row[0]) for row in b['maintenance_job_claims']] == expected_keys
            and [integer(row[0]) for row in a['maintenance_job_claims']] == expected_keys,
            'UI_MAINTENANCE_KEYS_CHANGED')
    require(audit['serverStarts'] in (2, 3), 'UI_START_COUNT_REFUSED')
    for old, new in zip(b['maintenance_job_claims'], a['maintenance_job_claims']):
        key = integer(old[0])
        require(len(old) == 4 and len(new) == 4 and old[1] == new[1] == ['null']
                and old[3] == new[3] == ['null'], 'UI_MAINTENANCE_OWNER_REMAINS')
        require(integer(new[2]) - integer(old[2]) == (audit['serverStarts'] if key in (1, 8, 9) else 0),
                'UI_MAINTENANCE_GENERATION_UNEXPLAINED')
    # Baseline admission requires no old relay rows. All normal consumers must
    # finish their real leases; their new cursors stay within owned event range.
    require(b['outbox_consumers'] == [], 'UI_PREEXISTING_RELAY_REFUSED')
    names = [row[0] for row in a['outbox_consumers']]
    require(sorted(names) == sorted([['text', name] for name in ('notifications', 'mail', 'push', 'webhooks', 'github')]),
            'UI_RELAY_REGISTRY_CHANGED')
    for row in a['outbox_consumers']:
        require(len(row) == 4 and first <= integer(row[1]) <= last
                and row[2] == row[3] == ['null'], 'UI_RELAY_FINISH_UNCONFIRMED')


def assert_preserved(before, after, audit=None):
    require(before['ledger'] == after['ledger'] and before['schemaSha256'] == after['schemaSha256']
            and before['lineage'] == after['lineage'], 'UI_CURRENT_LEDGER_CHANGED')
    require(before['fingerprints'].keys() == after['fingerprints'].keys(), 'UI_CURRENT_TABLES_CHANGED')
    for table, rows in before['fingerprints'].items():
        if audit is not None and table in AUDITED_COUNTERS:
            continue
        for sha, count in rows.items():
            require(after['fingerprints'][table].get(sha, 0) >= count, 'UI_PREEXISTING_ROW_CHANGED')
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


def identity(pid):
    path = Path('/proc') / str(pid)
    raw = (path / 'stat').read_text().rsplit(')', 1)[1].split()
    return {'pid': pid, 'startTicks': raw[19], 'exe': str((path / 'exe').resolve())}


def identities(server):
    rows = [identity(server.pid)]
    # Native children have the product process as parent; record identities,
    # never signal a process solely because its PID resembles an old receipt.
    for child in Path('/proc').iterdir():
        if child.name.isdigit():
            try:
                raw = (child / 'stat').read_text().rsplit(')', 1)[1].split()
                if int(raw[1]) == server.pid:
                    rows.append(identity(int(child.name)))
            except (FileNotFoundError, ProcessLookupError, PermissionError):
                pass
    return rows


def retired(row):
    try:
        return identity(row['pid'])['startTicks'] != row['startTicks']
    except (FileNotFoundError, ProcessLookupError):
        return True


def port_closed(base):
    with socket.socket() as probe:
        probe.settimeout(1)
        return probe.connect_ex(('127.0.0.1', int(base.rsplit(':', 1)[1]))) != 0


def stop(server, base):
    rows = identities(server)
    server.send_signal(signal.SIGTERM)
    code = server.wait(timeout=10)
    require(code == 0 and all(retired(row) for row in rows) and port_closed(base), 'UI_SERVER_CLOSURE_FAILED')
    return {'serverExit': code, 'portClosed': True, 'recordedIdentitiesRetired': True, 'identities': rows}


def start(manifest, environment, directory, expected_setup=False):
    logpath = directory / 'server.private.log'
    log = logpath.open('xb')
    os.fchmod(log.fileno(), 0o600)
    server = subprocess.Popen([manifest['binaries']['fvoci-migrate']['path'], '--start'],
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
        require(identity(server.pid)['exe'] == manifest['binaries']['fvoci-server']['path'], 'UI_NORMAL_MAIN_IDENTITY_FAILED')
        with build_opener(ProxyHandler({})).open(base + '/api/v1/setup', timeout=10) as response:
            require(response.status == 200 and json.loads(response.read())['needed'] is expected_setup, 'UI_INITIALIZED_SETUP_CHANGED')
        return server, base, log
    except BaseException:
        # An early failure has no graceful PASS. Reap only this owned process.
        if server.poll() is None:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=10)
        log.close()
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
        code = subprocess.run(args, env=env, cwd=W / 'apps/web', stdout=output, stderr=subprocess.STDOUT, timeout=900).returncode
    if report.exists():
        os.chmod(report, 0o600)
    require(code == 0, 'UI_ACTUAL_BROWSER_FAILED')
    return json.loads(report.read_text())


def execute_ui(manifest, baseline, environment):
    source = manifest['sourceInputs']
    require(startup_blockers(baseline) == [], 'UI_EXISTING_BACKGROUND_WORK_REFUSED')
    audit = {'namespaces': [], 'serverStarts': 0, 'observedFences': []}
    receipt = {'source': source['source'], 'tree': source['tree'], 'backend': 'libsql-remote',
               'counts': {'on': 0, 'restart': 0, 'off': 0}, 'restore': 'NOTRUN',
               'precisionWorkload': 'NOTRUN', 'matchedOnOffCost': 'NOTRUN', 'cleanupErrors': []}
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
            setup_needed = flow == 'on' and baseline['setupNeeded']
            owner = None if setup_needed else fixture(manifest, 'owner', env)
            if owner is not None:
                require(owner['commit'] == 'confirmed' and owner['freshPrimaryReadback'], 'UI_OWNER_READBACK_FAILED')
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
                require(len(titles) == (1 if flow == 'on' else 7), 'UI_EXPECTED_REGISTRATION_CHANGED')
                cases = report_cases(browser(manifest, directory, b_env, spec,
                                     '^selected normal main:' if flow == 'on' else None), spec, titles)
                receipt['counts'][flow] = len(cases)
                if flow == 'on':
                    seed = attachment(cases, 'selected-vue-native-readback.json')
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
                    old = stop(server, base)
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
            finally:
                if server is not None:
                    try:
                        stop(server, base)
                    except BaseException:
                        receipt['cleanupErrors'].append('UI_SERVER_CLOSURE_FAILED')
                        if server.poll() is None:
                            server.kill()
                            server.wait(timeout=10)
                if log is not None:
                    log.close()
            require(receipt['cleanupErrors'] == [], 'UI_RESOURCE_CLOSURE_FAILED')
            current_build()
            final = fixture(manifest, 'baseline', env)
            assert_preserved(baseline, final, audit)
            write(directory / 'preservation.private.json', {'before': baseline, 'after': final, 'audit': audit})
        require(receipt['counts'] == {'on': 1, 'restart': 1, 'off': 7}, 'UI_ACTUAL_COUNTS_FAILED')
        receipt['uiResult'] = 'PASS'
    except BaseException:
        receipt['uiResult'] = 'FAIL'
        raise
    finally:
        write(root() / 'ui-result.private.json', receipt)
    return receipt


def consume(phase, inputs):
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
    environment.update(FVOCI_DATABASE_BACKEND='libsql-remote', FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE='true',
        FVOCI_TEST_TURSO_DESTRUCTIVE='true', PASSWORD_PEPPER_KEYS=json.dumps({'fixture': secrets.token_hex(32)}),
        PASSWORD_PEPPER_ACTIVE_KEY_ID='fixture', ENCRYPTION_KEYS=json.dumps({'fixture': secrets.token_hex(32)}), ENCRYPTION_ACTIVE_KEY_ID='fixture')
    result = execute_ui(manifest, baseline, environment)
    print('TURSO_UI_ACK_PASS on=1 restart=1 off=7 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN')
    require(result['cleanupErrors'] == [], 'UI_RESOURCE_CLOSURE_FAILED')


def main():
    try:
        mode = sys.argv[1:]
        if mode == ['--record-before']:
            write(root() / 'source-before.json', hosted_identity())
        elif mode == ['--freeze']:
            freeze()
        elif mode == ['--actor']:
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


if __name__ == '__main__':
    sys.exit(main())
