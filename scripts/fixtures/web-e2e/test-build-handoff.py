#!/usr/bin/env python3
"""Pure packet/physical-byte controls; fake ELF/tool metadata, no native execution."""
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


if __name__=='__main__':unittest.main()
