#!/usr/bin/env python3
"""Pure packet/physical-byte controls; fake ELF/tool metadata, no native execution."""
import ast
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import stat
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[3]
spec = importlib.util.spec_from_file_location('handoff', ROOT / 'scripts/selected-backend-ci/web-build-handoff.py')
H = importlib.util.module_from_spec(spec); spec.loader.exec_module(H)
ORIGINAL_IDENTITY = H.CI.identity
SHA = 'a' * 40
TREE = 'b' * 40

# Frozen c7 selected-status footer: test data, independent of checkout history.
# Source c7ad4a2a9165a0dfb64ebc7f17150140b54ed1fe:scripts/run-web-e2e.sh
# Exact suffix SHA256 14b99c809b2a3c0ed445774f334378e74f38db5ce0abe373e47abf30c43fcd06; preserves the unsafe original execution.
ORIGINAL_C7_SELECTED_FOOTER = r"""selected_status=0
if [[ "$SELECTED_BACKENDS" == true ]]; then
  # Mandatory companion is attempted even after pending failure; keep its first status.
  # Preparation/build and original pending suite keep the existing CI runner UID.
  # Only this job-owned output/native prefix transfers to the1000 runtime actor.
  : "${FVOCI_SELECTED_CI_SQLITE_PARENT:?required exact job-owned SQLite parent}"
  python3 - "$SQLITE3_LIB_DIR" "$FVOCI_SELECTED_CI_SQLITE_PARENT" <<'PY_PARENT'
from pathlib import Path
import sys
assert Path(sys.argv[1]).resolve().is_relative_to(Path(sys.argv[2]).resolve())
PY_PARENT
  sudo chown -R 1000:1000 "$FVOCI_SELECTED_CI_OUTPUT" "$FVOCI_SELECTED_CI_SQLITE_PARENT"
  sudo install -d -o 1000 -g 1000 -m 0700 "$FVOCI_SELECTED_CI_OUTPUT/tmp"
  docker_gid="$(stat -c %g /var/run/docker.sock)"
  sudo --preserve-env=PATH,CI,GITHUB_ACTIONS,GITHUB_SHA,GITHUB_REPOSITORY,GITHUB_RUN_ID,GITHUB_RUN_ATTEMPT,GITHUB_JOB,PLAYWRIGHT_BROWSERS_PATH \
    setpriv --reuid=1000 --regid=1000 --groups="$docker_gid" \
    env TMPDIR="$FVOCI_SELECTED_CI_OUTPUT/tmp" \
      python3 "$ROOT/scripts/run-selected-backend-e2e.py" run --output "$FVOCI_SELECTED_CI_OUTPUT" || selected_status=$?
fi
if [[ "$pending_status" -ne 0 ]]; then exit "$pending_status"; fi
exit "$selected_status"
"""


class PacketTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(dir=ROOT / "scripts/fixtures/web-e2e"); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'repo'; self.root.mkdir()
        self.output = Path(self.tmp.name) / 'output'; self.output.mkdir(mode=0o700)
        self.packet = Path(self.tmp.name) / 'packet'
        self.header = self.root / 'header'; self.header.write_bytes(b'official fixture header')
        self.library = self.root / 'lib-fixture.so'; self.library.write_bytes(b'NOT ELF qualified library fixture')
        self.ldd_result = type('FixtureLdd', (), {'returncode':0, 'stdout':self.ldd_text('0x2222'), 'stderr':''})()
        self.dist = self.root / 'apps/web/dist'; self.dist.mkdir(parents=True); (self.dist / 'index.html').write_bytes(b'fresh fixture dist')
        self.target = self.root / 'target'
        env = {'CI':'true','GITHUB_ACTIONS':'true','GITHUB_JOB':'collaboration-build','FVOCI_WEB_BUILD_PHASE':'prepare',
               'GITHUB_REPOSITORY':'AISFlow/fvoci','GITHUB_RUN_ID':'123','GITHUB_RUN_ATTEMPT':'1','GITHUB_SHA':SHA,
               'FVOCI_SELECTED_CI_OUTPUT':str(self.output),'FVOCI_WEB_BUILD_HANDOFF':str(self.packet),
               'CARGO_TARGET_DIR':str(self.target),'GITHUB_OUTPUT':str(Path(self.tmp.name) / 'github-output'),
               'SQLITE3_LIB_DIR':'/fixed/lib','SQLITE3_INCLUDE_DIR':'/fixed/include','SQLITE3_STATIC':'1','SQLITE3_NO_PKG_CONFIG':'1'}
        self.env = patch.dict(os.environ, env, clear=True); self.env.start(); self.addCleanup(self.env.stop)
        self.patches = [patch.object(H,'ROOT',self.root),patch.object(H.CI,'identity',return_value='fixture'),
                        patch.object(H.CI.shutil,'disk_usage',return_value=type('FixtureSpace', (), {'free':20_000_000_000})()),
                        patch.object(H.CI,'inputs',side_effect=self.inputs),patch.object(H.CI,'build_env',return_value={'fixture-env':'fixed'}),
                        patch.object(H.CI,'abi_files',return_value=[str(self.library)]),patch.object(H.CI,'call',side_effect=self.call),
                        patch.object(H.CI.subprocess,'run',side_effect=self.ldd)]
        for p in self.patches:p.start();self.addCleanup(p.stop)
        self.put('before.json',self.inputs()); self.put('after.json',self.inputs()); self.put('build-env-inputs.json',{'fixture-env':'fixed'})
        stages=[]
        for n in H.STAGES:
            self.put(n+'-stage.json',{'source':SHA,'tree':TREE,'command':['actual-fixed-fixture',n],'exit_code':0,'seconds':1})
            (self.output/(n+'-compiler.jsonl')).write_text('{}\n')
            stages.append({**self.get(n+'-stage.json'),'compilerMessages':H.CI.reference(self.output/(n+'-compiler.jsonl'))})
        self.put('compile-receipt.json',{'source':SHA,'tree':TREE,'exit_code':0,'full_inputs_unchanged':True,'stages':stages})
        bins={}
        for name in ['fvoci-server','fvoci-migrate','fvoci-e2e-fixture','fvoci_server','selected_install_lifetime','collab-engine']:
            p=self.target/'debug'/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_bytes(('NOT ELF fixture '+name).encode());p.chmod(0o755)
            bins[str(p)]={'sha256':H.CI.sha(p),'bytes':p.stat().st_size,'target':{'name':name},'compiledSource':SHA,'targetTriple':'x86_64-unknown-linux-gnu',
                          'features':['default','worker'] if name=='collab-engine' else ['api-schema','db-tests'],
                          'profile':{'test':name in ('fvoci_server','selected_install_lifetime')}}
        core=self.target/'debug/deps/libfvoci_server.rlib';core.parent.mkdir();core.write_bytes(b'NOT RLIB fixture')
        self.put('bundle.json',{'source':SHA,'tree':TREE,'full_inputs_unchanged':True,'binaries':bins,'compiler_artifacts':[
            {'target':{'name':'fvoci_server'},'profile':{'test':False},'features':['api-schema','db-tests'],'filenames':[str(core)]}]})
        self.put('build-environment.json',{'rustc':self.call(['rustc','-Vv']),'cargo':'cargo 1.98.1','bun':'1.4.2',
            'os_release':Path('/etc/os-release').read_text(),'target':str(self.target),'features':['api-schema','db-tests'],'nativeFeatures':['worker'],
            'profile':'debug','devDebug':'0','testDebug':'0','sqlite':{k:os.environ[k] for k in ['SQLITE3_LIB_DIR','SQLITE3_INCLUDE_DIR','SQLITE3_STATIC','SQLITE3_NO_PKG_CONFIG']}})
        self.put('web-receipt.json',{'source':SHA,'tree':TREE,'exit_code':0,'full_inputs_unchanged':True,'dist_files':{'index.html':H.CI.sha(self.dist/'index.html')},'servedDist':str(self.dist)})
        self.put('abi-receipt.json',{'currentSource':SHA,'currentELFDependenciesVerified':True,'host_runtime_files':{str(self.library):H.CI.sha(self.library)},'actualCurrentELFldd':{p:self.ldd_text('0x1111') for p in bins}})

    def ldd_text(self, address):
        return f"linux-vdso.so.1 ({address})\nlib-fixture.so => {self.library} ({address})\n"
    def ldd(self, args, **kwargs):
        self.assertEqual(args[0], 'ldd')
        self.assertIn(args[1], self.get('bundle.json')['binaries'])
        self.assertEqual(kwargs, {'capture_output':True, 'text':True})
        return self.ldd_result
    def test_ldd_address_changes_preserve_qualified_dependency_identity(self):
        recorded = next(iter(self.get('abi-receipt.json')['actualCurrentELFldd'].values()))
        self.assertNotEqual(recorded, self.ldd_result.stdout)
        H.export()
        self.assertTrue(self.packet.exists())
        self.assertEqual(next(iter(self.get('abi-receipt.json')['actualCurrentELFldd'].values())), recorded)
    def test_ldd_missing_dependency_refused(self):
        self.ldd_result.stdout = 'lib-fixture.so => not found\n'
        with self.assertRaises(AssertionError):H.export()
        self.assertFalse(self.packet.exists())
    def test_ldd_unexpected_dependency_refused(self):
        extra = self.root / 'foreign.so';extra.write_bytes(b'NOT ELF foreign fixture')
        self.ldd_result.stdout += f'foreign.so => {extra} (0x3333)\n'
        with self.assertRaisesRegex(AssertionError, 'dependency set differs'):H.export()
        self.assertFalse(self.packet.exists())
    def test_ldd_matching_but_unqualified_dependency_refused(self):
        extra = self.root / 'foreign.so';extra.write_bytes(b'NOT ELF foreign fixture')
        self.ldd_result.stdout += f'foreign.so => {extra} (0x3333)\n'
        abi = self.get('abi-receipt.json')
        abi['actualCurrentELFldd'] = {p:self.ldd_result.stdout for p in abi['actualCurrentELFldd']}
        self.put('abi-receipt.json', abi)
        with self.assertRaisesRegex(AssertionError, 'unqualified current ELF dependency'):H.export()
        self.assertFalse(self.packet.exists())
    def test_ldd_changed_library_bytes_refused(self):
        self.library.write_bytes(b'changed qualified library bytes')
        with self.assertRaises(AssertionError):H.export()
        self.assertFalse(self.packet.exists())
    def test_ldd_failed_process_refused(self):
        self.ldd_result.returncode = 1
        with self.assertRaisesRegex(AssertionError, 'ldd failed'):H.export()
        self.assertFalse(self.packet.exists())
    def test_ldd_missing_recorded_dependency_refused(self):
        abi = self.get('abi-receipt.json')
        abi['actualCurrentELFldd'][next(iter(abi['actualCurrentELFldd']))] = 'linux-vdso.so.1 (0x1111)\n'
        self.put('abi-receipt.json', abi)
        with self.assertRaisesRegex(AssertionError, 'dependency set differs'):H.export()
        self.assertFalse(self.packet.exists())

    def call(self,args):
        if args[:2]==['git','rev-parse']:return SHA if args[2]=='HEAD' else TREE
        return {'rustc':'release: 1.98.1\nhost: x86_64-unknown-linux-gnu','cargo':'cargo 1.98.1','bun':'1.4.2','ldd':'fixture ldd'}[args[0]]
    def inputs(self):return {'head':SHA,'tree':TREE,'status':'','tracked':{'fixture-source':H.CI.sha(self.header)},'external':{str(self.header):H.CI.sha(self.header)},'untracked':{}}
    def put(self,name,data): (self.output/name).write_text(json.dumps(data))
    def get(self,name):return json.loads((self.output/name).read_text())
    def transfer(self):
        H.export()
        m=json.loads((self.packet/'handoff.json').read_text())
        # Remove only this test's produced destinations to emulate an empty consumer.
        for e in m['entries'].values():Path(e['path']).unlink()
        os.environ.update(GITHUB_JOB='collaboration-flow',FVOCI_WEB_BUILD_PHASE='consume',FVOCI_WEB_BUILD_HANDOFF_SHA256=H.CI.sha(self.packet/'handoff.json'))
        return m
    def change_manifest(self,m):
        (self.packet/'handoff.json').write_text(json.dumps(m));os.environ['FVOCI_WEB_BUILD_HANDOFF_SHA256']=H.CI.sha(self.packet/'handoff.json')
    def test_original_same_job_identity_and_explicit_builder_phase(self):
        class Status:
            returncode = 0
        with patch.object(H.CI.subprocess, 'run', return_value=Status()):
            os.environ['GITHUB_JOB']='collaboration-flow';os.environ.pop('FVOCI_WEB_BUILD_PHASE')
            self.assertEqual(ORIGINAL_IDENTITY(), 'github:AISFlow/fvoci:123:1:collaboration-flow')
            os.environ['GITHUB_JOB']='collaboration-build'
            with self.assertRaises(AssertionError):ORIGINAL_IDENTITY()
            os.environ['FVOCI_WEB_BUILD_PHASE']='prepare'
            self.assertEqual(ORIGINAL_IDENTITY(), 'github:AISFlow/fvoci:123:1:collaboration-build')
            os.environ['FVOCI_WEB_BUILD_PHASE']='consume'
            with self.assertRaises(AssertionError):ORIGINAL_IDENTITY()
            with self.assertRaises(AssertionError):H.CI.run(self.output)

    def test_exact_current_packet_positive(self):
        self.transfer();H.admit();H.consume();r=self.get('handoff-consumed.json');self.assertTrue(r['full_current_physical_inputs_equal']);self.assertTrue(r['fresh_dist_equal']);self.assertEqual(len(r['received']),23)
    def test_context_mismatch(self):
        original=self.transfer()
        for k in ('repository','run','attempt','source','tree','root','output','producer_job','consumer_job','schema'):
            with self.subTest(field=k):
                m=copy.deepcopy(original);m[k]='foreign';self.change_manifest(m)
                with self.assertRaises(AssertionError):H.admit()
        self.assertFalse((self.output/'handoff-consumed.json').exists())
    def test_consumer_disk_floor_refusal(self):
        self.transfer()
        class Space:
            free = 19_999_999_999
        with patch.object(H.CI.shutil, 'disk_usage', return_value=Space()):
            with self.assertRaisesRegex(AssertionError, 'consumer START disk floor'):H.admit()
        self.assertFalse((self.output/'handoff-consumed.json').exists())
    def test_missing_or_wrong_digest(self):
        self.transfer();os.environ['FVOCI_WEB_BUILD_HANDOFF_SHA256']='0'*64
        with self.assertRaises(AssertionError):H.admit()
        (self.packet/'handoff.json').unlink()
        with self.assertRaises(FileNotFoundError):H.admit()
    def test_failed_producer_and_missing_core_or_feature(self):
        original=self.get('compile-receipt.json');broken=copy.deepcopy(original);broken['exit_code']=7;self.put('compile-receipt.json',broken)
        with self.assertRaises(AssertionError):H.export()
        self.assertFalse(self.packet.exists());self.put('compile-receipt.json',original)
        bundle=self.get('bundle.json');bundle['compiler_artifacts']=[];self.put('bundle.json',bundle)
        with self.assertRaises(AssertionError):H.export()
    def test_corrupt_payload(self):
        self.transfer();p=self.packet/'payload.tar';p.write_bytes(p.read_bytes()+b'wrong')
        with self.assertRaises(AssertionError):H.consume()
    def test_foreign_or_existing_destination(self):
        m=self.transfer();e=next(iter(m['entries'].values()));Path(e['path']).write_bytes(b'foreign')
        with self.assertRaises(AssertionError):H.consume()
    def test_current_physical_input_changed(self):
        self.transfer();self.header.write_bytes(b'different actual input')
        with self.assertRaises(AssertionError):H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())
    def test_fresh_dist_changed(self):
        self.transfer();(self.dist/'index.html').write_bytes(b'different fresh dist')
        with self.assertRaises(AssertionError):H.consume()
    def test_wrong_features(self):
        b=self.get('bundle.json');next(iter(b['binaries'].values()))['features']=[];self.put('bundle.json',b)
        with self.assertRaises(AssertionError):H.export()
    def test_wrong_environment(self):
        self.put('build-env-inputs.json',{'different':'env'})
        with self.assertRaises(AssertionError):H.export()
    def test_missing_receipt(self):
        (self.output/'before.json').unlink()
        with self.assertRaises(FileNotFoundError):H.export()
    def test_missing_or_wrong_phase_job(self):
        self.transfer()
        for key,value in [('GITHUB_JOB','workspace-browser-shard'),('FVOCI_WEB_BUILD_PHASE','prepare'),('FVOCI_WEB_BUILD_PHASE','')]:
            original=os.environ[key];os.environ[key]=value
            with self.assertRaises(AssertionError):H.admit()
            os.environ[key]=original
    def test_symlink_manifest(self):
        self.transfer();p=self.packet/'handoff.json';q=self.packet/'actual.json';p.rename(q);p.symlink_to(q)
        with self.assertRaises(AssertionError):H.admit()
    def test_toolchain_or_profile_changed(self):
        original=self.get('build-environment.json')
        for key in ['rustc','cargo','bun','os_release','target','features','nativeFeatures','profile','devDebug','testDebug','sqlite']:
            with self.subTest(field=key):
                altered=copy.deepcopy(original);altered[key]='foreign';self.put('build-environment.json',altered)
                with self.assertRaises((AssertionError,TypeError)):H.export()
                self.put('build-environment.json',original)
    def test_extra_native_destination(self):
        m=self.transfer();e=copy.deepcopy(next(iter(m['entries'].values())));e['path']=str(self.target/'debug/foreign-native');m['entries']['extra']=e;self.change_manifest(m)
        # The missing member must reject before installing anything.
        with self.assertRaises(AssertionError):H.consume()
        self.assertFalse((self.target/'debug/foreign-native').exists())
    def test_changed_receipt_stage_or_native_bytes(self):
        original=self.get('main-stage.json');broken=copy.deepcopy(original);broken['exit_code']=7;self.put('main-stage.json',broken)
        with self.assertRaises(AssertionError):H.export()
        self.put('main-stage.json',original)
        b=self.get('bundle.json');Path(next(iter(b['binaries']))).write_bytes(b'changed physical ELF')
        with self.assertRaises(AssertionError):H.export()
    def test_link_member_refused_even_matching_archive_digest(self):
        m=self.transfer();archive=self.packet/'payload.tar'
        data=[]
        with tarfile.open(archive,'r:') as t:
            for item in t.getmembers():data.append((item,t.extractfile(item).read()))
        with tarfile.open(archive,'w') as t:
            for i,(item,body) in enumerate(data):
                if i==0:item.type=tarfile.SYMTYPE;item.linkname='/foreign';item.size=0;t.addfile(item)
                else:t.addfile(item,io.BytesIO(body))
        m['payload_sha256']=H.CI.sha(archive);self.change_manifest(m)
        with self.assertRaises(AssertionError):H.consume()
        self.assertFalse((self.output/'before.json').exists())

    def test_input_diagnostics_are_bounded_hashes_not_paths_or_values(self):
        private = 'https://private.invalid/token-secret'
        before = {**self.inputs(), 'status': private, 'unknown-private-field': private}
        current = {**before, 'untracked': {f'{private}/{i}': private for i in range(600)}}
        self.put('before.json', before); self.put('after.json', before)
        with patch.object(H.CI, 'inputs', return_value=current):
            with self.assertRaisesRegex(AssertionError, 'current physical inputs differ'):H.qualify(self.output)
        files = sorted(self.output.glob('handoff-input-*-safe.json'))
        self.assertEqual(len(files), 4)
        for file in files:
            self.assertNotIn(private, file.read_text())
            self.assertNotIn('unknown-private-field', file.read_text())
            self.assertEqual(stat.S_IMODE(file.stat().st_mode), 0o600)
            self.assertLess(file.stat().st_size, 256 * 1024)
        record = self.get('handoff-input-current-safe.json')['fields']['untracked']
        self.assertEqual(record['count'], 600); self.assertEqual(len(record['entries']), 512)
        self.assertTrue(record['truncated'])
        delta = self.get('handoff-input-delta-safe.json')['after_current']['untracked']
        self.assertEqual(delta['count'], 600); self.assertEqual(len(delta['entries']), 512)
        self.assertTrue(delta['truncated'])
        saved = {p: p.read_bytes() for p in files}
        with self.assertRaises(FileExistsError):H.input_diagnostics(self.output, before, before, current)
        self.assertEqual(saved, {p:p.read_bytes() for p in files})


class FreshImportTest(unittest.TestCase):
    """Fresh Python entrypoint; real loader/input guard, data-only tool boundary."""
    DRIVER = r'''
import ast, hashlib, importlib.util, json, pathlib, sys
root, output, header = map(pathlib.Path, sys.argv[1:4])
mode, mutation = sys.argv[4:6]
tracked = ['scripts/selected-backend-ci/web-build-handoff.py', 'scripts/run-selected-backend-e2e.py']
sha = lambda p: hashlib.sha256(p.read_bytes()).hexdigest()
def inputs():
    others = {str(p.relative_to(root)):sha(p) for p in root.rglob('*') if p.is_file() and str(p.relative_to(root)) not in tracked}
    return {'head':'a'*40, 'tree':'b'*40, 'status':json.dumps(sorted(others)),
            'tracked':{n:sha(root/n) for n in tracked}, 'external':{str(header):sha(header)}, 'untracked':others}
before = inputs()
for name in ('before.json','after.json'):(output/name).write_text(json.dumps(before))
(output/'build-env-inputs.json').write_text('{}')
if mutation == 'tracked':
    with (root/tracked[1]).open('ab') as f:f.write(b'\n# real tracked mutation\n')
elif mutation == 'header':header.write_bytes(b'changed actual header')
elif mutation == 'untracked':(root/'private-token-secret').write_bytes(b'private-value-secret')
class Boundary(Exception):pass
original_spec = importlib.util.spec_from_file_location
class Loader:
    def __init__(self, original):self.original = original
    def create_module(self, spec):return self.original.create_module(spec)
    def exec_module(self, module):
        self.original.exec_module(module)
        module.inputs = inputs
        def stop():raise Boundary()
        module.build_env = stop
def fixture_spec(name, path, *args, **kwargs):
    spec = original_spec(name, path, *args, **kwargs)
    if name == 'selected_ci':spec.loader = Loader(spec.loader)
    return spec
importlib.util.spec_from_file_location = fixture_spec
def context(job):
    if mode != 'export':raise Boundary()
    return {'source':'a'*40,'tree':'b'*40}
path = root/tracked[0]
tree = ast.parse(path.read_bytes(), filename=str(path))
# Substitute only the allocation/tool context; keep the import, main dispatch,
# export and strict qualify guard intact. Never execute compiler/ELF tools.
tree.body[-1:-1] = ast.parse('context = fixture_context\npaths = fixture_paths\n').body
ast.fix_missing_locations(tree)
sys.argv = [str(path), mode]
outcome = 'unexpected return'
try:
    exec(compile(tree,str(path),'exec'), {'__name__':'__main__','__file__':str(path),
         'fixture_context':context,'fixture_paths':lambda:(output,output.parent/'packet')})
except Boundary:outcome = 'tool boundary'
except AssertionError as error:outcome = str(error)
after = inputs()
print(json.dumps({'outcome':outcome, 'added':sorted(set(after['untracked'])-set(before['untracked'])),
                  'changed':sorted(k for k in before if before[k] != after[k]),
                  'packet':(output.parent/'packet').exists()}))
'''

    def run_fresh(self, *, original=False, mutation='', mode='export'):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp);root = base/'repo';root.mkdir()
            output = base/'output';output.mkdir(mode=0o700)
            header = base/'header';header.write_bytes(b'actual header fixture')
            for name in ('scripts/selected-backend-ci/web-build-handoff.py', 'scripts/run-selected-backend-e2e.py'):
                target = root/name;target.parent.mkdir(parents=True,exist_ok=True)
                data = (ROOT/name).read_text()
                if original and name.endswith('web-build-handoff.py'):
                    self.assertEqual(data.count('sys.dont_write_bytecode = True'), 1)
                    data = data.replace('sys.dont_write_bytecode = True', 'sys.dont_write_bytecode = False')
                target.write_text(data)
            result = subprocess.run([sys.executable, '-c', self.DRIVER, str(root), str(output), str(header), mode, mutation],
                                    capture_output=True, text=True, env={'PATH':os.defpath})
            self.assertEqual(result.returncode, 0, result.stderr)
            record = json.loads(result.stdout)
            safe = sorted(output.glob('handoff-input-*-safe.json'))
            for file in safe:
                self.assertNotIn(str(base), file.read_text())
                self.assertNotIn('private-token-secret', file.read_text())
                self.assertNotIn('private-value-secret', file.read_text())
            self.assertFalse(record['packet'])
            return record, len(safe)

    def test_original_bytecode_import_changes_live_map_and_is_refused(self):
        record, count = self.run_fresh(original=True)
        self.assertEqual(record['outcome'], 'current physical inputs differ')
        self.assertEqual(record['changed'], ['status','untracked'])
        self.assertEqual(len(record['added']), 1)
        self.assertRegex(record['added'][0], r'^scripts/__pycache__/run-selected-backend-e2e\.cpython-\d+\.pyc$')
        self.assertEqual(count, 4)

    def test_fixed_export_preserves_inputs_and_reaches_next_tool_boundary(self):
        record, count = self.run_fresh()
        self.assertEqual(record, {'outcome':'tool boundary','added':[],'changed':[],'packet':False})
        self.assertEqual(count, 0)

    def test_fixed_admit_and_consume_imports_preserve_inputs(self):
        for mode in ('admit','consume'):
            with self.subTest(mode=mode):
                record, count = self.run_fresh(mode=mode)
                self.assertEqual(record, {'outcome':'tool boundary','added':[],'changed':[],'packet':False})
                self.assertEqual(count, 0)

    def test_real_tracked_header_and_untracked_drift_still_refused(self):
        for mutation, fields in [('tracked',['tracked']), ('header',['external']), ('untracked',['status','untracked'])]:
            with self.subTest(mutation=mutation):
                record, count = self.run_fresh(mutation=mutation)
                self.assertEqual(record['outcome'], 'current physical inputs differ')
                self.assertEqual(record['changed'], fields)
                self.assertEqual(count, 4)



class RuntimePermissionsTest(unittest.TestCase):
    """Real kernel UID1000 access, owned temp only; no product/native execution.

    A root preparation process with ordinary primary GID1001 models a distinct
    CI preparation owner without creating a local account or granting sudo.
    The runtime actor is the unmodified real UID/GID1000; its backend body is
    replaced only in the fixture by the maintained runtime_access guard.
    """
    DOCKER_GID = 986

    def case(self, *, original=False, fault='', exit_code=0, pending_exit=0):
        self.assertEqual(subprocess.run(['sudo','-n','true'],capture_output=True).returncode,0,
                         'this real permission fixture needs its explicit owned-temp sudo boundary')
        with tempfile.TemporaryDirectory(prefix='fvoci-permission-fixture-') as tmp:
            temp=Path(tmp);repo=temp/'repo';scripts=repo/'scripts';scripts.mkdir(parents=True)
            output=temp/'fvoci-selected-current';output.mkdir(mode=0o700)
            sqlite=temp/'fvoci-sqlite';lib=sqlite/'lib';lib.mkdir(parents=True)
            header=repo/'header';header.write_bytes(b'qualified read-only fixture')
            native=lib/'libsqlite3.a';native.write_bytes(b'NOT native; owned fixture')
            chrome=repo/'chromium';chrome.write_text('#!/bin/sh\nexit 0\n');chrome.chmod(0o755)
            fake=repo/'bin';fake.mkdir()
            bun=fake/'bun';bun.write_text('#!/bin/sh\nprintf "%s\\n" "'+str(chrome)+'"\n');bun.chmod(0o755)
            sockstat=fake/'stat';sockstat.write_text('#!/bin/sh\nif [ "$1" = -c ] && [ "$2" = %g ] && [ "$3" = /var/run/docker.sock ]; then echo 986; else exec /usr/bin/stat "$@"; fi\n');sockstat.chmod(0o755)
            program=scripts/'run-selected-backend-e2e.py'
            tree=ast.parse((ROOT/'scripts/run-selected-backend-e2e.py').read_text())
            # Preserve CLI, permissions/write/read/runtime_access bodies. Replace
            # only Git allocation lookup and the downstream product boundary.
            replacements=ast.parse("""
def identity(*args):return 'owned-permission-fixture'
def prepare_browser(output, chromium):
    # Legacy39 controls isolate the original source/native ownership boundary;
    # the separate BrowserAssetsTest exercises the real private-copy boundary.
    return Path(chromium)
def run(output):
    assert os.getuid()==os.getgid()==1000
    assert output.stat().st_uid==1000 and output.stat().st_mode & 0o777 == 0o700
    before=read(output/'before.json')
    runtime_access(before['external'], {'bun':{'path':before['fixture_bun']},'chromium':{'path':before['fixture_chrome']},
        'chromium_directory_files':browser_inventory(Path(before['fixture_chrome']).parent)})
    write(output/'fixture-marker.json', {'uid':os.getuid(),'gid':os.getgid(),'groups':os.getgroups()})
    if before['fixture_incomplete']:
        (output/'runtime').mkdir()
        write(output/'install-allocation.json', {})
    if before['fixture_fault'] in ('closed','wrong-source','missing-process','live','wrong-flow','dropped-off','duplicate-root','foreign-owner','missing-port','invalid-port','missing-pid','unsafe-canary'):
        runtime=output/'runtime';runtime.mkdir()
        runs=[]
        for lane,flow in selected_runs():
            root=runtime/('root-current-'+lane+'-'+flow+'-fixture');root.mkdir()
            write(output/(lane+'-'+flow+'-allocation.json'),{})
            facts={'source':before['head'],'tree':before['tree'],'root_owner':identity(),'selected_flow':flow,'final_exit_code':0,'owned_container_absent':True,
                   'owned_loopback_port_closed':True,'recorded_process_identities_retired':True,'cleanup_errors':[]}
            if lane=='install':
                facts['actual_owned_process_receipts']=15
                retained=root/'retained-run';retained.mkdir()
                for i in range(15):write(retained/(str(i)+'-process.json'),{'status':None if before['fixture_fault']=='missing-process' and i==0 else 0})
            if lane=='postgres':write(root/'parent-receipt.json',{'source':before['head'],'tree':before['tree'],'root_owner':identity(),'selected_flow':flow,'all_owned_fixtures_closed':True})
            if before['fixture_fault']=='wrong-source':facts['source']='foreign-source'
            if before['fixture_fault']=='live':facts['recorded_process_identities_retired']=False
            if before['fixture_fault']=='wrong-flow' and flow=='off':facts['selected_flow']='on'
            if before['fixture_fault']=='foreign-owner':facts['root_owner']='foreign'
            if lane=='sqlite':
                facts['final_exit_code']=before['fixture_exit']
                if before['fixture_fault'] in ('missing-port','unsafe-canary'):facts.pop('owned_loopback_port_closed')
                if before['fixture_fault']=='invalid-port':facts['owned_loopback_port_closed']='PRIVATE_CANARY_URL'
                if before['fixture_fault']=='missing-pid':facts.pop('recorded_process_identities_retired')
                if before['fixture_fault']=='unsafe-canary':
                    facts['original_driver_failure']={'message':'PRIVATE_CANARY_URL secret=PRIVATE_CANARY_SECRET'}
                    facts['session']='PRIVATE_CANARY_SESSION';facts['headers']={'Authorization':'PRIVATE_CANARY_SECRET'}
            write(root/'receipt.json',facts)
            runs.append({'lane':lane,'flow':flow,'actualSource':before['head'],'runRoot':str(root),'exit':facts['final_exit_code']})
        if before['fixture_fault']=='dropped-off':runs.pop()
        if before['fixture_fault']=='duplicate-root':runs[-1]['runRoot']=runs[2]['runRoot']
        write(output/'selected-ci-receipt.json',{'owner':identity(),'source':before['head'],'tree':before['tree'],'runs':runs})
    return before['fixture_exit']
""").body
            for replacement in replacements:
                tree.body=[replacement if isinstance(n,ast.FunctionDef) and n.name==replacement.name else n for n in tree.body]
            program.write_text(ast.unparse(tree)+'\n')
            before={'head':SHA,'tree':TREE,'tracked':{'scripts/run-selected-backend-e2e.py':H.CI.sha(program)},
                    'external':{str(header):H.CI.sha(header),str(native):H.CI.sha(native)},
                    'fixture_bun':str(bun),'fixture_chrome':str(chrome),'fixture_exit':exit_code,'fixture_incomplete':fault=='incomplete','fixture_fault':fault}
            for name in ('before.json','after.json'):(output/name).write_text(json.dumps(before));(output/name).chmod(0o600)
            (output/'bundle.json').write_text(json.dumps({'binaries':{str(chrome):{}}}));(output/'bundle.json').chmod(0o600)
            source=(ROOT/'scripts/run-web-e2e.sh').read_text()
            if original:
                source=ORIGINAL_C7_SELECTED_FOOTER
            if not original:
                self.assertIn('\nSELECTED_PHASE="whole"\n', source[:source.index('selected_status=0\n')])
            footer=source[source.index('selected_status=0\n'):]
            environment={'PATH':str(fake)+':'+os.defpath,'ROOT':str(repo),'RUNNER_TEMP':str(temp),
                # Footer extraction omits the wrapper's real argument-parser default.
                'SELECTED_PHASE':'whole','SELECTED_PART':'whole',
                'SELECTED_BACKENDS':'true','FVOCI_SELECTED_CI_OUTPUT':str(output),
                'FVOCI_SELECTED_CI_SQLITE_PARENT':str(sqlite),'SQLITE3_LIB_DIR':str(lib),
                'CI':'true','GITHUB_ACTIONS':'true','GITHUB_JOB':'collaboration-flow',
                'GITHUB_OUTPUT':str(temp/'github-output'),'GITHUB_SHA':SHA,'GITHUB_REPOSITORY':'fixture/owned','GITHUB_RUN_ID':'1','GITHUB_RUN_ATTEMPT':'1'}
            script=temp/'footer.sh';script.write_text('set -euo pipefail\npending_status='+str(pending_exit)+'\n'+footer)
            if fault in ('destination-occupied','destination-symlink','destination-foreign','destination-mode'):
                destination=temp/'fvoci-selected-diagnostics'
                if fault=='destination-symlink':destination.symlink_to(output,target_is_directory=True)
                else:
                    destination.mkdir(mode=0o700)
                    if fault=='destination-mode':destination.chmod(0o755)
            subprocess.run(['sudo','-n','chown','-R','0:1001',str(temp)],check=True)
            if fault=='destination-foreign':subprocess.run(['sudo','-n','chown','1001:1001',str(destination)],check=True)
            subprocess.run(['sudo','-n','chmod','750',str(temp),str(repo)],check=True)
            if fault=='unreadable':subprocess.run(['sudo','-n','chmod','600',str(header)],check=True)
            if fault=='foreign':subprocess.run(['sudo','-n','chown','1001:1001',str(native)],check=True)
            if fault=='symlink':
                subprocess.run(['sudo','-n','ln','-s',str(header),str(sqlite/'foreign-link')],check=True)
            try:
                result=subprocess.run(['sudo','-n','setpriv','--reuid=0','--regid=1001','--clear-groups',
                    'env','-i',*[k+'='+v for k,v in environment.items()],'/bin/bash',str(script)],
                    capture_output=True,text=True)
                collector="""import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);o=root/'fvoci-selected-current';s=root/'fvoci-sqlite'
def facts(p):
 x=p.stat();return {'uid':x.st_uid,'gid':x.st_gid,'mode':x.st_mode & 0o777}
def optional(name):
 p=o/name;return json.loads(p.read_text()) if p.exists() else None
d=root/'fvoci-selected-diagnostics'
def safe_optional(name):
 p=d/name;return json.loads(p.read_text()) if p.exists() else None
published=(root/'github-output').read_text() if (root/'github-output').exists() else ''
print(json.dumps({'diagnostics':facts(d) if d.exists() else None,
 'safe_ownership':safe_optional('ownership-stage.json'),'safe_launcher':safe_optional('launcher-stage.json'),
 'private_published':'selected-private-diagnostics<<' in published,
 'safe_published':'selected-safe-diagnostics=' in published,
 'diagnostic_modes':{p.name:p.stat().st_mode & 0o777 for p in d.iterdir() if p.is_file()} if d.exists() else {},
 'output':facts(o),'sqlite':facts(s),'header':facts(root/'repo/header'),
 'marker':optional('fixture-marker.json'),'stage':optional('runtime-access-stage.json'),'close':optional('runtime-close-stage.json')}))
"""
                inspected=subprocess.run(['sudo','-n',sys.executable,'-c',collector,str(temp)],capture_output=True,text=True,check=True)
                facts=json.loads(inspected.stdout)
                self.assertNotIn(str(temp),json.dumps(facts['stage']))
                print(json.dumps({'control':'original' if original else (fault or 'fixed'),'selected_exit':exit_code,
                    'footer_exit':result.returncode,'err13': '[Errno 13] Permission denied' in result.stderr,
                    'stderr_sha256':hashlib.sha256(result.stderr.encode()).hexdigest(),**facts}),flush=True)
                return result,facts
            finally:
                # Retire only this positively created owned temporary fixture.
                subprocess.run(['sudo','-n','chown','-R',str(os.getuid())+':'+str(os.getgid()),str(temp)],check=True)

    def test_original_group_drop_cannot_open_runner_before_body(self):
        result,facts=self.case(original=True)
        self.assertEqual(result.returncode,2,result.stderr)
        self.assertIn('[Errno 13] Permission denied',result.stderr)
        self.assertIsNone(facts['marker']);self.assertIsNone(facts['stage'])
        self.assertEqual(facts['output'],{'uid':1000,'gid':1000,'mode':0o700})

    def test_fixed_exact_footer_real_access_then_private_owner_roundtrip(self):
        result,facts=self.case()
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertIsNone(facts['safe_launcher']['config_list_exit'])
        self.assertEqual(facts['marker'],{'uid':1000,'gid':1000,'groups':[986,1001]})
        self.assertEqual(facts['stage']['groups'],[986,1001])
        self.assertEqual(facts['stage']['preflight']['missing'],0)
        self.assertGreater(facts['stage']['required_group_path_count'],0)
        self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})
        self.assertEqual((facts['sqlite']['uid'],facts['sqlite']['gid']),(0,1001))

    def test_failed_child_first_status_kept_after_settlement_and_roundtrip(self):
        result,facts=self.case(exit_code=7)
        self.assertEqual(result.returncode,7,result.stderr)
        self.assertIsNotNone(facts['marker'])
        self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})

    def test_waited_launcher_without_resource_proof_keeps_private_ownership_and_first_status(self):
        result,facts=self.case(fault='incomplete',exit_code=7)
        self.assertEqual(result.returncode,7,result.stderr)
        self.assertIn('resource retirement proof incomplete',result.stderr)
        self.assertIsNotNone(facts['marker'])
        self.assertEqual(facts['output'],{'uid':1000,'gid':1000,'mode':0o700})

    def test_exact_closed_receipts_admit_but_foreign_source_unsettled_pid_or_live_resources_refuse(self):
        result,facts=self.case(fault='closed')
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(facts['close']['closed_current_runs'],[{'lane':lane,'flow':flow} for lane,flow in (('install','on'),('postgres','on'),('sqlite','on'),('postgres','off'),('sqlite','off'))])
        self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})
        for fault in ('wrong-source','missing-process','live','wrong-flow','dropped-off','duplicate-root','foreign-owner'):
            with self.subTest(fault=fault):
                result,facts=self.case(fault=fault)
                self.assertNotEqual(result.returncode,0)
                self.assertIsNone(facts['close'])
                self.assertEqual(facts['output'],{'uid':1000,'gid':1000,'mode':0o700})

    def test_declared_group_cannot_override_private_unreadable_input(self):
        result,facts=self.case(fault='unreadable')
        self.assertNotEqual(result.returncode,0)
        self.assertIn('runtime input access preflight failed',result.stderr)
        self.assertEqual(facts['stage']['preflight']['missing'],1)
        self.assertIsNone(facts['marker'])
        self.assertEqual(facts['header']['mode'],0o600)
        self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})

    def test_foreign_owner_and_symlink_refused_before_any_transfer(self):
        for fault in ('foreign','symlink'):
            with self.subTest(fault=fault):
                result,facts=self.case(fault=fault)
                self.assertNotEqual(result.returncode,0)
                self.assertIsNone(facts['marker']);self.assertIsNone(facts['stage'])
                self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})

    def test_private_diagnostics_owner_is_what_controls_nonroot_upload_access(self):
        with tempfile.TemporaryDirectory(prefix='fvoci-upload-permission-') as tmp:
            root=Path(tmp);output=root/'output';output.mkdir(mode=0o700)
            (output/'safe-stage.json').write_text('{}')
            try:
                subprocess.run(['sudo','-n','chmod','755',str(root)],check=True)
                subprocess.run(['sudo','-n','chown','-R','1000:1000',str(output)],check=True)
                argv=['sudo','-n','setpriv','--reuid=1001','--regid=1001','--clear-groups',sys.executable,'-c',
                      'import os,sys;print(os.listdir(sys.argv[1]))',str(output)]
                original=subprocess.run(argv,capture_output=True,text=True)
                self.assertNotEqual(original.returncode,0);self.assertIn('PermissionError',original.stderr)
                subprocess.run(['sudo','-n','chown','-R','1001:1001',str(output)],check=True)
                fixed=subprocess.run(argv,capture_output=True,text=True)
                self.assertEqual(fixed.returncode,0,fixed.stderr);self.assertIn('safe-stage.json',fixed.stdout)
                self.assertEqual(output.stat().st_mode & 0o777,0o700)
            finally:
                subprocess.run(['sudo','-n','chown','-R',str(os.getuid())+':'+str(os.getgid()),str(root)],check=True)


    def test_missing_or_invalid_closure_preserves_actual_driver_and_launcher_error(self):
        for fault in ('missing-port','invalid-port','missing-pid','unsafe-canary'):
            with self.subTest(fault=fault):
                result,facts=self.case(fault=fault,exit_code=7)
                self.assertEqual(result.returncode,7,result.stderr)
                self.assertIsNone(facts['close'])
                self.assertEqual(facts['output'],{'uid':1000,'gid':1000,'mode':0o700})
                self.assertEqual(facts['diagnostics'],{'uid':0,'gid':1001,'mode':0o700})
                self.assertFalse(facts['private_published']);self.assertTrue(facts['safe_published'])
                self.assertTrue(all(mode==0o600 for mode in facts['diagnostic_modes'].values()))
                summary=facts['safe_ownership'];lane=summary['lanes'][-1]
                self.assertFalse(summary['ownership_return_qualified'])
                self.assertEqual(summary['phase'],'sqlite')
                self.assertEqual(lane['launcher_observed_driver_exit'],7)
                self.assertEqual(lane['receipt_final_exit'],7)
                self.assertEqual(len(lane['receipt_sha256']),64)
                self.assertEqual(facts['safe_launcher'],{'actual_launcher_exit':7,'ownership_return_exit':1,'selected_final_exit':7,'pending_exit':0,'config_list_exit':None})
                self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
                key='recorded_process_identities_retired' if fault=='missing-pid' else 'owned_loopback_port_closed'
                self.assertIn(key,lane['invalid_required_fields'] if fault=='invalid-port' else lane['missing_required_fields'])
                self.assertIsNone(lane['closure_facts'][key])

    def test_absent_port_proof_cannot_convert_zero_launcher_to_success(self):
        result,facts=self.case(fault='missing-port')
        self.assertEqual(result.returncode,1,result.stderr)
        self.assertEqual(facts['safe_launcher']['actual_launcher_exit'],0)
        self.assertFalse(facts['safe_ownership']['ownership_return_qualified'])
        self.assertFalse(facts['private_published'])

    def test_qualified_return_publishes_original_allowlist_and_partial_refusal_only_safe_summary(self):
        result,facts=self.case(fault='closed',exit_code=7)
        self.assertEqual(result.returncode,7,result.stderr)
        self.assertTrue(facts['safe_ownership']['ownership_return_qualified'])
        self.assertTrue(facts['private_published'])
        for fault in ('incomplete','wrong-source','missing-process','live'):
            with self.subTest(fault=fault):
                result,facts=self.case(fault=fault,exit_code=7)
                self.assertEqual(result.returncode,7,result.stderr)
                self.assertFalse(facts['safe_ownership']['ownership_return_qualified'])
                self.assertFalse(facts['private_published'])
                self.assertEqual(facts['safe_launcher']['actual_launcher_exit'],7)

    def test_pending_first_error_kept_with_observed_selected_and_closure_errors(self):
        result,facts=self.case(fault='missing-port',exit_code=7,pending_exit=13)
        self.assertEqual(result.returncode,13,result.stderr)
        self.assertEqual(facts['safe_launcher']['actual_launcher_exit'],7)
        self.assertEqual(facts['safe_launcher']['selected_final_exit'],7)
        self.assertEqual(facts['safe_launcher']['pending_exit'],13)
        self.assertFalse(facts['safe_ownership']['ownership_return_qualified'])

    def test_nonroot_uploader_reads_only_its_owned_safe_prefix_when_runtime_stays_private(self):
        with tempfile.TemporaryDirectory(prefix='fvoci-safe-upload-fixture-') as tmp:
            root=Path(tmp);safe=root/'safe';private=root/'private'
            safe.mkdir(mode=0o700);private.mkdir(mode=0o700)
            (safe/'ownership-stage.json').write_text(json.dumps({'ownership_return_qualified':False,'lanes':[]}))
            (safe/'ownership-stage.json').chmod(0o600)
            (private/'receipt.json').write_text('PRIVATE_CANARY_SECRET')
            try:
                subprocess.run(['sudo','-n','chmod','755',str(root)],check=True)
                subprocess.run(['sudo','-n','chown','-R','1001:1001',str(safe)],check=True)
                subprocess.run(['sudo','-n','chown','-R','1000:1000',str(private)],check=True)
                probe="""import json,pathlib,sys
root=pathlib.Path(sys.argv[1]);safe=root/'safe';private=root/'private'
assert safe.stat().st_mode & 0o777==0o700
assert (safe/'ownership-stage.json').stat().st_mode & 0o777==0o600
receipt=json.loads((safe/'ownership-stage.json').read_text())
assert receipt['ownership_return_qualified'] is False
try:list(private.iterdir())
except PermissionError:print('safe-upload-readable;private-runtime-denied')
else:raise AssertionError('private runtime became upload-readable')
"""
                result=subprocess.run(['sudo','-n','setpriv','--reuid=1001','--regid=1001','--clear-groups',sys.executable,'-B','-c',probe,str(root)],capture_output=True,text=True)
                self.assertEqual(result.returncode,0,result.stderr)
                self.assertIn('safe-upload-readable;private-runtime-denied',result.stdout)
                self.assertNotIn('PRIVATE_CANARY',result.stdout)
                self.assertEqual(private.stat().st_uid,1000)
            finally:
                subprocess.run(['sudo','-n','chown','-h','-R',str(os.getuid())+':'+str(os.getgid()),str(root)],check=True)

    def test_diagnostics_destination_occupied_symlink_foreign_or_mode_refused_before_transfer(self):
        for fault in ('destination-occupied','destination-symlink','destination-foreign','destination-mode'):
            with self.subTest(fault=fault):
                result,facts=self.case(fault=fault)
                self.assertNotEqual(result.returncode,0)
                self.assertIsNone(facts['marker']);self.assertIsNone(facts['safe_launcher'])
                self.assertFalse(facts['safe_published']);self.assertFalse(facts['private_published'])
                self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})




class WebBuildCacheTest(unittest.TestCase):
    def workflow(self):
        import yaml
        return yaml.safe_load((ROOT/'.github/workflows/web.yml').read_text())['jobs']

    def assert_unique_writer_keys(self, jobs):
        writers = []
        for job, config in jobs.items():
            for step in config.get('steps', []):
                action = step.get('uses', '')
                settings = step.get('with', {})
                if action.startswith(('actions/cache@', 'actions/cache/save@')) and any(
                    path.strip() in ('target', 'crates/collab-engine/target')
                    for path in settings.get('path', '').splitlines()
                ):
                    writers.append((job, settings['key']))
        self.assertTrue(writers, 'no build-cache writers checked')
        keys = [key for _, key in writers]
        self.assertEqual(len(keys), len(set(keys)), 'duplicate Web build-cache writer keys: '+str(writers))

    def test_workflow_build_cache_writers_are_unique(self):
        self.assert_unique_writer_keys(self.workflow())

    def test_each_original_producer_collision_is_detected(self):
        for path in ('target', 'crates/collab-engine/target'):
            with self.subTest(path=path):
                jobs = self.workflow()
                ordinary = next(step['with'] for step in jobs['workspace-browser-build']['steps']
                                if step.get('with', {}).get('path') == path)
                selected = next(step['with'] for step in jobs['collaboration-build']['steps']
                                if step.get('with', {}).get('path') == path)
                ordinary['key'] = selected['key']
                with self.assertRaisesRegex(AssertionError, 'duplicate Web build-cache writer'):
                    self.assert_unique_writer_keys(jobs)


class BrowserPacketTest(unittest.TestCase):
    """Same packet format, default-feature binaries and transferred fresh dist."""
    put = PacketTest.put
    get = PacketTest.get
    inputs = PacketTest.inputs
    call = PacketTest.call
    ldd_text = PacketTest.ldd_text
    change_manifest = PacketTest.change_manifest

    def ldd(self, args, **kwargs):
        if args[0] == "git":
            return type('CleanGit', (), {'returncode': 0})()
        return PacketTest.ldd(self, args, **kwargs)

    def setUp(self):
        PacketTest.setUp(self)
        os.environ.update(GITHUB_JOB='workspace-browser-build', FVOCI_WEB_BUILD_PHASE='prepare')
        for name in ('before.json', 'after.json'):
            self.put(name, H.build_inputs())
        stages = []
        for name in H.BROWSER_STAGES:
            self.put(name+'-stage.json', {'source':SHA, 'tree':TREE, 'command':['actual-fixed-fixture', name], 'exit_code':0, 'seconds':1})
            (self.output/(name+'-compiler.jsonl')).write_text('{}\n')
            stages.append({**self.get(name+'-stage.json'), 'compilerMessages':H.CI.reference(self.output/(name+'-compiler.jsonl'))})
        receipt = self.get('compile-receipt.json'); receipt['stages'] = stages; self.put('compile-receipt.json', receipt)
        bundle = self.get('bundle.json'); bundle['compiler_artifacts'] = []
        bundle['binaries'] = {p:r for p,r in bundle['binaries'].items() if not r['profile']['test']}
        for record in bundle['binaries'].values():
            name = record['target']['name']
            record['target']['kind'] = ['bin']
            record['profile'] = {'test':False, 'opt_level':'0', 'debuginfo':0}
            record['features'] = ['default','worker'] if name=='collab-engine' else (['db-tests'] if name=='fvoci-e2e-fixture' else [])
        self.put('bundle.json', bundle)
        env = self.get('build-environment.json'); env['features'] = []; self.put('build-environment.json', env)
        abi = self.get('abi-receipt.json'); abi['actualCurrentELFldd'] = {p:self.ldd_text('0x1111') for p in bundle['binaries']}; self.put('abi-receipt.json', abi)

    def transfer(self):
        H.export()
        manifest = json.loads((self.packet/'handoff.json').read_text())
        for entry in manifest['entries'].values():
            Path(entry['path']).unlink()
        os.environ.update(GITHUB_JOB='workspace-browser-shard', FVOCI_WEB_BUILD_PHASE='consume',
                          FVOCI_WEB_BUILD_HANDOFF_SHA256=H.CI.sha(self.packet/'handoff.json'))
        return manifest

    def test_exact_input_and_asset_hashes_equal_without_consumer_build(self):
        before = H.build_inputs(); assets = H.dist_files()
        self.transfer(); H.consume()
        self.assertEqual(H.build_inputs(), before)
        self.assertEqual(H.dist_files(), assets)
        receipt = self.get('handoff-consumed.json')
        self.assertTrue(receipt['fresh_dist_equal'])
        self.assertEqual(len(receipt['received']), 19)
        self.assertEqual({r['target']['name']: r['features'] for r in self.get('bundle.json')['binaries'].values()},
                         {'fvoci-server':[], 'fvoci-migrate':[], 'fvoci-e2e-fixture':['db-tests'], 'collab-engine':['default','worker']})

    def test_missing_manifest_or_payload_refused(self):
        self.transfer()
        for name in ('handoff.json', 'payload.tar'):
            with self.subTest(name=name):
                path = self.packet/name; data = path.read_bytes(); path.unlink()
                with self.assertRaises(FileNotFoundError): H.consume()
                self.assertFalse((self.output/'handoff-consumed.json').exists())
                path.write_bytes(data)

    def test_wrong_tested_sha_or_platform_refused(self):
        manifest = self.transfer()
        for key in ('source', 'tree', 'run', 'attempt', 'consumer_job', 'producer_job', 'root', 'output', 'platform'):
            with self.subTest(key=key):
                altered = copy.deepcopy(manifest); altered[key] = 'foreign'; self.change_manifest(altered)
                with self.assertRaises(AssertionError): H.consume()
                self.assertFalse((self.output/'before.json').exists())
        self.change_manifest(manifest)
        os.environ['GITHUB_SHA'] = 'c'*40
        with self.assertRaisesRegex(AssertionError, 'tested SHA'): H.consume()

    def test_manifest_and_payload_hash_mismatch_refused_before_extraction(self):
        self.transfer()
        expected = os.environ['FVOCI_WEB_BUILD_HANDOFF_SHA256']
        os.environ['FVOCI_WEB_BUILD_HANDOFF_SHA256'] = '0'*64
        with self.assertRaisesRegex(AssertionError, 'manifest digest'): H.consume()
        os.environ['FVOCI_WEB_BUILD_HANDOFF_SHA256'] = expected
        archive = self.packet/'payload.tar'; archive.write_bytes(archive.read_bytes()+b'corrupt')
        with self.assertRaises(AssertionError): H.consume()
        self.assertFalse((self.output/'before.json').exists())

    def test_missing_asset_or_member_hash_refused_even_with_valid_outer_hash(self):
        manifest = self.transfer(); original = copy.deepcopy(manifest)
        asset = next(k for k,e in manifest['entries'].items() if e['path']==str(self.dist/'index.html'))
        for fault in ('missing', 'hash'):
            with self.subTest(fault=fault):
                m = copy.deepcopy(original)
                if fault == 'missing': del m['entries'][asset]
                else: m['entries'][asset]['sha256'] = '0'*64
                self.change_manifest(m)
                with self.assertRaises(AssertionError): H.consume()
                self.assertFalse((self.output/'before.json').exists())

    def test_changed_physical_source_refused(self):
        self.transfer(); self.header.write_bytes(b'drift')
        with self.assertRaisesRegex(AssertionError, 'physical inputs differ'): H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())

    def test_changed_frontend_environment_refused(self):
        self.transfer(); os.environ['VITE_FIXTURE_INPUT'] = 'drift'
        with self.assertRaisesRegex(AssertionError, 'physical inputs differ'): H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())

    def test_ignored_dotenv_input_refused(self):
        self.transfer(); (self.root/'apps/web/.env.production.local').write_bytes(b'VITE_FIXTURE=drift')
        with self.assertRaisesRegex(AssertionError, 'physical inputs differ'): H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())

    def test_schema_feature_server_cannot_replace_default_server(self):
        bundle = self.get('bundle.json')
        for record in bundle['binaries'].values():
            if record['target']['name']=='fvoci-server': record['features']=['api-schema','db-tests']
        self.put('bundle.json', bundle)
        with self.assertRaises(AssertionError): H.export()
        self.assertFalse(self.packet.exists())

    def test_host_runtime_bytes_and_target_triple_refused(self):
        original = self.get('bundle.json')
        changed = copy.deepcopy(original); next(iter(changed['binaries'].values()))['targetTriple'] = 'aarch64-unknown-linux-gnu'
        self.put('bundle.json', changed)
        with self.assertRaises(AssertionError): H.export()
        self.put('bundle.json', original); self.library.write_bytes(b'other runner ABI')
        with self.assertRaises(AssertionError): H.export()

    def test_existing_dist_destination_and_symlink_asset_refused(self):
        manifest = self.transfer(); (self.dist/'index.html').write_bytes(b'stale')
        with self.assertRaisesRegex(AssertionError, 'existing destination'): H.consume()
        self.assertFalse((self.output/'before.json').exists())
        (self.dist/'index.html').unlink(); (self.dist/'index.html').symlink_to(self.header)
        with self.assertRaises(AssertionError): H.consume()

    @staticmethod
    def invalid_binary_fields():
        # Expected metadata comes from the ordinary debug/zero-debuginfo contract.
        return [('profile', 'opt_level', '3'), ('profile', 'opt_level', None),
                ('profile', 'opt_level', 0), ('profile', 'debuginfo', 2),
                ('profile', 'debuginfo', 'missing'), ('profile', 'debuginfo', False),
                ('profile', 'test', True), ('profile', 'test', 'missing'),
                ('profile', 'test', 0), ('target', 'kind', ['lib']),
                ('target', 'kind', ['bin', 'lib']), ('target', 'kind', 'missing')]

    @staticmethod
    def change_binary_field(record, field, key, value):
        if value == 'missing' or (key == 'opt_level' and value is None):
            del record[field][key]
        else:
            record[field][key] = value

    def test_wrong_or_missing_profile_and_bin_kind_refused_for_each_binary(self):
        original = self.get('bundle.json')
        for path in original['binaries']:
            for field, key, value in self.invalid_binary_fields():
                with self.subTest(binary=original['binaries'][path]['target']['name'], field=key, value=value):
                    changed = copy.deepcopy(original)
                    self.change_binary_field(changed['binaries'][path], field, key, value)
                    self.put('bundle.json', changed)
                    with self.assertRaisesRegex(AssertionError, 'browser (target|opt_level|debuginfo|test)'):
                        H.export()
                    self.assertFalse(self.packet.exists())
        self.put('bundle.json', original)

    def write_emitted_logs(self, bundle):
        for stage in H.BROWSER_STAGES:
            emitted = []
            for path, record in bundle['binaries'].items():
                name = record['target']['name']
                if (stage == 'fixture' and name == 'fvoci-e2e-fixture') or (stage == 'default' and name in ('fvoci-server','fvoci-migrate')) or (stage == 'engine' and name == 'collab-engine'):
                    emitted.append({'reason':'compiler-artifact','target':record['target'], 'executable':path,
                                    'features':record['features'],'profile':record['profile']})
            (self.output/(stage+'-compiler.jsonl')).write_text(''.join(json.dumps(a)+'\n' for a in emitted))

    def test_emitted_wrong_or_missing_profile_and_kind_refused_for_each_binary(self):
        original = self.get('bundle.json')
        for path in original['binaries']:
            for field, key, value in self.invalid_binary_fields():
                with self.subTest(binary=original['binaries'][path]['target']['name'], field=key, value=value):
                    changed = copy.deepcopy(original)
                    self.change_binary_field(changed['binaries'][path], field, key, value)
                    self.write_emitted_logs(changed)
                    (self.output/'after.json').unlink(missing_ok=True)
                    with self.assertRaisesRegex(AssertionError, 'browser (target|opt_level|debuginfo|test)'):
                        H.browser_after()
                    self.assertEqual(self.get('bundle.json'), original)

    def reseal_packet_bundle(self, change):
        manifest = self.transfer()
        archive = self.packet/'payload.tar'
        members = []
        with tarfile.open(archive, 'r:') as tar:
            for member in tar.getmembers():
                data = tar.extractfile(member).read()
                if manifest['entries'][member.name]['path'] == str(self.output/'bundle.json'):
                    bundle = json.loads(data)
                    change(next(iter(bundle['binaries'].values())))
                    data = json.dumps(bundle).encode()
                    member.size = len(data)
                    manifest['entries'][member.name].update(bytes=len(data), sha256=H.digest(data))
                members.append((member, data))
        with tarfile.open(archive, 'w') as tar:
            for member, data in members:
                tar.addfile(member, io.BytesIO(data))
        manifest['payload_sha256'] = H.CI.sha(archive)
        self.change_manifest(manifest)

    def test_consumer_rejects_resealed_foreign_profile(self):
        self.reseal_packet_bundle(lambda r: r['profile'].update(opt_level='3', debuginfo=2))
        with self.assertRaisesRegex(AssertionError, 'browser opt_level'): H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())

    def test_consumer_rejects_resealed_incomplete_profile(self):
        self.reseal_packet_bundle(lambda r: r.update(profile={'test':False}))
        with self.assertRaisesRegex(AssertionError, 'browser opt_level'): H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())

    def test_consumer_rejects_resealed_non_bin_target(self):
        self.reseal_packet_bundle(lambda r: r['target'].update(kind=['lib']))
        with self.assertRaisesRegex(AssertionError, 'browser target'): H.consume()
        self.assertFalse((self.output/'handoff-consumed.json').exists())

    def test_present_null_debuginfo_is_admitted(self):
        bundle = self.get('bundle.json')
        for record in bundle['binaries'].values(): record['profile']['debuginfo'] = None
        self.put('bundle.json', bundle)
        self.transfer(); H.consume()
        self.assertTrue((self.output/'handoff-consumed.json').exists())

    def test_producer_records_actual_emitted_features_and_default_commands(self):
        bundle = self.get('bundle.json')
        for name in ('after.json', 'compile-receipt.json', 'bundle.json', 'web-receipt.json', 'abi-receipt.json'):
            (self.output/name).unlink()
        self.write_emitted_logs(bundle)
        H.browser_after(); H.export()
        self.assertEqual(self.get('bundle.json')['binaries'], bundle['binaries'])
        calls = []
        def compile(command, **kwargs):
            calls.append(command)
            return type('CompilerResult', (), {'returncode':0})()
        for stage in H.BROWSER_STAGES:
            for suffix in ('-stage.json', '-compiler.jsonl', '-stderr.log'):
                (self.output/(stage+suffix)).unlink(missing_ok=True)
            with patch.object(H.CI.subprocess, 'run', side_effect=lambda command, **kwargs: type('GitResult', (), {'returncode':0})() if command[0]=='git' else compile(command, **kwargs)):
                self.assertEqual(H.browser_stage(stage), 0)
        self.assertIn('--features', calls[0]); self.assertIn('db-tests', calls[0])
        self.assertNotIn('--features', calls[1]); self.assertIn('fvoci-server', calls[1]); self.assertIn('fvoci-migrate', calls[1])
        self.assertIn('worker', calls[2])



class BrowserAssetsTest(unittest.TestCase):
    """Owned POSIX data/subprocess only: no Chromium, Bun or native execution."""
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(prefix='fvoci-browser-assets-')
        self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name)
        self.cache = self.root/'cache'
        self.component = self.cache/'chromium-1243'
        self.directory = self.component/'chrome-linux64'
        self.directory.mkdir(parents=True)
        self.chrome = self.directory/'chrome'
        self.chrome.write_bytes(b'NOT a browser executable')
        self.chrome.chmod(0o755)
        self.asset = self.directory/'deb.deps'
        self.asset.write_bytes(b'qualified supplemental asset')
        self.asset.chmod(0o600)
        shell = self.cache/'chromium_headless_shell-1243';shell.mkdir()
        (shell/'headless_shell').write_bytes(b'NOT executable');(shell/'headless_shell').chmod(0o700)
        ffmpeg = self.cache/'ffmpeg-1011';ffmpeg.mkdir();(ffmpeg/'ffmpeg-linux').write_bytes(b'NOT executable');(ffmpeg/'ffmpeg-linux').chmod(0o700)
        # The private runtime copy has its own /tmp parent, independently of
        # the preparation actor's private source cache and checkout ancestors.
        self.output_tmp = tempfile.TemporaryDirectory(prefix='fvoci-browser-output-')
        self.addCleanup(self.output_tmp.cleanup)
        self.output = Path(self.output_tmp.name)
        self.environment = patch.dict(os.environ, GITHUB_SHA=SHA)
        self.environment.start();self.addCleanup(self.environment.stop)

    def stage(self, output=None):return H.CI.prepare_browser(self.output if output is None else output, str(self.chrome))

    def runtime_probe(self):
        # Copy the actual helper into the exclusively owned runtime prefix;
        # preparation may be1001, while admission/access always execute as1000.
        module=self.output/'run-selected-backend-e2e.py'
        module.write_bytes((ROOT/'scripts/run-selected-backend-e2e.py').read_bytes());module.chmod(0o600)
        program="""import importlib.util,json,os,pathlib,sys
s=importlib.util.spec_from_file_location('owned',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
output=pathlib.Path(sys.argv[2]);current=m.admitted_browser(output)
m.runtime_access({}, {'bun':{'path':'/bin/true'},'chromium':{'path':current},
                      'chromium_directory_files':m.browser_inventory(pathlib.Path(current).parent)})
print(json.dumps({'admitted':current,'uid':os.getuid(),'gid':os.getgid(),'groups':os.getgroups(),
                  'supplemental':(pathlib.Path(current).parent/'deb.deps').read_text()}))
"""
        try:
            subprocess.run(['sudo','-n','chown','-R','1000:1000',str(self.output)],check=True)
            return subprocess.run(['sudo','-n','setpriv','--reuid=1000','--regid=1000','--clear-groups',
                'env','GITHUB_SHA='+SHA,'PYTHONDONTWRITEBYTECODE=1',
                'PLAYWRIGHT_BROWSERS_PATH='+str(self.output/'browser'),sys.executable,'-B','-c',
                program,str(module),str(self.output)],capture_output=True,text=True)
        finally:
            subprocess.run(['sudo','-n','chown','-R',str(os.getuid())+':'+str(os.getgid()),str(self.output)],check=True)

    def test_complete_private_copy_preserves_bytes_and_all_installed_chromium_components(self):
        current = self.stage()
        result=self.runtime_probe()
        self.assertEqual(result.returncode,0,result.stderr)
        runtime=json.loads(result.stdout)
        self.assertEqual(runtime['admitted'],str(current))
        receipt = H.CI.read(self.output/'runtime-browser-stage.json')
        self.assertEqual(set(receipt['files']), {'chromium-1243','chromium_headless_shell-1243','ffmpeg-1011'})
        self.assertEqual((current.parent/'deb.deps').read_bytes(), self.asset.read_bytes())
        self.assertEqual(self.asset.stat().st_mode & 0o777,0o600)
        self.assertEqual(self.cache.stat().st_uid,os.getuid())
        self.assertEqual(runtime['uid'],1000);self.assertEqual(runtime['gid'],1000)
        self.assertEqual(runtime['groups'],[])
        self.assertEqual(self.output.stat().st_uid,os.getuid())
        self.assertEqual(self.output.stat().st_mode & 0o777,0o700)
        print(json.dumps({'control':'private-browser-actor-roundtrip','preparation_uid':os.getuid(),
                          'preparation_gid':os.getgid(),'runtime_uid':runtime['uid'],'runtime_gid':runtime['gid'],
                          'runtime_groups':runtime['groups'],'admission_exit':result.returncode,
                          'returned_owner':self.output.stat().st_uid,'returned_mode':self.output.stat().st_mode & 0o777}),flush=True)

    def test_original_executable_only_preflight_misses_real_supplemental_permission_failure(self):
        # A fresh runner1001 cache with an owner-only asset: exact kernel refusal,
        # not a claim about the unavailable remote inode's mode/ACL.
        try:
            subprocess.run(['sudo','-n','chmod','755',str(self.root)],check=True)
            subprocess.run(['sudo','-n','chown','-R','1001:1001',str(self.cache)],check=True)
            program="""import os,pathlib,sys
chrome=pathlib.Path(sys.argv[1])
assert os.access(chrome,os.R_OK|os.X_OK)
try:(chrome.parent/'deb.deps').read_bytes()
except PermissionError:print('executable-preflight-OK supplemental-read-DENIED');sys.exit(13)
raise AssertionError('fixture must refuse supplemental read')
"""
            result=subprocess.run(['sudo','-n','setpriv','--reuid=1000','--regid=1000','--groups=1001',sys.executable,'-c',program,str(self.chrome)],capture_output=True,text=True)
            self.assertEqual(result.returncode,13,result.stderr)
            self.assertIn('supplemental-read-DENIED',result.stdout)
            # Real nonprivileged runner1001 stages the same bytes; runtime1000
            # owns only the newly created copy, never the shared source cache.
            subprocess.run(['sudo','-n','chown','1001:1001',str(self.output)],check=True)
            module=self.root/'run-selected-backend-e2e.py'
            module.write_bytes((ROOT/'scripts/run-selected-backend-e2e.py').read_bytes());module.chmod(0o644)
            prepare="""import importlib.util,pathlib,sys
s=importlib.util.spec_from_file_location('owned',sys.argv[1]);m=importlib.util.module_from_spec(s);s.loader.exec_module(m)
m.prepare_browser(pathlib.Path(sys.argv[2]),sys.argv[3])
"""
            staged=subprocess.run(['sudo','-n','setpriv','--reuid=1001','--regid=1001','--clear-groups','env','GITHUB_SHA='+SHA,'PYTHONDONTWRITEBYTECODE=1',sys.executable,'-c',prepare,str(module),str(self.output),str(self.chrome)],capture_output=True,text=True)
            self.assertEqual(staged.returncode,0,staged.stderr)
            subprocess.run(['sudo','-n','chown','-R',str(os.getuid())+':'+str(os.getgid()),str(self.output)],check=True)
            current=self.output/'browser/chromium-1243/chrome-linux64/chrome'
            admitted=self.runtime_probe()
            self.assertEqual(admitted.returncode,0,admitted.stderr)
            self.assertEqual(json.loads(admitted.stdout)['admitted'],str(current))
            self.assertEqual(json.loads(admitted.stdout)['uid'],1000)
            self.assertEqual(json.loads(admitted.stdout)['gid'],1000)
            self.assertEqual(json.loads(admitted.stdout)['groups'],[])
            self.assertEqual(admitted.returncode,0,admitted.stderr)
            self.assertIn('qualified supplemental asset',json.loads(admitted.stdout)['supplemental'])
            source=subprocess.check_output(['sudo','-n','stat','-c','%u:%g:%a',str(self.asset)],text=True).strip()
            self.assertEqual(source,'1001:1001:600')
            print(json.dumps({'control':'original-incomplete-preflight-and-fixed-private-copy','original_exit':result.returncode,
                              'stage_exit':staged.returncode,'runtime_read_exit':admitted.returncode,'source_mode_preserved':source,'runtime_supplementary_groups':[]}),flush=True)
        finally:
            subprocess.run(['sudo','-n','chown','-R',str(os.getuid())+':'+str(os.getgid()),str(self.root)],check=True)

    def test_unreadable_asset_refused_before_copy_or_runtime(self):
        self.asset.chmod(0)
        try:
            with self.assertRaises(PermissionError):self.stage()
            self.assertFalse((self.output/'browser').exists())
        finally:self.asset.chmod(0o600)

    def test_foreign_root_group_and_symlink_assets_refused(self):
        foreign=max(os.getuid(),os.getgid(),1000)+1
        for index,(uid,gid) in enumerate(((foreign,foreign),(0,os.getgid()),(os.getuid(),0),(os.getuid(),foreign))):
            with self.subTest(uid=uid,gid=gid):
                try:
                    subprocess.run(['sudo','-n','chown',str(uid)+':'+str(gid),str(self.asset)],check=True)
                    output=self.output/('negative-'+str(index));output.mkdir(mode=0o700)
                    with self.assertRaisesRegex(AssertionError,'foreign or privileged'):self.stage(output)
                    self.assertFalse((output/'browser').exists())
                    print(json.dumps({'control':'browser-foreign-owner-refused','preparation_uid':os.getuid(),
                                      'preparation_gid':os.getgid(),'asset_uid':uid,'asset_gid':gid,
                                      'copy_created':False}),flush=True)
                finally:subprocess.run(['sudo','-n','chown',str(os.getuid())+':'+str(os.getgid()),str(self.asset)],check=True)
        self.asset.unlink();self.asset.symlink_to(self.chrome)
        with self.assertRaisesRegex(AssertionError,'nonregular'):self.stage()

    def test_runtime_new_missing_changed_and_unreadable_assets_refused(self):
        current=self.stage()
        with patch.dict(os.environ,PLAYWRIGHT_BROWSERS_PATH=str(self.output/'browser')):
            for change in ('new','missing','changed','unreadable','mode','directory'):
                with self.subTest(change=change):
                    asset=current.parent/'deb.deps'
                    if change=='new':(current.parent/'unexpected').write_bytes(b'new')
                    elif change=='missing':asset.unlink()
                    elif change=='changed':asset.write_bytes(b'drift')
                    elif change=='mode':asset.chmod(0o640)
                    elif change=='directory':(current.parent/'unexpected-empty').mkdir()
                    else:asset.chmod(0)
                    result=self.runtime_probe()
                    self.assertNotEqual(result.returncode,0)
                    self.assertRegex(result.stderr,'AssertionError|PermissionError')
                    if change=='new':(current.parent/'unexpected').unlink()
                    elif change=='directory':(current.parent/'unexpected-empty').rmdir()
                    else:asset.chmod(0o600) if asset.exists() else None;asset.write_bytes(b'qualified supplemental asset');asset.chmod(0o600)

    def test_source_drift_during_copy_and_occupied_prefix_refused(self):
        original=H.CI.shutil.copytree
        def mutate(source,destination,*args,**kwargs):
            result=original(source,destination,*args,**kwargs)
            if source==self.component:self.asset.write_bytes(b'drift')
            return result
        with patch.object(H.CI.shutil,'copytree',side_effect=mutate):
            with self.assertRaisesRegex(AssertionError,'source changed'):self.stage()
        self.assertFalse((self.output/'runtime-browser-stage.json').exists())
        self.asset.write_bytes(b'qualified supplemental asset')
        with self.assertRaises(FileExistsError):self.stage()


class HistoricalFixturePortabilityTest(unittest.TestCase):
    """Actual permission controls in an owned checkout without the c7 object."""
    def test_permission_controls_without_historical_git_object(self):
        self.assertEqual(hashlib.sha256(ORIGINAL_C7_SELECTED_FOOTER.encode()).hexdigest(),
                         '14b99c809b2a3c0ed445774f334378e74f38db5ce0abe373e47abf30c43fcd06')
        with tempfile.TemporaryDirectory(prefix='fvoci-no-history-fixture-') as tmp:
            root = Path(tmp)
            for name in ('scripts/fixtures/web-e2e/test-build-handoff.py',
                         'scripts/selected-backend-ci/web-build-handoff.py',
                         'scripts/run-selected-backend-e2e.py', 'scripts/run-web-e2e.sh'):
                destination = root/name
                destination.parent.mkdir(parents=True, exist_ok=True)
                destination.write_bytes((ROOT/name).read_bytes())
            subprocess.run(['git', 'init', '--quiet', str(root)], check=True)
            missing = subprocess.run(['git', 'cat-file', '-e',
                'c7ad4a2a9165a0dfb64ebc7f17150140b54ed1fe^{commit}'], cwd=root,
                capture_output=True, text=True)
            self.assertNotEqual(missing.returncode, 0)
            result = subprocess.run([sys.executable, '-B',
                str(root/'scripts/fixtures/web-e2e/test-build-handoff.py'),
                'RuntimePermissionsTest'], cwd=root, capture_output=True, text=True,
                env={**os.environ, 'PYTHONDONTWRITEBYTECODE':'1'})
            self.assertEqual(result.returncode, 0, result.stderr)
            records = [json.loads(line) for line in result.stdout.splitlines()
                       if line.startswith('{')]
            original = next(record for record in records if record['control']=='original')
            self.assertEqual(original['footer_exit'], 2)
            self.assertTrue(original['err13'])
            self.assertIsNone(original['marker'])
            fixed = next(record for record in records
                         if record['control']=='fixed' and record['selected_exit']==0)
            self.assertEqual(fixed['footer_exit'], 0)
            self.assertEqual(fixed['marker']['uid'], 1000)
            self.assertEqual(fixed['marker']['gid'], 1000)
            failed = next(record for record in records
                          if record['control']=='fixed' and record['selected_exit']==7)
            self.assertEqual(failed['footer_exit'], 7)
            for control in ('unreadable', 'foreign', 'symlink', 'incomplete',
                            'wrong-source', 'missing-process', 'live'):
                self.assertTrue(any(record['control']==control and record['footer_exit']!=0
                                    for record in records), control)
            print(json.dumps({'control':'no-history-checkout','historical_object_exit':missing.returncode,
                              'original_guard_exit':original['footer_exit'], 'original_err13':original['err13'],
                              'fixed_guard_exit':fixed['footer_exit'], 'first_child_status':failed['footer_exit'],
                              'negative_controls':7}), flush=True)


if __name__=='__main__':unittest.main()
