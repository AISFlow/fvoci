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
def identity():return 'owned-permission-fixture'
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
    if before['fixture_fault'] in ('closed','wrong-source','missing-process','live','missing-port','invalid-port','missing-pid','unsafe-canary'):
        runtime=output/'runtime';runtime.mkdir()
        runs=[]
        for lane in ('install','postgres','sqlite'):
            root=runtime/('root-current-'+lane+'-fixture');root.mkdir()
            write(output/(lane+'-allocation.json'),{})
            facts={'source':before['head'],'final_exit_code':0,'owned_container_absent':True,
                   'owned_loopback_port_closed':True,'recorded_process_identities_retired':True,'cleanup_errors':[]}
            if lane=='install':
                facts['actual_owned_process_receipts']=15
                retained=root/'retained-run';retained.mkdir()
                for i in range(15):write(retained/(str(i)+'-process.json'),{'status':None if before['fixture_fault']=='missing-process' and i==0 else 0})
            if lane=='postgres':write(root/'parent-receipt.json',{'all_owned_fixtures_closed':True})
            if before['fixture_fault']=='wrong-source':facts['source']='foreign-source'
            if before['fixture_fault']=='live':facts['recorded_process_identities_retired']=False
            if lane=='sqlite':
                facts['final_exit_code']=before['fixture_exit']
                if before['fixture_fault'] in ('missing-port','unsafe-canary'):facts.pop('owned_loopback_port_closed')
                if before['fixture_fault']=='invalid-port':facts['owned_loopback_port_closed']='PRIVATE_CANARY_URL'
                if before['fixture_fault']=='missing-pid':facts.pop('recorded_process_identities_retired')
                if before['fixture_fault']=='unsafe-canary':
                    facts['original_driver_failure']={'message':'PRIVATE_CANARY_URL secret=PRIVATE_CANARY_SECRET'}
                    facts['session']='PRIVATE_CANARY_SESSION';facts['headers']={'Authorization':'PRIVATE_CANARY_SECRET'}
            write(root/'receipt.json',facts)
            runs.append({'lane':lane,'runRoot':str(root),'exit':facts['final_exit_code']})
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
            footer=source[source.index('selected_status=0\n'):]
            environment={'PATH':str(fake)+':'+os.defpath,'ROOT':str(repo),'RUNNER_TEMP':str(temp),
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
        self.assertEqual(facts['close']['closed_current_lanes'],['install','postgres','sqlite'])
        self.assertEqual(facts['output'],{'uid':0,'gid':1001,'mode':0o700})
        for fault in ('wrong-source','missing-process','live'):
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
                self.assertEqual(facts['safe_launcher'],{'actual_launcher_exit':7,'ownership_return_exit':1,'selected_final_exit':7,'pending_exit':0})
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


class FailureOriginTest(unittest.TestCase):
    """Execute the actual exception/projection AST, with no product processes."""
    driver = ROOT / 'scripts/selected-backend-ci/current-sqlite-driver.py'

    def producer(self, phase, body, extra=None):
        tree = ast.parse(self.driver.read_text())
        original = next(node for node in tree.body if isinstance(node, ast.Try))
        controlled = ast.Try(body=[body], handlers=copy.deepcopy(original.handlers), orelse=[], finalbody=[])
        ast.copy_location(controlled, original)
        namespace = {'__file__':str(self.driver), 'receipt':{}, 'code':0,
                     'driver_phase':phase, 'sha':H.CI.sha, **(extra or {})}
        exec(compile(ast.fix_missing_locations(ast.Module(body=[controlled], type_ignores=[])),
                     str(self.driver), 'exec'), namespace)
        self.assertEqual(namespace['code'],1)
        return namespace['receipt']

    def project(self, facts, port=True, target_lane='sqlite'):
        with tempfile.TemporaryDirectory() as tmp:
            output=Path(tmp); runtime=output/'runtime';runtime.mkdir()
            selected_spec='apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
            (output/'before.json').write_text(json.dumps({'head':SHA,'tree':TREE,'tracked':{selected_spec:H.CI.sha(ROOT/selected_spec)}}))
            runs=[]
            for lane in ('install','postgres','sqlite'):
                root=runtime/('root-current-'+lane+'-fixture');root.mkdir()
                row={'source':SHA,'final_exit_code':7 if lane==target_lane else 0,
                     'owned_container_absent':True,'owned_loopback_port_closed':True,
                     'recorded_process_identities_retired':True,'cleanup_errors':[]}
                if lane=='install':
                    row['actual_owned_process_receipts']=15
                    retained=root/'retained-run';retained.mkdir()
                    for n in range(15):(retained/(str(n)+'-process.json')).write_text('{"status":0}')
                if lane=='postgres':(root/'parent-receipt.json').write_text('{"all_owned_fixtures_closed":true}')
                if lane==target_lane:
                    row.update(facts);row['driver_sha256']=H.CI.sha(self.driver)
                    if port is None:row.pop('owned_loopback_port_closed')
                    else:row['owned_loopback_port_closed']=port
                (root/'receipt.json').write_text(json.dumps(row))
                runs.append({'lane':lane,'runRoot':str(root),'exit':7 if lane==target_lane else 0})
            (output/'selected-ci-receipt.json').write_text(json.dumps({'owner':'fixture','source':SHA,'tree':TREE,'runs':runs}))
            captured=io.StringIO();error=None
            with patch.object(H.CI,'identity',return_value='fixture'), patch.object(os,'getuid',return_value=1000), \
                    patch.object(os,'getgid',return_value=1000), patch.dict(os.environ,{'GITHUB_JOB':'collaboration-flow','GITHUB_SHA':SHA}), \
                    patch('sys.stdout',captured):
                try:H.CI.runtime_ownership_return(output)
                except AssertionError as caught:error=caught
            return json.loads(captured.getvalue()),error,(output/'runtime-close-stage.json').exists()

    def hash_assertion(self):
        return next(node for node in ast.walk(ast.parse(self.driver.read_text()))
                    if isinstance(node,ast.Assert) and ast.unparse(node.test).startswith('[line.split()[0]'))

    def test_absent_optional_bindings_admit_closed_pg_and_sqlite_without_templates(self):
        with tempfile.TemporaryDirectory() as tmp, patch.object(H.CI, 'TEMPLATES', Path(tmp)/'missing-templates'):
            summary,error,closed=self.project({}, target_lane='install')
        self.assertIsNone(error);self.assertTrue(closed)
        self.assertEqual([lane['lane'] for lane in summary['lanes']], ['install','postgres','sqlite'])
        for lane in summary['lanes'][1:]:
            self.assertIsNone(lane['original_driver_failure_origin'])
            self.assertIsNone(lane['original_browser_failure'])
            self.assertIsNone(lane['network_mode_observation'])
        self.assertTrue(summary['ownership_return_qualified'])

    def test_optional_binding_omission_does_not_waive_declared_valid_driver_io(self):
        function=next(node for node in ast.parse((ROOT/'scripts/run-selected-backend-e2e.py').read_text()).body
                      if isinstance(node,ast.FunctionDef) and node.name=='runtime_ownership_return')
        binding=next(node for node in ast.walk(function) if isinstance(node,ast.Assign) and
                     any(isinstance(target,ast.Name) and target.id=='bound_driver' for target in node.targets))
        code=compile(ast.Module(body=[binding],type_ignores=[]),'<actual bound_driver AST>','exec')
        with tempfile.TemporaryDirectory() as tmp:
            namespace={'run':{'lane':'postgres'},'before':{'head':SHA},'driver':Path(tmp)/'missing-driver.py',
                       're':H.CI.re,'sha':H.CI.sha}
            for digest in (None,True,1,'','f'*63,'PRIVATE_CANARY_HASH'):
                with self.subTest(digest=digest):
                    namespace['facts']={'source':SHA,'driver_sha256':digest}
                    exec(code,namespace)
                    self.assertFalse(namespace['bound_driver'])
            namespace['facts']={'source':SHA,'driver_sha256':'f'*64}
            with self.assertRaises(FileNotFoundError):exec(code,namespace)
            with patch.dict(namespace,{'sha':lambda path: (_ for _ in ()).throw(PermissionError('private IO'))}):
                with self.assertRaises(PermissionError):exec(code,namespace)

    def test_original_hash_and_network_failures_have_exact_source_origin(self):
        check=self.hash_assertion()
        values={'hashes':'b'*64+' server\n'+'a'*64+' migrate\n'+'c'*64+' engine\n',
                'server':'server','migrate':'migrate','engine':'engine',
                'binaries':{name:{'sha256':char*64} for name,char in zip(('server','migrate','engine'),'abc')}}
        facts=self.producer('copied-native-hashes',copy.deepcopy(check),values)
        self.assertEqual(facts['original_driver_failure'],{'type':'AssertionError','message':''})
        origin=facts['original_driver_failure_origin']
        self.assertEqual(origin,{'phase':'copied-native-hashes','driver_sha256':H.CI.sha(self.driver),'line':check.lineno,'type':'AssertionError'})
        network=next(node for node in ast.walk(ast.parse(self.driver.read_text()))
                     if isinstance(node,ast.Assert) and 'actual-network-mode.log' in ast.unparse(node.test))
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);(root/'actual-network-mode.log').write_text('bridge\n')
            facts=self.producer('owned-network-mode',copy.deepcopy(network),{'run':root})
        self.assertEqual(facts['original_driver_failure_origin']['line'],network.lineno)
        self.assertEqual(facts['original_driver_failure_origin']['phase'],'owned-network-mode')

    def test_count_hash_and_order_checks_remain_strict(self):
        check=self.hash_assertion();namespace={'server':'server','migrate':'migrate','engine':'engine',
            'binaries':{name:{'sha256':char*64} for name,char in zip(('server','migrate','engine'),'abc')}}
        for chars in ('ab','abcd','bac','abd'):
            with self.subTest(chars=chars):
                namespace['hashes']=''.join(char*64+' file\n' for char in chars)
                with self.assertRaises(AssertionError):exec(compile(ast.Module(body=[check],type_ignores=[]),str(self.driver),'exec'),namespace)
        namespace['hashes']=''.join(char*64+' file\n' for char in 'abc')
        exec(compile(ast.Module(body=[check],type_ignores=[]),str(self.driver),'exec'),namespace)

    def test_missing_or_false_port_keeps_original_failure_and_refuses_return(self):
        facts=self.producer('copied-native-hashes',copy.deepcopy(self.hash_assertion()),
            {'hashes':'','server':'server','migrate':'migrate','engine':'engine','binaries':{name:{'sha256':'a'*64} for name in ('server','migrate','engine')}})
        for port in (None,False,'true',1):
            with self.subTest(port=port):
                summary,error,closed=self.project(facts,port)
                self.assertIsNotNone(error);self.assertFalse(closed)
                self.assertFalse(summary['ownership_return_qualified'])
                lane=summary['lanes'][-1]
                self.assertEqual(lane['launcher_observed_driver_exit'],7)
                self.assertEqual(lane['receipt_final_exit'],7)
                self.assertEqual(lane['original_driver_failure_origin'],facts['original_driver_failure_origin'])

    def test_unknown_or_foreign_origin_schema_type_and_source_are_not_published(self):
        valid={'phase':'copied-native-hashes','driver_sha256':H.CI.sha(self.driver),'line':self.hash_assertion().lineno,'type':'AssertionError'}
        for key,value in [('phase','PRIVATE_CANARY_URL'),('type','PRIVATE_CANARY_SECRET'),('line',True),('line',0),('line',10**6),('driver_sha256','f'*64),('extra','PRIVATE_CANARY_PATH')]:
            with self.subTest(key=key,value=value):
                origin={**valid,key:value};summary,error,closed=self.project({'original_driver_failure_origin':origin})
                self.assertIsNone(error);self.assertTrue(closed)
                self.assertIsNone(summary['lanes'][-1]['original_driver_failure_origin'])
                self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
        summary,error,closed=self.project({'source':'f'*40,'original_driver_failure_origin':valid})
        self.assertIsNotNone(error);self.assertFalse(closed)
        self.assertIsNone(summary['lanes'][-1]['original_driver_failure_origin'])

    def test_private_exception_canary_is_hashed_not_published(self):
        body=ast.parse("raise RuntimeError('PRIVATE_CANARY_URL secret=PRIVATE_CANARY_SECRET')").body[0]
        ast.copy_location(body,self.hash_assertion())
        facts=self.producer('normal-runtime',body)
        self.assertIn('PRIVATE_CANARY',facts['original_driver_failure']['message'])
        summary,error,closed=self.project(facts,None)
        self.assertIsNotNone(error);self.assertFalse(closed)
        self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
        self.assertEqual(summary['lanes'][-1]['original_driver_failure_origin']['type'],'RuntimeError')

    def network_observation(self, stdout, stderr, exit_code=0):
        tree=ast.parse(self.driver.read_text())
        original=next(node for node in tree.body if isinstance(node,ast.Try))
        start=next(i for i,node in enumerate(original.body) if isinstance(node,ast.Assign) and
                   ast.unparse(node)=="driver_phase = 'owned-network-mode'")
        end=next(i for i,node in enumerate(original.body[start+1:],start+1) if isinstance(node,ast.Assign) and
                 ast.unparse(node)=="driver_phase = 'normal-runtime'")
        controlled=ast.Try(body=copy.deepcopy(original.body[start:end]),handlers=copy.deepcopy(original.handlers),orelse=[],finalbody=[])
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp)
            result=subprocess.CompletedProcess(['SYNTHETIC_DOCKER_INSPECT'],exit_code,stdout,stderr)
            namespace={'command':lambda *args,**kwargs:result,'run':root,'name':'fixture','__file__':str(self.driver),
                       'sha':H.CI.sha,'os':os,'sys':sys,'hashlib':hashlib,'receipt':{},'code':0}
            exec(compile(ast.fix_missing_locations(ast.Module(body=[controlled],type_ignores=[])),str(self.driver),'exec'),namespace)
            self.assertEqual((root/'actual-network-mode.log').read_text(),stdout)
            self.assertEqual((root/'actual-network-mode-stderr.private.log').read_text(),stderr)
            self.assertEqual((root/'actual-network-mode-stderr.private.log').stat().st_mode&0o777,0o600)
            return namespace['receipt'],namespace['code']

    def test_actual_stdout_only_mode_keeps_nonhost_and_command_failure_strict(self):
        for stdout,stderr,exit_code,expected in [('host\n','PRIVATE_CANARY_WARNING',0,0),
                ('bridge\n','host\n',0,1),('','host\n',0,1),('host\nextra\n','',0,1),
                ('PRIVATE_CANARY_MODE','PRIVATE_CANARY_SECRET',0,1),('host\n','',9,1)]:
            with self.subTest(stdout=stdout,exit_code=exit_code):
                facts,code=self.network_observation(stdout,stderr,exit_code)
                self.assertEqual(code,expected)
                summary,error,closed=self.project(facts,None)
                self.assertIsNotNone(error);self.assertFalse(closed)
                observed=summary['lanes'][-1]['network_mode_observation']
                self.assertEqual(observed['exit'],exit_code)
                self.assertEqual(observed['stderr_present'],bool(stderr))
                self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
                if code:self.assertEqual(summary['lanes'][-1]['original_driver_failure_origin'],facts['original_driver_failure_origin'])

    def test_network_observation_unknown_and_foreign_canaries_are_not_published(self):
        facts,_=self.network_observation('host\n','PRIVATE_CANARY_SECRET')
        valid=facts['network_mode_observation']
        for key,value in [('phase','PRIVATE_CANARY_PHASE'),('driver_sha256','f'*64),('line',True),
                ('line',0),('line',10**6),('exit',True),('exit',999),('stdout_mode','PRIVATE_CANARY_URL'),
                ('stdout_sha256','PRIVATE_CANARY_SECRET'),('stderr_present','true'),('stderr_sha256','bad'),('extra','PRIVATE_CANARY_PAYLOAD')]:
            with self.subTest(key=key):
                summary,error,closed=self.project({'network_mode_observation':{**valid,key:value}})
                self.assertIsNone(error);self.assertTrue(closed)
                self.assertIsNone(summary['lanes'][-1]['network_mode_observation'])
                self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
        summary,error,closed=self.project({'source':'f'*40,'network_mode_observation':valid})
        self.assertIsNotNone(error);self.assertFalse(closed)
        self.assertIsNone(summary['lanes'][-1]['network_mode_observation'])

    def test_postgres_exception_first_origin_survives_private_preservation_failure(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            body=ast.parse("raise RuntimeError('PRIVATE_CANARY_SECRET')").body[0]
            ast.copy_location(body,self.hash_assertion())
            facts=self.producer('normal-runtime',body)
            first=copy.deepcopy(facts['original_driver_failure_origin'])
            original=next(node for node in ast.parse(self.driver.read_text()).body if isinstance(node,ast.Try))
            preservation=next(node for node in original.finalbody if isinstance(node,ast.Try))
            controlled=ast.Try(body=[copy.deepcopy(body)],handlers=copy.deepcopy(preservation.handlers),orelse=[],finalbody=[])
            namespace={'receipt':facts,'code':7,'sha':H.CI.sha,'__file__':str(self.driver)}
            exec(compile(ast.fix_missing_locations(ast.Module(body=[controlled],type_ignores=[])),str(self.driver),'exec'),namespace)
            self.assertEqual(namespace['code'],7)
            self.assertEqual(facts['original_driver_failure_origin'],first)
            summary,error,closed=self.project(facts,target_lane='postgres')
            self.assertIsNone(error);self.assertTrue(closed)
            pg=summary['lanes'][1]
            self.assertEqual(pg['original_driver_failure_origin'],first)
            self.assertRegex(pg['native_preservation_error_sha256'],'^[0-9a-f]{64}$')
            self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))

    def postgres_browser_failure(self, report=None):
        tree=ast.parse(self.driver.read_text())
        record=next(node for node in ast.walk(tree) if isinstance(node,ast.If) and
                    'original_browser_failure' in ast.unparse(node) and ast.unparse(node.test)=='code != 0')
        with tempfile.TemporaryDirectory() as tmp:
            root=Path(tmp);(root/'browser.log').write_text('PRIVATE_CANARY_BROWSER_SECRET')
            (root/'playwright-result.private.json').write_text(json.dumps(report if report is not None else {'secret':'PRIVATE_CANARY_REPORT'}))
            spec=ROOT/'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
            namespace={'receipt':{},'code':7,'driver_phase':'browser-run','sha':H.CI.sha,
                       '__file__':str(self.driver),'sys':sys,'json':json,'run':root,'W':ROOT,'SPEC':spec.name,
                       'before':{'tracked':{str(spec.relative_to(ROOT)):H.CI.sha(spec)}}}
            exec(compile(ast.Module(body=[copy.deepcopy(record)],type_ignores=[]),str(self.driver),'exec'),namespace)
            return namespace['receipt']

    def test_postgres_observed_browser_exit_private_hash_and_closure_refusal(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            facts=self.postgres_browser_failure()
            for port in (True,None,False,'true',1):
                with self.subTest(port=port):
                    summary,error,closed=self.project(facts,port,target_lane='postgres')
                    self.assertEqual(closed,port is True)
                    self.assertEqual(error is None,port is True)
                    pg=summary['lanes'][1]
                    self.assertEqual(pg['original_browser_failure'],facts['original_browser_failure'])
                    self.assertEqual(pg['receipt_final_exit'],7)
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))

    def test_postgres_browser_unknown_proof_and_artifact_secret_canaries_are_suppressed(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            valid=self.postgres_browser_failure()['original_browser_failure']
            for key,value in [('phase','PRIVATE_CANARY_PHASE'),('driver_sha256','f'*64),('line',True),
                    ('line',0),('line',10**6),('exit',True),('exit',0),('exit',999),
                    ('log_sha256','PRIVATE_CANARY_SECRET'),('report_sha256','PRIVATE_CANARY_ARTIFACT'),
                    ('extra','PRIVATE_CANARY_PAYLOAD')]:
                with self.subTest(key=key):
                    summary,error,closed=self.project({'original_browser_failure':{**valid,key:value}},target_lane='postgres')
                    self.assertIsNone(error);self.assertTrue(closed)
                    self.assertIsNone(summary['lanes'][1]['original_browser_failure'])
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))

    def test_postgres_exception_unknown_source_phase_line_type_are_not_published(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            valid={'phase':'normal-runtime','driver_sha256':H.CI.sha(self.driver),'line':self.hash_assertion().lineno,'type':'RuntimeError'}
            for key,value in [('phase','owned-network-mode'),('type','PRIVATE_CANARY_TYPE'),('line',True),
                    ('line',0),('line',10**6),('driver_sha256','f'*64),('extra','PRIVATE_CANARY_PAYLOAD')]:
                with self.subTest(key=key):
                    summary,error,closed=self.project({'original_driver_failure_origin':{**valid,key:value}},target_lane='postgres')
                    self.assertIsNone(error);self.assertTrue(closed)
                    self.assertIsNone(summary['lanes'][1]['original_driver_failure_origin'])
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
            summary,error,closed=self.project({'source':'f'*40,'original_driver_failure_origin':valid},target_lane='postgres')
            self.assertIsNotNone(error);self.assertFalse(closed)
            self.assertIsNone(summary['lanes'][1]['original_driver_failure_origin'])

    def test_postgres_diagnostic_never_substitutes_malformed_retirement_proof(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            facts=self.postgres_browser_failure()
            for key,value in [('owned_container_absent',False),('owned_container_absent',1),
                    ('recorded_process_identities_retired',False),('recorded_process_identities_retired','true'),
                    ('cleanup_errors',['PRIVATE_CANARY_CLEANUP']),('cleanup_errors',False),
                    ('final_exit_code',True),('final_exit_code',0)]:
                with self.subTest(key=key):
                    summary,error,closed=self.project({**facts,key:value},target_lane='postgres')
                    self.assertIsNotNone(error);self.assertFalse(closed)
                    self.assertFalse(summary['ownership_return_qualified'])
                    self.assertEqual(summary['lanes'][1]['original_browser_failure'],facts['original_browser_failure'])
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))

    def browser_error_report(self, location, status='failed', retry=0):
        return {'suites':[{'specs':[{'tests':[{'results':[{'status':status,'retry':retry,
            'errors':[{'location':location,'message':'PRIVATE_CANARY_MESSAGE','stack':'PRIVATE_CANARY_STACK',
                       'expected':'PRIVATE_CANARY_PASSWORD','actual':'PRIVATE_CANARY_URL'}]}]}]}]}]}

    def test_first_actual_browser_assertion_location_requires_exact_committed_spec(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            spec=ROOT/'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
            report=self.browser_error_report({'file':str(spec),'line':361,'column':1})
            facts=self.postgres_browser_failure(report)
            expected={'spec_sha256':H.CI.sha(spec),'line':361}
            self.assertEqual(facts['original_browser_failure']['assertion_location'],expected)
            summary,error,closed=self.project(facts,target_lane='postgres')
            self.assertIsNone(error);self.assertTrue(closed)
            self.assertEqual(summary['lanes'][1]['original_browser_failure']['assertion_location'],expected)
            self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
            for location in (None,{'file':'PRIVATE_CANARY_PATH','line':361},
                    {'file':str(spec),'line':True},{'file':str(spec),'line':0},
                    {'file':str(spec),'line':10**6}):
                with self.subTest(location=location):
                    observed=self.postgres_browser_failure(self.browser_error_report(location))
                    self.assertIsNone(observed['original_browser_failure']['assertion_location'])
            for status,retry in [('passed',0),('failed',1),('failed',True)]:
                observed=self.postgres_browser_failure(self.browser_error_report({'file':str(spec),'line':361},status,retry))
                self.assertIsNone(observed['original_browser_failure']['assertion_location'])
            for malformed in ({}, {'suites':[]}, {'suites':[{'specs':[{'tests':[{'results':[[]]}]}]}]}):
                observed=self.postgres_browser_failure(malformed)
                self.assertEqual(observed['original_browser_failure']['exit'],7)
                self.assertIsNone(observed['original_browser_failure']['assertion_location'])
            first_unknown=self.browser_error_report(None)
            first_unknown['suites'][0]['specs'][0]['tests'][0]['results'][0]['errors'].append({'location':{'file':str(spec),'line':361}})
            self.assertIsNone(self.postgres_browser_failure(first_unknown)['original_browser_failure']['assertion_location'])

    def test_browser_assertion_location_foreign_hash_line_and_artifact_canaries_suppressed(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            facts=self.postgres_browser_failure()
            spec=ROOT/'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
            valid={'spec_sha256':H.CI.sha(spec),'line':361}
            for key,value in [('spec_sha256','f'*64),('spec_sha256','PRIVATE_CANARY_SECRET'),
                    ('line',True),('line',0),('line',10**6),('extra','PRIVATE_CANARY_PAYLOAD')]:
                with self.subTest(key=key):
                    invalid={**facts,'original_browser_failure':{**facts['original_browser_failure'],'assertion_location':{**valid,key:value}}}
                    summary,error,closed=self.project(invalid,target_lane='postgres')
                    self.assertIsNone(error);self.assertTrue(closed)
                    self.assertIsNone(summary['lanes'][1]['original_browser_failure']['assertion_location'])
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))

    def test_secondary_failure_origins_preserve_browser_first_exit_and_require_source_binding(self):
        with patch.object(self,'driver',ROOT/'scripts/selected-backend-ci/current-postgres-driver.py'):
            tree=ast.parse(self.driver.read_text())
            main=next(node for node in tree.body if isinstance(node,ast.Try))
            post=next(node for node in main.finalbody if isinstance(node,ast.Try) and 'post_input_failure_origin' in ast.unparse(node))
            body=ast.parse("raise PermissionError('PRIVATE_CANARY_PERMISSION')").body[0]
            ast.copy_location(body,self.hash_assertion())
            controlled=ast.Try(body=[body],handlers=copy.deepcopy(post.handlers),orelse=[],finalbody=[])
            facts=self.postgres_browser_failure();first=copy.deepcopy(facts['original_browser_failure'])
            namespace={'receipt':facts,'code':7,'cleanup_errors':[],'sha':H.CI.sha,'__file__':str(self.driver)}
            exec(compile(ast.fix_missing_locations(ast.Module(body=[controlled],type_ignores=[])),str(self.driver),'exec'),namespace)
            self.assertEqual(facts['original_browser_failure'],first)
            summary,error,closed=self.project(facts,target_lane='postgres')
            self.assertIsNone(error);self.assertTrue(closed)
            observed=summary['lanes'][1]['secondary_failure_origins']['post_input_failure_origin']
            self.assertEqual(observed,facts['post_input_failure_origin'])
            self.assertEqual(summary['lanes'][1]['original_browser_failure'],first)
            self.assertEqual(summary['lanes'][1]['receipt_final_exit'],7)
            self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
            for key,value in [('phase','PRIVATE_CANARY_PHASE'),('type','PRIVATE_CANARY_TYPE'),
                    ('line',True),('line',0),('line',10**6),('driver_sha256','f'*64),('extra','PRIVATE_CANARY_PAYLOAD')]:
                with self.subTest(key=key):
                    invalid={**facts,'post_input_failure_origin':{**observed,key:value}}
                    summary,error,closed=self.project(invalid,target_lane='postgres')
                    self.assertIsNone(summary['lanes'][1]['secondary_failure_origins']['post_input_failure_origin'])
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))


    def observed_browser_diagnostic(self, lane, report=None, raw='PRIVATE_CANARY_STREAM', altered_inputs=False):
        driver=ROOT/('scripts/selected-backend-ci/current-'+lane+'-driver.py')
        function=next(n for n in ast.parse(driver.read_text()).body if isinstance(n,ast.FunctionDef) and n.name=='browser_failure_diagnostic')
        with tempfile.TemporaryDirectory() as tmp:
            run=Path(tmp);(run/'browser.log').write_text(raw)
            if report is not None:
                (run/'playwright-result.private.json').write_text(report if type(report) is str else json.dumps(report))
            cache=run/'browser';chromium=cache/'chromium-1243/chrome-linux64/chrome'
            chromium.parent.mkdir(parents=True);chromium.write_bytes(b'NOT BROWSER fixture')
            spec=ROOT/'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
            namespace={'sha':H.CI.sha,'__file__':str(driver),'sys':sys,'json':json,'Path':Path,'W':ROOT,'SPEC':spec.name,
                       're':__import__('re'),'before':{'tracked':{str(spec.relative_to(ROOT)):H.CI.sha(spec)}}}
            exec(compile(ast.Module(body=[copy.deepcopy(function)],type_ignores=[]),str(driver),'exec'),namespace)
            inputs={'bun':{'sha256':'c'*64},'chromium':{'path':str(chromium),'sha256':H.CI.sha(chromium)}}
            if altered_inputs:inputs['bun']['sha256']='PRIVATE_CANARY_INPUT_HASH'
            return namespace['browser_failure_diagnostic'](run,7,{'PLAYWRIGHT_BROWSERS_PATH':str(cache)},inputs)

    def test_both_driver_diagnostics_have_identical_bounded_producer_ast(self):
        trees=[ast.parse((ROOT/('scripts/selected-backend-ci/current-'+lane+'-driver.py')).read_text()) for lane in ('postgres','sqlite')]
        functions=[next(n for n in tree.body if isinstance(n,ast.FunctionDef) and n.name=='browser_failure_diagnostic') for tree in trees]
        self.assertEqual(ast.dump(functions[0]),ast.dump(functions[1]))
        for tree in trees:
            calls=[n for n in ast.walk(tree) if isinstance(n,ast.Call) and isinstance(n.func,ast.Name) and n.func.id=='browser_failure_diagnostic']
            self.assertEqual(len(calls),1)
            parent=next(n for n in ast.walk(tree) if isinstance(n,ast.If) and calls[0] in list(ast.walk(n)) and ast.unparse(n.test)=='code != 0')
            self.assertEqual(ast.unparse(parent.test),'code != 0')

    def test_diagnostic_missing_malformed_partial_and_hostile_reports_stay_unknown(self):
        for lane in ('postgres','sqlite'):
            for report,state in [(None,'MISSING'),('PRIVATE_CANARY_NON_JSON','MALFORMED'),([], 'MALFORMED'),
                                 ({'secret':'PRIVATE_CANARY_PASSWORD'},'MALFORMED'),({'suites':[]},'MALFORMED')]:
                with self.subTest(lane=lane,report=report):
                    result=self.observed_browser_diagnostic(lane,report,altered_inputs=True)
                    self.assertEqual(result['error_kind'],'UNKNOWN')
                    self.assertIsNone(result['source_location'])
                    self.assertEqual(result['report_state'],state)
                    self.assertEqual(result['report_exists'],report is not None)
                    self.assertIsNone(result['input_observation']['bun_sha256'])
                    self.assertEqual(result['exit'],7)
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(result))
                    self.assertNotIn('/tmp/',json.dumps(result))

    def test_known_first_browser_errors_publish_only_static_enum_and_hashes(self):
        cases=[('Playwright must run under Bun in CI (bun --bun x playwright)','BUN_CI_CONFIG_GUARD'),
               ("browserType.launch: Executable doesn't exist at PRIVATE_CANARY_PATH?token=PRIVATE_CANARY_TOKEN",'BROWSER_EXECUTABLE_MISSING'),
               ('Playwright must run under Bun in CI (bun --bun x playwright) PRIVATE_CANARY_SUFFIX','UNKNOWN')]
        for lane in ('postgres','sqlite'):
            for message,kind in cases:
                with self.subTest(lane=lane,kind=kind):
                    report={'errors':[{'message':message,'stack':'PRIVATE_CANARY_STACK','location':{'file':'PRIVATE_CANARY_PATH','line':1}}]}
                    result=self.observed_browser_diagnostic(lane,report)
                    self.assertEqual(result['error_kind'],kind)
                    self.assertIsNone(result['source_location'])
                    self.assertEqual(result['report_state'],'AVAILABLE')
                    self.assertRegex(result['report_sha256'],'^[0-9a-f]{64}$')
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(result))
            result=self.observed_browser_diagnostic(lane,None,'Error: Playwright must run under Bun in CI (bun --bun x playwright)\n')
            self.assertEqual(result['error_kind'],'BUN_CI_CONFIG_GUARD')
            self.assertEqual(result['report_state'],'MISSING')

    def test_selected_diagnostic_location_requires_first_current_committed_error(self):
        spec=ROOT/'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
        for lane in ('postgres','sqlite'):
            for status,kind in [('failed','SELECTED_SPEC_FAILURE'),('timedOut','SELECTED_SPEC_TIMEOUT')]:
                result=self.observed_browser_diagnostic(lane,self.browser_error_report({'file':str(spec),'line':361},status))
                self.assertEqual(result['error_kind'],kind)
                self.assertEqual(result['source_location'],{'spec_sha256':H.CI.sha(spec),'line':361})
                self.assertNotIn('PRIVATE_CANARY',json.dumps(result))
            for location in [None,{'file':'PRIVATE_CANARY_PATH','line':361},{'file':str(spec),'line':True},
                             {'file':str(spec),'line':0},{'file':str(spec),'line':10**6}]:
                result=self.observed_browser_diagnostic(lane,self.browser_error_report(location))
                self.assertEqual(result['error_kind'],'UNKNOWN');self.assertIsNone(result['source_location'])
            report=self.browser_error_report(None)
            report['suites'][0]['specs'][0]['tests'][0]['results'][0]['errors'].append({'location':{'file':str(spec),'line':361}})
            self.assertEqual(self.observed_browser_diagnostic(lane,report)['error_kind'],'UNKNOWN')
            for status,retry in [('passed',0),('failed',1),('failed',True)]:
                result=self.observed_browser_diagnostic(lane,self.browser_error_report({'file':str(spec),'line':361},status,retry))
                self.assertIsNone(result['source_location']);self.assertEqual(result['error_kind'],'UNKNOWN')

    def test_source_bound_diagnostic_projects_both_lanes_without_changing_failure_or_closure(self):
        for lane in ('postgres','sqlite'):
            with patch.object(self,'driver',ROOT/('scripts/selected-backend-ci/current-'+lane+'-driver.py')):
                diagnostic=self.observed_browser_diagnostic(lane)
                for port in (True,None,False,'true',1):
                    summary,error,closed=self.project({'browser_exit':7,'browser_failure_diagnostic':diagnostic},port,target_lane=lane)
                    row=next(r for r in summary['lanes'] if r['lane']==lane)
                    self.assertEqual(row['browser_failure_diagnostic'],diagnostic)
                    self.assertEqual(row['receipt_final_exit'],7)
                    self.assertEqual(closed,port is True);self.assertEqual(error is None,port is True)
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
                summary,error,closed=self.project({},target_lane=lane)
                self.assertIsNone(next(r for r in summary['lanes'] if r['lane']==lane)['browser_failure_diagnostic'])

    def test_diagnostic_projection_refuses_unknown_fields_bindings_and_hostile_values(self):
        for lane in ('postgres','sqlite'):
            with patch.object(self,'driver',ROOT/('scripts/selected-backend-ci/current-'+lane+'-driver.py')):
                valid=self.observed_browser_diagnostic(lane)
                mutations=[('schema',True),('schema',2),('driver_sha256','f'*64),('line',True),('line',0),('line',10**6),
                           ('exit',0),('exit',True),('exit',8),('error_kind','PRIVATE_CANARY_MESSAGE'),('report_exists',1),
                           ('report_exists',True),('report_state','PRIVATE_CANARY_PATH'),('report_sha256','PRIVATE_CANARY_TOKEN'),
                           ('log_sha256','PRIVATE_CANARY_URL'),('extra','PRIVATE_CANARY_STACK')]
                for key,value in mutations:
                    with self.subTest(lane=lane,key=key,value=value):
                        summary,error,closed=self.project({'browser_exit':7,'browser_failure_diagnostic':{**valid,key:value}},target_lane=lane)
                        self.assertIsNone(next(r for r in summary['lanes'] if r['lane']==lane)['browser_failure_diagnostic'])
                        self.assertIsNone(error);self.assertTrue(closed);self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
                for key,value in [('explicit_cache_path_present',1),('cache_owned_by_runtime','PRIVATE_CANARY'),
                                  ('bun_sha256','PRIVATE_CANARY'),('extra','PRIVATE_CANARY')]:
                    invalid={**valid,'input_observation':{**valid['input_observation'],key:value}}
                    summary,error,closed=self.project({'browser_exit':7,'browser_failure_diagnostic':invalid},target_lane=lane)
                    self.assertIsNone(next(r for r in summary['lanes'] if r['lane']==lane)['browser_failure_diagnostic'])
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))
                summary,error,closed=self.project({'source':'f'*40,'browser_exit':7,'browser_failure_diagnostic':valid},target_lane=lane)
                self.assertFalse(closed);self.assertIsNotNone(error)
                self.assertIsNone(next(r for r in summary['lanes'] if r['lane']==lane)['browser_failure_diagnostic'])

    def test_diagnostic_projection_demotes_hostile_source_location_without_leaking(self):
        spec=ROOT/'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
        for lane in ('postgres','sqlite'):
            with patch.object(self,'driver',ROOT/('scripts/selected-backend-ci/current-'+lane+'-driver.py')):
                valid=self.observed_browser_diagnostic(lane,self.browser_error_report({'file':str(spec),'line':361}))
                for location in [{'spec_sha256':'PRIVATE_CANARY','line':361}, {'spec_sha256':H.CI.sha(spec),'line':True},
                                 {'spec_sha256':H.CI.sha(spec),'line':10**6}, {**valid['source_location'],'path':'PRIVATE_CANARY'}]:
                    summary,error,closed=self.project({'browser_exit':7,'browser_failure_diagnostic':{**valid,'source_location':location}},target_lane=lane)
                    observed=next(r for r in summary['lanes'] if r['lane']==lane)['browser_failure_diagnostic']
                    self.assertIsNone(observed['source_location']);self.assertEqual(observed['error_kind'],'UNKNOWN')
                    self.assertNotIn('PRIVATE_CANARY',json.dumps(summary))

    def test_collaboration_upload_adds_only_exact_scalar_ime_basename(self):
        workflow=(ROOT/'.github/workflows/web.yml').read_text()
        self.assertEqual(workflow.count('/playwright-output/**/ime-home-observations.json'),1)
        collaboration=workflow[workflow.index('name: collaboration-browser-failure-'):]
        self.assertIn('${{ steps.browser.outputs.failure-artifacts }}/playwright-output/**/ime-home-observations.json',collaboration)
        self.assertNotIn('/**/*.json',workflow)

class SelectedBrowserLauncherTest(unittest.TestCase):
    def browser_args(self, source, values):
        assignments = [node for node in ast.walk(ast.parse(source.read_text()))
                       if isinstance(node, ast.Assign) and any(isinstance(target, ast.Name) and target.id == 'args'
                                                              for target in node.targets)
                       and isinstance(node.value, ast.List) and any(isinstance(item, ast.Constant) and item.value == 'test'
                                                                  for item in node.value.elts)]
        self.assertEqual(len(assignments), 1)
        return eval(compile(ast.Expression(assignments[0].value), str(source), 'eval'), {'str': str}, values)

    def test_both_selected_commands_use_installed_cli_without_package_resolution(self):
        root = Path('/owned/locked-checkout'); bun = root / 'verified-bun'
        for lane in ('postgres', 'sqlite'):
            args = self.browser_args(ROOT / f'scripts/selected-backend-ci/current-{lane}-driver.py',
                                     {'BUN': bun, 'W': root, 'SPEC': 'workspace-wiki-selected-backend.spec.ts'})
            self.assertEqual(args, [str(bun), str(root / 'node_modules/.bin/playwright'), 'test', '--config',
                                    'e2e-pending/collab-playwright.config.ts', '--reporter=line,json',
                                    'workspace-wiki-selected-backend.spec.ts'])

    def test_restart_command_keeps_exact_readback_selection_and_reporters(self):
        root = Path('/owned/locked-checkout'); bun = root / 'verified-bun'
        args = self.browser_args(ROOT / 'scripts/selected-backend-ci/restart_checkpoint.py',
                                 {'g': {'BUN': bun, 'W': root, 'SPEC': 'workspace-wiki-selected-backend.spec.ts'},
                                  'TITLE': 'selected normal main restart:'})
        self.assertEqual(args, [str(bun), str(root / 'node_modules/.bin/playwright'), 'test', '--config',
                                'e2e-pending/collab-playwright.config.ts', '--reporter=line,json', '--grep',
                                'selected normal main restart:', 'workspace-wiki-selected-backend.spec.ts'])

class RestartBrowserDiagnosticTest(unittest.TestCase):
    driver = FailureOriginTest.driver
    project = FailureOriginTest.project

    def report(self, status='failed'):
        spec = ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
        return {'config': {'workers': 1}, 'errors': [],
                'stats': {'expected': int(status == 'passed'), 'unexpected': int(status != 'passed'), 'flaky': 0, 'skipped': 0},
                'suites': [{'specs': [{'title': 'selected normal main restart: fresh actor reads persisted native history and manual revision',
                    'file': spec.name, 'line': 690, 'tests': [{'results': [{'status': status, 'retry': 0,
                    'errors': [{'message': 'PRIVATE_CANARY_ASSERTION', 'location': {'file': str(spec), 'line': 715}}]}]}]}]}]}

    def observed(self, lane, report, log='PRIVATE_CANARY_LOG?token=PRIVATE_CANARY_TOKEN'):
        driver = ROOT / f'scripts/selected-backend-ci/current-{lane}-driver.py'
        function = next(n for n in ast.parse(driver.read_text()).body
                        if isinstance(n, ast.FunctionDef) and n.name == 'browser_failure_diagnostic')
        with tempfile.TemporaryDirectory() as tmp:
            run = Path(tmp); cache = run / 'cache'; chromium = cache / 'chromium-fixture/chrome-linux/chrome'
            chromium.parent.mkdir(parents=True); chromium.write_bytes(b'NOT BROWSER fixture')
            (run / 'restart-browser.log').write_text(log)
            if report is not None:
                (run / 'restart-playwright-result.private.json').write_text(report if type(report) is str else json.dumps(report))
            spec = ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'
            env = {'sha': H.CI.sha, '__file__': str(driver), 'sys': sys, 'json': json, 'Path': Path, 'W': ROOT,
                   'SPEC': spec.name, 're': __import__('re'), 'before': {'tracked': {str(spec.relative_to(ROOT)): H.CI.sha(spec)}}}
            exec(compile(ast.Module(body=[copy.deepcopy(function)], type_ignores=[]), str(driver), 'exec'), env)
            return env['browser_failure_diagnostic'](run, 7, {}, {'bun': {'sha256': 'c' * 64},
                'chromium': {'path': str(chromium), 'sha256': H.CI.sha(chromium)}}, restart=True)

    def envelope(self, observation):
        helper = ROOT / 'scripts/selected-backend-ci/restart_checkpoint.py'
        return {'helper_sha256': H.CI.sha(helper), 'helper_line': 73, 'observation': observation}

    def projected(self, lane, record, *, port=True, exit=7, helper_binding=True):
        original_read = H.CI.read
        helper = ROOT / 'scripts/selected-backend-ci/restart_checkpoint.py'
        def read(path):
            value = original_read(path)
            if Path(path).name == 'before.json' and helper_binding:
                value['tracked'][str(helper.relative_to(ROOT))] = H.CI.sha(helper)
            return value
        with patch.object(self, 'driver', ROOT / f'scripts/selected-backend-ci/current-{lane}-driver.py'), \
                patch.object(H.CI, 'read', side_effect=read):
            return self.project({'browser_exit': 0, 'restart_browser_exit': exit,
                                 'restart_browser_diagnostic': record}, port=port, target_lane=lane)

    def test_both_restart_verdicts_project_without_replacing_first_failure_or_close_states(self):
        for lane in ('postgres', 'sqlite'):
            observed = self.observed(lane, self.report())
            self.assertEqual(observed['error_kind'], 'SELECTED_SPEC_FAILURE')
            self.assertEqual(observed['restart_verdict'], {
                'registered_test': 'SELECTED_NORMAL_RESTART',
                'spec_sha256': H.CI.sha(ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'),
                'registration_line': 690, 'status': 'failed',
                'counts': {'expected': 0, 'unexpected': 1, 'flaky': 0, 'skipped': 0}})
            record = self.envelope(observed)
            for port in (True, False, None, 'true', 1):
                summary, error, closed = self.projected(lane, record, port=port)
                row = next(r for r in summary['lanes'] if r['lane'] == lane)
                self.assertEqual(row['restart_browser_diagnostic'], record)
                self.assertEqual(row['receipt_final_exit'], 7)
                self.assertEqual(closed, port is True); self.assertEqual(error is None, port is True)
                self.assertNotIn('PRIVATE_CANARY', json.dumps(summary))

    def test_missing_partial_restart_reports_preserve_known_guard_and_unknown_without_disclosure(self):
        for lane in ('postgres', 'sqlite'):
            for report in (None, 'PRIVATE_CANARY_BAD_JSON', [], {'secret': 'PRIVATE_CANARY_PASSWORD'}):
                observed = self.observed(lane, report)
                self.assertIsNone(observed['restart_verdict']); self.assertEqual(observed['error_kind'], 'UNKNOWN')
                self.assertNotIn('PRIVATE_CANARY', json.dumps(observed)); self.assertEqual(observed['exit'], 7)
            observed = self.observed(lane, {'errors': [{'message': 'Playwright must run under Bun in CI (bun --bun x playwright)'}]})
            self.assertEqual(observed['error_kind'], 'BUN_CI_CONFIG_GUARD'); self.assertIsNone(observed['restart_verdict'])
            observed = self.observed(lane, None, 'Error: Playwright must run under Bun in CI (bun --bun x playwright)\n')
            self.assertEqual(observed['error_kind'], 'BUN_CI_CONFIG_GUARD'); self.assertIsNone(observed['restart_verdict'])

    def test_restart_registration_status_count_and_first_error_mutations_are_not_invented_verdicts(self):
        for lane in ('postgres', 'sqlite'):
            changes = [lambda d: d['suites'][0]['specs'][0].update(title='PRIVATE_CANARY_TITLE'),
                       lambda d: d['suites'][0]['specs'][0].update(file='PRIVATE_CANARY_PATH'),
                       lambda d: d['suites'][0]['specs'][0].update(line=True),
                       lambda d: d['suites'][0]['specs'][0].update(line=691),
                       lambda d: d['stats'].update(unexpected=True),
                       lambda d: d['stats'].update(unexpected=-1),
                       lambda d: d['stats'].update(unexpected=2),
                       lambda d: d['stats'].update(expected=1),
                       lambda d: d['suites'][0]['specs'][0]['tests'][0]['results'][0].update(status='PRIVATE_CANARY_STATUS'),
                       lambda d: d['suites'][0]['specs'][0]['tests'][0]['results'][0].update(retry=1)]
            for change in changes:
                report = self.report(); change(report); observed = self.observed(lane, report)
                self.assertIsNone(observed['restart_verdict']); self.assertNotIn('PRIVATE_CANARY', json.dumps(observed))
            report = self.report(); report['suites'][0]['specs'][0]['tests'][0]['results'][0]['errors'] = [
                {'message': 'PRIVATE_CANARY_FIRST', 'location': {'file': 'PRIVATE_CANARY_FOREIGN', 'line': 715}},
                {'location': {'file': str(ROOT / 'apps/web/e2e-pending/workspace-wiki-selected-backend.spec.ts'), 'line': 715}}]
            observed = self.observed(lane, report)
            self.assertIsNone(observed['source_location']); self.assertEqual(observed['error_kind'], 'UNKNOWN')

    def test_restart_projector_refuses_foreign_helper_parent_exit_and_unknown_fields(self):
        for lane in ('postgres', 'sqlite'):
            valid = self.envelope(self.observed(lane, self.report()))
            mutations = [lambda d: d.update(helper_sha256='f' * 64), lambda d: d.update(helper_line=True),
                         lambda d: d.update(helper_line=10**6), lambda d: d.update(helper_line=1),
                         lambda d: d.update(extra='PRIVATE_CANARY_TOKEN'),
                         lambda d: d['observation'].update(driver_sha256='f' * 64),
                         lambda d: d['observation'].update(exit=True), lambda d: d['observation'].update(exit=8),
                         lambda d: d['observation'].update(line=1),
                         lambda d: d['observation'].update(schema=True),
                         lambda d: d['observation'].update(error_kind='PRIVATE_CANARY_MESSAGE'),
                         lambda d: d['observation'].update(extra='PRIVATE_CANARY_URL')]
            for mutation in mutations:
                record = copy.deepcopy(valid); mutation(record)
                summary, error, closed = self.projected(lane, record)
                row = next(r for r in summary['lanes'] if r['lane'] == lane)
                self.assertIsNone(row['restart_browser_diagnostic']); self.assertEqual(row['receipt_final_exit'], 7)
                self.assertTrue(closed); self.assertIsNone(error); self.assertNotIn('PRIVATE_CANARY', json.dumps(summary))
            for options in ({'exit': 8}, {'exit': True}, {'helper_binding': False}):
                summary, error, closed = self.projected(lane, valid, **options)
                self.assertIsNone(next(r for r in summary['lanes'] if r['lane'] == lane)['restart_browser_diagnostic'])
            record = copy.deepcopy(valid); record['observation']['restart_verdict']['registered_test'] = 'PRIVATE_CANARY_TITLE'
            summary, error, closed = self.projected(lane, record)
            self.assertIsNone(next(r for r in summary['lanes'] if r['lane'] == lane)['restart_browser_diagnostic']['observation']['restart_verdict'])
            self.assertNotIn('PRIVATE_CANARY', json.dumps(summary))

    def test_restart_optional_capture_runs_before_original_assert_and_cannot_mask_it(self):
        helper = ROOT / 'scripts/selected-backend-ci/restart_checkpoint.py'
        tree = ast.parse(helper.read_text())
        record = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == 'record_restart_browser_diagnostic')
        restart = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == 'restart_same_app')
        body = next(n for n in restart.body if isinstance(n, ast.Try)).body
        index = next(i for i, n in enumerate(body) if isinstance(n, ast.Expr) and isinstance(n.value, ast.Call)
                     and isinstance(n.value.func, ast.Name) and n.value.func.id == 'record_restart_browser_diagnostic')
        self.assertIsInstance(body[index + 1], ast.Assert)
        self.assertEqual(ast.unparse(body[index + 1].test), 'result.returncode == 0')
        for broken in (False, True):
            receipt = {}
            def diagnostic(*args, **kwargs):
                self.assertIs(kwargs['restart'], True)
                if broken: raise PermissionError('PRIVATE_CANARY_DIAGNOSTIC_ERROR')
                return {'bounded': True}
            g = {'receipt': receipt, 'browser_failure_diagnostic': diagnostic, 'browser_inputs': {}, 'sys': sys}
            env = {'g': g, 'run': Path('/not-executed'), 'env': {}, '__file__': str(helper),
                   'digest': H.CI.sha, 'result': type('ObservedOriginalExit', (), {'returncode': 7})()}
            module = ast.Module(body=[copy.deepcopy(record), *copy.deepcopy(body[index:index + 2])], type_ignores=[])
            with self.assertRaisesRegex(AssertionError, '^preserve original restart browser failure$'):
                exec(compile(module, str(helper), 'exec'), env)
            self.assertEqual(receipt['restart_browser_exit'], 7)
            self.assertEqual('restart_browser_diagnostic' in receipt, not broken)
            self.assertNotIn('PRIVATE_CANARY', json.dumps(receipt))

    def test_passed_restart_metadata_cannot_convert_later_driver_failure_to_pass(self):
        for lane in ('postgres', 'sqlite'):
            observed = self.observed(lane, self.report('passed')); observed['exit'] = 0
            record = self.envelope(observed)
            summary, error, closed = self.projected(lane, record, exit=0)
            row = next(r for r in summary['lanes'] if r['lane'] == lane)
            self.assertEqual(row['restart_browser_diagnostic']['observation']['restart_verdict']['status'], 'passed')
            self.assertEqual(row['receipt_final_exit'], 7); self.assertTrue(closed); self.assertIsNone(error)

if __name__=='__main__':unittest.main()
