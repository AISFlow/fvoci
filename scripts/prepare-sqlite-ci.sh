#!/usr/bin/env bash
# Build-only entry: bash scripts/prepare-sqlite-ci.sh [--parent OWNED_DIR] -- cargo build --locked
# Requires native GCC/ar, Python 3, curl and LIBCLANG_PATH (Ubuntu 24: llvm-18;
# Debian bookworm builder: llvm-14). Never installs host packages or changes Cargo.
# The reviewed helper owns all source hashes, C flags and SQLite export policy.
set -euo pipefail
exec python3 - "$0" "$@" <<'PY'
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess
import sys
import tempfile


def main():
    parser = argparse.ArgumentParser(description='Pinned native SQLite prerequisite and root Cargo entry')
    parser.add_argument('--parent', help='existing task/run-owned directory; never a shared cache')
    parser.add_argument('--env-file', help='write verified shell exports after success')
    parser.add_argument('--github-env')
    parser.add_argument('--github-output')
    parser.add_argument('command', nargs=argparse.REMAINDER)
    args = parser.parse_args(sys.argv[2:])
    command = args.command
    if command[:1] == ['--']:
        command = command[1:]
    helper = Path(sys.argv[1]).absolute().with_name('prepare-sqlite-build.sh')
    helper_bytes = helper.read_bytes()
    # Read literals, never execute helper Python to discover the authoritative pin.
    tree = ast.parse(helper_bytes.decode().split("<<'PY'\n", 1)[1].rsplit('\nPY', 1)[0])
    pins = {n.targets[0].id: ast.literal_eval(n.value) for n in tree.body
            if isinstance(n, ast.Assign) and isinstance(n.targets[0], ast.Name)
            and n.targets[0].id in {'VERSION', 'SOURCE_ID'}}
    major, minor, patch = map(int, pins['VERSION'].split('.'))
    archive_name = f'sqlite-amalgamation-{major * 1000000 + minor * 10000 + patch * 100}.zip'
    url = f"https://www.sqlite.org/{pins['SOURCE_ID'][:4]}/{archive_name}"
    target = {'x86_64': 'x86_64-unknown-linux-gnu',
              'aarch64': 'aarch64-unknown-linux-gnu'}.get(platform.machine())
    if platform.system() != 'Linux' or not target:
        raise ValueError('only native Linux GNU x86_64/aarch64 builds supported')
    requested_targets = [os.environ.get('CARGO_BUILD_TARGET', target)]
    for i, arg in enumerate(command):
        if arg == '--target':
            requested_targets.append(command[i + 1] if i + 1 < len(command) else '')
        elif arg.startswith('--target='):
            requested_targets.append(arg.split('=', 1)[1])
    if any(t != target for t in requested_targets):
        raise ValueError('Cargo target must match the native SQLite GNU target')
    clang_dir = Path(os.environ.get('LIBCLANG_PATH', '/nonexistent'))
    clang = (clang_dir / 'libclang.so').resolve(strict=True)
    if not clang.is_file():
        raise ValueError('LIBCLANG_PATH must contain build-time libclang.so')
    for tool in ('curl', 'cc', 'ar', 'rustc'):
        if not shutil.which(tool):
            raise ValueError(f'build tool unavailable: {tool}')
    names = {'SQLITE3_LIB_DIR', 'SQLITE3_INCLUDE_DIR', 'SQLITE3_STATIC', 'SQLITE3_NO_PKG_CONFIG'}
    inherited = {k: os.environ[k] for k in names if k in os.environ}
    parent_arg = args.parent
    if not parent_arg and inherited:
        if set(inherited) != names:
            raise ValueError('partial SQLite environment refused')
        parent_arg = str(Path(inherited['SQLITE3_LIB_DIR']).parent.parent)
    parent = Path(os.path.abspath(parent_arg)) if parent_arg else Path(tempfile.mkdtemp(prefix='fvoci-sqlite-'))
    for p in (parent, *parent.parents):
        if p.is_symlink():
            raise ValueError(f'symlink build parent refused: {p}')
    if not parent.is_dir() or parent.stat().st_uid != os.getuid():
        raise ValueError('build parent must exist and be owned by the current user')
    prefix = parent / target
    if inherited and Path(inherited['SQLITE3_LIB_DIR']) != prefix / 'lib':
        raise ValueError('SQLite environment does not match the owned build prefix')
    archive = parent / archive_name
    # Existing archive is checked by the helper; never overwrite/re-download a bad cache.
    if not archive.exists():
        fd, download = tempfile.mkstemp(prefix='sqlite-download-', dir=parent)
        os.close(fd)
        try:
            subprocess.run(['curl', '--fail', '--silent', '--show-error', '--proto', '=https',
                            '--tlsv1.2', '--connect-timeout', '15', '--max-time', '90',
                            '--max-filesize', str(16 * 1024 * 1024), '--retry', '0',
                            '--output', download, url], check=True, timeout=95)
            # No replacement if another writer published an archive during download.
            os.link(download, archive)
        finally:
            os.unlink(download)
    result = subprocess.run(['bash', str(helper), '--archive', str(archive),
                             '--prefix', str(prefix), '--target', target],
                            check=True, capture_output=True, text=True, timeout=300)
    print(result.stderr, end='', file=sys.stderr)
    exports = result.stdout
    if exports != (prefix / 'env.sh').read_text():
        raise ValueError('helper export/file mismatch')
    env = {}
    for line in exports.splitlines():
        parts = shlex.split(line)
        if len(parts) != 2 or parts[0] != 'export' or '=' not in parts[1]:
            raise ValueError('unexpected helper export')
        key, value = parts[1].split('=', 1)
        if key not in names or key in env or '\n' in value or '\r' in value:
            raise ValueError('unexpected/duplicate helper environment')
        env[key] = value
    if set(env) != names or (inherited and inherited != env):
        raise ValueError('incomplete or mismatched SQLite environment')
    # Fresh/revalidated exact archive/header precede restoring Cargo output caches.
    # Manifest includes source/helper/profile/compiler/archiver/header/output hashes.
    rustc = subprocess.check_output(['rustc', '-vV'], text=True, timeout=30)
    if f'host: {target}' not in rustc.splitlines():
        raise ValueError('rustc host must match the native SQLite GNU target')
    build_env = {k: v for k, v in os.environ.items()
                 if k in {'LIBCLANG_PATH', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS', 'RUSTC',
                          'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'CARGO_BUILD_TARGET'}
                 or k.startswith(('BINDGEN_EXTRA_CLANG_ARGS', 'CARGO_PROFILE_', 'CARGO_TARGET_'))}
    identity = {'manifest': json.loads((prefix / 'manifest.json').read_text()),
                'exports': env, 'wrapper_sha256': hashlib.sha256(Path(sys.argv[1]).read_bytes()).hexdigest(),
                'python': sys.version,
                'packages': (parent / 'build-packages.txt').read_text() if (parent / 'build-packages.txt').is_file() else None,
                'libclang_path': str(clang), 'libclang_sha256': hashlib.sha256(clang.read_bytes()).hexdigest(),
                # Fingerprint build overrides without publishing their values.
                'build_environment_sha256': hashlib.sha256(json.dumps(build_env, sort_keys=True).encode()).hexdigest(),
                'rustc': rustc}
    (parent / 'consumer-inputs.json').write_text(json.dumps(identity, indent=2) + '\n')
    cache_identity = hashlib.sha256(json.dumps(identity, sort_keys=True).encode()).hexdigest()
    if args.env_file:
        Path(args.env_file).write_text(exports)
    if args.github_env:
        with open(args.github_env, 'a') as output:
            output.write(''.join(f'{k}={v}\n' for k, v in sorted(env.items())))
    if args.github_output:
        with open(args.github_output, 'a') as output:
            output.write(f'cache_identity={cache_identity}\n')
    if command:
        return subprocess.run(command, env={**os.environ, **env}).returncode
    if not (args.env_file or args.github_env):
        print(exports, end='')
    return 0


try:
    sys.exit(main())
except (ValueError, OSError, KeyError, subprocess.SubprocessError) as error:
    print(f'prepare-sqlite-ci: {error}', file=sys.stderr)
    if isinstance(error, subprocess.CalledProcessError) and error.stderr:
        print(error.stderr, end='', file=sys.stderr)
    sys.exit(1)
PY
