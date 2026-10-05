#!/usr/bin/env python3
"""Fixed same-job CI companion for the three maintained selected drivers.
Record actual current build inputs/artifacts; then execute install, PG and SQLite
serially. No product fixture, SQL writer, protocol parser or build fallback here.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import subprocess
import sys
import time

ROOT=Path(__file__).resolve().parents[1]
TEMPLATES=ROOT/'scripts/selected-backend-ci'


def sha(path):
    h=hashlib.sha256()
    with Path(path).open('rb') as f:
        for chunk in iter(lambda:f.read(1048576),b''):h.update(chunk)
    return h.hexdigest()


def write(path,data):
    path=Path(path)
    with path.open('x') as f:
        os.fchmod(f.fileno(),0o600)
        f.write(json.dumps(data,indent=2)+'\n')


def read(path):return json.loads(Path(path).read_text())


def call(args):
    if args[0]=='git':args=['git','-c','safe.directory='+str(ROOT),*args[1:]]
    return subprocess.check_output(args,cwd=ROOT,text=True).strip()


def identity():
    assert os.environ['CI']=='true' and os.environ['GITHUB_ACTIONS']=='true', 'allocated GitHub CI job only'
    assert call(['git','rev-parse','HEAD'])==os.environ['GITHUB_SHA']
    assert subprocess.run(['git','-c','safe.directory='+str(ROOT),'diff','--quiet','HEAD'],cwd=ROOT).returncode==0, 'current tracked source must equal the tested SHA'
    assert re.fullmatch('[0-9]+',os.environ['GITHUB_RUN_ID'])
    assert re.fullmatch('[0-9]+',os.environ['GITHUB_RUN_ATTEMPT'])
    assert ((os.environ['GITHUB_JOB']=='collaboration-flow' and os.environ.get('FVOCI_WEB_BUILD_PHASE') in (None,'consume')) or
            (os.environ['GITHUB_JOB']=='collaboration-build' and os.environ.get('FVOCI_WEB_BUILD_PHASE')=='prepare')), 'wrong current build/runtime phase'
    return 'github:'+':'.join(os.environ[k] for k in ('GITHUB_REPOSITORY','GITHUB_RUN_ID','GITHUB_RUN_ATTEMPT','GITHUB_JOB'))


def inputs():
    # One fresh-host closure before/after compilation; no historical/local receipt reuse.
    tracked={p:sha(ROOT/p) for p in call(['git','ls-files','-z']).split('\0') if p}
    untracked={p:sha(ROOT/p) for p in call(['git','ls-files','--others','--exclude-standard','-z']).split('\0') if p and (ROOT/p).is_file()}
    external={}
    def add(path):
        path=Path(path)
        if path.is_dir():
            for f in path.rglob('*'):
                if f.is_file():external[str(f)]=sha(f)
        elif path.is_file():external[str(path)]=sha(path)
        else:raise RuntimeError('missing qualified build input: '+str(path))
    cargo_home=Path(os.environ.get('CARGO_HOME',str(Path.home()/'.cargo'))).resolve()
    for subdirectory in ('registry','git'):
        if (cargo_home/subdirectory).exists():add(cargo_home/subdirectory)
    for config in (cargo_home/'config',cargo_home/'config.toml',Path(os.environ.get('FVOCI_SELECTED_CI_OUTPUT','/nonexistent'))/'build-env-inputs.json'):
        if config.exists():add(config)
    assert not (cargo_home/'credentials').exists() and not (cargo_home/'credentials.toml').exists(), 'public offline CI cannot borrow account credentials'
    for directory in [ROOT/'node_modules',Path(call(['rustc','--print','sysroot'])),Path(os.environ['LIBCLANG_PATH']),Path('/usr/include'),Path(os.environ['SQLITE3_LIB_DIR']).parent]:add(directory)
    add(Path(call(['cc','-print-file-name=include'])).parent)
    for name in ('cargo','rustc','cc','ar','ld','bun'):
        tool=shutil.which(name);assert tool, name;add(Path(tool).resolve())
        result=subprocess.run(['ldd',str(Path(tool).resolve())],capture_output=True,text=True)
        for library in elf_dependencies(result.returncode,result.stdout,result.stderr):add(Path(library).resolve())
    add(Path('/etc/os-release'))
    for path in abi_files():add(path)
    return {'head':call(['git','rev-parse','HEAD']),'tree':call(['git','rev-parse','HEAD^{tree}']),
            'status':subprocess.check_output(['git','-c','safe.directory='+str(ROOT),'status','--short'],cwd=ROOT,text=True),
            'tracked':tracked,'external':external,'untracked':untracked}


def elf_dependencies(code,stdout,stderr):
    text=stdout+'\n'+stderr
    if code==0:
        return [str(Path(x).resolve()) for x in re.findall(r'(?:=>\s*)?(/[^\s]+)\s+\(',text)]
    if code==1 and ('not a dynamic executable' in text or 'statically linked' in text):return []
    raise RuntimeError('ldd prerequisite failed; exit='+str(code))


def build_env():
    names=[k for k in os.environ if re.match(r'^(CARGO_|RUST|CC(?:_|$)|CXX(?:_|$)|HOST_CC$|TARGET_CC$|CFLAGS|CPPFLAGS|CXXFLAGS|LDFLAGS|LIBCLANG|BINDGEN_|SQLITE3_|PKG_CONFIG|CPATH$|C_INCLUDE_PATH$|CPLUS_INCLUDE_PATH$|LIBRARY_PATH$|PATH$)',k)]
    assert not any(re.search(r'TOKEN|PASSWORD|SECRET|CREDENTIAL',k) for k in names), 'no compiler account credentials'
    assert not os.environ.get('RUSTC_WRAPPER') and not os.environ.get('RUSTC_WORKSPACE_WRAPPER'), 'unqualified compiler wrapper'
    return {k:hashlib.sha256(os.environ[k].encode()).hexdigest() for k in sorted(names)}


def runtime_access(files,browser):
    # Real mode/ownership/access checks occur as the actual1000 runtime process.
    assert os.getuid()==os.getgid()==1000
    for path in files:
        assert Path(path).is_file() and os.access(path,os.R_OK), ('input is inaccessible to runtime1000',path)
    for path in (browser['bun']['path'],browser['chromium']['path']):
        assert os.access(path,os.R_OK|os.X_OK), ('browser executable is inaccessible to runtime1000',path)


def abi_files():
    return [str(Path(p).resolve()) for p in ('/lib64/ld-linux-x86-64.so.2','/lib/x86_64-linux-gnu/libc.so.6','/lib/x86_64-linux-gnu/libm.so.6','/lib/x86_64-linux-gnu/libgcc_s.so.1')]


def reference(path):return {'path':str(Path(path).resolve()),'sha256':sha(path)}


def record_before(output):
    identity();assert not (output/'before.json').exists()
    os_release=dict(line.split('=',1) for line in Path('/etc/os-release').read_text().splitlines() if '=' in line)
    assert os_release['ID'].strip(chr(34))=='ubuntu' and os_release['VERSION_ID'].strip(chr(34))=='26.04'
    assert 'release: 1.98.1' in call(['rustc','-Vv']) and 'host: x86_64-unknown-linux-gnu' in call(['rustc','-Vv'])
    assert os.environ.get('CARGO_BUILD_TARGET','x86_64-unknown-linux-gnu')=='x86_64-unknown-linux-gnu'
    assert call(['bun','-v'])=='1.4.2'
    assert os.environ['SQLITE3_STATIC']==os.environ['SQLITE3_NO_PKG_CONFIG']=='1'
    assert shutil.disk_usage(output).free>=20_000_000_000
    write(output/'build-env-inputs.json',build_env())
    write(output/'before.json',inputs())
    write(output/'build-environment.json',{'sqlite':{k:os.environ[k] for k in ('SQLITE3_LIB_DIR','SQLITE3_INCLUDE_DIR','SQLITE3_STATIC','SQLITE3_NO_PKG_CONFIG')},
          'rustc':call(['rustc','-Vv']),'cargo':call(['cargo','-V']),'bun':call(['bun','-v']),
          'os_release':Path('/etc/os-release').read_text(),
          'target':str(Path(os.environ['CARGO_TARGET_DIR']).resolve()),'features':['api-schema','db-tests'],
          'nativeFeatures':['worker'],'profile':'debug','devDebug':os.environ.get('CARGO_PROFILE_DEV_DEBUG'),
          'testDebug':os.environ.get('CARGO_PROFILE_TEST_DEBUG'),'compilerBeforeRecorded':True})


def record_after(output):
    identity();before=read(output/'before.json');assert read(output/'build-env-inputs.json')==build_env(), 'compiler environment changed'
    after=inputs();write(output/'after.json',after)
    assert before==after, 'source/native/dependency/toolchain changed during current compile'
    stages=[];artifacts=[]
    for name in ('main','lib','install','engine'):
        log=output/(name+'-compiler.jsonl');stage=read(output/(name+'-stage.json'))
        assert stage['exit_code']==0 and stage['source']==before['head'] and stage['tree']==before['tree']
        stage['compilerMessages']=reference(log);stages.append(stage)
        for line in log.read_text().splitlines():
            r=json.loads(line)
            if r.get('reason')=='compiler-artifact':artifacts.append(r)
    bins={}
    for name in ('fvoci-server','fvoci-migrate','fvoci-e2e-fixture','fvoci_server','selected_install_lifetime','collab-engine'):
        matches=[a for a in artifacts if a['target']['name']==name and a['executable'] and
                 a['profile']['test']==(name in ('fvoci_server','selected_install_lifetime'))]
        assert matches,(name,'missing current emitted artifact')
        unique={a['executable'] for a in matches};assert len(unique)==1,(name,unique)
        a=matches[-1]
        assert all(m['target']==a['target'] and m['features']==a['features'] and m['profile']==a['profile'] for m in matches);path=a['executable'];metadata=Path(path).stat()
        # Cargo lists every activated feature, including a declared `default` (collab-engine declares `default = []`).
        expected=['default','worker'] if name=='collab-engine' else ['api-schema','db-tests']
        assert sorted(a['features'])==expected,(name,'actual emitted features',sorted(a['features']),'expected',expected)
        bins[path]={'sha256':sha(path),'bytes':metadata.st_size,'mode':oct(metadata.st_mode),'inode':metadata.st_ino,
                    'compiledSource':before['head'],'targetTriple':'x86_64-unknown-linux-gnu',
                    'target':a['target'],'features':a['features'],'profile':a['profile'],'cargo_fresh':a['fresh']}
    compile_receipt={'source':before['head'],'tree':before['tree'],'exit_code':0,'full_inputs_unchanged':True,'stages':stages}
    write(output/'compile-receipt.json',compile_receipt)
    write(output/'bundle.json',{'source':before['head'],'tree':before['tree'],'full_inputs_unchanged':True,'binaries':bins,'compiler_artifacts':artifacts})
    dist=ROOT/'apps/web/dist';assets={str(p.relative_to(dist)):sha(p) for p in dist.rglob('*') if p.is_file()};assert assets
    write(output/'web-receipt.json',{'source':before['head'],'tree':before['tree'],'exit_code':0,'full_inputs_unchanged':True,'dist_files':assets,'servedDist':str(dist),'scope':'maintained current build before current source/native snapshot; actual emitted assets'})
    libs={p:sha(p) for p in abi_files()};ldd={}
    for p in bins:
        text=call(['ldd',p]);ldd[p]=text
        actual=[str(Path(x).resolve()) for x in re.findall(r'(?:=>\s*)?(/[^\s]+)\s+\(',text)]
        assert actual and set(actual)<=set(libs), ('unqualified current ELF dependencies',p,text)
    server=next(p for p in bins if p.endswith('/fvoci-server'))
    write(output/'abi-receipt.json',{'currentSource':before['head'],'currentServerSha256':bins[server]['sha256'],
          'currentELFDependenciesVerified':True,'actualCurrentELFldd':ldd,'host_runtime_files':libs,
          'os_release':Path('/etc/os-release').read_text(),
          'scope':'current Ubuntu 26.04 build-host ABI evidence only; runtime uses its own image libraries'})


def stage(output,name,command):
    identity();assert name in ('main','lib','install','engine') and command
    before=read(output/'before.json');assert call(['git','rev-parse','HEAD'])==before['head']
    start=time.monotonic()
    with (output/(name+'-compiler.jsonl')).open('x') as out,(output/(name+'-stderr.log')).open('x') as err:
        r=subprocess.run(command,cwd=ROOT,stdout=out,stderr=err)
    write(output/(name+'-stage.json'),{'source':before['head'],'tree':before['tree'],'command':command,'exit_code':r.returncode,'seconds':time.monotonic()-start})
    return r.returncode


def run(output):
    assert os.environ['GITHUB_JOB']=='collaboration-flow', 'build producer cannot start runtime'
    assert os.getuid()==os.getgid()==1000, 'normal SQLite browser/fixture/app file ownership must be1000:1000'
    owner=identity();before=read(output/'before.json');assert read(output/'after.json')==before
    runtime=output/'runtime';runtime.mkdir(mode=0o700)
    bun=str(Path(shutil.which('bun')).resolve());chromium=call([bun,'--eval',"console.log(require('@playwright/test').chromium.executablePath())"])
    browser={'bun':{'path':bun,'sha256':sha(bun)},'chromium':{'path':chromium,'sha256':sha(chromium)},
             'chromium_directory_files':{str(f.relative_to(Path(chromium).parent)):sha(f) for f in Path(chromium).parent.rglob('*') if f.is_file()}}
    runtime_access(before['external'],browser)
    bundle=read(output/'bundle.json');web=read(output/'web-receipt.json');abi=read(output/'abi-receipt.json')
    for executable in bundle['binaries']:
        assert os.access(executable,os.R_OK|os.X_OK), ('current cohort executable inaccessible to1000',executable)
    source_written={k:before[k] for k in ('head','tree','status','tracked','external','untracked')}
    env=dict(os.environ);env.update(BUN_RUNTIME_TRANSPILER_CACHE_PATH=str(output/'bun-transpiler-cache'),FVOCI_CI_OWNER=owner,FVOCI_CI_BUN=bun,FVOCI_CI_SELECTED_RUNS=str(runtime),FVOCI_ROOT_RUN_OWNER=owner,PYTHONDONTWRITEBYTECODE='1')
    closed=None;results=[];code=0
    for lane in ('install','postgres','sqlite'):
        runroot=runtime/('root-current-'+lane+'-'+secrets.token_hex(6));driver=TEMPLATES/('current-'+lane+'-driver.py')
        m={'schema':1,'ready':True,'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],
           'sourceInputsBefore':reference(output/'before.json'),'sourceInputsAfter':reference(output/'after.json'),
           'bundle':reference(output/'bundle.json'),'compileReceipt':reference(output/'compile-receipt.json'),
           'webReceipt':reference(output/'web-receipt.json'),'abiReceipt':reference(output/'abi-receipt.json'),
           'nativeQualification':None,'browserInputs':browser,'closedInstallReceipt':closed}
        if lane!='install':
            assert closed, 'mandatory actual current installation4 failed; cannot qualify normal main'
            binding={'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'backend':lane,'runRoot':str(runroot),
                     'parentDriverSha256':sha(driver),'restartHelperSha256':sha(TEMPLATES/'restart_checkpoint.py'),
                     'sourceInputsSha256':hashlib.sha256((json.dumps(source_written,indent=2)+'\n').encode()).hexdigest(),
                     'artifactHashes':{p:r['sha256'] for p,r in bundle['binaries'].items()},'assetHashes':web['dist_files'],'browserInputs':browser,'abiHashes':abi['host_runtime_files']}
            restart=output/(lane+'-restart-allocation.json');write(restart,{'schema':1,'status':'GRANTED','owner':owner,'exclusiveCIJob':True,'currentCIJobConfirmed':True,
                       'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'backend':lane,'binding':binding})
            m['restartAllocation']=reference(restart);env['FVOCI_ROOT_RESTART_GRANT']=str(restart)
        manifest=output/(lane+'-binding.json');write(manifest,m)
        allocation=output/(lane+'-allocation.json');write(allocation,{'schema':1,'status':'GRANTED','owner':owner,'exclusiveCIJob':True,'currentCIJobConfirmed':True,
                   'runId':env['GITHUB_RUN_ID'],'runAttempt':env['GITHUB_RUN_ATTEMPT'],'source':before['head'],'tree':before['tree'],'compiledSource':before['head'],'lane':lane,'backend':None if lane=='install' else lane,
                   'runRoot':str(runroot),'driverSha256':sha(driver),'bindingSha256':sha(manifest),'bindingModuleSha256':sha(TEMPLATES/'current_binding.py')})
        env.update(FVOCI_ROOT_CURRENT_BINDING=str(manifest),FVOCI_ROOT_CURRENT_ALLOCATION=str(allocation))
        if lane=='postgres':env['FVOCI_E2E_SELECTED_AUXILIARY']='normal-api'
        else:env.pop('FVOCI_E2E_SELECTED_AUXILIARY',None)
        with (output/(lane+'-driver.log')).open('x') as log:r=subprocess.run([sys.executable,str(driver)],env=env,cwd=ROOT,stdout=log,stderr=subprocess.STDOUT)
        results.append({'lane':lane,'exit':r.returncode,'actualSource':before['head'],'runRoot':str(runroot)})
        code=code or r.returncode
        if lane=='postgres':
            parent=read(runroot/'parent-receipt.json')
            assert parent['all_owned_fixtures_closed'], 'PG/Meili closure failed: preserve failure, no overlapping SQLite start'
            app=read(runroot/'receipt.json')
            assert app['owned_container_absent'] and app['owned_loopback_port_closed'] and app['recorded_process_identities_retired']
        if lane=='install':
            assert r.returncode==0,'current installation4 failed; preserve original log/15process receipts'
            closed=reference(runroot/'receipt.json')
        elif r.returncode==0:
            record=read(runroot/'receipt.json');assert record['current_schema_server_restart']['restartBrowserExit']==0
            assert record['actual_browser_tests']==1 and record['retries']==0
    write(output/'selected-ci-receipt.json',{'source':before['head'],'tree':before['tree'],'owner':owner,'runs':results,'exit':code,'normalBothAndRestartRequired':True,'sqliteAuxiliary':'BLOCKED: normal writers unported','whole060Complete':False})
    return code


def main():
    parser=argparse.ArgumentParser();parser.add_argument('mode',choices=['record-before','stage','record-after','run']);parser.add_argument('--output',required=True);parser.add_argument('--stage-name');args,command=parser.parse_known_args()
    assert args.mode=='stage' or not command, 'unexpected arguments outside compiler stage'
    output=Path(args.output).resolve();assert output.is_dir() and output.stat().st_uid==os.getuid() and output.stat().st_mode&0o777==0o700
    if args.mode=='record-before':record_before(output)
    elif args.mode=='record-after':record_after(output)
    elif args.mode=='stage':return stage(output,args.stage_name,command[1:] if command[:1]==['--'] else command)
    else:return run(output)
    return 0


if __name__=='__main__':sys.exit(main())
