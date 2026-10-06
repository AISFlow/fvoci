#!/usr/bin/env python3
"""Literal pure observer controls; no real cgroup/Docker/build observation."""
import ast
import copy
import importlib.util
import io
import json
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
        self.assertEqual(receipt, {"scope": "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED", "reader_sha": "05c9611c5c140030df582dd99fd518a022948a31",
            "reader_source_sha256": "4712f6e06d653c50e04020fdabfbfdd48e7b2ea090876b277b3aeb317698d038", "result": code,
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


if __name__ == "__main__":
    unittest.main()
