#!/usr/bin/env python3
"""Prepare only owned GitHub collaboration CI metadata before input capture.

Rustup 1.29.1's schema-3 components list has name-based semantics but an
unstable row order. Keep its exact installed set and every compiled byte.
This helper never installs, updates, or relaxes a build-handoff input check.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import stat
import subprocess
import sys

sys.dont_write_bytecode = True
TOOLCHAIN = '1.98.1-x86_64-unknown-linux-gnu'
ROWS = tuple(name + '-x86_64-unknown-linux-gnu' for name in ('cargo', 'clippy-preview', 'rust-std', 'rustc'))
# Rustup's public component list reverses the manifest's clippy -> clippy-preview rename.
PUBLIC_ROWS = tuple(name + '-x86_64-unknown-linux-gnu' for name in ('cargo', 'clippy', 'rust-std', 'rustc'))
CANONICAL = ('\n'.join(ROWS) + '\n').encode('ascii')
RELATIVE = 'lib/rustlib/components'


def require(condition, reason):
    if not condition:
        raise ValueError(reason)


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def physical(path):
    path = Path(path)
    require(path.is_absolute() and path.resolve() == path, 'nonphysical-path')
    for ancestor in (path, *path.parents):
        require(not ancestor.is_symlink(), 'symlink-ancestor')
    return path


def owned_directory(path):
    path = physical(path)
    info = path.lstat()
    require(stat.S_ISDIR(info.st_mode) and (info.st_uid, info.st_gid) == (os.getuid(), os.getgid()), 'unowned-directory')
    return path


def file_identity(info):
    return (info.st_dev, info.st_ino, info.st_mode, info.st_uid, info.st_gid, info.st_nlink)


def regular(path, mode=None, owned=False):
    physical(path)
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'nonregular-or-hardlinked-file')
    if mode is not None:
        require(stat.S_IMODE(info.st_mode) == mode, 'unexpected-file-mode')
    if owned:
        require((info.st_uid, info.st_gid) == (os.getuid(), os.getgid()), 'unowned-file')
    return info


def exact_rows(raw):
    require(len(raw) == 136 and raw.endswith(b'\n') and b'\r' not in raw, 'components-byte-format')
    rows = raw[:-1].decode('ascii').split('\n')
    require(len(rows) == 4 and len(set(rows)) == 4 and set(rows) == set(ROWS), 'components-set')
    return rows


def command(argv, cwd=None, strip=True):
    result = subprocess.run(argv, cwd=cwd, env={**os.environ, 'RUSTUP_AUTO_INSTALL': '0'},
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    require(result.returncode == 0, 'inspection-command-failed')
    require(len(result.stdout) <= 1024 * 1024, 'inspection-output-bound')
    text = result.stdout.decode('ascii')
    return text.strip() if strip else text


def scope(output):
    env = os.environ
    for name in ('CI', 'GITHUB_ACTIONS'):
        require(env.get(name) == 'true', 'not-github-ci')
    require(env.get('GITHUB_JOB') in ('collaboration-build', 'collaboration-flow'), 'unallocated-job')
    require(env.get('RUNNER_ENVIRONMENT') == 'github-hosted' and env.get('RUNNER_OS') == 'Linux'
            and env.get('RUNNER_ARCH') == 'X64', 'unsupported-runner')
    require(platform.system() == 'Linux' and platform.machine() == 'x86_64', 'unsupported-platform')
    release = dict(line.split('=', 1) for line in Path('/etc/os-release').read_text().splitlines() if '=' in line)
    require(release.get('ID', '').strip('"') == 'ubuntu' and release.get('VERSION_ID', '').strip('"') == '26.04', 'unsupported-os')
    home = owned_directory(Path(env.get('HOME', '')))
    require(home == Path('/home/runner'), 'not-ephemeral-runner-home')
    rustup_home = owned_directory(home / '.rustup')
    require(env.get('RUSTUP_HOME', str(rustup_home)) == str(rustup_home), 'rustup-home-override')
    owned_directory(rustup_home / 'toolchains')
    root = owned_directory(rustup_home / 'toolchains' / TOOLCHAIN)
    temporary = owned_directory(Path(env.get('RUNNER_TEMP', '')))
    require(Path(output) == temporary / 'fvoci-rustup-ci-metadata', 'receipt-prefix-override')
    workspace = physical(Path(env.get('GITHUB_WORKSPACE', '')))
    require(workspace == Path.cwd().resolve(), 'checkout-path-mismatch')
    head = command(['git', 'rev-parse', 'HEAD'], workspace)
    require(re.fullmatch('[0-9a-f]{40}', head) is not None and head == env.get('GITHUB_SHA'), 'source-head-mismatch')
    tree = command(['git', 'rev-parse', 'HEAD^{tree}'], workspace)
    require(re.fullmatch('[0-9a-f]{40}', tree) is not None, 'source-tree-format')
    require(command(['git', 'status', '--porcelain=v1', '--untracked-files=all'], workspace) == '', 'source-drift')
    rustup = physical(home / '.cargo/bin/rustup')
    regular(rustup, mode=0o755)
    require(shutil.which('rustup') == str(rustup), 'rustup-executable-mismatch')
    version = command([str(rustup), '--version'])
    require(re.fullmatch(r'rustup 1\.29\.1 \([0-9a-f]+ [0-9]{4}-[0-9]{2}-[0-9]{2}\)', version) is not None, 'unsupported-rustup-version')
    return root, rustup, {'source_head': head, 'source_tree': tree, 'job': env['GITHUB_JOB'],
                          'rustup_version': version, 'rustup_sha256': sha(rustup.read_bytes()),
                          'rustup_identity': file_identity(rustup.lstat())}


def installed(rustup):
    text = command([str(rustup), 'component', 'list', '--installed', '--toolchain', TOOLCHAIN], strip=False)
    rows = text.removesuffix('\n').split('\n')
    # Never echo unknown CLI output: only fixed public labels, counts and its raw hash.
    print(json.dumps({'rustup_installed': {'recognized': sorted(set(rows) & set(PUBLIC_ROWS)),
                                         'row_count': len(rows),
                                         'unknown_count': sum(row not in PUBLIC_ROWS for row in rows),
                                         'raw_sha256': sha(text.encode('ascii'))}}))
    require(len(rows) == 4 and len(set(rows)) == 4 and set(rows) == set(PUBLIC_ROWS), 'public-installed-set')
    return sorted(rows)


def closure(root):
    """Stream the exact toolchain subtree, preserving all other file identities."""
    entries, total = [], 0
    for path in sorted(root.rglob('*')):
        relative = str(path.relative_to(root))
        if relative == RELATIVE:
            continue
        info = path.lstat()
        facts = [relative, *file_identity(info)]
        if stat.S_ISREG(info.st_mode):
            total += info.st_size
            require(total <= 4 * 1024**3, 'toolchain-byte-bound')
            digest = hashlib.sha256()
            with path.open('rb') as stream:
                require(file_identity(os.fstat(stream.fileno())) == file_identity(info), 'closure-file-race')
                for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                    digest.update(chunk)
                require(file_identity(path.lstat()) == file_identity(info), 'closure-file-race')
            facts.extend([info.st_size, digest.hexdigest()])
        elif stat.S_ISLNK(info.st_mode):
            require(path.resolve().is_relative_to(root), 'external-toolchain-symlink')
            facts.append(os.readlink(path))
        else:
            require(stat.S_ISDIR(info.st_mode), 'unsupported-toolchain-entry')
        entries.append(facts)
        require(len(entries) <= 50000, 'toolchain-entry-bound')
    encoded = json.dumps(entries, separators=(',', ':')).encode()
    return {'entries': entries, 'entry_count': len(entries), 'regular_file_bytes': total, 'sha256': sha(encoded)}


def receipt_file(directory, name, raw):
    path = directory / name
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(fd, 'wb') as stream:
        require(stat.S_IMODE(os.fstat(stream.fileno()).st_mode) == 0o600, 'receipt-mode')
        stream.write(raw)
        stream.flush()
        os.fsync(stream.fileno())


def prepare(root, output, rustup, context):
    root, output, rustup = Path(root), Path(output), Path(rustup)
    owned_directory(root)
    owned_directory(root / 'lib')
    owned_directory(root / 'lib/rustlib')
    component = root / RELATIVE
    original_info = regular(component, mode=0o644, owned=True)
    schema = root / 'lib/rustlib/rust-installer-version'
    regular(schema, owned=True)
    require(schema.read_bytes() in (b'3', b'3\n'), 'unsupported-installer-schema')
    for name in ROWS:
        manifest = root / 'lib/rustlib' / ('manifest-' + name)
        regular(manifest, owned=True)
        require(manifest.stat().st_size > 0, 'missing-installed-manifest')
    before_set = installed(rustup)
    before_closure = closure(root)
    before_rustup = (sha(rustup.read_bytes()), file_identity(rustup.lstat()))
    require(before_rustup == (context['rustup_sha256'], tuple(context['rustup_identity'])), 'rustup-identity-drift')
    owned_directory(output.parent)
    # mkdir refuses every existing destination, including symlinks and files.
    with open(component, 'r+b', opener=lambda path, flags: os.open(path, flags | os.O_NOFOLLOW)) as current:
        require(file_identity(os.fstat(current.fileno())) == file_identity(original_info), 'components-fd-race')
        raw = current.read(4096)
        order = exact_rows(raw)
        output.mkdir(mode=0o700)
        require(stat.S_IMODE(owned_directory(output).lstat().st_mode) == 0o700, 'receipt-directory-mode')
        receipt_file(output, 'original-components.txt', raw)
        before = {**context, 'toolchain': TOOLCHAIN, 'installer_schema': 3,
                  'original_order': order, 'original_sha256': sha(raw),
                  'component_identity': file_identity(original_info),
                  'public_installed': before_set, 'other_toolchain_inputs': before_closure}
        receipt_file(output, 'before.json', (json.dumps(before, sort_keys=True, indent=2) + '\n').encode())
        require(file_identity(component.lstat()) == file_identity(original_info), 'components-path-race')
        current.seek(0)
        require(current.read() == raw, 'components-byte-race')
        if raw != CANONICAL:
            current.seek(0)
            require(current.write(CANONICAL) == len(CANONICAL), 'components-short-write')
            current.flush()
            os.fsync(current.fileno())
        current.seek(0)
        require(current.read() == CANONICAL, 'components-write-verification')
        require(file_identity(os.fstat(current.fileno())) == file_identity(original_info)
                and file_identity(component.lstat()) == file_identity(original_info), 'components-identity-drift')
    require(installed(rustup) == before_set, 'public-installed-set-drift')
    require((sha(rustup.read_bytes()), file_identity(rustup.lstat())) == before_rustup, 'rustup-identity-drift')
    after_closure = closure(root)
    require(after_closure == before_closure, 'compiled-toolchain-input-drift')
    with open(component, 'rb', opener=lambda path, flags: os.open(path, flags | os.O_NOFOLLOW)) as final:
        require(file_identity(os.fstat(final.fileno())) == file_identity(original_info)
                and file_identity(component.lstat()) == file_identity(original_info), 'components-final-identity-drift')
        require(final.read(4096) == CANONICAL, 'components-final-byte-drift')
    after = {**context, 'changed': raw != CANONICAL, 'canonical_order': list(ROWS),
             'canonical_sha256': sha(CANONICAL), 'component_identity': file_identity(component.lstat()),
             'other_toolchain_inputs_sha256': after_closure['sha256'],
             'other_toolchain_entry_count': after_closure['entry_count'],
             'other_toolchain_regular_file_bytes': after_closure['regular_file_bytes'],
             'public_installed_set_unchanged': True, 'compiled_toolchain_inputs_unchanged': True}
    receipt_file(output, 'after.json', (json.dumps(after, sort_keys=True, indent=2) + '\n').encode())
    return after


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', required=True)
    args = parser.parse_args()
    root, rustup, context = scope(args.output)
    result = prepare(root, Path(args.output), rustup, context)
    print(json.dumps({name: result[name] for name in ('source_head', 'source_tree', 'changed', 'canonical_sha256',
                                                   'other_toolchain_inputs_sha256', 'other_toolchain_entry_count')}))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, UnicodeError) as error:
        # Never echo subprocess stderr, environment, or private filesystem paths.
        print('Rustup CI metadata preparation refused: ' + (str(error) if isinstance(error, ValueError) else type(error).__name__), file=sys.stderr)
        raise SystemExit(1)
