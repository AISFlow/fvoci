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
    require(before == hosted_identity(), 'UI_BUILD_INPUTS_CHANGED')
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
    require(manifest['sourceInputs'] == hosted_identity(), 'UI_SOURCE_CHANGED')
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


def fixture(manifest, mode, environment, input=None):
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
    allocation = next(i for i, a in enumerate(_PROCESSES.allocations) if a['process'] is server)
    code = _PROCESSES.finish(server, True)
    rows = [e['identity'] for e in _PROCESSES.entries.values() if e['allocation'] in (allocation, None)]
    require(code == 0 and rows and all(retired(row) for row in rows) and port_closed(base), 'UI_SERVER_CLOSURE_FAILED')
    stopped = {'serverExit': code, 'portClosed': True, 'recordedIdentitiesRetired': True}
    write(directory / ('server-identities-' + secrets.token_hex(6) + '.private.json'),
          {'stopped': stopped, 'identities': rows})
    return stopped


def start(manifest, environment, directory, expected_setup=False):
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
                server_allocations.append((server, directory, identity(server.pid)))
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
                    server_allocations.append((server, restart_dir, identity(server.pid)))
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
        require(receipt['counts'] == {'on': 1, 'restart': 1, 'off': 7}, 'UI_ACTUAL_COUNTS_FAILED')
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
        for server, directory, allocation_identity in server_allocations:
            try:
                audit['servers'].append(maintenance_receipts(directory / 'server.private.log', allocation_identity, audit['targetSha256']))
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
    print('TURSO_UI_ACK_PASS on=1 restart=1 off=7 retries=0 ignored=0 restore=NOTRUN precision=NOTRUN cost=NOTRUN')
    require(result['cleanupErrors'] == [], 'UI_RESOURCE_CLOSURE_FAILED')



def consume(phase, inputs):
    with UiProcesses():
        return _consume(phase, inputs)

def main():
    try:
        mode = sys.argv[1:]
        if mode == ['--record-before']:
            write(root() / 'source-before.json', hosted_identity())
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


if __name__ == '__main__':
    sys.exit(main())
