"""Evidence-only addition to the two accepted normal selected driver bodies.
Root rebinds the entire parent driver to freshly qualified current compilation,
then calls restart_same_app(globals()) after its successful seed diagnostics.
This module never prepares a fixture, builds, imports a parent driver or resets DB.
"""
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import stat
import subprocess
import time
from urllib.request import ProxyHandler, Request, build_opener

OWNER = os.environ['FVOCI_CI_OWNER']
TITLE = 'selected normal main restart:'


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def private_write(path, value):
    with Path(path).open('x') as output:
        os.fchmod(output.fileno(), 0o600)
        output.write(json.dumps(value, indent=2) + '\n')


def validate_allocation(allocation, binding):
    assert allocation['schema'] == 1 and allocation['status'] == 'GRANTED'
    assert allocation['owner'] == OWNER and allocation['exclusiveCIJob'] is True
    assert allocation['source'] == allocation['compiledSource'] == binding['source']
    assert allocation['tree'] == binding['tree'] and allocation['backend'] == binding['backend']
    assert re.fullmatch('[0-9a-f]{40}', allocation['source'])
    assert re.fullmatch('[0-9a-f]{40}', allocation['tree'])
    assert allocation['backend'] in ('postgres', 'sqlite')
    assert allocation['binding'] == binding
    assert allocation['runId'] == binding['runId'] and allocation['runAttempt'] == binding['runAttempt']
    assert re.fullmatch('[0-9]+', allocation['runId'])
    assert re.fullmatch('[0-9]+', allocation['runAttempt'])
    assert allocation['currentCIJobConfirmed'] is True


def single_attachment(path, name):
    report = json.loads(Path(path).read_text())
    assert report['config']['workers'] == 1 and report['errors'] == []
    assert report['stats']['expected'] == 1
    assert all(report['stats'][key] == 0 for key in ('unexpected', 'flaky', 'skipped'))
    tests = [test for suite in report['suites'] for spec in suite['specs'] for test in spec['tests']]
    assert len(tests) == 1 and len(tests[0]['results']) == 1
    actual = tests[0]['results'][0]
    assert actual['status'] == 'passed' and actual['retry'] == 0 and actual['errors'] == []
    entries = [a for a in actual['attachments'] if a['name'] == name]
    assert len(entries) == 1 and entries[0]['contentType'] == 'application/json'
    return json.loads(base64.b64decode(entries[0]['body'], validate=True))


def port_closed(base):
    with socket.socket() as probe:
        probe.settimeout(1)
        return probe.connect_ex(('127.0.0.1', int(base.rsplit(':', 1)[1]))) != 0


def restart_same_app(g):
    # Names are the concrete state of the two accepted drivers, not a product DI.
    run, name, selected = g['run'], g['name'], g['browser_env']['FVOCI_E2E_SELECTED_BACKEND']
    assert os.getuid() == os.getgid() == 1000
    assert os.environ.get('FVOCI_ROOT_RUN_OWNER') == OWNER
    assert selected in ('postgres', 'sqlite') and g['code'] == 0
    assert g['HEAD'] == g['COMPILED_HEAD'], 'changed current Rust requires its actual compiled SHA'
    grant_path = Path(os.environ['FVOCI_ROOT_RESTART_GRANT'])
    assert grant_path.is_absolute() and not grant_path.is_symlink()
    allocation = json.loads(grant_path.read_text())
    binding = {'runId': os.environ['GITHUB_RUN_ID'],
               'runAttempt': os.environ['GITHUB_RUN_ATTEMPT'], 'source': g['HEAD'], 'tree': g['TREE'], 'compiledSource': g['COMPILED_HEAD'],
               'backend': selected, 'runRoot': str(run.resolve()),
               'parentDriverSha256': digest(g['__file__']), 'restartHelperSha256': digest(__file__),
               'sourceInputsSha256': digest(run / 'source-inputs-before.json'),
               'artifactHashes': {str(path): record['sha256'] for path, record in g['binaries'].items()},
               'assetHashes': g['assets']['dist_files'],
               'browserInputs': g['browser_inputs'], 'abiHashes': g['abi']['exact_copied_runtime_files']}
    validate_allocation(allocation, binding)
    receipt = {'scope': 'current-schema same owned DB/storage server restart only; not upgrade/archive/restore/Turso/whole0.6',
               'binding': binding, 'allocationSha256': digest(grant_path), 'stage': 'validated',
               'restartBrowserExit': None, 'cleanupErrors': []}
    restart_process = None
    restart_log = None
    restart_server = None
    restart_base = None
    before_rows = []
    def inputs():
        assert g['input_check'](g['before']) == g['source_before']
        for path, record in g['binaries'].items():
            assert digest(path) == record['sha256']
        assert g['tree_hashes'](g['dist']) == g['assets']['dist_files']
        for path, expected in g['abi']['exact_copied_runtime_files'].items():
            assert digest(path) == expected
        browser = g['browser_inputs']
        assert digest(browser['bun']['path']) == browser['bun']['sha256']
        assert digest(browser['chromium']['path']) == browser['chromium']['sha256']
        assert g['tree_hashes'](Path(browser['chromium']['path']).parent) == browser['chromium_directory_files']
        assert digest(grant_path) == receipt['allocationSha256']
        assert digest(__file__) == binding['restartHelperSha256']
        assert digest(g['__file__']) == binding['parentDriverSha256']
        copied = g['command'](['docker', 'exec', name, 'sha256sum', '/fvoci/bin/fvoci-server',
                              '/fvoci/bin/fvoci-migrate', '/fvoci/bin/collab-engine']).stdout
        assert [line.split()[0] for line in copied.splitlines()] == [g['binaries'][p]['sha256'] for p in (g['server'], g['migrate'], g['engine'])]
    try:
        inputs()
        storage_inode = (g['storage'].stat().st_dev, g['storage'].stat().st_ino)
        receipt['storageInode'] = list(storage_inode)
        seed = single_attachment(run / 'playwright-result.private.json', 'selected-vue-native-readback.json')
        assert seed['selected'] == selected and seed['firstAck'] != seed['finalAck']
        assert seed['creatorId'] != seed['freshActorId']
        assert len(seed['canonicalEmojiOracleControls']) == 6 and len(seed['nativeHistoryOracleControls']) == 2
        receipt['seedReportSha256'] = digest(run / 'playwright-result.private.json')
        label = g['command'](['docker', 'inspect', '--format', '{{index .Config.Labels "fvoci.owner"}}', name]).stdout.strip()
        assert label == OWNER
        if selected == 'sqlite':
            original_inode = (g['db'].stat().st_dev, g['db'].stat().st_ino)
            assert list(original_inode) == g['receipt']['database_inode']
        before_rows = [row for row in g['owned_rows'](name) if row['args'].startswith('/fvoci/bin/')]
        assert any(row['pid'] == g['server_row']['pid'] and row['start_ticks'] == g['server_row']['start_ticks'] for row in before_rows)
        stopped = g['command'](['docker', 'exec', '--user', '0', name, '/bin/kill', '-TERM', str(g['server_row']['namespace_pid'])], required=False)
        receipt['seedSigtermExit'] = stopped.returncode
        assert stopped.returncode == 0
        receipt['seedServerExit'] = g['server_process'].wait(timeout=10)
        assert receipt['seedServerExit'] == 0
        assert all(g['identity_gone'](row) for row in before_rows), 'old server/native identities must retire before restart'
        assert port_closed(g['base']), 'old listen socket must be closed before restart'
        receipt['oldRecordedRows'] = before_rows
        receipt['oldPortClosed'] = True
        if selected == 'sqlite':
            # Only our confirmed actor receipt is relocated; keep its bytes and inode.
            actor_files = sorted(g['dbroot'].glob('actor-*.json'))
            assert len(actor_files) == 1
            actor_file = actor_files[0]
            metadata = actor_file.lstat()
            assert stat.S_ISREG(metadata.st_mode) and metadata.st_nlink == 1
            assert metadata.st_uid == metadata.st_gid == 1000 and stat.S_IMODE(metadata.st_mode) in (0o600, 0o644)
            assert stat.S_IMODE(g['dbroot'].stat().st_mode) == 0o700, 'nonsecret actor receipt stays in owned private parent'
            actor = json.loads(actor_file.read_text())
            assert actor['backend'] == 'sqlite' and actor['commit'] == 'confirmed'
            assert actor['poolClosed'] and actor['connectionClose'] == 'confirmed' and actor['operationSucceeded']
            assert actor_file.name == 'actor-' + seed['freshActorId'] + '.json'
            saved_hash = digest(actor_file)
            dest = run / ('preserved-' + actor_file.name)
            assert not dest.exists() and dest.parent.stat().st_dev == metadata.st_dev
            actor_file.rename(dest)
            moved = dest.lstat()
            assert (moved.st_dev, moved.st_ino) == (metadata.st_dev, metadata.st_ino) and digest(dest) == saved_hash
            assert stat.S_IMODE(moved.st_mode) == stat.S_IMODE(metadata.st_mode)
            receipt['preservedActorReceipt'] = {'path': str(dest), 'sha256': saved_hash, 'sameInode': True,
                                               'originalMode': stat.S_IMODE(metadata.st_mode)}
            assert (g['db'].stat().st_dev, g['db'].stat().st_ino) == original_inode
        checkpoint = {'schema': 1, 'source': g['HEAD'], 'tree': g['TREE'], 'compiledSource': g['COMPILED_HEAD'],
                      'selected': selected, 'stopped': {'serverExit': 0, 'portClosed': True, 'recordedIdentitiesRetired': True}, 'seed': seed}
        checkpoint_path = run / 'restart-checkpoint.private.json'
        private_write(checkpoint_path, checkpoint)
        receipt['checkpointSha256'] = digest(checkpoint_path)
        inputs()
        restart_log = (run / 'restarted-normal-server.log').open('x')
        # Same container, literal private environment, DB and storage; port0 again.
        restart_process = subprocess.Popen(['docker', 'exec', name, '/bin/sh', '-ec',
                 '. /fvoci/inputs/environment.sh; exec /fvoci/bin/fvoci-migrate --start'],
                 stdin=subprocess.DEVNULL, stdout=restart_log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 10
        while restart_base is None:
            raw = (run / 'restarted-normal-server.log').read_text()
            match = re.search(r'fvoci-server listening on (http://127\.0\.0\.1:\d+)', raw)
            if match:
                restart_base = match[1]
                break
            assert restart_process.poll() is None, 'restarted normal preparation exited before listen; preserve log'
            assert time.monotonic() < deadline, 'unchanged10s listen observation expired'
            time.sleep(0.02)
        rows = g['owned_rows'](name)
        servers = [row for row in rows if row['args'] == '/fvoci/bin/fvoci-server' and not row.get('already_retired_at_observation')]
        assert len(servers) == 1
        restart_server = servers[0]
        assert restart_server['uid'] == restart_server['gid'] == 1000
        assert (restart_server['pid'], restart_server['start_ticks']) != (g['server_row']['pid'], g['server_row']['start_ticks'])
        opener = build_opener(ProxyHandler({}))
        with opener.open(Request(restart_base + '/api/v1/setup'), timeout=10) as response:
            assert response.status == 200 and json.loads(response.read())['needed'] is False
        if selected == 'sqlite':
            meta = g['db'].lstat()
            assert stat.S_ISREG(meta.st_mode) and (meta.st_dev, meta.st_ino) == original_inode
            assert (meta.st_uid, meta.st_gid, stat.S_IMODE(meta.st_mode), meta.st_nlink) == (1000, 1000, 0o600, 1)
        else:
            facts = g['pg_sql']("""SELECT jsonb_build_object('user',current_user,'version',current_setting('server_version_num'),
              'superuser',r.rolsuper,'bypassrls',r.rolbypassrls,
              'owns_schema',EXISTS(SELECT 1 FROM pg_namespace WHERE nspname='fvoci' AND pg_get_userbyid(nspowner)=current_user),
              'owns_tables',EXISTS(SELECT 1 FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci' AND pg_get_userbyid(c.relowner)=current_user),
              'versions',(SELECT jsonb_agg(version ORDER BY version) FROM fvoci.schema_migrations),
              'rls',(SELECT jsonb_object_agg(c.relname,jsonb_build_object('enabled',c.relrowsecurity,'forced',c.relforcerowsecurity))
                     FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname='fvoci'
                     AND c.relname IN('documents','document_states','document_collab_updates','wiki_create_commands','revisions')))
              FROM pg_roles r WHERE r.rolname=current_user""", app=True)
            assert facts == g['receipt']['actual_restricted_role_schema_rls']
            assert facts['user'] == g['role'] and not facts['superuser'] and not facts['bypassrls']
            assert not facts['owns_schema'] and not facts['owns_tables']
            assert len(facts['rls']) == 5 and all(row['enabled'] for row in facts['rls'].values())
            receipt['restartedAppRoleRegistryRLS'] = facts
            key_metadata = g['command'](['docker', 'exec', name, 'stat', '-c', '%u %g %a', '/run/fvoci/meili/api_key']).stdout.strip()
            assert key_metadata == '0 1000 640'
            key = g['command'](['docker', 'exec', '--user', '1000', name, 'cat', '/run/fvoci/meili/api_key']).stdout.strip()
            assert len(key) >= 16 and key != g['master_key']
            assert 'meilisearch enabled' in (run / 'restarted-normal-server.log').read_text()
            assert 'outbox dispatcher started' in (run / 'restarted-normal-server.log').read_text()
            receipt['restartedScopedSearchStartup'] = True
        assert (g['storage'].stat().st_dev, g['storage'].stat().st_ino) == storage_inode
        receipt.update(stage='restarted normal main ready', restartedBaseURL=restart_base, restartedServer=restart_server)
        # Neither owner DB URL/pepper nor actor executable reaches readback phase.
        env = {key: value for key, value in g['browser_env'].items() if key in ('PATH', 'LANG', 'PLAYWRIGHT_BROWSERS_PATH','BUN_RUNTIME_TRANSPILER_CACHE_PATH','TMPDIR')}
        env.update(CI='true', PLAYWRIGHT_BASE_URL=restart_base, FVOCI_E2E_SELECTED_BACKEND=selected,
                   FVOCI_E2E_SELECTED_RESTART_SOURCE=g['HEAD'], FVOCI_E2E_SELECTED_RESTART_CHECKPOINT=str(checkpoint_path),
                   FVOCI_E2E_RESULT_DIR=str(run / 'restart-browser'), PLAYWRIGHT_JSON_OUTPUT_FILE=str(run / 'restart-playwright-result.private.json'))
        args = [str(g['BUN']), '--bun', 'x', 'playwright', 'test', '--config', 'e2e-pending/collab-playwright.config.ts',
                '--reporter=line,json', '--grep', TITLE, g['SPEC']]
        receipt['restartBrowserCommand'] = args
        started = time.monotonic()
        result = g['command'](args, run / 'restart-browser.log', required=False, env=env, cwd=g['W'] / 'apps/web')
        receipt['restartBrowserExit'] = result.returncode
        receipt['restartBrowserSeconds'] = time.monotonic() - started
        report_path = run / 'restart-playwright-result.private.json'
        if report_path.exists(): os.chmod(report_path, 0o600)
        assert result.returncode == 0, 'preserve original restart browser failure'
        readback = single_attachment(report_path, 'selected-vue-restart-readback.json')
        assert readback['source'] == g['HEAD'] and readback['tree'] == g['TREE'] and readback['selected'] == selected
        assert readback['workspaceId'] == seed['workspaceId'] and readback['documentId'] == seed['document']['id']
        assert readback['freshActorId'] == seed['freshActorId'] and readback['persisted'] == seed['persisted'] and readback['revision'] == seed['revision']
        assert readback['firstAck'] == seed['firstAck'] and readback['finalAck'] == seed['finalAck']
        assert len(readback['canonicalEmojiOracleControls']) == 6 and len(readback['nativeHistoryOracleControls']) == 2
        private_write(run / 'selected-vue-restart-readback.private.json', readback)
        receipt.update(stage='readback passed', actualRestartBrowserTests=1, retries=0, ignored=0,
                       restartReportSha256=digest(report_path), restartAttachmentSha256=digest(run / 'selected-vue-restart-readback.private.json'))
        inputs()
    except BaseException as error:
        receipt['originalFailure'] = {'type': type(error).__name__, 'message': str(error)}
        raise
    finally:
        # Caller retains original finally for its container and PG/Meili wrappers.
        # This block owns only the second exec/server; timeout remains a failure.
        if restart_process is not None:
            try:
                current = g['owned_rows'](name)
                live = [row for row in current if row['args'] == '/fvoci/bin/fvoci-server' and not row.get('already_retired_at_observation')]
                if live:
                    assert len(live) == 1
                    restart_server = live[0]
                    assert restart_server['uid'] == restart_server['gid'] == 1000
                    stopped = g['command'](['docker', 'exec', '--user', '0', name, '/bin/kill', '-TERM', str(restart_server['namespace_pid'])], required=False)
                    receipt['restartSigtermExit'] = stopped.returncode
                    assert stopped.returncode == 0
                receipt['restartServerExit'] = restart_process.wait(timeout=10)
                assert receipt['restartServerExit'] == 0
                receipt['restartedRecordedIdentitiesRetired'] = all(g['identity_gone'](row) for row in current if row['args'].startswith('/fvoci/bin/'))
                assert receipt['restartedRecordedIdentitiesRetired']
                if restart_base is not None:
                    receipt['restartedPortClosed'] = port_closed(restart_base)
                    assert receipt['restartedPortClosed']
            except BaseException as error:
                receipt['cleanupErrors'].append({'type': type(error).__name__, 'message': str(error)})
        if restart_log is not None:
            restart_log.close()
            receipt['restartServerLogSha256'] = digest(run / 'restarted-normal-server.log')
        private_write(run / 'restart-receipt.private.json', receipt)
    assert receipt['cleanupErrors'] == [], 'restart cleanup failure is not PASS; parent retains original failure'
    return receipt
