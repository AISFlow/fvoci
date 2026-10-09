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
p.add_argument('--identity-only', action='store_true')
a = p.parse_args(sys.argv[2:])
with open(os.environ['TRACE'], 'a') as t: t.write(('inspect ' if a.identity_only else 'helper ')+a.target+'\\n')
if os.environ.get('HELPER_FAIL'): sys.exit(9)
prefix = pathlib.Path(a.prefix)
if pathlib.Path(a.archive).read_bytes() != b'fixture source': sys.exit(7)
env = {'SQLITE3_LIB_DIR': str(prefix/'lib'), 'SQLITE3_INCLUDE_DIR': str(prefix/'include'),
       'SQLITE3_STATIC': '1', 'SQLITE3_NO_PKG_CONFIG': '1'}
exports = ''.join('export '+k+'='+shlex.quote(v)+'\\n' for k,v in env.items())
inputs = {'helper_sha256': hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest(),
          'target': a.target, 'flags': ['fixture profile']}
timings = {'archive': {'elapsed_seconds': 0.002, 'timeout_seconds': 180, 'exit_code': 0}}
if a.identity_only:
    print(json.dumps({'inputs': inputs, 'exports': exports, 'timings': {}}))
    sys.exit(0)
if not prefix.exists():
    (prefix/'lib').mkdir(parents=True)
    (prefix/'include').mkdir()
    (prefix/'lib/libsqlite3.a').write_bytes(b'fixture static archive')
    (prefix/'include/sqlite3.h').write_bytes(b'fixture exact header')
    (prefix/'env.sh').write_text(exports)
    manifest = {'inputs': inputs,
                'outputs': {n:hashlib.sha256((prefix/n).read_bytes()).hexdigest()
                            for n in ['env.sh','lib/libsqlite3.a','include/sqlite3.h']}}
    (prefix/'manifest.json').write_text(json.dumps(manifest))
else:
    manifest = json.loads((prefix/'manifest.json').read_text())
    for n,h in manifest['outputs'].items():
        if hashlib.sha256((prefix/n).read_bytes()).hexdigest() != h: sys.exit(8)
prefix.with_name(prefix.name + '.prepare-timings.json').write_text(json.dumps(timings))
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
        self.assertEqual(trace[1], 'inspect '+TARGET)
        self.assertTrue(trace[-1].startswith('consumer '))
        self.assertIn('https://www.sqlite.org/2026/sqlite-amalgamation-3530400.zip', trace[0])
        for flag in ("'--proto', '=https'", "'--max-time', '90'", "'--retry', '0'", "'--max-filesize', '16777216'"):
            self.assertIn(flag, trace[0])
        self.assertIn('cache: MISS; building pinned source', result.stderr)
        identity = json.loads((self.parent / 'consumer-inputs.json').read_text())
        self.assertIn('outputs', identity['manifest'])
        self.assertIn('flags', identity['manifest']['inputs'])
        self.assertEqual(identity['exports'], exports)
        self.assertEqual(identity['os_release'], Path('/etc/os-release').read_text())
        self.assertEqual(identity['architecture'], platform.machine())
        self.assertEqual(len(dict(line.split('=', 1) for line in self.github_output.read_text().splitlines())['cache_identity']), 64)

    def test_download_failure_never_runs_helper_or_consumer(self):
        self.env['CURL_FAIL'] = '1'
        self.rejected(self.invoke('--', 'consumer'))
        self.assertNotIn('helper ', self.trace.read_text())
        self.assertEqual({p.name for p in self.parent.iterdir()}, {'preparation-timings.json'})

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

    def test_helper_archive_budget_and_timeout_timings_are_bounded(self):
        # Exercise the actual subprocess/timing helper without invoking a native tool.
        names = {'TIMINGS', 'ENV', 'COMPILE_TIMEOUT', 'ARCHIVE_TIMEOUT'}
        body = [node for node in TREE.body if isinstance(node, (ast.Import, ast.ImportFrom))
                or (isinstance(node, ast.FunctionDef) and node.name == 'run')
                or (isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
                    and node.targets[0].id in names)]
        namespace = {}
        exec(compile(ast.Module(body=body, type_ignores=[]), '<actual-helper-timing>', 'exec'), namespace)
        self.assertEqual(namespace['ARCHIVE_TIMEOUT'], namespace['COMPILE_TIMEOUT'])
        self.assertEqual(namespace['ARCHIVE_TIMEOUT'], 180)
        with mock.patch.object(namespace['subprocess'], 'run', side_effect=subprocess.TimeoutExpired(['ar'], 180)):
            with mock.patch.object(namespace['time'], 'monotonic', side_effect=[0, 90000]):
                with self.assertRaises(subprocess.TimeoutExpired):
                    namespace['run'](['ar'], timeout=180, step='archive')
        self.assertEqual(namespace['TIMINGS']['archive'],
                         {'elapsed_seconds': 86400, 'timeout_seconds': 180, 'exit_code': -1})
        with mock.patch.object(namespace['subprocess'], 'run', return_value=subprocess.CompletedProcess(['ar'], 0, '', '')):
            with mock.patch.object(namespace['time'], 'monotonic', side_effect=[5, 5.125]):
                namespace['run'](['ar'], timeout=180, step='archive')
        self.assertEqual(namespace['TIMINGS']['archive'],
                         {'elapsed_seconds': 0.125, 'timeout_seconds': 180, 'exit_code': 0})

    def test_preflight_exports_no_build_paths_and_matches_verified_identity(self):
        self.assertEqual(self.invoke('--identity-only').returncode, 0)
        self.assertEqual(self.github_env.read_text(), '')
        self.assertFalse((self.parent / TARGET).exists())
        expected = dict(line.split('=', 1) for line in self.github_output.read_text().splitlines())['cache_identity']
        self.assertEqual(self.invoke('--expected-cache-identity', expected).returncode, 0)
        self.assertEqual(json.loads((self.parent / 'consumer-inputs.json').read_text())['cache_identity'], expected)
        self.assertIn('archive', json.loads((self.parent / 'consumer-inputs.json').read_text())['helper_timings'])

    def test_wrong_expected_identity_cannot_export_or_build(self):
        self.rejected(self.invoke('--expected-cache-identity', 'f' * 64))
        self.assertFalse((self.parent / TARGET).exists())

    def test_timings_do_not_change_cache_identity(self):
        self.assertEqual(self.invoke().returncode, 0)
        first = self.github_output.read_text().splitlines()[-1]
        timing_path = self.parent / (TARGET + '.prepare-timings.json')
        timing_path.write_text(json.dumps({'archive': {'elapsed_seconds': 30.1}}))
        manifest_path = self.parent / TARGET / 'manifest.json'
        before = manifest_path.read_bytes()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('cache: HIT;', result.stderr)
        self.assertEqual(first, self.github_output.read_text().splitlines()[-1])
        self.assertEqual(before, manifest_path.read_bytes())
        for timing in json.loads((self.parent / 'consumer-inputs.json').read_text())['timings'].values():
            self.assertGreaterEqual(timing['elapsed_seconds'], 0)
            self.assertLessEqual(timing['elapsed_seconds'], 86400)
            self.assertGreater(timing['timeout_seconds'], 0)

    def test_explicit_cache_fallback_preserves_rejected_prefix_and_logs_build(self):
        self.assertEqual(self.invoke().returncode, 0)
        archive = self.parent / TARGET / 'lib/libsqlite3.a'
        archive.write_bytes(b'rejected bytes')
        result = self.invoke('--cache-fallback')
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('cache: REJECTED;', result.stderr)
        self.assertIn('cache: MISS/BUILD;', result.stderr)
        rejected = list(self.parent.glob(TARGET + '.rejected-*'))
        self.assertEqual(len(rejected), 1)
        self.assertEqual((rejected[0] / 'lib/libsqlite3.a').read_bytes(), b'rejected bytes')
        self.assertEqual(archive.read_bytes(), b'fixture static archive')

    def test_cache_fallback_cannot_bypass_source_authentication_or_symlink(self):
        (self.parent / 'sqlite-amalgamation-3530400.zip').write_bytes(b'wrong source')
        self.rejected(self.invoke('--cache-fallback'))
        (self.parent / 'sqlite-amalgamation-3530400.zip').unlink()
        (self.parent / TARGET).symlink_to(self.root)
        self.rejected(self.invoke('--cache-fallback'))

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
        consumers = {'rust': {'fast', 'native-arm64', 'postgres-build', 'postgres', 'collaboration'},
                     'web': {'web-checks', 'web-native-checks', 'workspace-browser-build', 'workspace-browser-shard', 'collaboration-build', 'collaboration-install-on', 'collaboration-postgres-on', 'collaboration-sqlite-on', 'collaboration-postgres-off', 'collaboration-sqlite-off'},
                     'documents': {'native-extraction'}}
        for workflow, expected in consumers.items():
            # Bounded textual contract; the CI planner separately parses/validates YAML.
            text = (ROOT / '.github/workflows' / (workflow+'.yml')).read_text()
            jobs = dict(re.findall(r'^  ([a-z][\w-]*):\n(.*?)(?=^  [a-z][\w-]*:|\Z)',
                                   text, re.M | re.S))
            actual = {name for name,body in jobs.items() if re.search(r'^        id: sqlite$', body, re.M)}
            self.assertEqual(actual, expected)
            for job in expected:
                body = jobs[job]
                if workflow == 'rust' and job == 'postgres':
                    self.assertNotIn('path: target', body)
                    self.assertIn('Restore prepared SQLite prefix', body)
                    self.assertLess(body.index('Verify cached SQLite prefix'), body.index('Download required postgres'))
                elif workflow == 'web' and job == 'workspace-browser-shard':
                    self.assertNotIn('path: target', body)
                    self.assertIn('needs: [ci-plan, workspace-browser-build]', body)
                    self.assertIn('artifact-ids: ${{ needs.workspace-browser-build.outputs.artifact_id }}', body)
                    self.assertIn('--ci-consume-browser', body)
                    self.assertLess(body.index('id: sqlite'), body.index('Download this run'))
                elif workflow == 'web' and job in {
                        'collaboration-install-on', 'collaboration-postgres-on', 'collaboration-sqlite-on',
                        'collaboration-postgres-off', 'collaboration-sqlite-off'}:
                    # Each lane prepares its own SDK before admitting the producer's exact artifact.
                    self.assertNotIn('path: target', body)
                    self.assertNotIn('path: crates/collab-engine/target', body)
                    if job == 'collaboration-install-on':
                        self.assertIn('needs: [ci-plan, collaboration-build]\n', body)
                    else:
                        self.assertIn('needs: [ci-plan, collaboration-build, collaboration-install-on]\n', body)
                    self.assertIn('artifact-ids: ${{ needs.collaboration-build.outputs.artifact_id }}', body)
                    self.assertIn('FVOCI_WEB_BUILD_HANDOFF_SHA256: ${{ needs.collaboration-build.outputs.handoff_sha256 }}', body)
                    self.assertIn('bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-consume-selected', body)
                    self.assertLess(body.index('id: sqlite'), body.index('Download this run'))
                    self.assertLess(body.index('Download this run'), body.index('id: browser'))
                else:
                    self.assertLess(body.index('id: sqlite'), body.index('path: target'))
                    root_cache = body.split('path: target',1)[1].split('      - ',1)[0]
                    self.assertIn('${{ steps.sqlite.outputs.cache_identity }}', root_cache)
                    self.assertNotIn('restore-keys:', root_cache)
                self.assertIn('--github-env "$GITHUB_ENV" --github-output "$GITHUB_OUTPUT"', body)
                self.assertIn('libclang-18-dev=1:18.1.8-20ubuntu8', body)
                # Every root output cache and compilation must follow usable
                # preparation; source-download caches do not contain build output.
                blocks = list(re.finditer(r'^      - .*?(?=^      - |\Z)', body, re.M | re.S))
                prepare = next(block for block in blocks if re.search(r'^        id: sqlite$', block.group(), re.M))
                ready = prepare
                if workflow == 'rust':
                    verified = [block for block in blocks if 'name: Verify cached SQLite prefix or build\n' in block.group()]
                    self.assertEqual(len(verified), 1, 'required SQLite verification step missing/duplicated')
                    ready = verified[0]
                    self.assertLessEqual(prepare.end(), ready.start())
                    prefix_restore = next(block for block in blocks if 'name: Restore prepared SQLite prefix\n' in block.group())
                    self.assertLessEqual(prepare.end(), prefix_restore.start())
                    self.assertLessEqual(prefix_restore.end(), ready.start())
                    self.assertIn('--expected-cache-identity "${{ steps.sqlite.outputs.cache_identity }}"', ready.group())
                    self.assertIn('--github-env "$GITHUB_ENV"', ready.group())
                for preparation in (prepare, ready):
                    self.assertNotRegex(preparation.group(), r'(?m)^        (?:if|continue-on-error):', 'preparation cannot be skipped or masked')
                for block in blocks:
                    text = block.group()
                    if re.search(r'^          path: target(?:/[^\n]+)?$', text, re.M):
                        self.assertLessEqual(ready.end(), block.start())
                        if 'actions/cache/save@' in text:
                            # Fast cites cache-primary-key. Other main-only saves repeat the restore key verbatim;
                            # the registry rejects a primary-key citation unless that save is explicitly exempt.
                            key = re.search(r'key: \$\{\{ steps\.([a-z_]+)\.outputs\.cache-primary-key \}\}', text)
                            if key is None:
                                literal = re.search(r'(?m)^          key: (.+)$', text)
                                hit = re.search(r'steps\.([A-Za-z0-9_]+)\.outputs\.cache-hit', text)
                                self.assertIsNotNone(literal, 'save must use its qualified restore key')
                                self.assertIsNotNone(hit, 'save must use its qualified restore key')
                                restored = [entry for entry in blocks if re.search(
                                    r'(?m)^        id: ' + re.escape(hit.group(1)) + r'$', entry.group())]
                                self.assertEqual(len(restored), 1, 'save must use its qualified restore key')
                                restore_key = re.search(r'(?m)^          key: (.+)$', restored[0].group())
                                self.assertIsNotNone(restore_key, 'save must use its qualified restore key')
                                self.assertEqual(restore_key.group(1), literal.group(1),
                                                 'save must use its qualified restore key')
                            else:
                                restored = [entry for entry in blocks if re.search(
                                    r'^        id: ' + key.group(1) + '$', entry.group(), re.M)]
                                self.assertEqual(len(restored), 1)
                            self.assertLessEqual(restored[0].end(), block.start())
                            self.assertIn('actions/cache/restore@', restored[0].group())
                            self.assertIn('${{ steps.sqlite.outputs.cache_identity }}', restored[0].group())
                            self.assertEqual(re.search(r'^          path: (target[^\n]*)$', text, re.M).group(1),
                                             re.search(r'^          path: (target[^\n]*)$', restored[0].group(), re.M).group(1))
                        else:
                            self.assertIn('${{ steps.sqlite.outputs.cache_identity }}', text)
                        self.assertNotIn('restore-keys:', text)
                    # Documents defaults to the independent native crate; root steps override it.
                    if (workflow != 'documents' or 'working-directory: .' in text):
                        root_work = ('working-directory: crates/' not in text and any(
                            re.search(r'cargo (?:build|test|clippy|check)\b|scripts/ci_selection.py rust-binaries build|bash scripts/run-web-e2e.sh', line)
                            and not re.search(r'--manifest-path [\"\']?crates/', line) for line in text.splitlines()))
                        if root_work:
                            self.assertLessEqual(ready.end(), block.start())
                    if 'path: crates/collab-engine/target' in text:
                        self.assertNotIn('steps.sqlite.outputs.cache_identity', text, 'independent helper cache must retain its own inputs')
            if workflow == 'documents':
                body = jobs['native-extraction']
                self.assertLess(body.index('Production helper rejects test controls'), body.index('id: sqlite'))
                self.assertIn('working-directory: .', body.split('id: sqlite',1)[1].split('      - ',1)[0])
        for workflow in ('collab-engine', 'install'):
            self.assertNotIn('prepare-sqlite-ci', (ROOT / '.github/workflows' / (workflow+'.yml')).read_text())
        docker = (ROOT / 'infra/rust/Dockerfile').read_text()
        builder, runtime = docker.split(' AS runtime', 1)
        self.assertIn('libclang-18-dev=1:18.1.8-20ubuntu8', builder)
        self.assertIn('FROM ubuntu:26.04@sha256:f144425ff09be612d6d9ad965196e9cdc23dae1f42110a8a11a3e9a8198759f7 AS ubuntu', builder)
        self.assertIn('FROM ubuntu AS rust-sources', builder)
        self.assertIn('FROM ubuntu AS web-build', builder)
        self.assertIn('prepare-sqlite-ci.sh --parent /sqlite-build -- cargo build', builder)
        for tool in ('libclang', 'python3', 'gcc', '/sqlite-build'):
            self.assertNotIn(tool, runtime)

    def test_new_producer_mutations_fail_same_wiring_contract(self):
        fixture = self.root / 'producer-workflow-mutations'
        (fixture / '.github/workflows').mkdir(parents=True)
        (fixture / 'infra/rust').mkdir(parents=True)
        for workflow in ('rust', 'web', 'documents', 'collab-engine', 'install'):
            shutil.copy(ROOT / '.github/workflows' / (workflow + '.yml'), fixture / '.github/workflows' / (workflow + '.yml'))
        shutil.copy(ROOT / 'infra/rust/Dockerfile', fixture / 'infra/rust/Dockerfile')
        for workflow, job in (('rust', 'postgres-build'), ('web', 'workspace-browser-build')):
            path = fixture / '.github/workflows' / (workflow + '.yml')
            original = path.read_text()
            match = re.search(r'^  ' + job + r':\n(.*?)(?=^  [a-z][\w-]*:|\Z)', original, re.M | re.S)
            self.assertIsNotNone(match)
            body = match.group(1)
            blocks = list(re.finditer(r'^      - .*?(?=^      - |\Z)', body, re.M | re.S))
            prep = next(block for block in blocks if re.search(r'^        id: sqlite$', block.group(), re.M))
            root_cache = next(block for block in blocks if re.search(r'^          path: target(?:/[^\n]+)?$', block.group(), re.M))
            helper_cache = next(block for block in blocks if 'path: crates/collab-engine/target' in block.group())
            changes = {
                'missing producer preparation': body.replace('id: sqlite\n', 'id: missing-sqlite\n', 1),
                'output cache before preparation': body[:prep.start()] + root_cache.group() + body[prep.start():root_cache.start()] + body[root_cache.end():],
                'unqualified producer cache': body.replace(root_cache.group(), root_cache.group().replace('steps.sqlite.outputs.cache_identity', 'foreign_identity'), 1),
                'fallback producer cache': body.replace(root_cache.group(), root_cache.group() + '          restore-keys: unsafe\n', 1),
                'compile before preparation': body[:prep.start()] + '      - run: cargo build --locked\n' + body[prep.start():],
                'skipped producer preparation': body.replace('id: sqlite\n', 'id: sqlite\n        if: false\n', 1),
                'masked producer preparation': body.replace('id: sqlite\n', 'id: sqlite\n        continue-on-error: true\n', 1),
                'second unqualified root cache': body + '      - uses: actions/cache@fixture\n        with:\n          path: target/extra\n          key: unqualified\n',
                'helper borrows SQLite inputs': body.replace(helper_cache.group(), helper_cache.group() + '          sqlite: ${{ steps.sqlite.outputs.cache_identity }}\n', 1),
            }
            if workflow == 'rust':
                verify = next(block for block in blocks if 'name: Verify cached SQLite prefix or build\n' in block.group())
                changes['missing producer verification'] = body[:verify.start()] + body[verify.end():]
                changes['output cache before verification'] = body[:verify.start()] + root_cache.group() + body[verify.start():root_cache.start()] + body[root_cache.end():]
            if workflow == 'web':
                save = next(block for block in blocks if 'actions/cache/save@' in block.group()
                            and re.search(r'(?m)^          path: target$', block.group()))
                changes['save key drifts from its restore'] = body.replace(
                    save.group(), save.group().replace(
                        'v1-web-browser-default-fixture-', 'v1-web-browser-drifted-', 1), 1)
            with mock.patch.dict(globals(), ROOT=fixture):
                self.test_workflow_root_cache_preparation_order_and_independent_crates()
            for label, changed in changes.items():
                with self.subTest(workflow=workflow, job=job, mutation=label):
                    self.assertNotEqual(body, changed)
                    path.write_text(original[:match.start(1)] + changed + original[match.end(1):])
                    with mock.patch.dict(globals(), ROOT=fixture):
                        with self.assertRaises(AssertionError):
                            self.test_workflow_root_cache_preparation_order_and_independent_crates()
            path.write_text(original)

    def test_web_producer_consumer_mutations_fail_same_wiring_contract(self):
        fixture = self.root / 'web-workflow-mutations'
        (fixture / '.github/workflows').mkdir(parents=True)
        (fixture / 'infra/rust').mkdir(parents=True)
        for workflow in ('rust', 'web', 'documents', 'collab-engine', 'install'):
            shutil.copy(ROOT / '.github/workflows' / (workflow+'.yml'), fixture / '.github/workflows' / (workflow+'.yml'))
        shutil.copy(ROOT / 'infra/rust/Dockerfile', fixture / 'infra/rust/Dockerfile')
        web = fixture / '.github/workflows/web.yml'; original = web.read_text()
        consumer = re.search(r'^  collaboration-install-on:\n(.*?)(?=^  [a-z][\w-]*:|\Z)', original, re.M | re.S)
        producer = re.search(r'^  collaboration-build:\n(.*?)(?=^  [a-z][\w-]*:|\Z)', original, re.M | re.S)
        self.assertIsNotNone(consumer);self.assertIsNotNone(producer)
        changes = [(producer, 'id: sqlite', 'id: missing-sqlite'),
            (consumer, 'id: sqlite', 'id: missing-sqlite'),
            (consumer, 'artifact-ids: ${{ needs.collaboration-build.outputs.artifact_id }}', 'artifact-ids: foreign'),
            (consumer, 'FVOCI_WEB_BUILD_HANDOFF_SHA256: ${{ needs.collaboration-build.outputs.handoff_sha256 }}', 'FVOCI_WEB_BUILD_HANDOFF_SHA256: foreign'),
            (consumer, '--ci-consume-selected', '--with-selected-backends'),
            (consumer, '      - name: Download this run', '      - uses: actions/cache@fixture\n        with:\n          path: target\n      - name: Download this run')]
        with mock.patch.dict(globals(), ROOT=fixture):
            self.test_workflow_root_cache_preparation_order_and_independent_crates()
        for match, old, new in changes:
            with self.subTest(mutation=old):
                body = match.group(1);self.assertIn(old, body)
                changed = body.replace(old, new, 1);self.assertNotEqual(body, changed)
                web.write_text(original[:match.start(1)] + changed + original[match.end(1):])
                with mock.patch.dict(globals(), ROOT=fixture):
                    with self.assertRaises(AssertionError):
                        self.test_workflow_root_cache_preparation_order_and_independent_crates()

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
