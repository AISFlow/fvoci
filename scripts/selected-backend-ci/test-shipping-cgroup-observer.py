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
        for pid in ["self", "1"]:
            (self.proc / pid / "ns").mkdir(parents=True)
            (self.proc / pid / "ns/cgroup").symlink_to("cgroup:[fixture]")
        (self.proc / "self/mountinfo").write_text(f"1 0 0:1 / {self.cg} rw - cgroup2 cgroup rw\n")
        (self.proc / "self/cgroup").write_text("0::/parent\n")
        (self.cg / "parent").mkdir(parents=True)
        (self.cg / "cgroup.controllers").write_text("cpu memory\n")
        for name, text in {"memory.max": "max", "memory.current": "100", "memory.peak": "200",
                           "memory.events": "oom 0\noom_kill 0\noom_group_kill 0\n", "cpu.max": "max 100000"}.items():
            (self.cg / "parent" / name).write_text(text)
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
        self.assertEqual(receipt, {"scope": "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED", "reader_sha": "288dacdcdbe1f76a7f885e6a13c17014dc4bb95d",
            "reader_source_sha256": "7abda2c2006ca4547811e243249f315e022b0505e2fc8b3238371999387ae644", "result": code,
            "diagnostic": {"primitive": primitive, "exception_class": kind, "errno": errno, "source_line": line}})

    def test_healthy_once_never_build_or_process(self):
        code, receipt = self.run_reader()
        self.assertEqual(code, 0); self.assertEqual(receipt["result"], "CGROUP_READER_OBSERVED_OK_NOT_IMAGE_QUALIFIED")
        self.assertEqual(receipt["scope"], "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED"); self.assertIsNone(receipt["diagnostic"])

    def test_enoent_eacces_keep_closed_primitive_not_hostile_text_filename(self):
        read = Path.read_text
        for number in [2, 13]:
            def failing(path, *a, **kw):
                if path.name == "memory.peak": raise OSError(number, "::error::PRIVATE_TOKEN https://private.invalid", "/private/SECRET\n::error::")
                return read(path, *a, **kw)
            with patch.object(Path, "read_text", failing):
                result = self.run_reader()
            self.assert_diagnostic(result, "CGROUP_METADATA_UNKNOWN", "MEMORY_PEAK_READ_PARSE", "OSError", number, 186)
            text = json.dumps(result)
            for raw in ["PRIVATE_TOKEN", "https://", "/private/", "SECRET", "::error::"]: self.assertNotIn(raw, text)

    def test_parse_error_never_emit_rejected_bytes(self):
        (self.cg / "parent/memory.max").write_text("PRIVATE_TOKEN https://private.invalid")
        result = self.run_reader()
        self.assert_diagnostic(result, "CGROUP_METADATA_UNKNOWN", "MEMORY_MAX_PARSE", "ValueError", None, 184)
        self.assertNotIn("PRIVATE_TOKEN", json.dumps(result)); self.assertNotIn("https://", json.dumps(result))

    def test_decode_error_class_closed(self):
        (self.cg / "parent/memory.peak").write_bytes(b"\xffPRIVATE_TOKEN")
        self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", "MEMORY_PEAK_READ_PARSE", "ValueError", None, 186)

    def test_explicit_shape_and_counter_refusals(self):
        for text, primitive, line in [("oom 0\noom 0\n", "MEMORY_EVENTS_SHAPE", 188),
                                      ("oom 0\n", "MEMORY_COUNTERS_PREDICATE", 190)]:
            (self.cg / "parent/memory.events").write_text(text)
            self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", primitive, "Refusal", None, line)

    def test_explicit_namespace_refusal_keeps_original_code(self):
        (self.proc / "1/ns/cgroup").unlink(); (self.proc / "1/ns/cgroup").symlink_to("different:[PRIVATE_TOKEN]")
        self.assert_diagnostic(self.run_reader(), "CGROUP_ANCESTORS_HIDDEN", "MOUNT_NAMESPACE_PREDICATE", "Refusal", None, 170)

    def test_namespace_readlink_errno_disambiguated_without_path(self):
        original = self.reader.os.readlink
        for target, primitive in [("self", "SELF_NAMESPACE_READLINK"), ("1", "INIT_NAMESPACE_READLINK")]:
            def failing(path, *a, **kw):
                if path == self.proc / target / "ns/cgroup": raise OSError(13, "PRIVATE_TOKEN", f"/proc/{target}/ns/cgroup")
                return original(path, *a, **kw)
            with patch.object(self.reader.os, "readlink", failing):
                self.assert_diagnostic(self.run_reader(), "CGROUP_METADATA_UNKNOWN", primitive, "OSError", 13, 171)

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
        self.assertIn('"memory.peak"', lines[185]); self.assertIn('"memory.events"', lines[186])
        self.assertIn('"memory.max"', lines[182]); self.assertEqual((min(O.STAGES), max(O.STAGES)), (168, 194))

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


if __name__ == "__main__":
    unittest.main()
