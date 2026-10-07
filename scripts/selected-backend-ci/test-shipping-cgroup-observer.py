#!/usr/bin/env python3
"""Literal pure observer controls; no real cgroup/Docker/build observation."""
import ast
import copy
import importlib.util
import io
import json
import os
import stat
import subprocess
from contextlib import contextmanager
import tempfile
import types
import unittest
from contextlib import redirect_stdout
from pathlib import Path
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("observer", HERE / "shipping-cgroup-observer.py")
O = importlib.util.module_from_spec(spec); spec.loader.exec_module(O)


class ObserverControls(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name); self.proc = self.root / "proc"; self.cg = self.root / "cg"
        with patch.object(O, "READER_PATH", HERE / "shipping-image-producer.py"):
            self.reader, self.filename = O.load_reader()
        number = str(self.reader.os.getpid()); (self.proc / number).mkdir(parents=True)
        (self.proc / "self").symlink_to(number)
        (self.cg / "parent").mkdir(parents=True)
        dev = self.cg.stat().st_dev; device = f"{self.reader.os.major(dev)}:{self.reader.os.minor(dev)}"
        (self.proc / "self/mountinfo").write_text(f"1 0 {device} / {self.cg} rw - cgroup2 cgroup rw\n2 0 {device} / {self.proc} rw - proc proc rw\n")
        (self.proc / "self/stat").write_text(number + " (observer fixture) S " + "0 "*18 + "100 0\n")
        (self.proc / "self/cgroup").write_text("0::/parent\n")
        (self.cg / "cgroup.controllers").write_text("cpu memory\n")
        for name, text in {"cgroup.type": "domain", "cgroup.procs": number + "\n", "memory.max": "max", "memory.current": "100", "memory.peak": "200",
                           "memory.events": "oom 0\noom_kill 0\noom_group_kill 0\n", "cpu.max": "max 100000"}.items():
            (self.cg / "parent" / name).write_text(text)
        # Explicit synthetic kernel-filesystem seam; no installed stat executes.
        self.filesystem = patch.object(self.reader, "cgroup_filesystem", return_value="synthetic-tool-hash")
        self.filesystem.start(); self.addCleanup(self.filesystem.stop)
        self.build = patch.object(self.reader.Producer, "build", side_effect=AssertionError("BUILD_MUST_NEVER_RUN"))
        self.build_mock = self.build.start(); self.addCleanup(self.build.stop)
        self.command = patch.object(self.reader.Producer, "command", side_effect=AssertionError("COMMAND_MUST_NEVER_RUN"))
        self.command_mock = self.command.start(); self.addCleanup(self.command.stop)
        self.no_process = patch.object(self.reader.subprocess, "Popen", side_effect=AssertionError("NO_NATIVE_PROCESS"))
        self.no_process_mock = self.no_process.start(); self.addCleanup(self.no_process.stop)

    def run_reader(self):
        call = Mock(side_effect=lambda: self.reader.cgroup_chain(root=self.cg, proc=self.proc))
        target = types.SimpleNamespace(cgroup_chain=call, Refusal=self.reader.Refusal)
        result = O.observe(target, self.filename)
        call.assert_called_once_with(); self.build_mock.assert_not_called(); self.command_mock.assert_not_called(); self.no_process_mock.assert_not_called()
        return result

    def assert_diagnostic(self, result, code, primitive, kind, errno, line):
        exit_code, receipt = result
        self.assertEqual(exit_code, 1)
        self.assertEqual(receipt, {"scope": "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED", "reader_sha": "b949b21cba8cf562487ec7cc80a9d2cf40f9975f",
            "reader_source_sha256": "448a87a119d9035c75225679127097a48611bd14e423f49b0eb098b39d7d860e", "result": code,
            "diagnostic": {"primitive": primitive, "exception_class": kind, "errno": errno, "source_line": line}})

    def test_healthy_once_never_build_or_process(self):
        code, receipt = self.run_reader()
        self.assertEqual(code, 0); self.assertEqual(receipt["result"], "CGROUP_READER_OBSERVED_OK_NOT_IMAGE_QUALIFIED")
        self.assertEqual(receipt["scope"], "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED"); self.assertIsNone(receipt["diagnostic"])

    def test_enoent_eacces_keep_closed_primitive_not_hostile_text_filename(self):
        original = self.reader.os.open
        for number in [2, 13]:
            def failing(name, *a, **kw):
                if name == "memory.peak": raise OSError(number, "::error::PRIVATE_TOKEN https://private.invalid", "/private/SECRET\n::error::")
                return original(name, *a, **kw)
            with patch.object(self.reader.os, "open", side_effect=failing):
                result = self.run_reader()
            self.assert_diagnostic(result, "CGROUP_METADATA_UNKNOWN", "MEMORY_PEAK_READ_PARSE", "OSError", number, 184)
            text = json.dumps(result)
            for raw in ["PRIVATE_TOKEN", "https://", "/private/", "SECRET", "::error::"]: self.assertNotIn(raw, text)

    def test_parse_error_never_emit_rejected_bytes(self):
        (self.cg / "parent/memory.max").write_text("PRIVATE_TOKEN https://private.invalid")
        result = self.run_reader()
        self.assert_diagnostic(result, "CGROUP_METADATA_UNKNOWN", "MEMORY_MAX_PARSE", "ValueError", None, 319)
        self.assertNotIn("PRIVATE_TOKEN", json.dumps(result)); self.assertNotIn("https://", json.dumps(result))

    def test_decode_error_class_closed(self):
        (self.cg / "parent/memory.peak").write_bytes(b"\xffPRIVATE_TOKEN")
        self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", "MEMORY_PEAK_READ_PARSE", "ValueError", None, 192)

    def test_explicit_shape_and_counter_refusals(self):
        for text, primitive, line in [("oom 0\noom 0\n", "MEMORY_EVENTS_SHAPE", 323),
                                      ("oom 0\n", "MEMORY_COUNTERS_PREDICATE", 325)]:
            (self.cg / "parent/memory.events").write_text(text)
            self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", primitive, "Refusal", None, line)

    def test_hidden_subtree_refusal_keeps_closed_code(self):
        # ROOT-approved replacement of the old literal namespace mismatch.
        (self.cg / "cgroup.type").write_text("domain")
        self.assert_diagnostic(self.run_reader(), "CGROUP_HOST_ROOT_INVALID", "ROOT_MARKER_PREDICATE", "Refusal", None, 293)

    def test_root_marker_errno_disambiguated_without_path(self):
        original = self.reader.os.open
        for target, primitive in [("cgroup.type", "CGROUP_TYPE_LOOKUP"), ("memory.max", "ROOT_MEMORY_MAX_LOOKUP")]:
            def failing(name, *a, **kw):
                if name == target and self.reader.os.fstat(kw["dir_fd"]).st_ino == self.cg.stat().st_ino:
                    raise OSError(13, "PRIVATE_TOKEN", name)
                return original(name, *a, **kw)
            with patch.object(self.reader.os, "open", side_effect=failing):
                self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", primitive, "OSError", 13, 184)

    def test_unknown_trace_and_hostile_refusal_or_error_never_stringified(self):
        class Hostile:
            def __str__(self): raise AssertionError("RAW_STRINGIFICATION")
        for error in [RuntimeError(Hostile()), self.reader.Refusal(Hostile()), self.reader.Refusal("PRIVATE_TOKEN https://private.invalid")]:
            target = types.SimpleNamespace(cgroup_chain=Mock(side_effect=error), Refusal=self.reader.Refusal)
            code, receipt = O.observe(target, self.filename)
            self.assertEqual(code, 1); self.assertEqual(receipt["result"], "CGROUP_OBSERVER_UNEXPECTED_FAILURE")
            self.assertEqual(receipt["diagnostic"]["primitive"], "UNKNOWN"); self.assertIsNone(receipt["diagnostic"]["source_line"])
            self.assertNotIn("PRIVATE_TOKEN", json.dumps(receipt))

    def test_errno_invalid_or_unattributed_filename_stays_unknown(self):
        for number in [None, -1, 999999]:
            error = OSError(number, "PRIVATE_TOKEN", "/sys/fs/cgroup/private/memory.peak")
            target = types.SimpleNamespace(cgroup_chain=Mock(side_effect=error), Refusal=self.reader.Refusal)
            code, receipt = O.observe(target, self.filename)
            self.assertEqual(code, 1); self.assertEqual(receipt["diagnostic"], {"primitive": "UNKNOWN", "exception_class": "OSError", "errno": None, "source_line": None})

    def test_reader_hash_path_and_line_map_verification(self):
        source = self.root / "source.py"; source.write_bytes((HERE / "shipping-image-producer.py").read_bytes())
        with patch.object(O, "READER_PATH", source): self.assertEqual(O.load_reader()[1], str(source))
        source.write_bytes(source.read_bytes() + b"\n")
        with patch.object(O, "READER_PATH", source):
            with self.assertRaisesRegex(ValueError, "OBSERVER_SOURCE_UNVERIFIED"): O.load_reader()
        link = self.root / "link.py"; link.symlink_to(source)
        with patch.object(O, "READER_PATH", link):
            with self.assertRaisesRegex(ValueError, "OBSERVER_SOURCE_UNVERIFIED"): O.load_reader()
        lines = (HERE / "shipping-image-producer.py").read_text().splitlines()
        self.assertIn('"memory.peak"', lines[320]); self.assertIn('"memory.events"', lines[321])
        self.assertIn('"memory.max"', lines[317]); self.assertEqual(O.FUNCTION_RANGES["cgroup_chain"], (273, 351))
        with patch.dict(O.FUNCTION_RANGES, {"cgroup_text": (183, 194)}), patch.object(O, "READER_PATH", HERE / "shipping-image-producer.py"):
            with self.assertRaisesRegex(ValueError, "OBSERVER_SOURCE_UNVERIFIED"): O.load_reader()

    def test_main_cli_inputs_and_source_failure_closed_never_observe(self):
        for args, result in [(["--reader", "/private/SECRET"], "OBSERVER_ARGUMENT_REFUSED"), ([], "OBSERVER_SOURCE_UNVERIFIED")]:
            stream = io.StringIO()
            with patch.object(O, "load_reader", side_effect=OSError(13, "PRIVATE_TOKEN", "/private/SECRET")), patch.object(O, "observe") as observe, redirect_stdout(stream):
                self.assertEqual(O.main(args), 1)
            observe.assert_not_called(); self.assertEqual(json.loads(stream.getvalue())["result"], result)
            self.assertNotIn("PRIVATE_TOKEN", stream.getvalue()); self.assertNotIn("/private/", stream.getvalue())

    def test_main_prints_exact_closed_healthy_and_unhealthy_receipts(self):
        for error in [None, self.reader.Refusal("CGROUP_METADATA_UNKNOWN")]:
            call = Mock(return_value=[], side_effect=error); target = types.SimpleNamespace(cgroup_chain=call, Refusal=self.reader.Refusal)
            stream = io.StringIO()
            with patch.object(O, "load_reader", return_value=(target, self.filename)), redirect_stdout(stream): code = O.main([])
            call.assert_called_once_with(); self.assertEqual(code, 0 if error is None else 1)
            receipt = json.loads(stream.getvalue())
            self.assertEqual(receipt["result"], "CGROUP_READER_OBSERVED_OK_NOT_IMAGE_QUALIFIED" if error is None else "CGROUP_METADATA_UNKNOWN")
            self.build_mock.assert_not_called(); self.no_process_mock.assert_not_called()

    def test_foreign_direct_membership_and_proc_masks_closed(self):
        file = self.cg / "parent/cgroup.procs"; original = file.read_bytes(); file.write_text("42\n")
        self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", "DIRECT_MEMBERSHIP_PREDICATE", "Refusal", None, 314)
        file.write_bytes(original)
        mount = self.proc / "self/mountinfo"
        mount.write_text(mount.read_text() + f"3 2 0:9 / {self.proc}/{self.reader.os.getpid()}/stat rw - tmpfs tmpfs rw\n")
        self.assert_diagnostic(self.run_reader(), "CGROUP_ANCESTORS_HIDDEN", "PROC_MOUNT_MASK_PREDICATE", "Refusal", None, 255)

    def test_genuine_visibility_never_invokes_init_readlink(self):
        original = self.reader.os.readlink
        def own_pid_only(path):
            self.assertEqual(path, self.proc / "self"); return original(path)
        with patch.object(self.reader.os, "readlink", side_effect=own_pid_only):
            self.assertEqual(self.run_reader()[0], 0)

    def test_foreign_proc_pid_view_closed_without_foreign_directory_read(self):
        (self.proc / "self").unlink(); (self.proc / "self").symlink_to("42")
        self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", "PROC_SELF_PID_BIND", "Refusal", None, 282)

    def test_unknown_foreign_frame_cannot_claim_pinned_primitive(self):
        error = OSError(13, "PRIVATE_TOKEN", "memory.max")
        target = types.SimpleNamespace(cgroup_chain=Mock(side_effect=error), Refusal=self.reader.Refusal)
        code, receipt = O.observe(target, self.filename)
        self.assertEqual(code, 1); self.assertEqual(receipt["diagnostic"],
            {"primitive": "UNKNOWN", "exception_class": "OSError", "errno": 13, "source_line": None})



class InstalledStatControls(unittest.TestCase):
    """Synthetic metadata/children only; never inspect installed system tools."""
    @staticmethod
    def info(**changes):
        fields = dict(st_dev=1, st_ino=2, st_mode=stat.S_IFREG | 0o755, st_uid=0, st_gid=0,
                      st_size=80000, st_mtime_ns=1, st_ctime_ns=1)
        fields.update(changes); return types.SimpleNamespace(**fields)

    def test_each_original_condition_independent_including_two_size_bounds(self):
        healthy = O.stat_predicate(self.info())
        self.assertEqual(healthy, {"booleans": dict(regular=True, uid_zero=True, no_group_other_write=True, positive_size=True, size_le1048576=True), "original_predicate": True})
        for key, change in [("regular", dict(st_mode=stat.S_IFLNK | 0o755)), ("uid_zero", dict(st_uid=1000)),
                            ("no_group_other_write", dict(st_mode=stat.S_IFREG | 0o777)),
                            ("positive_size", dict(st_size=0)), ("size_le1048576", dict(st_size=1048577))]:
            with self.subTest(key=key):
                result = O.stat_predicate(self.info(**change))
                self.assertFalse(result["original_predicate"]); self.assertFalse(result["booleans"][key])
                self.assertEqual(sum(not value for value in result["booleans"].values()), 1)
        self.assertTrue(O.stat_predicate(self.info(st_size=1048576))["original_predicate"])

    def provenance(self, links=None, changes=None):
        links = links or {}; changes = changes or {}
        def lstat(path):
            if str(path) in changes: return self.info(**changes[str(path)])
            return self.info(st_mode=(stat.S_IFLNK | 0o777) if str(path) in links else
                             (stat.S_IFREG | 0o755) if str(path).endswith(("stat", "coreutils", "readelf", "dpkg-query")) else stat.S_IFDIR | 0o755)
        return patch.object(Path, "lstat", lstat), patch.object(O.os, "readlink", side_effect=lambda p: links[str(p)])

    def test_system_symlink_provenance_never_changes_original_regular_refusal(self):
        a, b = self.provenance({"/usr/bin/stat": "coreutils"})
        with a, b:
            path, info, watches, hops = O.system_leaf(Path('/usr/bin/stat'), float('inf'))
            self.assertEqual(path, Path('/usr/bin/coreutils')); self.assertEqual(hops, 1)
            self.assertFalse(O.stat_predicate(Path('/usr/bin/stat').lstat())["original_predicate"])
            O.check_system_watches(watches)

    def test_symlink_loop_escape_more_than8_and_parent_mode_refused(self):
        cases = [({"/usr/bin/stat": "stat"}, {}, "LINK_LOOP_OR_DEPTH"),
                 ({"/usr/bin/stat": "/private/SECRET"}, {}, "SYSTEM_PATH"),
                 ({f"/usr/bin/{name}": str(n+1) for n,name in enumerate(['stat',*map(str,range(1,9))])}, {}, "LINK_HOPS"),
                 ({}, {"/usr/bin": {"st_mode": stat.S_IFDIR | 0o777}}, "SYSTEM_MODE"),
                 ({}, {"/usr": {"st_uid": 42}}, "SYSTEM_UID")]
        for links, changes, reason in cases:
            with self.subTest(reason=reason):
                a,b=self.provenance(links, changes)
                with a,b,self.assertRaisesRegex(O.MetadataMissing, reason): O.system_leaf(Path('/usr/bin/stat'), float('inf'))

    def test_provenance_identity_drift_and_metadata_byte_cap(self):
        with patch.object(Path,'lstat',return_value=self.info(st_ino=3)), self.assertRaisesRegex(O.MetadataMissing,'IDENTITY_DRIFT'):
            O.check_system_watches({Path('/usr/bin/stat'): O.metadata_identity(self.info())})
        with patch.object(O,'system_leaf',return_value=(Path('/usr/bin/stat'), self.info(st_size=32*1024**2+1), {}, 0)), patch.object(O.os,'open') as opened:
            with self.assertRaisesRegex(O.MetadataMissing,'DIAGNOSTIC_INPUT_BYTES'):
                with O.held_system_leaf(Path('/usr/bin/stat'),float('inf')): self.fail('oversize accepted')
            opened.assert_not_called()

    def test_held_leaf_closes_on_consumer_error_and_closing_drift(self):
        @contextmanager
        def unused(): yield
        for drift in [False,True]:
            with patch.object(O,'system_leaf',return_value=(Path('/usr/bin/stat'), self.info(), {}, 0)), patch.object(O.os,'open',return_value=90), \
                 patch.object(O.os,'fstat',side_effect=[self.info(),self.info(st_ino=3)] if drift else [self.info()]), patch.object(O.os,'close') as closed:
                with self.assertRaises(O.MetadataMissing if drift else RuntimeError):
                    with O.held_system_leaf(Path('/usr/bin/stat'),float('inf')):
                        if not drift: raise RuntimeError('PRIVATE_TOKEN')
                closed.assert_called_once_with(90)

    @contextmanager
    def fake_leaf(self,*args,**kwargs):
        yield 90, Path('/usr/bin/stat'), self.info(), {}, 0

    def fake_child(self, code=0, live=False):
        child=Mock(pid=123,returncode=code);child.stdout=Mock();child.stderr=Mock()
        child.stdout.fileno.return_value=100;child.stderr.fileno.return_value=101
        child.poll.return_value=None if live else code
        return child

    def test_query_exact_env_held_exec_fd_output_and_positive_reap(self):
        child=self.fake_child();queries=[]
        def read(fd,size):
            if fd==100 and not getattr(read,'done',False):
                read.done=True;return b'package: /usr/bin/stat\n'
            return b''
        with patch.object(O,'held_system_leaf',side_effect=self.fake_leaf),patch.object(O,'metadata_hash',return_value='h'), \
             patch.object(O.subprocess,'Popen',return_value=child) as popen,patch.object(O.select,'select',side_effect=lambda pending,*a:(pending,[],[])), \
             patch.object(O.os,'read',side_effect=read):
            data=O.metadata_query(['/usr/bin/dpkg-query','--no-pager','--search','--','/usr/bin/stat'],float('inf'),queries)
        self.assertIn(b'package',data);self.assertTrue(queries[0]['reaped']);self.assertEqual(queries[0]['exit'],0)
        self.assertEqual(popen.call_args.kwargs['env'],{'LC_ALL':'C.UTF-8','PATH':'/usr/bin:/bin'})
        self.assertEqual(popen.call_args.kwargs['executable'],'/proc/self/fd/90');self.assertEqual(popen.call_args.kwargs['pass_fds'],(90,))
        child.wait.assert_called();child.stdout.close.assert_called_once();child.stderr.close.assert_called_once()

    def test_query_deadline_output_exit_hash_error_kill_reap_closed(self):
        for kind in ['deadline','stdout','stderr','exit','hash']:
            child=self.fake_child(code=7 if kind=='exit' else 0,live=True);queries=[]
            def reads(fd,size):return (b'x'*32769 if fd==100 else b'') if kind=='stdout' else (b'x'*4097 if fd==101 else b'') if kind=='stderr' else b''
            with self.subTest(kind=kind),patch.object(O,'held_system_leaf',side_effect=self.fake_leaf), \
                 patch.object(O,'metadata_hash',side_effect=['before','after'] if kind=='hash' else lambda *a:'h'),patch.object(O.subprocess,'Popen',return_value=child), \
                 patch.object(O.select,'select',side_effect=lambda pending,*a:([] if kind=='deadline' else pending,[],[])),patch.object(O.os,'read',side_effect=reads):
                with self.assertRaises(O.MetadataMissing):O.metadata_query(['/usr/bin/readelf'],float('inf'),queries,input_fd=91)
            child.kill.assert_called_once();child.wait.assert_called();self.assertTrue(queries[0]['reaped'])
            self.assertEqual(queries[0]['result'],'MISSING');child.stdout.close.assert_called_once();child.stderr.close.assert_called_once()

    def test_query_spawn_errno_no_raw_text_and_no_fourth_or_foreign_tool(self):
        queries=[]
        with patch.object(O,'held_system_leaf',side_effect=self.fake_leaf),patch.object(O,'metadata_hash',return_value='h'), \
             patch.object(O.subprocess,'Popen',side_effect=OSError(13,'PRIVATE_TOKEN','/private/SECRET')):
            with self.assertRaises(OSError):O.metadata_query(['/usr/bin/readelf'],float('inf'),queries)
        self.assertEqual(queries[0]['errno'],13);self.assertIsNone(queries[0]['pid']);self.assertNotIn('SECRET',json.dumps(queries))
        for argv,used in [(['/usr/bin/stat'],[]),(['/usr/bin/readelf'],[{}, {}, {}])]:
            with patch.object(O.subprocess,'Popen') as popen,self.assertRaisesRegex(O.MetadataMissing,'QUERY_COUNT'):
                O.metadata_query(argv,float('inf'),used)
            popen.assert_not_called()

    def test_package_two_queries_exact_paths_validated_fields(self):
        queries=[];outputs=[b'coreutils: /usr/bin/stat\n',b'coreutils\t9.7-1ubuntu1\tamd64\tinstalled\tcoreutils\t9.7-1ubuntu1\n']
        with patch.object(O,'metadata_query',side_effect=outputs) as query:
            result=O.installed_packages([Path('/usr/bin/stat')],float('inf'),queries)
        self.assertEqual(result['records'][0]['package'],'coreutils');self.assertEqual(query.call_count,2)
        self.assertEqual(query.call_args_list[0].args[0],['/usr/bin/dpkg-query','--no-pager','--search','--','/usr/bin/stat'])
        self.assertEqual(query.call_args_list[1].args[0][-2:],['--','coreutils'])

    def test_package_missing_ambiguous_injected_or_wrong_fields_closed(self):
        for output in [b'',b'one, two: /usr/bin/stat\n',b'coreutils: /private/SECRET\n',b'diversion by PRIVATE_TOKEN from: /usr/bin/stat\n']:
            with patch.object(O,'metadata_query',return_value=output) as query,self.assertRaises(O.MetadataMissing):
                O.installed_packages([Path('/usr/bin/stat')],float('inf'),[])
            self.assertEqual(query.call_count,1)
        with patch.object(O,'metadata_query',side_effect=[b'coreutils: /usr/bin/stat\n',b'coreutils\tPRIVATE_TOKEN\tamd64\tinstalled\tcoreutils\t1\n']),self.assertRaisesRegex(O.MetadataMissing,'PACKAGE_FIELDS'):
            O.installed_packages([Path('/usr/bin/stat')],float('inf'),[])

    def elf_output(self,loader='/lib64/ld-linux-x86-64.so.2'):
        return ("  Class: ELF64\n  Data: 2's complement, little endian\n  Type: DYN (Position-Independent Executable file)\n"
                "  Machine: Advanced Micro Devices X86-64\n"+f" [Requesting program interpreter: {loader}]\n"+
                " 0x0000000000000001 (NEEDED) Shared library: [libc.so.6]\n").encode()

    def test_elf_headers_interpreter_identity_without_loading_or_soname_leak(self):
        with patch.object(O,'metadata_query',return_value=self.elf_output()) as query,patch.object(O,'system_leaf',return_value=(Path('/usr/lib/loader'),self.info(),{},1)), \
             patch.object(O.subprocess,'Popen',side_effect=AssertionError('NO_LOADER')):
            result=O.installed_elf(91,float('inf'),[])
        self.assertEqual(result['class'],'ELF64');self.assertEqual(result['machine'],'X86_64');self.assertEqual(result['type'],'DYN')
        self.assertEqual(result['result'],'OBSERVED_METADATA_NOT_LOADED');self.assertNotIn('libc.so.6',json.dumps(result))
        self.assertEqual(query.call_args.kwargs['input_fd'],91);self.assertEqual(query.call_args.args[0][-1],'/proc/self/fd/91')

    def test_nonelf_or_private_interpreter_and_helper_missing_are_closed(self):
        for output in [b'PRIVATE_TOKEN',self.elf_output('/private/SECRET')]:
            with patch.object(O,'metadata_query',return_value=output),self.assertRaises(O.MetadataMissing):O.installed_elf(91,float('inf'),[])
        for error in [OSError(2,'PRIVATE_TOKEN','/private/SECRET'),O.MetadataMissing('QUERY_EXIT'),subprocess.TimeoutExpired('/private/SECRET',5)]:
            result=O.metadata_error(error);self.assertEqual(result['result'],'MISSING');self.assertNotIn('SECRET',json.dumps(result))

    def metadata_run(self,info=None,drift=False,query_missing=False,unreaped=False,lstat_errno=None):
        info=info or self.info(st_size=2*1024**2);original=Path.lstat;calls=0
        def lstat(path):
            nonlocal calls
            if path==Path('/usr/bin/stat'):
                if lstat_errno is not None: raise OSError(lstat_errno, 'PRIVATE_TOKEN', '/private/SECRET')
                calls+=1;return self.info(st_ino=9) if drift and calls>1 else info
            if path in Path('/usr/bin/stat').parents:return self.info(st_mode=stat.S_IFDIR|0o755)
            return original(path)
        def package_query(paths,deadline,queries):
            if unreaped: queries.append({'pid':123,'reaped':False,'exit':None})
            if query_missing: raise OSError(2,'PRIVATE_TOKEN')
            return {'result':'OBSERVED'}
        with patch.object(O,'READER_PATH',HERE/'shipping-image-producer.py'),patch.object(Path,'lstat',lstat), \
             patch.object(O,'system_leaf',return_value=(Path('/usr/bin/stat'),info,{},0)),patch.object(O,'held_system_leaf',side_effect=self.fake_leaf), \
             patch.object(O,'metadata_hash',return_value='h'),patch.object(O,'installed_packages',side_effect=package_query), \
             patch.object(O,'installed_elf',side_effect=OSError(2,'PRIVATE_TOKEN') if query_missing else None,return_value={'result':'OBSERVED_METADATA_NOT_LOADED'}), \
             patch.object(O,'installed_kernel',return_value={'result':'VISIBLE_KERNEL_ONLY'}),patch.object(O,'observe',side_effect=AssertionError('NO_ORIGINAL_OBSERVATION')), \
             patch.object(O,'load_reader',side_effect=AssertionError('NO_READER_EXEC')),patch.object(O.subprocess,'Popen',side_effect=AssertionError('NO_REAL_CHILD')):
            return O.installed_stat_metadata()

    def test_metadata_positive_records_false_original_size_and_optional_missing(self):
        for missing in [False,True]:
            code,receipt=self.metadata_run(query_missing=missing);self.assertEqual(code,0)
            self.assertEqual(receipt['result'],'METADATA_OBSERVED_NOT_ADMITTED');self.assertFalse(receipt['stat']['original_predicate'])
            self.assertFalse(receipt['stat']['booleans']['size_le1048576']);self.assertNotIn('PRIVATE_TOKEN',json.dumps(receipt))
            self.assertEqual(receipt['packages']['result'],'MISSING' if missing else 'OBSERVED')

    def test_closing_drift_and_wrong_source_not_green(self):
        code,receipt=self.metadata_run(drift=True);self.assertEqual(code,1);self.assertEqual(receipt['result'],'METADATA_MISSING_NOT_ADMITTED')
        with patch.object(O,'READER_PATH',HERE/'test-shipping-cgroup-observer.py'),patch.object(O,'system_leaf') as system:
            code,receipt=O.installed_stat_metadata()
        self.assertEqual(code,1);self.assertEqual(receipt['result'],'OBSERVER_SOURCE_UNVERIFIED');system.assert_not_called()

    def test_query_five_second_cap_includes_closing_reserve(self):
        child=self.fake_child(live=True);queries=[]
        with patch.object(O,'held_system_leaf',side_effect=self.fake_leaf),patch.object(O,'metadata_hash',return_value='h'), \
             patch.object(O.time,'monotonic',return_value=100),patch.object(O.subprocess,'Popen',return_value=child), \
             patch.object(O.select,'select',return_value=([],[],[])) as select:
            with self.assertRaisesRegex(O.MetadataMissing,'QUERY_DEADLINE'):
                O.metadata_query(['/usr/bin/readelf'],200,queries)
        self.assertEqual(select.call_args.args[-1],4.75)
        self.assertEqual(child.wait.call_args.kwargs['timeout'],5)
        self.assertTrue(queries[0]['reaped']);self.assertEqual(queries[0]['exit'],0)

    def test_kernel_os_release_projection_omits_node_and_version_private_text(self):
        uname=types.SimpleNamespace(release='6.17.0-1001-azure',machine='x86_64',version='PRIVATE_TOKEN',nodename='PRIVATE_NODE')
        with patch.object(O.os,'uname',return_value=uname),patch.object(O,'held_system_leaf',side_effect=self.fake_leaf), \
             patch.object(O.os,'read',return_value=b'ID=ubuntu\nVERSION_ID="26.04"\n'):
            result=O.installed_kernel(float('inf'))
        self.assertEqual(result['id'],'ubuntu');self.assertEqual(result['version_id'],'26.04');self.assertEqual(result['machine'],'x86_64')
        self.assertNotIn('PRIVATE',json.dumps(result));self.assertEqual(len(result['version_sha256']),64)
        with patch.object(O.os,'uname',return_value=uname),patch.object(O,'held_system_leaf',side_effect=self.fake_leaf), \
             patch.object(O.os,'read',return_value=b'x'*4097),self.assertRaisesRegex(O.MetadataMissing,'OS_RELEASE_BYTES'):
            O.installed_kernel(float('inf'))

    def test_metadata_lstat_errno_and_unreaped_child_refuse_with_closed_receipt(self):
        for number in [2,13]:
            code,receipt=self.metadata_run(lstat_errno=number);self.assertEqual(code,1)
            self.assertEqual(receipt['provenance_failure']['primitive'],'STAT_LSTAT');self.assertEqual(receipt['provenance_failure']['errno'],number)
            self.assertNotIn('PRIVATE',json.dumps(receipt))
        code,receipt=self.metadata_run(unreaped=True);self.assertEqual(code,1)
        self.assertEqual(receipt['closing']['reason'],'CHILD_NOT_REAPED');self.assertFalse(receipt['queries'][0]['reaped'])

    def test_query_closing_wait_failure_does_not_claim_reap_and_closes_pipes(self):
        child=self.fake_child(live=True);child.wait.side_effect=subprocess.TimeoutExpired('/private/SECRET',5);queries=[]
        with patch.object(O,'held_system_leaf',side_effect=self.fake_leaf),patch.object(O,'metadata_hash',return_value='h'), \
             patch.object(O.subprocess,'Popen',return_value=child),patch.object(O.select,'select',return_value=([],[],[])):
            with self.assertRaises(subprocess.TimeoutExpired):O.metadata_query(['/usr/bin/readelf'],float('inf'),queries)
        child.kill.assert_called_once();self.assertFalse(queries[0]['reaped']);self.assertEqual(queries[0]['kind'],'QUERY_TIMEOUT')
        child.stdout.close.assert_called_once();child.stderr.close.assert_called_once();self.assertNotIn('SECRET',json.dumps(queries))

    def test_streamed_diagnostic_hash_bound_never_increases_original_stat_gate(self):
        total=0
        def read(fd,size):
            nonlocal total
            part=b'x'*size;total+=size;return part
        with patch.object(O.os,'lseek'),patch.object(O.os,'read',side_effect=read),self.assertRaisesRegex(O.MetadataMissing,'DIAGNOSTIC_INPUT_BYTES'):
            O.metadata_hash(90,float('inf'))
        self.assertEqual(total,33554433);self.assertFalse(O.stat_predicate(self.info(st_size=33554432))['original_predicate'])
        with patch.object(O.os,'lseek'),patch.object(O.os,'read',side_effect=[b'abc',b'']):
            self.assertEqual(O.metadata_hash(90,float('inf')),'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')

    def test_metadata_global_deadline_and_invalid_system_path_never_spawn(self):
        with patch.object(O.time,'monotonic',return_value=31),self.assertRaisesRegex(O.MetadataMissing,'DEADLINE'):
            O.metadata_tick(30)
        for name in ['/private/SECRET','/usr/bin/*','/usr/bin/../SECRET']:
            with patch.object(Path,'lstat') as lstat,self.assertRaisesRegex(O.MetadataMissing,'SYSTEM_PATH'):
                O.system_leaf(Path(name),float('inf'))
            lstat.assert_not_called()

    def test_aarch64_elf_positive_and_hostile_closed_error(self):
        output=self.elf_output().replace(b'Advanced Micro Devices X86-64',b'AArch64')
        with patch.object(O,'metadata_query',return_value=output),patch.object(O,'system_leaf',return_value=(Path('/usr/lib/loader'),self.info(),{},0)):
            self.assertEqual(O.installed_elf(90,float('inf'),[])['machine'],'AARCH64')
        class Hostile:
            def __str__(self):raise AssertionError('NO_STRINGIFICATION')
        for error in [RuntimeError(Hostile()),O.MetadataMissing(Hostile()),O.MetadataMissing('PRIVATE_TOKEN')]:
            self.assertEqual(O.metadata_error(error)['result'],'MISSING');self.assertNotIn('PRIVATE',json.dumps(O.metadata_error(error)))

    def test_present_truncated_interpreter_with_valid_headers_refuses(self):
        valid=self.elf_output()
        marker=b'[Requesting program interpreter: /lib64/ld-linux-x86-64.so.2]'
        for replacement in [b'[Requesting program interpreter: /private/SECRET',
                            b'Requesting program interpreter: /private/SECRET',
                            marker+b' [Requesting program interpreter: /private/SECRET']:
            output=valid.replace(marker,replacement)
            with self.subTest(record=replacement),patch.object(O,'metadata_query',return_value=output), \
                 patch.object(O,'system_leaf',return_value=(Path('/usr/lib/loader'),self.info(),{},0)) as system,patch.object(O.subprocess,'Popen',side_effect=AssertionError('NO_INSTALLED_IO')):
                with self.assertRaisesRegex(O.MetadataMissing,'^ELF_INTERPRETER$'):O.installed_elf(91,float('inf'),[])
            system.assert_not_called()

    def test_present_truncated_needed_with_valid_headers_refuses(self):
        valid=self.elf_output();marker=b'(NEEDED) Shared library: [libc.so.6]'
        for replacement in [b'(NEEDED) Shared library: [libc.so.6',b'(NEEDED) Shared library: ',
                            b'(NEEDED',marker+b' (NEEDED) Shared library: [truncated']:
            output=valid.replace(marker,replacement)
            with self.subTest(record=replacement),patch.object(O,'metadata_query',return_value=output), \
                 patch.object(O,'system_leaf',return_value=(Path('/usr/lib/loader'),self.info(),{},0)), \
                 patch.object(O.subprocess,'Popen',side_effect=AssertionError('NO_INSTALLED_IO')):
                with self.assertRaisesRegex(O.MetadataMissing,'^ELF_NEEDED$'):O.installed_elf(91,float('inf'),[])

    def test_absent_static_interpreter_and_needed_remain_observed(self):
        output=b'\n'.join(line for line in self.elf_output().splitlines()
                          if b'Requesting program interpreter' not in line and b'(NEEDED)' not in line)+b'\n'
        output=output.replace(b'DYN (Position-Independent Executable file)',b'EXEC (Executable file)')
        with patch.object(O,'metadata_query',return_value=output),patch.object(O,'system_leaf') as system, \
             patch.object(O.subprocess,'Popen',side_effect=AssertionError('NO_INSTALLED_IO')):
            result=O.installed_elf(91,float('inf'),[])
        self.assertEqual(result['result'],'OBSERVED_METADATA_NOT_LOADED');self.assertEqual(result['type'],'EXEC')
        self.assertIsNone(result['interpreter']);self.assertEqual(result['needed_sha256'],[]);system.assert_not_called()

    def actual_metadata_elf_projection(self,output):
        original=Path.lstat;info=self.info()
        def lstat(path):
            if path==Path('/usr/bin/stat'):return info
            if path in Path('/usr/bin/stat').parents:return self.info(st_mode=stat.S_IFDIR|0o755)
            return original(path)
        with patch.object(O,'READER_PATH',HERE/'shipping-image-producer.py'),patch.object(Path,'lstat',lstat), \
             patch.object(O,'system_leaf',return_value=(Path('/usr/bin/stat'),info,{},0)),patch.object(O,'held_system_leaf',side_effect=self.fake_leaf), \
             patch.object(O,'metadata_hash',return_value='h'),patch.object(O,'installed_packages',return_value={'result':'OBSERVED'}), \
             patch.object(O,'metadata_query',return_value=output),patch.object(O,'installed_kernel',return_value={'result':'VISIBLE_KERNEL_ONLY'}), \
             patch.object(O,'observe',side_effect=AssertionError('NO_ORIGINAL_COLLECTOR')),patch.object(O,'load_reader',side_effect=AssertionError('NO_READER_EXEC')), \
             patch.object(O.subprocess,'Popen',side_effect=AssertionError('NO_INSTALLED_IO')):
            # Actual caller AND actual installed_elf; only installed IO is mocked.
            return O.installed_stat_metadata()

    def test_actual_metadata_caller_keeps_malformed_optional_elf_missing_not_admitted(self):
        headers=b'\n'.join(line for line in self.elf_output().splitlines()
                           if b'Requesting program interpreter' not in line and b'(NEEDED)' not in line)+b'\n'
        for record,reason in [(b' [Requesting program interpreter: /private/SECRET\n','ELF_INTERPRETER'),
                              (b' 0x0000000000000001 (NEEDED) Shared library: [libc.so.6\n','ELF_NEEDED')]:
            with self.subTest(reason=reason):
                code,receipt=self.actual_metadata_elf_projection(headers+record)
                self.assertEqual(code,0);self.assertEqual(receipt['result'],'METADATA_OBSERVED_NOT_ADMITTED')
                self.assertEqual(receipt['elf'],{'result':'MISSING','kind':'METADATA_BOUNDARY','reason':reason,'errno':None})
                self.assertTrue(receipt['stat']['original_predicate']);self.assertNotIn('PRIVATE',json.dumps(receipt))
                self.assertNotIn('/private/',json.dumps(receipt));self.assertNotIn('OBSERVED_METADATA_NOT_LOADED',json.dumps(receipt['elf']))

    def test_literal_mode_only_and_extra_args_refused_never_observe(self):
        expected={'scope':'INSTALLED_STAT_METADATA_ONLY_NOT_ADMITTED_NOT_IMAGE_QUALIFIED','result':'METADATA_OBSERVED_NOT_ADMITTED'}
        stream=io.StringIO()
        with patch.object(O,'installed_stat_metadata',return_value=(0,expected)) as mode,patch.object(O,'observe') as observe,redirect_stdout(stream):
            self.assertEqual(O.main(['--installed-stat-metadata']),0)
        mode.assert_called_once_with();observe.assert_not_called();self.assertEqual(json.loads(stream.getvalue()),expected)
        for args in [['--installed-stat-metadata','--reader','/private/SECRET'],['--installed-stat-metadata=true'],['--installed-stat-metadata','--installed-stat-metadata']]:
            with patch.object(O,'installed_stat_metadata') as mode,patch.object(O,'observe') as observe,redirect_stdout(io.StringIO()) as stream:
                self.assertEqual(O.main(args),1)
            mode.assert_not_called();observe.assert_not_called();self.assertEqual(json.loads(stream.getvalue())['result'],'OBSERVER_ARGUMENT_REFUSED')


if __name__ == "__main__":
    unittest.main()
