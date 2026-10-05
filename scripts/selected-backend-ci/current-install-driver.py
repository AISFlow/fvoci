import datetime
import hashlib
import json
import os
import pathlib
import re
import secrets
import shutil
import subprocess
import sys
import time

from current_binding import load_current
current = load_current('install', __file__)
P = current['run']
E = P.parent
W = pathlib.Path(__file__).resolve().parents[2]
H = current['manifest']['source']
IMAGE = 'ubuntu@sha256:534baea6a22c03a63003dbc8dbe78fe34bc0d7e595d9a9dc9834884ff530eb55'
NAME = 'fvoci-v060-install-current-' + secrets.token_hex(4)

def now():
    return datetime.datetime.now(datetime.timezone.utc).isoformat()

def sha(path):
    h = hashlib.sha256()
    with pathlib.Path(path).open('rb') as f:
        for chunk in iter(lambda: f.read(1048576), b''):
            h.update(chunk)
    return h.hexdigest()

def write(path, obj):
    path.write_text(json.dumps(obj, indent=2) + '\n')

def command(args, *, logfile=None, check=True):
    if logfile:
        with logfile.open('w') as log:
            result = subprocess.run(args, stdout=log, stderr=subprocess.STDOUT)
    else:
        result = subprocess.run(args, capture_output=True, text=True)
    if check and result.returncode:
        raise RuntimeError(f'owned prerequisite failed code={result.returncode}: {args[0:3]}')
    return result

assert not P.exists(), 'literal one-shot owned run; preserve original failures'
P.mkdir(mode=0o700)
build = current['build']
before = current['before']
bins = build['binaries']
server = next(n for n in bins if n.endswith('/fvoci-server'))
migrate = next(n for n in bins if n.endswith('/fvoci-migrate'))
engine = next(n for n in bins if n.endswith('/collab-engine'))
test = next(n for n,r in bins.items() if r['target']['name'] == 'selected_install_lifetime')
test_build = {'test_executable': {'path': test, 'sha256': bins[test]['sha256']}}
assets = current['assets']['dist_files']
dist = W / 'apps/web/dist'
write(P / 'source-inputs-before.json', before)

# Only synthetic run-local keyrings. Values remain in a mode0600 private input.
env = {
    'PATH': '/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin',
    'PASSWORD_PEPPER_KEYS': json.dumps({'fixture': secrets.token_hex(32)}),
    'PASSWORD_PEPPER_ACTIVE_KEY_ID': 'fixture',
    'ENCRYPTION_KEYS': json.dumps({'fixture': secrets.token_hex(32)}),
    'ENCRYPTION_ACTIVE_KEY_ID': 'fixture',
    'FVOCI_PUBLIC_ORIGIN': 'http://127.0.0.1:8080',
    'FVOCI_COOKIE_SECURE': '0',
    'STORAGE_DRIVER': 'local',
    'FVOCI_STORAGE_DIR': '/fvoci/storage',
    'FVOCI_COLLAB_ENGINE': '/fvoci/bin/collab-engine',
    'FVOCI_COLLAB_FAMILY_LEASE_MS': '30000',
    'FVOCI_COLLAB_FAMILY_RENEW_MS': '5000',
    'FVOCI_STATIC_DIR': '/srv/fvoci-web',
}
private = P / 'environment.private.json'
with private.open('x') as f:
    os.fchmod(f.fileno(), 0o600)
    json.dump(env, f)

receipt = {
    'source': H, 'tree': before['tree'], 'start_utc': now(), 'pid': os.getpid(),
    'image': IMAGE, 'container': NAME, 'root_owner': os.environ['FVOCI_CI_OWNER'],
    'scope': 'actual SQLite normal migrate --start process/install/lifetime controls, not browser/PG/Turso/OFF/fullCI',
    'test_executable': test_build['test_executable'], 'actual_binary_inputs': bins,
    'environment_names': sorted(env), 'environment_private_file': str(private),
    'original_failure': 'root-d49-install-runtime/test.log ownership4FAIL and owned-copy/test.log ABI1PASS3FAIL preserved; this execution corrects only ephemeral copiedowner+actualhost ABI prerequisites',
    'expected_rust_tests': 4, 'expected_owned_child_processes': 15,
    'free_before': shutil.disk_usage(P).free, 'retry': 0,
}
write(P / 'start.json', receipt)
created = False
code = 1
try:
    command(['docker', 'create', '--name', NAME, '--network', 'none', '--user', '0',
             '--label', 'fvoci.owner=' + os.environ['FVOCI_CI_OWNER'],
             '--label', 'fvoci.test-run=v060-current-install', '--entrypoint', '/bin/sleep', IMAGE, '1800'])
    created = True
    command(['docker', 'start', NAME])
    runtime_abi=current['abi']
    write(P/'runtime-abi-inputs.json', runtime_abi)
    for source, expected in runtime_abi['exact_copied_runtime_files'].items():
        assert sha(source)==expected, source
        command(['docker','cp',source,NAME+':/lib/x86_64-linux-gnu/'+pathlib.Path(source).name])
    receipt['runtime_abi']='runtime-abi-inputs.json: exact actualhost libc/loader/libm/libgcc copied onlyintoown ephemeraldevelopmentcontainer; original Ubuntu24 image compatibility NOTclaimed'
    command(['docker','exec',NAME,'/bin/sh','-ec','ldd --version | head -1; /bin/true'], logfile=P/'actual-runtime-abi.log')
    setup = ('mkdir -p ' + str(pathlib.Path(server).parent) + ' /fvoci/bin /fvoci/run /fvoci/inputs /fvoci/storage /srv/fvoci-web; '
             'chown 0:1000 /fvoci/run; chmod 0710 /fvoci/run; '
             'chown 1000:1000 /fvoci/storage; chmod 0700 /fvoci/storage; chmod 0700 /fvoci/inputs')
    command(['docker', 'exec', NAME, '/bin/sh', '-ec', setup])
    for source, dest in [(server, server), (migrate, migrate), (engine, '/fvoci/bin/collab-engine'),
                         (test, '/fvoci/bin/install-test'), (str(private), '/fvoci/inputs/environment.json')]:
        command(['docker', 'cp', source, NAME + ':' + dest])
    command(['docker', 'cp', str(dist) + '/.', NAME + ':/srv/fvoci-web'])
    copied = [server, migrate, '/fvoci/bin/collab-engine', '/fvoci/bin/install-test', '/fvoci/inputs/environment.json']
    command(['docker','exec',NAME,'stat','-c','%n %u %g %a',*copied], logfile=P/'copied-files-before.log')
    command(['docker','exec',NAME,'chown','0:0',*copied])
    command(['docker','exec',NAME,'chmod','0755',*copied[:-1]])
    command(['docker','exec',NAME,'chmod','0600',copied[-1]])
    command(['docker','exec',NAME,'chown','-R','0:0','/srv/fvoci-web'])
    command(['docker','exec',NAME,'stat','-c','%n %u %g %a',*copied], logfile=P/'copied-files-after.log')
    command(['docker','exec',NAME,'sha256sum',*copied[:-1]], logfile=P/'copied-executable-hashes.log')
    receipt['copied_inode_ownership_correction'] = 'Only exact newly copied own container files; original host binaries untouched; actual before/after UID GID mode logs retained'
    command(['docker', 'exec', NAME, '/bin/sh', '-ec',
             'chmod 0600 /fvoci/inputs/environment.json; id; ldd --version | head -1; '
             'command -v kill; test ! -e /usr/bin/node; test ! -e /usr/bin/bun'],
            logfile=P / 'prerequisite.log')
    args = ['docker', 'exec',
            '--env', 'FVOCI_SELECTED_INSTALL_RUN_ROOT=/fvoci/run',
            '--env', 'FVOCI_SELECTED_INSTALL_ENV_FILE=/fvoci/inputs/environment.json',
            '--env', 'FVOCI_SELECTED_INSTALL_MIGRATE_SHA256=' + bins[migrate]['sha256'],
            '--env', 'FVOCI_SELECTED_INSTALL_SERVER_SHA256=' + bins[server]['sha256'],
            NAME, '/fvoci/bin/install-test', '--test-threads=1', '--nocapture']
    receipt.update(command=args, body_start_utc=now())
    write(P / 'progress.json', receipt)
    started = time.monotonic()
    result = command(args, logfile=P / 'test.log', check=False)
    code = result.returncode
    receipt.update(body_end_utc=now(), exit_code=code, body_seconds=time.monotonic()-started,
                   log_sha256=sha(P / 'test.log'))
    copy = command(['docker', 'cp', NAME + ':/fvoci/run', str(P / 'retained-run')], check=False)
    receipt['durable_receipt_copy_exit'] = copy.returncode
    raw = (P / 'test.log').read_text()
    if code == 0:
        assert re.search(r'test result: ok\. 4 passed; 0 failed; 0 ignored;', raw), 'exact executed counts'
        processes = list((P / 'retained-run').rglob('*process.json'))
        assert len(processes) == 15, ('actual child receipt count', len(processes))
        assert all(json.loads(f.read_text())['status'] is not None for f in processes)
        receipt.update(actual_tests=4, ignored=0, actual_owned_process_receipts=15)
except BaseException as error:
    receipt['driver_error'] = str(error)
    code = code or 1
finally:
    if created:
        # Only this uniquely owned bounded container. Preserve test receipts first.
        if not (P / 'retained-run').exists():
            command(['docker', 'cp', NAME + ':/fvoci/run', str(P / 'retained-run')], check=False)
        cleanup = command(['docker', 'rm', '-f', '-v', NAME], check=False)
        receipt['owned_container_cleanup_exit'] = cleanup.returncode
        absence = command(['docker', 'inspect', NAME], check=False)
        receipt['owned_container_absent'] = absence.returncode != 0
        if cleanup.returncode or not receipt['owned_container_absent']:
            code = code or 1
    changed_inputs = [n for n, h in before['tracked'].items() if sha(W/n) != h] + [n for n, h in before['external'].items() if sha(n) != h]
    changed_binaries = [n for n in bins if sha(n) != bins[n]['sha256']]
    receipt.update(full_current_source_external_unchanged=not changed_inputs, actual_binary_hashes_unchanged=not changed_binaries, changed_inputs=changed_inputs, changed_binaries=changed_binaries)
    if changed_inputs or changed_binaries: code = code or 1
    receipt.update(end_utc=now(), final_exit_code=code, free_after=shutil.disk_usage(P).free,
                   final_source=subprocess.check_output(['git', '-c', 'safe.directory=' + str(W), '-C', str(W), 'rev-parse', 'HEAD'], text=True).strip())
    write(P / 'receipt.json', receipt)
    print(json.dumps({k: receipt.get(k) for k in ['source', 'final_exit_code', 'actual_tests',
          'actual_owned_process_receipts', 'driver_error', 'owned_container_absent', 'body_seconds']}), flush=True)
sys.exit(code)
