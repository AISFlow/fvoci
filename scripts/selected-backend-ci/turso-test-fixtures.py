#!/usr/bin/env python3
"""Credential-free, network-free admission regressions; not Turso runtime tests."""

import contextlib
import copy
import importlib.util
import io
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest import mock

GUARD_PATH = Path(__file__).with_name("turso-test-guard.py")
spec = importlib.util.spec_from_file_location("turso_test_guard", GUARD_PATH)
guard = importlib.util.module_from_spec(spec)
spec.loader.exec_module(guard)


class AdmissionTests(unittest.TestCase):
    def setUp(self):
        self.context = {
            "event_name": "workflow_dispatch",
            "repository": "AISFlow/fvoci",
            "ref": "refs/heads/main",
            "sha": "a" * 40,
        }
        self.host = "isolated-owner.aws-us-east-1.turso.io"
        self.inputs = {
            "phase": "connection",
            "destructive": False,
        }
        self.settings = {
            "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "false",
        }
        # Literal invented fixture values, never a credential lookup.
        self.secrets = {
            "FVOCI_TEST_TURSO_DATABASE_URL": "libsql://" + self.host,
            "FVOCI_TEST_TURSO_AUTH_TOKEN": "FAKE_FIXTURE_TOKEN_NEVER_REAL",
        }

    def denied(self, code, function, *args):
        with self.assertRaises(guard.AdmissionError) as caught:
            function(*args)
        self.assertEqual(str(caught.exception), code)

    def test_trusted_dispatch_configuration(self):
        self.assertEqual(guard.validate_dispatch(self.context, self.inputs, "a" * 40), "connection")

    def test_denied_event_repository_and_ref(self):
        for key, value in (
            ("event_name", "pull_request"),
            ("event_name", "pull_request_target"),
            ("repository", "attacker/fvoci"),
            ("ref", "refs/heads/topic"),
            ("ref", "refs/tags/main"),
        ):
            with self.subTest(key=key, value=value):
                changed = dict(self.context, **{key: value})
                self.denied("UNTRUSTED_DISPATCH", guard.validate_dispatch, changed, self.inputs, "a" * 40)

    def test_empty_stale_and_arbitrary_checkout(self):
        for sha, checkout in (("", ""), ("a" * 40, "b" * 40), ("main", "main")):
            with self.subTest(sha=sha, checkout=checkout):
                self.denied("CHECKOUT_MISMATCH", guard.validate_dispatch, dict(self.context, sha=sha), self.inputs, checkout)

    def test_connection_has_no_host_or_database_name_requirement(self):
        inputs = {"phase": "connection", "destructive": False}
        self.assertEqual(guard.validate_dispatch(self.context, inputs, "a" * 40), "connection")
        self.assertEqual(guard.validate_target(inputs, {}, self.secrets), "connection")

    def test_boolean_does_not_accept_truthy_strings(self):
        for value in ("false", "true", 0, 1, None):
            self.denied("INVALID_BOOLEAN", guard.validate_dispatch, self.context, dict(self.inputs, destructive=value), "a" * 40)

    def test_connection_refuses_destructive(self):
        self.denied("CONNECTION_MUST_BE_READ_ONLY", guard.validate_target, dict(self.inputs, destructive=True), self.settings, self.secrets)

    def test_future_phase_requires_explicit_dispatch_confirmation(self):
        for phase in guard.PHASES[1:]:
            self.denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", guard.validate_dispatch, self.context, dict(self.inputs, phase=phase), "a" * 40)

    def test_tls_primary_configuration_and_input_unchanged(self):
        original = copy.deepcopy((self.inputs, self.settings, self.secrets))
        self.assertEqual(guard.validate_target(self.inputs, self.settings, self.secrets), "connection")
        self.assertEqual((self.inputs, self.settings, self.secrets), original)
        https = dict(self.secrets, FVOCI_TEST_TURSO_DATABASE_URL="https://" + self.host + "/")
        self.assertEqual(guard.validate_target(self.inputs, self.settings, https), "connection")

    def test_both_missing_secrets_fail_closed(self):
        for key in self.secrets:
            for value in ("", None):
                self.denied("MISSING_SECRET", guard.validate_target, self.inputs, self.settings, dict(self.secrets, **{key: value}))

    def test_no_url_credentials_query_fragment_port_or_local_fallback(self):
        host = self.host
        for url in (
            "http://" + host, "file:///tmp/database", "libsql://localhost",
            "libsql://127.0.0.1", "libsql://other.example.org", "libsql://user:password@" + host,
            "https://" + host + ":443", "https://" + host + "/replica",
            "https://" + host + "?token=FAKE", "https://" + host + "#FAKE",
            "https://" + host + "?", "https://" + host + "#",
            "https://" + host + "\\@wrong.turso.io", "https://[broken",
        ):
            with self.subTest(url=url):
                self.denied("INVALID_PRIMARY_URL", guard.validate_target, self.inputs, self.settings, dict(self.secrets, FVOCI_TEST_TURSO_DATABASE_URL=url))

    def test_nonprimary_host_shape_denied(self):
        for host in ("localhost", "127.0.0.1", "owner.example.org", "owner.turso.io.evil.org", "OWNER.turso.io", "turso.io"):
            self.denied("INVALID_PRIMARY_URL", guard.validate_target, self.inputs, self.settings, dict(self.secrets, FVOCI_TEST_TURSO_DATABASE_URL="https://" + host))

    def test_control_characters_and_oversize_not_truncated(self):
        for token in ("FAKE\nTOKEN", " FAKE", "x" * 16385):
            self.denied("INVALID_SECRET_FORMAT", guard.validate_target, self.inputs, self.settings, dict(self.secrets, FVOCI_TEST_TURSO_AUTH_TOKEN=token))

    def test_destructive_requires_both_gates(self):
        for flag, allow in ((False, "true"), (True, "false"), (True, "TRUE"), (True, "")):
            self.denied("DESTRUCTIVE_NOT_ALLOWED", guard.validate_target, dict(self.inputs, phase="crud", destructive=flag), dict(self.settings, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE=allow), self.secrets)
        self.assertEqual(guard.validate_target(dict(self.inputs, phase="crud", destructive=True), dict(self.settings, FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE="true"), self.secrets), "crud")

    def test_unknown_phase_not_shell_command(self):
        self.denied("UNKNOWN_PHASE", guard.validate_dispatch, self.context, dict(self.inputs, phase="echo FAKE"), "a" * 40)
        self.denied("UNKNOWN_PHASE", guard.require_implemented, "echo FAKE")

    def test_every_fixed_phase_refuses_successful_noop(self):
        for phase in ("crud", "transactions", "migration", "persistence", "restore", "ui-ack"):
            self.denied("NOT_IMPLEMENTED", guard.require_implemented, phase)
        # Permission to select a real fixture is not its execution or PASS.
        self.assertIsNone(guard.require_implemented("connection"))

    def test_real_cli_unavailable_phase_and_redaction(self):
        sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        environment = {
            "PATH": os.environ.get("PATH", ""),
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REPOSITORY": "AISFlow/fvoci",
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_SHA": sha,
        }
        with tempfile.TemporaryDirectory(prefix="fvoci-turso-pure-") as directory:
            event = Path(directory) / "event.json"
            environment["GITHUB_EVENT_PATH"] = str(event)
            event.write_text(json.dumps({"inputs": dict(self.inputs, destructive="false")}), encoding="utf-8")
            result = subprocess.run(["python3", str(GUARD_PATH), "--consume"], env=environment, capture_output=True, text=True, check=False)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (78, "", "MISSING_SECRET\n"))
            # Actual parse failure, with sentinel input that must never leak.
            event.write_text('{"FAKE_SECRET_SENTINEL_NEVER_REAL":', encoding="utf-8")
            result = subprocess.run(["python3", str(GUARD_PATH), "--consume"], env=environment, capture_output=True, text=True, check=False)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (78, "", "ADMISSION_FAILED\n"))

    def test_pure_validation_does_not_print_inputs(self):
        output = io.StringIO()
        with contextlib.redirect_stdout(output), contextlib.redirect_stderr(output):
            self.denied("INVALID_PRIMARY_URL", guard.validate_target, self.inputs, self.settings, dict(self.secrets, FVOCI_TEST_TURSO_DATABASE_URL="https://FAKE_SECRET_SENTINEL@wrong.turso.io"))
        self.assertEqual(output.getvalue(), "")

    def test_two_secrets_connection_needs_no_host_name_or_allow_variable(self):
        settings = dict(self.settings)
        del settings["FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE"]
        self.assertEqual(guard.validate_target(self.inputs, settings, self.secrets), "connection")
        self.denied("DESTRUCTIVE_NOT_ALLOWED", guard.validate_target, dict(self.inputs, phase="crud", destructive=True), settings, self.secrets)

    def test_preexisting_environment_current_null_policy_allowed(self):
        environment = {"name": "fvoci-turso-test", "id": 123, "deployment_branch_policy": None}
        self.assertIsNone(guard.validate_environment(environment))
        for changed in ({}, dict(environment, id=0), dict(environment, name="other")):
            self.denied("ENVIRONMENT_POLICY_DENIED", guard.validate_environment, changed)

    def test_metadata_reads_are_anonymous_fixed_no_redirect(self):
        environment = {"name": "fvoci-turso-test", "id": 123, "deployment_branch_policy": None}
        responses = []
        for suffix, value in (("", environment),):
            response = mock.MagicMock()
            response.__enter__.return_value = response
            response.status = 200
            response.geturl.return_value = guard.API_ROOT + suffix
            response.read.return_value = json.dumps(value).encode()
            responses.append(response)
        opener = mock.Mock()
        opener.open.side_effect = responses
        with mock.patch.object(guard, "build_opener", return_value=opener):
            self.assertEqual(guard.environment_metadata(), 123)
        for call in opener.open.call_args_list:
            request = call.args[0]
            self.assertTrue(request.full_url.startswith(guard.API_ROOT))
            self.assertEqual(request.get_method(), "GET")
            self.assertFalse(request.has_header("Authorization"))
            self.assertEqual(call.kwargs, {"timeout": 15})
        self.assertIsNone(guard.NoRedirect().redirect_request(None, None, 302, "", {}, "https://wrong.example"))
        opener.open.side_effect = OSError("FAKE_PRIVATE_BODY_NEVER_PRINT")
        with mock.patch.object(guard, "build_opener", return_value=opener):
            self.denied("ENVIRONMENT_METADATA_UNAVAILABLE", guard.environment_metadata)

    def test_actual_runtime_command_and_zero_test_denial_no_sdk_execution(self):
        # This is a pure wrapper control with a mocked result, not ELF/SDK proof.
        with tempfile.TemporaryDirectory(prefix="fvoci-turso-binding-pure-") as directory:
            root = Path(directory)
            binary = root / "turso-connection-libtest"
            binary.write_bytes(b"\x7fELFpure fixture, never executed")
            import hashlib
            native = root / "fvoci-sqlite" / "consumer-inputs.json"
            native.parent.mkdir()
            native.write_bytes(b"pure metadata fixture, not native proof")
            manifest = {"sha": "a" * 40, "source_digest": "fixture", "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "native_input_sha256": hashlib.sha256(native.read_bytes()).hexdigest()}
            (root / "turso-connection-build.json").write_text(json.dumps(manifest))
            environment = {**self.secrets, **self.settings, "RUNNER_TEMP": directory, "UNRELATED_FAKE_CREDENTIAL": "never forwarded"}
            valid = ("test " + guard.TEST_NAME + " ... FVOCI_TURSO_RECEIPT primary=OK rollback=OK close=OK leases=ZERO\nok\n"
                     "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n").encode()
            with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(guard, "source_digest", return_value="fixture"), mock.patch.object(guard.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, valid)
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    guard.run_connection("a" * 40, self.inputs)
                self.assertIn("TURSO_CONNECTION_PASS tests=1 ignored=0", output.getvalue())
                self.assertEqual(run.call_args.args[0][1:], [guard.TEST_NAME, "--ignored", "--exact", "--test-threads=1", "--nocapture"])
                self.assertNotIn("UNRELATED_FAKE_CREDENTIAL", run.call_args.kwargs["env"])
                for raw, exit_code in ((valid.replace(b"1 passed", b"0 passed"), 0), (valid.replace(b"0 ignored", b"1 ignored"), 0), (valid, 1), (valid.replace(b"rollback=OK", b"rollback=FAILED"), 0)):
                    run.return_value = subprocess.CompletedProcess([], exit_code, raw + b"FAKE_PRIVATE_BODY_NEVER_PRINT")
                    with contextlib.redirect_stdout(io.StringIO()) as output:
                        self.denied("TURSO_CONNECTION_FAILED", guard.run_connection, "a" * 40, self.inputs)
                    self.assertNotIn("FAKE_PRIVATE_BODY", output.getvalue())
                run.reset_mock()
                self.denied("COMPILED_TEST_BINDING_FAILED", guard.run_connection, "b" * 40, self.inputs)
                run.assert_not_called()

    def test_cargo_emission_freeze_controls_are_file_only_not_compilation(self):
        for mutation in ("valid", "wrong_feature", "wrong_source", "duplicate", "failed_build", "foreign_path"):
            with self.subTest(mutation=mutation), tempfile.TemporaryDirectory(prefix="fvoci-turso-freeze-pure-") as directory:
                root = Path(directory)
                binary = root / "turso-target" / "debug" / "deps" / "fixture-libtest"
                binary.parent.mkdir(parents=True)
                binary.write_bytes(b"\x7fELFfile-only fixture, never executed")
                native = root / "fvoci-sqlite" / "consumer-inputs.json"
                native.parent.mkdir()
                native.write_text("pure metadata fixture, not native proof")
                artifact = {"reason": "compiler-artifact", "target": {"kind": ["lib"], "name": "fvoci_server", "src_path": str(Path.cwd() / "src" / "lib.rs")}, "profile": {"test": True}, "features": ["db-tests"], "executable": str(binary)}
                if mutation == "wrong_feature": artifact["features"] = ["db-tests", "api-schema"]
                if mutation == "wrong_source": artifact["target"]["src_path"] = "/foreign/src/lib.rs"
                if mutation == "foreign_path": artifact["executable"] = "/foreign/test"
                emissions = [artifact] * (2 if mutation == "duplicate" else 1) + [{"reason": "build-finished", "success": mutation != "failed_build"}]
                (root / "turso-compile.json").write_text("\n".join(json.dumps(value) for value in emissions))
                with mock.patch.dict(os.environ, {"RUNNER_TEMP": directory}, clear=True), mock.patch.object(guard, "source_digest", return_value="pure source fixture"):
                    if mutation == "valid":
                        guard.freeze_compiled_test("a" * 40)
                        manifest = json.loads((root / "turso-connection-build.json").read_text())
                        self.assertEqual(manifest["sha"], "a" * 40)
                        self.assertEqual((root / "turso-connection-libtest").read_bytes(), binary.read_bytes())
                    else:
                        self.denied("COMPILED_TEST_BINDING_FAILED", guard.freeze_compiled_test, "a" * 40)


if __name__ == "__main__":
    unittest.main(verbosity=2)
