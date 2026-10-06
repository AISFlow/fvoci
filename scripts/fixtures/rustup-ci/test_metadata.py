#!/usr/bin/env python3
"""Pure metadata/job guard fixtures; no installed toolchain or compiler calls."""
import contextlib
import importlib.util
import itertools
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('prepare_rustup_ci_metadata', Path(__file__).resolve().parents[2] / 'prepare-rustup-ci-metadata.py')
metadata = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(metadata)

# Literal Rustup 1.29.1 public --installed format, independent of internal ROWS.
PUBLIC_INSTALLED = ('cargo-x86_64-unknown-linux-gnu\n'
                    'clippy-x86_64-unknown-linux-gnu\n'
                    'rust-std-x86_64-unknown-linux-gnu\n'
                    'rustc-x86_64-unknown-linux-gnu')


class MetadataControls(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='fvoci-rustup-metadata-fixture-')
        self.addCleanup(self.temp.cleanup)
        self.parent = Path(self.temp.name)
        self.root = self.parent / 'toolchain'
        (self.root / 'lib/rustlib').mkdir(parents=True)
        self.component = self.root / metadata.RELATIVE
        self.component.write_bytes(metadata.CANONICAL)
        self.component.chmod(0o644)
        self.schema = self.root / 'lib/rustlib/rust-installer-version'
        self.schema.write_bytes(b'3\n')
        for name in metadata.ROWS:
            (self.root / 'lib/rustlib' / ('manifest-' + name)).write_bytes(b'file:owned fixture\n')
        (self.root / 'bin').mkdir()
        self.compiler = self.root / 'bin/rustc'
        self.compiler.write_bytes(b'owned synthetic compiled byte canary\n')
        self.compiler.chmod(0o755)
        self.rustup = self.parent / 'rustup'
        self.rustup.write_bytes(b'owned synthetic rustup inspection executable\n')
        self.rustup.chmod(0o755)
        self.output = self.parent / 'receipts'
        self.context = {'source_head': '5' * 40, 'source_tree': '6' * 40, 'job': 'collaboration-build',
                        'rustup_version': 'rustup 1.29.1 (d95a37b6a 2026-08-13)',
                        'rustup_sha256': metadata.sha(self.rustup.read_bytes()),
                        'rustup_identity': metadata.file_identity(self.rustup.lstat())}

    def prepare(self):
        with patch.object(metadata, 'command', return_value=PUBLIC_INSTALLED):
            return metadata.prepare(self.root, self.output, self.rustup, self.context)

    def test_literal_public_names_and_exact_invocation(self):
        with patch.object(metadata, 'command', return_value=PUBLIC_INSTALLED) as command:
            self.assertEqual(metadata.installed(self.rustup), PUBLIC_INSTALLED.split('\n'))
            command.assert_called_once_with([str(self.rustup), 'component', 'list', '--installed', '--toolchain',
                                             '1.98.1-x86_64-unknown-linux-gnu'], strip=False)
        self.assertIn('clippy-preview-x86_64-unknown-linux-gnu', metadata.ROWS)
        self.assertNotIn('clippy-x86_64-unknown-linux-gnu', metadata.ROWS)
        self.assertEqual(metadata.CANONICAL,
                         b'cargo-x86_64-unknown-linux-gnu\nclippy-preview-x86_64-unknown-linux-gnu\n'
                         b'rust-std-x86_64-unknown-linux-gnu\nrustc-x86_64-unknown-linux-gnu\n')

    def test_literal_public_set_refusals_before_write(self):
        rows = PUBLIC_INSTALLED.split('\n')
        invalid = ['\n'.join(rows[:-1]), PUBLIC_INSTALLED + '\nunknown-x86_64-unknown-linux-gnu',
                   '\n'.join((rows[0],) * 4), '\n'.join(rows[:3] + [rows[0]]),
                   PUBLIC_INSTALLED.replace('x86_64', 'aarch64'),
                   PUBLIC_INSTALLED.replace('clippy-', 'clippy-preview-'),
                   PUBLIC_INSTALLED.replace('clippy-', 'rustfmt-'),
                   PUBLIC_INSTALLED.replace('clippy-', 'clippy-nightly-'),
                   PUBLIC_INSTALLED.replace('\n', ' (installed)\n') + ' (installed)',
                   PUBLIC_INSTALLED.replace('\n', '\r\n'),
                   PUBLIC_INSTALLED.replace('\n', '\x0b'),
                   PUBLIC_INSTALLED.replace('\n', '\n\n')]
        for text in invalid:
            with self.subTest(text=text), patch.object(metadata, 'command', return_value=text):
                with self.assertRaisesRegex(ValueError, 'public-installed-set'):
                    metadata.prepare(self.root, self.output, self.rustup, self.context)
                self.assertEqual(self.component.read_bytes(), metadata.CANONICAL)
                self.assertFalse(self.output.exists())

    def test_public_diagnostics_never_echo_unknown_output(self):
        import io
        raw = PUBLIC_INSTALLED + '\nunknown-component https://example.invalid/private-canary\n'
        output = io.StringIO()
        with contextlib.redirect_stdout(output), patch.object(metadata, 'command', return_value=raw):
            with self.assertRaisesRegex(ValueError, 'public-installed-set'):
                metadata.installed(self.rustup)
        diagnostic = json.loads(output.getvalue())['rustup_installed']
        self.assertEqual(diagnostic, {'recognized': PUBLIC_INSTALLED.split('\n'), 'row_count': 5,
                                      'unknown_count': 1, 'raw_sha256': metadata.sha(raw.encode('ascii'))})
        self.assertNotIn('unknown-component', output.getvalue())
        self.assertNotIn('private-canary', output.getvalue())
        self.assertNotIn('https://', output.getvalue())

    def test_literal_public_terminal_lf_preserves_raw_hash(self):
        import io
        raw = PUBLIC_INSTALLED + '\n'
        output = io.StringIO()
        with contextlib.redirect_stdout(output), patch.object(metadata, 'command', return_value=raw):
            self.assertEqual(metadata.installed(self.rustup), PUBLIC_INSTALLED.split('\n'))
        self.assertEqual(json.loads(output.getvalue())['rustup_installed']['raw_sha256'], metadata.sha(raw.encode()))

    def test_public_diagnostic_uses_unstripped_command_stdout(self):
        import io
        raw = (PUBLIC_INSTALLED + '\n').encode('ascii')
        output = io.StringIO()
        result = subprocess.CompletedProcess([], 0, stdout=raw, stderr=b'')
        with contextlib.redirect_stdout(output), patch.object(metadata.subprocess, 'run', return_value=result):
            self.assertEqual(metadata.installed(self.rustup), PUBLIC_INSTALLED.split('\n'))
        self.assertEqual(json.loads(output.getvalue())['rustup_installed']['raw_sha256'], metadata.sha(raw))

    def refuses(self, action=None):
        if action:
            action()
        info = metadata.file_identity(self.component.lstat())
        raw = self.component.read_bytes() if self.component.is_file() else None
        with self.assertRaises((ValueError, UnicodeError, OSError)):
            self.prepare()
        self.assertEqual(metadata.file_identity(self.component.lstat()), info)
        if raw is not None:
            self.assertEqual(self.component.read_bytes(), raw)

    def test_every_valid_order_preserves_tools_and_receipts(self):
        for index, order in enumerate(itertools.permutations(metadata.ROWS)):
            with self.subTest(order=order):
                self.output = self.parent / ('receipts-' + str(index))
                raw = ('\n'.join(order) + '\n').encode('ascii')
                self.component.write_bytes(raw)
                before = metadata.closure(self.root)
                info = metadata.file_identity(self.component.lstat())
                result = self.prepare()
                self.assertEqual(self.component.read_bytes(), metadata.CANONICAL)
                self.assertEqual(metadata.file_identity(self.component.lstat()), info)
                self.assertEqual(before, metadata.closure(self.root))
                self.assertEqual((self.output / 'original-components.txt').read_bytes(), raw)
                self.assertEqual(json.loads((self.output / 'before.json').read_text())['original_order'], list(order))
                self.assertEqual(result['changed'], raw != metadata.CANONICAL)
                self.assertEqual(self.output.stat().st_mode & 0o777, 0o700)
                for file in self.output.iterdir():
                    self.assertEqual(file.stat().st_mode & 0o777, 0o600)
                    self.assertEqual((file.stat().st_uid, file.stat().st_gid), (os.getuid(), os.getgid()))

    def test_second_invocation_new_receipt_is_no_write(self):
        self.component.write_bytes(('\n'.join(reversed(metadata.ROWS)) + '\n').encode())
        self.prepare()
        before = self.component.stat().st_mtime_ns
        self.output = self.parent / 'second-receipts'
        self.assertFalse(self.prepare()['changed'])
        self.assertEqual(self.component.stat().st_mtime_ns, before)

    def test_literal_lf_five_reviewed_separator_refusals(self):
        for separator in (b'\x0b', b'\x0c', b'\x1c', b'\x1d', b'\x1e'):
            with self.subTest(separator=separator):
                raw = metadata.CANONICAL.replace(b'\n', separator, 1)
                self.assertEqual(len(raw), 136)
                self.component.write_bytes(raw)
                self.refuses()
                self.assertFalse(self.output.exists())

    def test_malformed_rows_refuse_before_receipt_or_write(self):
        raws = [metadata.CANONICAL[:-1], metadata.CANONICAL+b'\n', metadata.CANONICAL.replace(b'\n', b'\r\n'),
                b'\xff'+metadata.CANONICAL[1:], metadata.CANONICAL.replace(b'cargo', b'cargx', 1),
                metadata.CANONICAL.replace(b'x86_64', b'aarch64', 1),
                ('\n'.join(metadata.ROWS[:-1])+'\n').encode(), metadata.CANONICAL+b'unknown\n',
                ('\n'.join((metadata.ROWS[0],)+metadata.ROWS[1:3]+(metadata.ROWS[0],))+'\n').encode()]
        for raw in raws:
            with self.subTest(raw=raw):
                self.component.write_bytes(raw)
                self.refuses()
                self.assertFalse(self.output.exists())

    def test_component_symlink(self):
        target = self.parent / 'retained'; self.component.rename(target); self.component.symlink_to(target)
        self.refuses()

    def test_ancestor_symlink(self):
        target = self.root / 'retained'; (self.root / 'lib/rustlib').rename(target); (self.root / 'lib/rustlib').symlink_to(target)
        self.refuses()

    def test_hardlink(self):
        os.link(self.component, self.parent / 'hardlink'); self.refuses()

    def test_mode(self):
        self.refuses(lambda: self.component.chmod(0o666))

    def test_foreign_owner_guard(self):
        real = metadata.regular
        def foreign(path, mode=None, owned=False):
            if Path(path) == self.component:
                original = Path.lstat
                def lstat(instance, *args, **kwargs):
                    info = original(instance, *args, **kwargs)
                    if instance == self.component:
                        values = list(info); values[4] = os.getuid() + 123
                        return os.stat_result(values)
                    return info
                with patch.object(Path, 'lstat', lstat): return real(path, mode, owned)
            return real(path, mode, owned)
        with patch.object(metadata, 'regular', foreign): self.refuses()

    def test_schema(self):
        self.refuses(lambda: self.schema.write_bytes(b'4\n'))

    def test_schema_symlink(self):
        target=self.parent/'schema'; self.schema.rename(target); self.schema.symlink_to(target); self.refuses()

    def test_missing_manifest(self):
        self.refuses(lambda: (self.root / 'lib/rustlib' / ('manifest-'+metadata.ROWS[0])).unlink())

    def test_empty_manifest(self):
        self.refuses(lambda: (self.root / 'lib/rustlib' / ('manifest-'+metadata.ROWS[0])).write_bytes(b''))

    def test_component_directory(self):
        self.component.unlink(); self.component.mkdir(); self.refuses()

    def test_component_fifo(self):
        self.component.unlink(); os.mkfifo(self.component); self.refuses()

    def test_existing_destination(self):
        self.output.mkdir(); self.refuses()

    def test_symlink_destination(self):
        self.output.symlink_to(self.parent); self.refuses()

    def test_file_destination(self):
        self.output.write_bytes(b'owned occupied destination'); self.refuses()

    def test_original_receipt_failure_prevents_write(self):
        self.component.write_bytes(('\n'.join(reversed(metadata.ROWS))+'\n').encode())
        with patch.object(metadata, 'receipt_file', side_effect=OSError('owned injected receipt failure')): self.refuses()

    def test_public_installed_set_rejections(self):
        for text in ('\n'.join(metadata.ROWS[:-1]), '\n'.join(metadata.ROWS)+ '\nunknown',
                     '\n'.join((metadata.ROWS[0],)*4), '\n'.join(metadata.ROWS).replace('x86_64','aarch64')):
            with self.subTest(text=text), patch.object(metadata, 'command', return_value=text):
                with self.assertRaises(ValueError): metadata.installed(self.rustup)
                self.assertFalse(self.output.exists())

    def test_compiled_bytes_drift_refuses_admission(self):
        real = metadata.closure
        calls = [0]
        def drift(root):
            calls[0] += 1
            if calls[0] == 2: self.compiler.write_bytes(b'owned injected compiled byte drift')
            return real(root)
        with patch.object(metadata, 'closure', drift): self.refuses()
        self.assertTrue((self.output/'before.json').exists())
        self.assertFalse((self.output/'after.json').exists())

    def test_compiled_mode_drift_refuses_admission(self):
        real = metadata.closure; calls = [0]
        def drift(root):
            calls[0] += 1
            if calls[0] == 2: self.compiler.chmod(0o644)
            return real(root)
        with patch.object(metadata, 'closure', drift): self.refuses()

    def test_toolchain_symlink_identity_and_external_refusal(self):
        link = self.root/'bin/internal-link'; link.symlink_to('rustc')
        self.assertTrue(self.prepare()['compiled_toolchain_inputs_unchanged'])
        link.unlink(); link.symlink_to(self.rustup)
        self.output = self.parent/'another-receipt'
        self.refuses()

    def test_public_set_drift_refuses_admission(self):
        with patch.object(metadata, 'installed', side_effect=[list(metadata.ROWS), ['unknown']]): self.refuses()

    def test_public_inspection_metadata_drift_refuses_final_receipt(self):
        drift = ('\n'.join(reversed(metadata.ROWS)) + '\n').encode()
        calls = [0]
        def installed(rustup):
            calls[0] += 1
            if calls[0] == 2:
                self.component.write_bytes(drift)
            return sorted(metadata.ROWS)
        with patch.object(metadata, 'installed', installed), self.assertRaisesRegex(ValueError, 'components-final-byte-drift'):
            self.prepare()
        self.assertEqual(self.component.read_bytes(), drift)
        self.assertTrue((self.output/'before.json').exists())
        self.assertFalse((self.output/'after.json').exists())

    def test_late_public_inspection_compiled_bytes_refuse_final_receipt(self):
        calls = [0]
        def installed(rustup):
            calls[0] += 1
            if calls[0] == 2:
                self.compiler.write_bytes(b'owned late public-inspection compiled byte drift')
            return sorted(metadata.ROWS)
        with patch.object(metadata, 'installed', installed), self.assertRaisesRegex(ValueError, 'compiled-toolchain-input-drift'):
            self.prepare()
        self.assertTrue((self.output/'before.json').exists())
        self.assertFalse((self.output/'after.json').exists())

    def test_late_public_inspection_compiled_mode_refuses_final_receipt(self):
        calls = [0]
        def installed(rustup):
            calls[0] += 1
            if calls[0] == 2:
                self.compiler.chmod(0o644)
            return sorted(metadata.ROWS)
        with patch.object(metadata, 'installed', installed), self.assertRaisesRegex(ValueError, 'compiled-toolchain-input-drift'):
            self.prepare()
        self.assertTrue((self.output/'before.json').exists())
        self.assertFalse((self.output/'after.json').exists())

    def test_rustup_identity_drift_before_write(self):
        self.rustup.write_bytes(b'owned changed manager bytes'); self.refuses()

    def test_fd_identity_race_before_write(self):
        real = os.fstat
        def changed(fd):
            if os.readlink('/proc/self/fd/' + str(fd)) == str(self.component):
                return self.rustup.stat()
            return real(fd)
        with patch.object(metadata.os, 'fstat', changed): self.refuses()

    def test_unowned_root_before_write(self):
        with patch.object(metadata.os, 'getuid', return_value=os.getuid()+123): self.refuses()

    def test_prewrite_components_path_race(self):
        real = metadata.receipt_file
        def replace(directory, name, raw):
            real(directory, name, raw)
            if name == 'before.json':
                self.component.rename(self.parent/'retained-original')
                self.component.write_bytes(metadata.CANONICAL)
        with patch.object(metadata, 'receipt_file', replace), self.assertRaises(ValueError): self.prepare()
        self.assertEqual((self.parent/'retained-original').read_bytes(), metadata.CANONICAL)
        self.assertFalse((self.output/'after.json').exists())


class ScopeControls(unittest.TestCase):
    def check_scope(self, overrides=None, version=None, head=None, tree=None, status='', which=None, release=None, system='Linux', machine='x86_64', output='/home/runner/work/_temp/fvoci-rustup-ci-metadata'):
        env = {'CI':'true','GITHUB_ACTIONS':'true','GITHUB_JOB':'collaboration-build',
               'RUNNER_ENVIRONMENT':'github-hosted','RUNNER_OS':'Linux','RUNNER_ARCH':'X64',
               'HOME':'/home/runner','RUNNER_TEMP':'/home/runner/work/_temp',
               'GITHUB_WORKSPACE':str(Path.cwd()),'GITHUB_SHA':'5'*40}
        env.update(overrides or {})
        def command(argv, cwd=None):
            if argv[:2] == ['git','rev-parse']:
                return (head if head is not None else '5'*40) if argv[-1]=='HEAD' else (tree if tree is not None else '6'*40)
            if argv[:2] == ['git','status']: return status
            return version or 'rustup 1.29.1 (d95a37b6a 2026-08-13)'
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.dict(os.environ, env, clear=True))
            stack.enter_context(patch.object(metadata, 'owned_directory', side_effect=lambda p:Path(p)))
            stack.enter_context(patch.object(metadata, 'physical', side_effect=lambda p:Path(p)))
            stack.enter_context(patch.object(metadata, 'regular'))
            stack.enter_context(patch.object(metadata, 'command', command))
            stack.enter_context(patch.object(metadata.platform,'system',return_value=system))
            stack.enter_context(patch.object(metadata.platform,'machine',return_value=machine))
            stack.enter_context(patch.object(Path,'read_text',return_value=release or 'ID=ubuntu\nVERSION_ID="26.04"\n'))
            stack.enter_context(patch.object(Path,'read_bytes',return_value=b'owned synthetic executable'))
            stack.enter_context(patch.object(Path,'lstat',return_value=Path(__file__).stat()))
            stack.enter_context(patch.object(metadata.shutil,'which',return_value=which or '/home/runner/.cargo/bin/rustup'))
            return metadata.scope(output)

    def test_two_allocated_jobs_positive(self):
        for job in ('collaboration-build','collaboration-flow'):
            with self.subTest(job=job):
                root, _, receipt = self.check_scope({'GITHUB_JOB':job})
                self.assertEqual(str(root), '/home/runner/.rustup/toolchains/'+metadata.TOOLCHAIN)
                self.assertEqual(receipt['source_head'],'5'*40)
                self.assertEqual(receipt['source_tree'],'6'*40)

    def test_unallocated_environment_refusals(self):
        cases = {'CI':'false','GITHUB_ACTIONS':'false','GITHUB_JOB':'web-checks',
                 'RUNNER_ENVIRONMENT':'self-hosted','RUNNER_OS':'Windows','RUNNER_ARCH':'ARM64',
                 'HOME':'/home/other','RUSTUP_HOME':'/shared/rustup','GITHUB_SHA':'7'*40,
                 'GITHUB_WORKSPACE':'/other/checkout'}
        for name, value in cases.items():
            with self.subTest(name=name), self.assertRaises(ValueError): self.check_scope({name:value})

    def test_source_version_platform_refusals(self):
        cases = [{'version':'rustup 1.29.0 (abc 2026-01-01)'},{'head':'notasha'},{'tree':'invalid'},
                 {'status':'?? drift.py'},{'which':'/shared/rustup'},{'release':'ID=ubuntu\nVERSION_ID="24.04"'},
                 {'release':'ID=debian\nVERSION_ID="26.04"'},{'system':'Darwin'},{'machine':'aarch64'},
                 {'output':'/shared/receipts'}]
        for args in cases:
            with self.subTest(args=args), self.assertRaises(ValueError): self.check_scope(**args)

    def test_actual_cli_local_environment_refuses(self):
        result = subprocess.run(['python3','-B',str(Path(metadata.__file__)), '--output','/unused-owned-fixture-output'],
                                env={**os.environ,'CI':'false','GITHUB_ACTIONS':'false'},capture_output=True,text=True)
        self.assertEqual(result.returncode,1)
        self.assertIn('not-github-ci',result.stderr)
        self.assertNotIn('/unused-owned-fixture-output', result.stderr)


if __name__ == '__main__':
    unittest.main(verbosity=2)
