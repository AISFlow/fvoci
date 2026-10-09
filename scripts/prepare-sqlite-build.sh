#!/usr/bin/env bash
# Build-only prerequisite for SQLx 0.8.6 sqlite-unbundled/libsqlite3-sys 0.30.1.
# No downloads, system installation, Cargo source edits, CLI or extension artifacts.
# Usage: bash scripts/prepare-sqlite-build.sh --archive FILE --prefix NEW_PATH
#        --target x86_64-unknown-linux-gnu [--cc cc] [--ar ar]
# Source PREFIX/env.sh only after success, with the matching Rust target/features.
# Python stdlib handles authenticated ZIP extraction and build provenance only.
set -euo pipefail
exec python3 - "$0" "$@" <<'PY'
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import zipfile

ARCHIVE_HASH = '1e71ddf93849c6a6ecf58b827c0692073d2dd7ee40196158068f7b29f422e87d'
C_HASH = '67f423e9ebbbdc473cbc4772c872ee6b89f31fde4ed0279a5c25d5f65c043a16'
HEADER_HASH = '919e7f2e8ed1d8f56ac17b412b8971c76aa5d1a879752cc6058f75e7d5910e1d'
SOURCE_ID = '2026-07-24 19:02:57 bf7c7f30031888f4e796e429ab3978879485813aaca6f641c7b33e4e09459bcc'
VERSION = '3.53.4'
# Required SQLx C APIs plus conservative libsqlite3-sys bundled defaults.
# JSON is built in upstream. Optional FTS/RTREE/preupdate features are not enabled.
# SQLx unconditionally references load_extension; retain its API, disabled by
# default at connection open. No extension is built, loaded, or installed here.
FLAGS = ['-O2', '-fPIC', '-std=c11', '-pthread', '-DSQLITE_CORE',
         '-DSQLITE_THREADSAFE=1', '-DSQLITE_DEFAULT_FOREIGN_KEYS=1',
         '-DSQLITE_ENABLE_API_ARMOR', '-DSQLITE_ENABLE_COLUMN_METADATA',
         '-DSQLITE_ENABLE_UNLOCK_NOTIFY',
         '-DSQLITE_USE_URI', '-DHAVE_USLEEP=1', '-DHAVE_ISNAN=1']
ENV = {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C', 'LANG': 'C'}
# Required consumer inputs cannot be removed by editing the cache manifest.
REQUIRED_OUTPUTS = {'env.sh', 'include/sqlite3.h', 'lib/libsqlite3.a',
                    'proof.txt', 'smoke', 'smoke.c', 'sqlite3.c', 'sqlite3.o'}
TIMINGS = {}
COMPILE_TIMEOUT = 180
# Hosted Rust run 37740220181 postgres-c ar exceeded 30s; use the C compile's bounded 180s class.
ARCHIVE_TIMEOUT = COMPILE_TIMEOUT


def fail(message):
    raise ValueError(message)


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run(argv, timeout=30, step='inspection'):
    print('+ ' + shlex.join(map(str, argv)), file=sys.stderr)
    started = time.monotonic()
    status = -1
    try:
        result = subprocess.run(list(map(str, argv)), env=ENV, capture_output=True,
                                text=True, timeout=timeout)
        status = result.returncode
    finally:
        TIMINGS[step] = {'elapsed_seconds': round(min(time.monotonic() - started, 86400), 6),
                         'timeout_seconds': timeout, 'exit_code': status}
        print('sqlite-prepare step=' + step + ' ' + json.dumps(TIMINGS[step], sort_keys=True), file=sys.stderr)
    if result.returncode:
        fail(f'command exit {result.returncode}: {result.stdout}{result.stderr}')
    if result.stderr:
        print(result.stderr, end='', file=sys.stderr)
    return result.stdout


def tool(name):
    executable = shutil.which(name, path=os.environ.get('PATH'))
    if not executable:
        fail(f'tool unavailable: {name}')
    path = Path(executable).resolve(strict=True)
    return path, {'path': str(path), 'sha256': digest(path.read_bytes()),
                  'version': run([path, '--version'], step=name + '_version')}


def no_symlinks(path):
    for part in (path, *path.parents):
        if part.is_symlink():
            fail(f'symlink path refused: {part}')


# Every function API used by the default SQLx SQLite driver, including metadata,
# notify, hooks, serialize/deserialize, progress and extension entry points.
# Type names and optional preupdate/regexp implementation names are excluded.
SYMBOLS = '''bind_blob64 bind_double bind_int bind_int64 bind_null
bind_parameter_count bind_parameter_name bind_text64 busy_timeout changes
clear_bindings close column_blob column_bytes column_count column_database_name
column_decltype column_double column_int column_int64 column_name
column_origin_name column_table_name column_type column_value commit_hook
create_collation_v2 create_function_v2 db_config db_handle deserialize errmsg
errstr exec extended_errcode extended_result_codes finalize free get_autocommit
get_auxdata last_insert_rowid load_extension malloc malloc64 open open_v2
prepare_v2 prepare_v3 progress_handler reset result_error_code result_int
rollback_hook serialize set_auxdata sql step stmt_readonly table_column_metadata
unlock_notify update_hook value_blob value_bytes value_double value_dup
value_free value_int value_int64 value_text value_type'''.split()
SMOKE = r'''
#include "sqlite3.h"
#include <stdio.h>
#include <string.h>
static void (*volatile required[])(void) = { SYMBOL_ADDRESSES };
static int scalar(sqlite3 *db, const char *sql, int expected) {
    sqlite3_stmt *s = 0;
    int ok = sqlite3_prepare_v2(db, sql, -1, &s, 0) == SQLITE_OK &&
        sqlite3_step(s) == SQLITE_ROW && sqlite3_column_int(s, 0) == expected;
    sqlite3_finalize(s);
    return ok;
}
static int check(sqlite3 *db) {
    sqlite3_stmt *s = 0;
    int enabled = -1;
    if (sqlite3_db_config(db, SQLITE_DBCONFIG_ENABLE_LOAD_EXTENSION, -1,
                         &enabled) != SQLITE_OK || enabled != 0) return 10;
    if (!scalar(db, "PRAGMA foreign_keys", 1)) return 11;
    if (sqlite3_exec(db, "PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;",
                     0, 0, 0) != SQLITE_OK ||
        !scalar(db, "PRAGMA synchronous", 2)) return 19;
    if (sqlite3_prepare_v2(db, "PRAGMA journal_mode", -1, &s, 0) != SQLITE_OK) return 20;
    int wal = sqlite3_step(s) == SQLITE_ROW &&
        strcmp((const char *)sqlite3_column_text(s, 0), "wal") == 0;
    sqlite3_finalize(s);
    s = 0;
    if (!wal) return 21;
    if (sqlite3_exec(db, "CREATE TABLE p(id INTEGER PRIMARY KEY);"
        "CREATE TABLE c(id INTEGER REFERENCES p(id));BEGIN IMMEDIATE;"
        "INSERT INTO p VALUES(1);ROLLBACK;", 0, 0, 0) != SQLITE_OK) return 12;
    if (!sqlite3_get_autocommit(db) || !scalar(db, "SELECT count(*) FROM p", 0)) return 13;
    if (sqlite3_exec(db, "INSERT INTO c VALUES(999)", 0, 0, 0) != SQLITE_CONSTRAINT) return 14;
    if (sqlite3_exec(db, "BEGIN IMMEDIATE;INSERT INTO p VALUES(1);"
        "INSERT INTO c VALUES(1);COMMIT;", 0, 0, 0) != SQLITE_OK) return 15;
    if (!scalar(db, "SELECT count(*) FROM c", 1) ||
        !scalar(db, "SELECT json_valid('{\"ok\":true}')", 1)) return 16;
    if (sqlite3_prepare_v3(db, "SELECT id FROM p", -1, SQLITE_PREPARE_PERSISTENT,
                         &s, 0) != SQLITE_OK) return 17;
    const char *origin = sqlite3_column_origin_name(s, 0);
    int ok = origin && strcmp(origin, "id") == 0;
    sqlite3_finalize(s);
    if (!ok || sqlite3_unlock_notify(db, 0, 0) != SQLITE_OK) return 18;
    return 0;
}
int main(int argc, char **argv) {
    if (argc != 2) return 7;
    for (unsigned i = 0; i < sizeof(required)/sizeof(required[0]); ++i)
        if (!required[i]) return 1;
    if (strcmp(sqlite3_libversion(), "3.53.4") ||
        strcmp(sqlite3_sourceid(), "EXPECTED_SOURCE_ID") ||
        sqlite3_libversion_number() != 3053004 || sqlite3_threadsafe() != 1) return 2;
    const char *needed[] = {"ENABLE_COLUMN_METADATA", "ENABLE_UNLOCK_NOTIFY",
        "ENABLE_API_ARMOR", "DEFAULT_FOREIGN_KEYS", "THREADSAFE=1", "USE_URI"};
    for (unsigned i = 0; i < sizeof(needed)/sizeof(needed[0]); ++i)
        if (!sqlite3_compileoption_used(needed[i])) return 3;
    const char *omitted[] = {"OMIT_FOREIGN_KEY", "OMIT_TRIGGER", "OMIT_WAL",
        "OMIT_LOAD_EXTENSION", "OMIT_PROGRESS_CALLBACK", "OMIT_SHARED_CACHE",
        "ENABLE_LOAD_EXTENSION"};
    for (unsigned i = 0; i < sizeof(omitted)/sizeof(omitted[0]); ++i)
        if (sqlite3_compileoption_used(omitted[i])) return 4;
    sqlite3 *db = 0;
    if (sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READWRITE |
        SQLITE_OPEN_CREATE | SQLITE_OPEN_NOMUTEX, 0) != SQLITE_OK) return 5;
    int result = check(db);
    if (sqlite3_close(db) != SQLITE_OK) return 6;
    if (result) return result;
    if (sqlite3_open_v2(argv[1], &db, SQLITE_OPEN_READWRITE |
        SQLITE_OPEN_NOMUTEX, 0) != SQLITE_OK) return 22;
    if (!scalar(db, "SELECT count(*) FROM c", 1) ||
        !scalar(db, "PRAGMA foreign_keys", 1)) return 23;
    if (sqlite3_close(db) != SQLITE_OK) return 24;
    printf("version=%s\nsource_id=%s\n", sqlite3_libversion(), sqlite3_sourceid());
    for (int i = 0; sqlite3_compileoption_get(i); ++i)
        printf("compile_option=%s\n", sqlite3_compileoption_get(i));
    puts("sqlx_symbols/metadata/unlock_notify/threadsafe/fk/rollback/commit/json/wal/full/fresh_connection/extensions_disabled=PASS");
    return 0;
}
'''.replace('EXPECTED_SOURCE_ID', SOURCE_ID).replace(
    'SYMBOL_ADDRESSES', ', '.join('(void (*)(void))sqlite3_' + s for s in SYMBOLS))


def smoke(prefix):
    with tempfile.TemporaryDirectory(prefix='smoke-run-', dir=prefix) as directory:
        return run([prefix / 'smoke', Path(directory) / 'smoke.db'], step='smoke_run')


def main():
    parser = argparse.ArgumentParser(description='Authenticated native SQLite static build; no fallback')
    parser.add_argument('--archive', required=True)
    parser.add_argument('--prefix', required=True)
    parser.add_argument('--target', required=True)
    parser.add_argument('--cc', default='cc', help='single native compiler executable')
    parser.add_argument('--ar', default='ar', help='single archiver executable')
    parser.add_argument('--identity-only', action='store_true', help='authenticate inputs without compiling or exporting a usable prefix')
    args = parser.parse_args(sys.argv[2:])
    archive = Path(os.path.abspath(args.archive))
    prefix = Path(os.path.abspath(args.prefix))
    no_symlinks(archive)
    no_symlinks(prefix)
    if not archive.is_file() or archive.stat().st_size > 16 * 1024 * 1024:
        fail('archive must be a regular file no larger than 16 MiB')
    data = archive.read_bytes()
    if digest(data) != ARCHIVE_HASH:
        fail('archive SHA-256 mismatch; no source extraction/build/fallback')
    with zipfile.ZipFile(io.BytesIO(data)) as source:
        root = 'sqlite-amalgamation-3530400/'
        expected = {root, *(root + n for n in ('sqlite3.c', 'sqlite3.h', 'sqlite3ext.h', 'shell.c'))}
        entries = source.infolist()
        if len(entries) != len(expected) or {i.filename for i in entries} != expected:
            fail('unexpected archive entries')
        for entry in entries:
            if entry.file_size > 16 * 1024 * 1024 or stat.S_ISLNK(entry.external_attr >> 16):
                fail('unsafe archive entry')
        c_source = source.read(root + 'sqlite3.c')
        header = source.read(root + 'sqlite3.h')
    if hashlib.sha3_256(c_source).hexdigest() != C_HASH:
        fail('sqlite3.c official SHA3-256 mismatch')
    if digest(header) != HEADER_HASH:
        fail('pinned sqlite3.h SHA-256 mismatch')
    for content in (c_source, header):
        if (f'"{SOURCE_ID}"'.encode() not in content or
                f'"{VERSION}"'.encode() not in content):
            fail('source/header version or source ID mismatch')
    targets = {'x86_64': ('x86_64-unknown-linux-gnu', 'x86_64-linux-gnu'),
               'aarch64': ('aarch64-unknown-linux-gnu', 'aarch64-linux-gnu')}
    host = targets.get(platform.machine())
    if platform.system() != 'Linux' or not host or args.target != host[0]:
        fail('only explicit matching native Linux GNU x86_64/aarch64 targets supported; no cross build')
    cc, cc_id = tool(args.cc)
    ar, ar_id = tool(args.ar)
    machine = run([cc, '-dumpmachine'], step='compiler_target').strip()
    if machine != host[1]:
        fail(f'compiler target mismatch: {machine}, expected {host[1]}')
    if not prefix.parent.is_dir() or prefix.parent.stat().st_uid != os.getuid():
        fail('prefix parent must be an existing directory owned by the current user')
    inputs = {'schema': 1, 'archive_sha256': ARCHIVE_HASH, 'sqlite3_c_sha3_256': C_HASH,
              'header_sha256': digest(header), 'source_id': SOURCE_ID, 'version': VERSION,
              'helper_sha256': digest(Path(sys.argv[1]).read_bytes()),
              'compiler': cc_id, 'archiver': ar_id, 'compiler_target': machine,
              'target': args.target, 'host_arch': platform.machine(), 'flags': FLAGS,
              'link_flags': ['-pthread', '-ldl', '-lm'], 'environment': ENV}
    env_text = ''.join(f'export {k}={shlex.quote(v)}\n' for k, v in {
        'SQLITE3_LIB_DIR': str(prefix / 'lib'), 'SQLITE3_INCLUDE_DIR': str(prefix / 'include'),
        'SQLITE3_STATIC': '1', 'SQLITE3_NO_PKG_CONFIG': '1'}.items())
    if args.identity_only:
        print(json.dumps({'inputs': inputs, 'exports': env_text, 'timings': TIMINGS}))
        return
    lock = prefix.with_name(prefix.name + '.lock')
    # O_EXCL refuses another writer or a stale lock without touching its files.
    fd = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    os.close(fd)
    stage = None
    try:
        if prefix.exists():
            if not prefix.is_dir() or prefix.stat().st_uid != os.getuid():
                fail('existing prefix is not an owned build directory')
            manifest = json.loads((prefix / 'manifest.json').read_text())
            if manifest['inputs'] != inputs:
                fail('existing build input identity differs; use a new owned prefix')
            if (not isinstance(manifest['outputs'], dict) or
                    set(manifest['outputs']) != REQUIRED_OUTPUTS):
                fail('existing prefix required output inventory differs')
            actual = {str(p.relative_to(prefix)) for p in prefix.rglob('*') if not p.is_dir()}
            if actual != {*REQUIRED_OUTPUTS, 'manifest.json'}:
                fail('existing prefix output inventory differs')
            for name in sorted(REQUIRED_OUTPUTS):
                p = prefix / name
                no_symlinks(p)
                if (not p.is_file() or
                        digest(p.read_bytes()) != manifest['outputs'][name]):
                    fail(f'existing build output hash/type mismatch: {name}')
            if (prefix / 'env.sh').read_text() != env_text:
                fail('existing build environment differs')
            proof = smoke(prefix)
            if proof != manifest['proof']:
                fail('cached actual smoke proof differs')
            print('Verified same-input SQLite build reuse', file=sys.stderr)
        else:
            stage = Path(tempfile.mkdtemp(prefix=prefix.name + '.build-', dir=prefix.parent))
            (stage / 'lib').mkdir()
            (stage / 'include').mkdir()
            (stage / 'sqlite3.c').write_bytes(c_source)
            (stage / 'include/sqlite3.h').write_bytes(header)
            (stage / 'smoke.c').write_text(SMOKE)
            run([cc, *FLAGS, '-c', stage / 'sqlite3.c', '-o', stage / 'sqlite3.o'], timeout=COMPILE_TIMEOUT, step='compile')
            run([ar, 'rcs', stage / 'lib/libsqlite3.a', stage / 'sqlite3.o'], timeout=ARCHIVE_TIMEOUT, step='archive')
            # Exact archive filename: never -lsqlite3/-L or a system lookup.
            run([cc, '-std=c11', '-I', stage / 'include', stage / 'smoke.c',
                 stage / 'lib/libsqlite3.a', '-pthread', '-ldl', '-lm', '-o', stage / 'smoke'], step='smoke_link')
            proof = smoke(stage)
            (stage / 'proof.txt').write_text(proof)
            (stage / 'env.sh').write_text(env_text)
            outputs = {str(p.relative_to(stage)): digest(p.read_bytes())
                       for p in sorted(stage.rglob('*')) if p.is_file()}
            (stage / 'manifest.json').write_text(json.dumps(
                {'inputs': inputs, 'outputs': outputs, 'proof': proof}, indent=2) + '\n')
            if prefix.exists():
                fail('prefix appeared during build; refusing overwrite')
            stage.rename(prefix)
            stage = None
            print('Built and linked pinned SQLite 3.53.4', file=sys.stderr)
        # Keep elapsed provenance outside the prefix: Web hashes every prefix file.
        prefix.with_name(prefix.name + '.prepare-timings.json').write_text(json.dumps(TIMINGS, indent=2) + '\n')
        print(env_text, end='')
    finally:
        if stage is not None:
            (stage / 'failure-timings.json').write_text(json.dumps(TIMINGS, indent=2) + '\n')
            print(f'Incomplete build preserved for diagnosis: {stage}', file=sys.stderr)
        lock.unlink()


try:
    main()
except (ValueError, OSError, KeyError, zipfile.BadZipFile,
        subprocess.SubprocessError) as error:
    print(f'prepare-sqlite-build: {error}', file=sys.stderr)
    sys.exit(1)
PY
