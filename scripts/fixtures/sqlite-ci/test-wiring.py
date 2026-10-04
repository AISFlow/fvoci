#!/usr/bin/env python3
"""Build wiring only: owned tool/helper stubs; no C/Rust/image/network execution."""
import ast
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[3]
NAMES = {'SQLITE3_LIB_DIR', 'SQLITE3_INCLUDE_DIR', 'SQLITE3_STATIC', 'SQLITE3_NO_PKG_CONFIG'}
TARGET = {'x86_64': 'x86_64-unknown-linux-gnu', 'aarch64': 'aarch64-unknown-linux-gnu'}[platform.machine()]
REAL_HELPER = (ROOT / 'scripts/prepare-sqlite-build.sh').read_text()
TREE = ast.parse(REAL_HELPER.split("<<'PY'\n", 1)[1].rsplit('\nPY', 1)[0])
PINS = {n.targets[0].id: ast.literal_eval(n.value) for n in TREE.body
        if isinstance(n, ast.Assign) and isinstance(n.targets[0], ast.Name)
        and n.targets[0].id in {'VERSION', 'SOURCE_ID'}}
HELPER_STUB = '''#!/usr/bin/env bash
exec python3 - "$0" "$@" <<'PY'
import argparse, hashlib, json, os, pathlib, shlex, sys
VERSION = VERSION_LITERAL
SOURCE_ID = SOURCE_ID_LITERAL
p = argparse.ArgumentParser()
for name in ('archive', 'prefix', 'target'): p.add_argument('--'+name, required=True)
a = p.parse_args(sys.argv[2:])
with open(os.environ['TRACE'], 'a') as t: t.write('helper '+a.target+'\\n')
if os.environ.get('HELPER_FAIL'): sys.exit(9)
prefix = pathlib.Path(a.prefix)
if pathlib.Path(a.archive).read_bytes() != b'fixture source': sys.exit(7)
env = {'SQLITE3_LIB_DIR': str(prefix/'lib'), 'SQLITE3_INCLUDE_DIR': str(prefix/'include'),
       'SQLITE3_STATIC': '1', 'SQLITE3_NO_PKG_CONFIG': '1'}
exports = ''.join('export '+k+'='+shlex.quote(v)+'\\n' for k,v in env.items())
if not prefix.exists():
    (prefix/'lib').mkdir(parents=True)
    (prefix/'include').mkdir()
    (prefix/'lib/libsqlite3.a').write_bytes(b'fixture static archive')
    (prefix/'include/sqlite3.h').write_bytes(b'fixture exact header')
    (prefix/'env.sh').write_text(exports)
    manifest = {'inputs': {'helper_sha256': hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest(),
                           'target': a.target, 'flags': ['fixture profile']},
                'outputs': {n:hashlib.sha256((prefix/n).read_bytes()).hexdigest()
                            for n in ['env.sh','lib/libsqlite3.a','include/sqlite3.h']}}
    (prefix/'manifest.json').write_text(json.dumps(manifest))
else:
    manifest = json.loads((prefix/'manifest.json').read_text())
    for n,h in manifest['outputs'].items():
        if hashlib.sha256((prefix/n).read_bytes()).hexdigest() != h: sys.exit(8)
if os.environ.get('BAD_EXPORT'): exports += 'export UNEXPECTED=bad\\n'
print(exports, end='')
PY
'''.replace('VERSION_LITERAL', repr(PINS['VERSION'])).replace('SOURCE_ID_LITERAL', repr(PINS['SOURCE_ID']))


class WiringTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='fvoci-sqlite-wiring-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.scripts = self.root / 'scripts'
        self.scripts.mkdir()
        shutil.copy(ROOT / 'scripts/prepare-sqlite-ci.sh', self.scripts)
        (self.scripts / 'prepare-sqlite-build.sh').write_text(HELPER_STUB)
        self.parent = self.root / 'owned parent'
        self.parent.mkdir()
        self.bin = self.root / 'bin'
        self.bin.mkdir()
        self.clang = self.root / 'clang'
        self.clang.mkdir()
        (self.clang / 'libclang.so').write_bytes(b'fixture libclang')
        self.trace = self.root / 'trace'
        self.trace.write_text('')
        self.env = {k:v for k,v in os.environ.items() if k not in NAMES | {'CARGO_BUILD_TARGET'}}
        self.env.update(PATH=str(self.bin)+':'+os.environ['PATH'], TRACE=str(self.trace), LIBCLANG_PATH=str(self.clang))
        self.env['RUSTC_HOST'] = TARGET
        self.tool('curl', '''import os, pathlib, sys
with open(os.environ['TRACE'],'a') as t: t.write('curl '+repr(sys.argv[1:])+'\\n')
if os.environ.get('CURL_FAIL'): sys.exit(22)
pathlib.Path(sys.argv[sys.argv.index('--output')+1]).write_bytes(b'fixture source')
''')
        self.tool('rustc', '''import os, sys
with open(os.environ['TRACE'],'a') as t: t.write('rustc '+repr(sys.argv[1:])+'\\n')
if os.environ.get('RUSTC_FAIL'): sys.exit(2)
print(os.environ.get('RUSTC_VERSION','fixture rustc native target'))
print('host: '+os.environ['RUSTC_HOST'])
''')
        for tool in ('cc','ar'):
            self.tool(tool, "raise AssertionError('native tool must not execute in FAST fixtures')\n")
        self.tool('consumer', '''import json, os, pathlib, sys
with open(os.environ['TRACE'],'a') as t: t.write('consumer '+repr(sys.argv[1:])+'\\n')
pathlib.Path(os.environ['CONSUMER_ENV']).write_text(json.dumps({k:v for k,v in os.environ.items() if k.startswith('SQLITE3_')}))
sys.exit(int(os.environ.get('CONSUMER_EXIT','0')))
''')
        self.env['CONSUMER_ENV'] = str(self.root / 'consumer.json')
        self.github_env = self.root / 'github-env'
        self.github_env.write_text('')
        self.github_output = self.root / 'github-output'
        self.github_output.write_text('')

    def tool(self, name, body):
        p = self.bin / name
        p.write_text('#!/usr/bin/env python3\n'+body)
        p.chmod(0o755)

    def invoke(self, *extra, parent=True):
        args = ['bash', str(self.scripts / 'prepare-sqlite-ci.sh')]
        if parent: args += ['--parent', str(self.parent)]
        args += ['--github-env', str(self.github_env), '--github-output', str(self.github_output), *extra]
        result = subprocess.run(args, env=self.env, text=True, capture_output=True, timeout=10)
        print(json.dumps({'test':self.id(), 'argv':args, 'exit':result.returncode, 'stderr':result.stderr}))
        return result

    def rejected(self, result):
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(self.github_env.read_text(), '')
        self.assertEqual(self.github_output.read_text(), '')
        self.assertFalse((self.root / 'consumer.json').exists())

    def test_verified_env_before_consumer_and_bounded_official_download(self):
        result = self.invoke('--', 'consumer', '--fixture-argument')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, '')
        exports = json.loads((self.root / 'consumer.json').read_text())
        self.assertEqual(set(exports), NAMES)
        self.assertEqual(exports['SQLITE3_LIB_DIR'], str(self.parent / TARGET / 'lib'))
        self.assertEqual(exports['SQLITE3_INCLUDE_DIR'], str(self.parent / TARGET / 'include'))
        self.assertEqual(exports['SQLITE3_STATIC'], '1')
        self.assertEqual(exports['SQLITE3_NO_PKG_CONFIG'], '1')
        self.assertEqual(dict(line.split('=',1) for line in self.github_env.read_text().splitlines()), exports)
        trace = self.trace.read_text().splitlines()
        self.assertTrue(trace[0].startswith('curl '))
        self.assertEqual(trace[1], 'helper '+TARGET)
        self.assertTrue(trace[-1].startswith('consumer '))
        self.assertIn('https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip', trace[0])
        for flag in ("'--proto', '=https'", "'--max-time', '90'", "'--retry', '0'", "'--max-filesize', '16777216'"):
            self.assertIn(flag, trace[0])
        identity = json.loads((self.parent / 'consumer-inputs.json').read_text())
        self.assertIn('outputs', identity['manifest'])
        self.assertIn('flags', identity['manifest']['inputs'])
        self.assertEqual(identity['exports'], exports)
        self.assertEqual(len(self.github_output.read_text().strip().split('=')[1]), 64)

    def test_download_failure_never_runs_helper_or_consumer(self):
        self.env['CURL_FAIL'] = '1'
        self.rejected(self.invoke('--', 'consumer'))
        self.assertNotIn('helper ', self.trace.read_text())
        self.assertEqual(list(self.parent.iterdir()), [])

    def test_helper_failure_never_exports_or_runs_consumer(self):
        self.env['HELPER_FAIL'] = '1'
        self.rejected(self.invoke('--', 'consumer'))
        self.assertNotIn('rustc ', self.trace.read_text())

    def test_real_helper_rejects_wrong_pin_before_native_tools(self):
        (self.scripts / 'prepare-sqlite-build.sh').write_text(REAL_HELPER)
        result = self.invoke('--', 'consumer')
        self.rejected(result)
        self.assertIn('archive SHA-256 mismatch', result.stderr)
        self.assertFalse((self.parent / TARGET).exists())
        # A second invocation must reject retained bad bytes, without downloading again.
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(self.trace.read_text().count('curl '), 1)

    def test_bad_exports_never_reach_github_or_consumer(self):
        self.env['BAD_EXPORT'] = '1'
        self.rejected(self.invoke('--', 'consumer'))

    def test_missing_clang_fails_before_download(self):
        (self.clang / 'libclang.so').unlink()
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(self.trace.read_text(), '')

    def test_symlink_parent_never_downloads(self):
        actual = self.parent
        self.parent = self.root / 'link'
        self.parent.symlink_to(actual, target_is_directory=True)
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(self.trace.read_text(), '')

    def test_partial_environment_refused(self):
        self.env['SQLITE3_STATIC'] = '1'
        self.rejected(self.invoke('--', 'consumer', parent=False))
        self.assertEqual(self.trace.read_text(), '')

    def test_cross_cargo_target_fails_before_download(self):
        self.env['CARGO_BUILD_TARGET'] = 'unsupported-cross-target'
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(self.trace.read_text(), '')
        del self.env['CARGO_BUILD_TARGET']
        self.rejected(self.invoke('--', 'consumer', '--target=unsupported-cross-target'))
        self.assertEqual(self.trace.read_text(), '')

    def test_rustc_host_mismatch_never_exports_or_consumes(self):
        self.env['RUSTC_HOST'] = 'unsupported-host'
        self.rejected(self.invoke('--', 'consumer'))

    def test_rustc_failure_never_exports_or_consumes(self):
        self.env['RUSTC_FAIL'] = '1'
        self.rejected(self.invoke('--', 'consumer'))

    def test_unmatched_prefix_never_downloads_or_overwrites(self):
        self.env.update(SQLITE3_LIB_DIR='/unowned/lib', SQLITE3_INCLUDE_DIR='/unowned/include',
                        SQLITE3_STATIC='1', SQLITE3_NO_PKG_CONFIG='1')
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(self.trace.read_text(), '')

    def test_cache_identity_changes_with_clang_and_rust_toolchain(self):
        self.assertEqual(self.invoke().returncode, 0)
        first = self.github_output.read_text().splitlines()[-1]
        (self.clang / 'libclang.so').write_bytes(b'different fixture libclang')
        self.assertEqual(self.invoke().returncode, 0)
        second = self.github_output.read_text().splitlines()[-1]
        self.assertNotEqual(first, second)
        self.env['RUSTC_VERSION'] = 'different fixture toolchain'
        self.assertEqual(self.invoke().returncode, 0)
        self.assertNotEqual(second, self.github_output.read_text().splitlines()[-1])
        self.assertEqual(self.trace.read_text().count('curl '), 1)

    def test_cache_identity_tracks_bindgen_environment_without_disclosing_values(self):
        self.assertEqual(self.invoke().returncode, 0)
        first = self.github_output.read_text().splitlines()[-1]
        self.env['BINDGEN_EXTRA_CLANG_ARGS'] = '-Dfixture_private_build_override'
        self.assertEqual(self.invoke().returncode, 0)
        self.assertNotEqual(first, self.github_output.read_text().splitlines()[-1])
        inputs = (self.parent / 'consumer-inputs.json').read_text()
        self.assertNotIn('fixture_private_build_override', inputs)
        self.assertIn('build_environment_sha256', inputs)

    def test_unknown_prefix_is_preserved(self):
        prefix = self.parent / TARGET
        prefix.mkdir()
        (prefix / 'sentinel').write_bytes(b'keep')
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(list(prefix.iterdir()), [prefix / 'sentinel'])
        self.assertEqual((prefix / 'sentinel').read_bytes(), b'keep')

    def test_corrupt_static_archive_not_replaced_or_consumed(self):
        self.assertEqual(self.invoke().returncode, 0)
        self.github_env.write_text('')
        self.github_output.write_text('')
        archive = self.parent / TARGET / 'lib/libsqlite3.a'
        archive.write_bytes(b'corrupt')
        self.rejected(self.invoke('--', 'consumer'))
        self.assertEqual(archive.read_bytes(), b'corrupt')

    def test_verified_existing_environment_revalidates_helper(self):
        self.assertEqual(self.invoke().returncode, 0)
        self.env.update(dict(line.split('=',1) for line in self.github_env.read_text().splitlines()))
        self.assertEqual(self.invoke('--', 'consumer', parent=False).returncode, 0)
        self.assertEqual(self.trace.read_text().count('curl '), 1)
        self.assertEqual(self.trace.read_text().count('helper '), 2)

    def test_consumer_exit_is_propagated(self):
        self.env['CONSUMER_EXIT'] = '23'
        self.assertEqual(self.invoke('--', 'consumer').returncode, 23)

    def test_local_root_callers_require_preflight_and_forward_cargo_arguments(self):
        for name in ('generate-api.sh', 'run-db-tests.sh'):
            shutil.copy(ROOT / 'scripts' / name, self.scripts)
        (self.root / 'apps/web').mkdir(parents=True)
        self.tool('bun', "import sys\nprint('fixture bun')\n")
        self.env['TEST_DATABASE_URL'] = 'fixture-unused-url'
        self.env['CARGO_TARGET_DIR'] = str(self.root / 'target')
        (self.root / 'target/debug').mkdir(parents=True)
        binary = self.root / 'target/debug/fvoci-export-openapi'
        binary.write_text('#!/usr/bin/env bash\nprintf \'{}\\n\'\n')
        binary.chmod(0o755)
        # Stub only the build prerequisite, recording the actual wrapper argv.
        (self.scripts / 'prepare-sqlite-ci.sh').write_text('''#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >>"$TRACE"
exit "${PREREQ_EXIT:-0}"
''')
        self.tool('cargo', "raise AssertionError('caller bypassed SQLite prerequisite')\n")
        for script, expected in (
                ('generate-api.sh', '-- cargo build --locked --offline --bin fvoci-export-openapi --features api-schema'),
                ('run-db-tests.sh', '-- cargo test --locked --offline --features db-tests --test db_integration -- --nocapture')):
            with self.subTest(script=script):
                result = subprocess.run(['bash', str(self.scripts/script)], env=self.env,
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertEqual(self.trace.read_text().splitlines()[-1], expected)
                self.env['PREREQ_EXIT'] = '19'
                result = subprocess.run(['bash', str(self.scripts/script)], env=self.env,
                                        capture_output=True, text=True, timeout=10)
                self.assertEqual(result.returncode, 19)
                del self.env['PREREQ_EXIT']

    def test_workflow_root_cache_preparation_order_and_independent_crates(self):
        consumers = {'rust': {'fast', 'native-arm64', 'postgres', 'collaboration'},
                     'web': {'web-checks', 'workspace-browser-shard', 'collaboration-flow'},
                     'documents': {'native-extraction'}}
        for workflow, expected in consumers.items():
            # Bounded textual contract; the CI planner separately parses/validates YAML.
            text = (ROOT / '.github/workflows' / (workflow+'.yml')).read_text()
            jobs = dict(re.findall(r'^  ([a-z][\w-]*):\n(.*?)(?=^  [a-z][\w-]*:|\Z)',
                                   text, re.M | re.S))
            actual = {name for name,body in jobs.items() if 'id: sqlite' in body}
            self.assertEqual(actual, expected)
            for job in expected:
                body = jobs[job]
                self.assertLess(body.index('id: sqlite'), body.index('path: target'))
                root_cache = body.split('path: target',1)[1].split('      - ',1)[0]
                self.assertIn('${{ steps.sqlite.outputs.cache_identity }}', root_cache)
                self.assertNotIn('restore-keys:', root_cache)
                self.assertIn('--github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"', body)
                self.assertIn('libclang-18-dev=1:18.1.3-1ubuntu1', body)
            if workflow == 'documents':
                body = jobs['native-extraction']
                self.assertLess(body.index('Production helper rejects test controls'), body.index('id: sqlite'))
                self.assertIn('working-directory: .', body.split('id: sqlite',1)[1].split('      - ',1)[0])
        for workflow in ('collab-engine', 'install'):
            self.assertNotIn('prepare-sqlite-ci', (ROOT / '.github/workflows' / (workflow+'.yml')).read_text())
        docker = (ROOT / 'infra/rust/Dockerfile').read_text()
        builder, runtime = docker.split(' AS runtime', 1)
        self.assertIn('libclang-14-dev=1:14.0.6-12', builder)
        self.assertIn('prepare-sqlite-ci.sh --parent /sqlite-build -- cargo build', builder)
        for tool in ('libclang', 'python3', 'gcc', '/sqlite-build'):
            self.assertNotIn(tool, runtime)

    def test_native_arm64_preparation_mutations_fail_same_wiring_contract(self):
        fixture = self.root / 'workflow-mutations'
        (fixture / '.github/workflows').mkdir(parents=True)
        (fixture / 'infra/rust').mkdir(parents=True)
        for workflow in ('rust', 'web', 'documents', 'collab-engine', 'install'):
            shutil.copy(ROOT / '.github/workflows' / (workflow+'.yml'),
                        fixture / '.github/workflows' / (workflow+'.yml'))
        shutil.copy(ROOT / 'infra/rust/Dockerfile', fixture / 'infra/rust/Dockerfile')
        rust = fixture / '.github/workflows/rust.yml'
        original = rust.read_text()
        native = re.search(r'^  native-arm64:\n(.*?)(?=^  [a-z][\w-]*:|\Z)',
                           original, re.M | re.S)
        self.assertIsNotNone(native)
        body = native.group(1)
        prep = body.index('      - name: Prepare pinned SQLite root build inputs')
        cache = body.index('      - name: Restore server build outputs')
        fetch = body.index('      - run: cargo fetch --locked')
        mutations = {
            'missing preparation': (body[:prep] + body[cache:], 'native-arm64'),
            'cache before preparation': (body[:prep] + body[cache:fetch] + body[prep:cache] + body[fetch:],
                                         'not less than'),
            'unqualified cache': (body.replace('steps.sqlite.outputs.cache_identity', 'fixture_static_identity'),
                                  r'steps\.sqlite\.outputs\.cache_identity'),
        }
        # Challenge the exact original assertions, on copies only. Every other
        # workflow and every existing consumer remains part of that contract.
        with mock.patch.dict(globals(), ROOT=fixture):
            self.test_workflow_root_cache_preparation_order_and_independent_crates()
        for label, (changed, error) in mutations.items():
            with self.subTest(mutation=label):
                self.assertNotEqual(body, changed)
                rust.write_text(original[:native.start(1)] + changed + original[native.end(1):])
                with mock.patch.dict(globals(), ROOT=fixture):
                    with self.assertRaisesRegex(AssertionError, error):
                        self.test_workflow_root_cache_preparation_order_and_independent_crates()

    def test_web_build_stops_on_preflight_failure(self):
        shutil.copy(ROOT / 'scripts/run-web-e2e.sh', self.scripts)
        (self.root / 'apps/web').mkdir(parents=True)
        self.tool('bun', '''import os, sys
with open(os.environ['TRACE'],'a') as t: t.write('bun '+repr(sys.argv[1:])+'\\n')
assert sys.argv[1:] == ['--bun','x','--no-install','playwright','--version']
''')
        (self.scripts / 'prepare-sqlite-ci.sh').write_text('''#!/usr/bin/env bash
printf '%s\\n' "preflight $*" >>"$TRACE"
exit 19
''')
        result = subprocess.run(['bash', str(self.scripts / 'run-web-e2e.sh')], env=self.env,
                                text=True, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 1, result.stderr)
        trace = self.trace.read_text()
        self.assertIn('preflight --env-file', trace)
        self.assertNotIn('helper ', trace)
        self.assertNotIn('cargo', trace)
        self.assertNotIn('bun run', trace)


if __name__ == '__main__':
    unittest.main()
