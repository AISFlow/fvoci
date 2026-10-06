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


class WorkflowTargetTests(unittest.TestCase):
    def test_target_published_before_preparation_for_later_steps(self):
        # Only inspect and execute the literal shell prefix. Admission fixtures
        # must not depend on PyYAML or launch the compiler/native preparation.
        root = Path(__file__).resolve().parents[2]
        workflow = (root / ".github/workflows/turso-test.yml").read_text()
        initialization = 'printf \'CARGO_TARGET_DIR=%s/turso-target\\n\' "$RUNNER_TEMP" >> "$GITHUB_ENV"\n'
        prefix = "set -euo pipefail\n" + initialization
        marker = "      - name: Credential-free compiler and maintained SQLite inputs\n        run: |\n"
        self.assertIn(marker, workflow)
        preparation = workflow.split(marker, 1)[1].split("      - name:", 1)[0]
        self.assertTrue(preparation.startswith("".join("          " + line for line in prefix.splitlines(keepends=True))))
        self.assertNotIn("      CARGO_TARGET_DIR:", workflow)
        with tempfile.TemporaryDirectory(prefix="fvoci-turso-env-pure-") as directory:
            envfile = Path(directory) / "github-env"
            result = subprocess.run(["bash", "-c", prefix], env={"PATH": os.environ["PATH"], "RUNNER_TEMP": directory, "GITHUB_ENV": str(envfile)}, capture_output=True, text=True, check=False)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""))
            self.assertEqual(envfile.read_text(), "CARGO_TARGET_DIR=" + directory + "/turso-target\n")


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

    def test_fixed_reviewed_branch_bootstrap_and_manual_only_consumption(self):
        self.assertEqual(guard.validate_dispatch(dict(self.context, event_name="push", ref=guard.REVIEWED_REF), {}, "a" * 40), "connection")
        self.assertEqual(guard.validate_dispatch(dict(self.context, ref=guard.REVIEWED_REF), self.inputs, "a" * 40), "connection")
        for event, ref in (("push", "refs/heads/main"), ("push", "refs/heads/topic"), ("workflow_dispatch", "refs/heads/topic")):
            self.denied("UNTRUSTED_DISPATCH", guard.validate_dispatch, dict(self.context, event_name=event, ref=ref), self.inputs, "a" * 40)
        sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        with tempfile.TemporaryDirectory(prefix="fvoci-turso-bootstrap-pure-") as directory:
            event = Path(directory) / "event.json"
            event.write_text('{"inputs": {}}')
            environment = {"PATH": os.environ.get("PATH", ""), "GITHUB_EVENT_PATH": str(event), "GITHUB_EVENT_NAME": "push", "GITHUB_REPOSITORY": "AISFlow/fvoci", "GITHUB_REF": guard.REVIEWED_REF, "GITHUB_SHA": sha}
            result = subprocess.run(["python3", str(GUARD_PATH), "--admit"], env=environment, capture_output=True, text=True, check=False)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN\n", ""))
            result = subprocess.run(["python3", str(GUARD_PATH), "--consume"], env=environment, capture_output=True, text=True, check=False)
            self.assertEqual((result.returncode, result.stdout, result.stderr), (78, "", "SECRET_MODE_REQUIRES_MANUAL\n"))

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
        for phase in ("crud", "transactions", "persistence", "restore", "ui-ack"):
            self.denied("NOT_IMPLEMENTED", guard.require_implemented, phase)
        # Permission to select a real fixture is not its execution or PASS.
        self.assertIsNone(guard.require_implemented("connection"))
        self.assertIsNone(guard.require_implemented("migration"))

    def test_real_cli_unavailable_phase_and_redaction(self):
        sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
        environment = {
            "PATH": os.environ.get("PATH", ""),
            "GITHUB_EVENT_NAME": "workflow_dispatch",
            "GITHUB_REPOSITORY": "AISFlow/fvoci",
            "GITHUB_REF": "refs/heads/main",
            "GITHUB_SHA": sha,
            "FVOCI_DATABASE_BACKEND": "libsql-remote",
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
            environment = {**self.settings, "FVOCI_DATABASE_BACKEND": "libsql-remote", "FVOCI_LIBSQL_URL": self.secrets["FVOCI_TEST_TURSO_DATABASE_URL"], "FVOCI_LIBSQL_AUTH_TOKEN": self.secrets["FVOCI_TEST_TURSO_AUTH_TOKEN"], "RUNNER_TEMP": directory, "UNRELATED_FAKE_CREDENTIAL": "never forwarded"}
            valid = ("test " + guard.TEST_NAME + " ... FVOCI_TURSO_RECEIPT primary=OK rollback=OK close=OK leases=ZERO\nok\n"
                     "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n").encode()
            with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(guard, "source_digest", return_value="fixture"), mock.patch.object(guard.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, valid)
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    guard.run_connection("a" * 40, self.inputs)
                self.assertIn("TURSO_CONNECTION_PASS tests=1 ignored=0", output.getvalue())
                self.assertEqual(run.call_args.args[0][1:], [guard.TEST_NAME, "--ignored", "--exact", "--test-threads=1", "--nocapture"])
                self.assertNotIn("UNRELATED_FAKE_CREDENTIAL", run.call_args.kwargs["env"])
                self.assertEqual(run.call_args.kwargs["env"]["FVOCI_DATABASE_BACKEND"], "libsql-remote")
                self.assertIn("FVOCI_LIBSQL_AUTH_TOKEN", run.call_args.kwargs["env"])
                self.assertNotIn("FVOCI_TEST_TURSO_AUTH_TOKEN", run.call_args.kwargs["env"])
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

    def test_migration_requires_manual_dispatch_and_both_destructive_flags(self):
        inputs = {"phase": "migration", "destructive": True}
        self.assertEqual(guard.validate_dispatch(self.context, inputs, "a" * 40), "migration")
        self.denied("SECRET_MODE_REQUIRES_MANUAL", guard.validate_dispatch,
                    dict(self.context, event_name="push", ref=guard.REVIEWED_REF), inputs, "a" * 40)
        self.denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", guard.validate_dispatch,
                    self.context, dict(inputs, destructive=False), "a" * 40)
        for settings in ({}, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "false"}):
            self.denied("DESTRUCTIVE_NOT_ALLOWED", guard.validate_target, inputs, settings, self.secrets)
        self.assertEqual(guard.validate_target(inputs, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true"}, self.secrets), "migration")
        self.denied("WRONG_CONSUMER_PHASE", guard.run_connection, "a" * 40, inputs)
        self.denied("WRONG_CONSUMER_PHASE", guard.run_migration, "a" * 40, self.inputs)

    def test_migration_receipt_rejects_missing_partial_wrong_test_and_zero_execution(self):
        valid = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK current=OK restart=OK close=OK leases=ZERO\nok\n"
                 "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n")
        success = subprocess.CompletedProcess([], 0, b"")
        with contextlib.redirect_stdout(io.StringIO()) as output:
            guard.migration_result(success, valid)
        self.assertIn("TURSO_MIGRATION_PASS tests=1 ignored=0", output.getvalue())
        for changed, code in (
            (valid.replace("1 passed", "0 passed"), "TURSO_MIGRATION_FAILED"),
            (valid.replace("0 ignored", "1 ignored"), "TURSO_MIGRATION_FAILED"),
            (valid.replace(guard.MIGRATION_TEST_NAME, guard.TEST_NAME), "TURSO_MIGRATION_FAILED"),
            (valid.replace("fk_rollback=OK", "fk_rollback=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"),
            (valid.replace("restart=OK", "restart=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"),
            (valid.replace("close=OK", "close=FAILED"), "TURSO_MIGRATION_FAILED"),
            (valid.replace("leases=ZERO", "leases=FAILED"), "TURSO_MIGRATION_FAILED"),
            (valid.replace("FVOCI_TURSO_MIGRATION_RECEIPT", "FAKE_RECEIPT"), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid + valid, "TURSO_MIGRATION_RECEIPT_MISSING"),
        ):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                self.denied(code, guard.migration_result, success, changed + "FAKE_PRIVATE_TOKEN_NEVER_PRINT")
            self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        with contextlib.redirect_stdout(io.StringIO()):
            self.denied("TURSO_MIGRATION_FAILED", guard.migration_result, subprocess.CompletedProcess([], 1, b""), valid)

    def test_migration_binding_exports_exact_four_flags_only_after_confirmation(self):
        with tempfile.TemporaryDirectory(prefix="fvoci-turso-migration-pure-") as directory:
            root = Path(directory)
            binary = root / "turso-connection-libtest"
            binary.write_bytes(b"\x7fELFpure fixture, never executed")
            import hashlib
            native = root / "fvoci-sqlite" / "consumer-inputs.json"
            native.parent.mkdir()
            native.write_bytes(b"pure metadata fixture, not native proof")
            manifest = {"sha": "a" * 40, "source_digest": "fixture", "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "native_input_sha256": hashlib.sha256(native.read_bytes()).hexdigest()}
            (root / "turso-connection-build.json").write_text(json.dumps(manifest))
            environment = {"PATH": os.environ.get("PATH", ""), "FVOCI_DATABASE_BACKEND": "libsql-remote", "FVOCI_LIBSQL_URL": self.secrets["FVOCI_TEST_TURSO_DATABASE_URL"], "FVOCI_LIBSQL_AUTH_TOKEN": self.secrets["FVOCI_TEST_TURSO_AUTH_TOKEN"], "RUNNER_TEMP": directory, "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true", "UNRELATED_FAKE_CREDENTIAL": "never forwarded"}
            inputs = {"phase": "migration", "destructive": True}
            valid = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK current=OK restart=OK close=OK leases=ZERO\nok\n"
                     "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n").encode()
            with mock.patch.dict(os.environ, environment, clear=True), mock.patch.object(guard, "source_digest", return_value="fixture"), mock.patch.object(guard.subprocess, "run") as run:
                run.return_value = subprocess.CompletedProcess([], 0, valid)
                with contextlib.redirect_stdout(io.StringIO()):
                    guard.run_migration("a" * 40, inputs)
                self.assertEqual(run.call_args.args[0][1:], [guard.MIGRATION_TEST_NAME, "--ignored", "--exact", "--test-threads=1", "--nocapture"])
                child = run.call_args.kwargs["env"]
                for name, expected in (("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"), ("FVOCI_TEST_TURSO_PHASE", "migration"), ("FVOCI_TEST_TURSO_DESTRUCTIVE", "true"), ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "true")):
                    self.assertEqual(child[name], expected)
                self.assertNotIn("FVOCI_TEST_TURSO_CONNECTION_SELECTED", child)
                self.assertNotIn("UNRELATED_FAKE_CREDENTIAL", child)
                run.reset_mock()
                with mock.patch.dict(os.environ, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "false"}):
                    self.denied("DESTRUCTIVE_NOT_ALLOWED", guard.run_migration, "a" * 40, inputs)
                run.assert_not_called()
                native.write_bytes(b"changed input")
                self.denied("COMPILED_TEST_BINDING_FAILED", guard.run_migration, "a" * 40, inputs)
                run.assert_not_called()


class MigrationDiagnosticTests(unittest.TestCase):
    success = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK current=OK restart=OK close=OK leases=ZERO\nok\n"
               "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n")
    failure = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=FAILED prefix=NOT_CONFIRMED fk_rollback=NOT_CONFIRMED current=NOT_CONFIRMED restart=NOT_CONFIRMED close=OK leases=ZERO\nFAILED\n"
               "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out;\n")

    def failed_output(self, text, status=1):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaises(guard.AdmissionError) as caught:
                guard.migration_result(subprocess.CompletedProcess([], status, b""), text)
        self.assertEqual(str(caught.exception), "TURSO_MIGRATION_FAILED")
        self.assertNotIn("TURSO_MIGRATION_PASS", output.getvalue())
        self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        return output.getvalue()

    def test_exact_static_codes_only_and_failures_never_become_pass(self):
        # Contract extracted from the existing post-owner consumer failures.
        expected = {
            "BEGIN_FAILED",
            "CLOSE_FAILED",
            "COMMIT_UNCONFIRMED",
            "CURRENT_APPLY_FAILED",
            "CURRENT_GATE_FAILED",
            "CURRENT_GATE_MISMATCH",
            "CURRENT_LINEAGE_CHANGED",
            "DATA_DECODE_FAILED",
            "DATA_QUERY_FAILED",
            "DATA_WRITE_FAILED",
            "DATA_WRITE_MISMATCH",
            "DDL_FAILED",
            "FENCE_WRITE_FAILED",
            "FK_DECODE_FAILED",
            "FK_FAILURE_MISSING",
            "FK_QUERY_FAILED",
            "FK_ROLLBACK_PREFIX_CHANGED",
            "FOREIGN_KEYS_NOT_ONE",
            "GENERATION_WRITE_FAILED",
            "GENERATION_WRITE_MISMATCH",
            "INCOMPLETE_PREFIX_REFUSAL_NOT_CONFIRMED",
            "LEASES_NOT_ZERO",
            "LITERAL_DECODE_FAILED",
            "LITERAL_MISMATCH",
            "LITERAL_QUERY_FAILED",
            "NEGATIVE_REFUSAL_NOT_CONFIRMED",
            "NEGATIVE_ROLLBACK_CHANGED_CURRENT",
            "NEGATIVE_WRITE_FAILED",
            "PREFIX_APPLY_FAILED",
            "PREFIX_RECEIPTS_CHANGED",
            "PREFIX_VALIDATION_FAILED",
            "PRESERVED_DATA_MISMATCH",
            "RECONNECT_FAILED",
            "RESTART_APPLY_FAILED",
            "RESTART_RECEIPTS_OR_SCHEMA_CHANGED",
            "ROLLBACK_UNCONFIRMED",
            "SCHEMA_VALIDATION_FAILED",
            "SEED_DECODE_FAILED",
            "SEED_MISMATCH",
            "SEED_QUERY_FAILED",
            "UNEXPECTED_TARGET_DATA",
            "WRONG_BACKEND",
            "WRONG_FK_FAILURE",
        }
        self.assertEqual(guard.MIGRATION_PRIMARY_CODES, expected)
        self.assertEqual(guard.MIGRATION_CLOSE_CODES, {"CLOSE_FAILED", "LEASES_NOT_ZERO"})
        for primary in expected:
            for close in ("OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"):
                with self.subTest(primary=primary, close=close):
                    receipt = self.failure if close == "OK" else self.failure.replace("close=OK", "close=FAILED")
                    diagnostic = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close + "\n"
                    output = self.failed_output(receipt + diagnostic + "SDK FAKE_PRIVATE_TOKEN\n")
                    self.assertIn("TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close + "\n", output)
        for close in ("CLOSE_FAILED", "LEASES_NOT_ZERO"):
            output = self.failed_output(self.failure.replace("close=OK", "close=FAILED") +
                                        "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=OK close=" + close + "\n")
            self.assertIn("TURSO_MIGRATION_DIAGNOSTIC primary=OK close=" + close, output)

    def test_unknown_malformed_duplicate_or_injected_diagnostics_do_not_echo(self):
        known = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n"
        for diagnostic in (
            known + known,
            known + "FVOCI_TURSO_MIGRATION_DIAGNOSTIC FAKE_PRIVATE_TOKEN\n",
            known.replace("SCHEMA_VALIDATION_FAILED", "UNKNOWN_ERROR"),
            known.replace("SCHEMA_VALIDATION_FAILED", "FAKE_PRIVATE_TOKEN"),
            known.replace("SCHEMA_VALIDATION_FAILED", "SCHEMA_VALIDATION_FAILED_EXTRA"),
            known.replace("SCHEMA_VALIDATION_FAILED", "schema_validation_failed"),
            known.replace("SCHEMA_VALIDATION_FAILED", "CONNECT_FAILED"),
            known.replace("SCHEMA_VALIDATION_FAILED", "MISSING_SECRET"),
            known.replace("SCHEMA_VALIDATION_FAILED", ""),
            known.replace("SCHEMA_VALIDATION_FAILED", "SCHEMA_VALIDATION_FAILED\nFAKE_PRIVATE_TOKEN"),
            known.replace("SCHEMA_VALIDATION_FAILED", "SCHEMA_VALIDATION_FAILED\rFAKE_PRIVATE_TOKEN"),
            known.replace("SCHEMA_VALIDATION_FAILED", "libsql://FAKE_PRIVATE_TOKEN.example.org"),
            known.replace("close=OK", "close=BEGIN_FAILED"),
            known.replace("close=OK", "close=FAKE_PRIVATE_TOKEN"),
            known.replace("close=OK", "close=CLOSE_FAILED"),
            known.replace("SCHEMA_VALIDATION_FAILED", "OK"),
            "FAKE_PRIVATE_TOKEN " + known,
            known.rstrip("\n") + " FAKE_PRIVATE_TOKEN\n",
            known.replace("primary=", "private="),
            known.replace(" close=", "\tclose="),
        ):
            with self.subTest(diagnostic=diagnostic):
                output = self.failed_output(self.failure + diagnostic)
                self.assertNotIn("TURSO_MIGRATION_DIAGNOSTIC", output)
        # Missing diagnostic still fails; it is never fabricated from raw Err.
        self.assertNotIn("TURSO_MIGRATION_DIAGNOSTIC", self.failed_output(self.failure + "SDK FAKE_PRIVATE_TOKEN\n"))

    def test_diagnostic_cannot_replace_success_execution_or_cleanup_receipts(self):
        diagnostic = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n"
        for text, status in (
            (self.success + diagnostic, 0),
            (self.success + diagnostic, 1),
            (self.failure + diagnostic, 0),
            (self.failure.replace(guard.MIGRATION_TEST_NAME, guard.TEST_NAME) + diagnostic, 0),
            (self.failure.replace("0 ignored", "1 ignored") + diagnostic, 0),
            (self.failure.replace("1 failed", "0 failed") + diagnostic, 0),
            (self.failure.replace("close=OK", "close=FAILED") + diagnostic, 0),
        ):
            with self.subTest(text=text, status=status):
                self.failed_output(text, status)
        with contextlib.redirect_stdout(io.StringIO()) as output:
            guard.migration_result(subprocess.CompletedProcess([], 0, b""), self.success + "SDK FAKE_PRIVATE_TOKEN\n")
        self.assertIn("TURSO_MIGRATION_PASS tests=1 ignored=0", output.getvalue())
        self.assertNotIn("TURSO_MIGRATION_DIAGNOSTIC", output.getvalue())
        self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())


class DiagnosticUnitRegistrationTests(unittest.TestCase):
    listing = (guard.DIAGNOSTIC_UNIT_NAME + ": test\n\n1 test, 0 benchmarks\n").encode()
    success = ("running 1 test\ntest " + guard.DIAGNOSTIC_UNIT_NAME + " ... ok\n\n"
               "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n").encode()

    @contextlib.contextmanager
    def frozen(self):
        with tempfile.TemporaryDirectory(prefix="fvoci-diagnostic-unit-pure-") as directory:
            root = Path(directory)
            binary = root / "turso-connection-libtest"
            binary.write_bytes(b"\x7fELFpure fixture, never executed")
            native = root / "fvoci-sqlite" / "consumer-inputs.json"
            native.parent.mkdir()
            native.write_text("pure native-input binding, not a native producer")
            cargo = root / "turso-compile.json"
            cargo.write_text("pure retained compile-output binding")
            manifest = {"sha": "a" * 40, "source_digest": "fixture",
                        "binary_sha256": guard.file_digest(binary),
                        "native_input_sha256": guard.file_digest(native),
                        "cargo_output_sha256": guard.file_digest(cargo)}
            (root / "turso-connection-build.json").write_text(json.dumps(manifest))
            env = {"PATH": os.environ["PATH"], "RUNNER_TEMP": directory,
                   "FVOCI_LIBSQL_URL": "FAKE_PRIVATE_TOKEN", "FVOCI_LIBSQL_AUTH_TOKEN": "FAKE_PRIVATE_TOKEN",
                   "GITHUB_TOKEN": "FAKE_PRIVATE_TOKEN", "FVOCI_TEST_TURSO_MIGRATION_SELECTED": "1"}
            with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(guard, "source_digest", return_value="fixture"), mock.patch.object(guard.subprocess, "run") as run:
                run.side_effect = [subprocess.CompletedProcess([], 0, self.listing), subprocess.CompletedProcess([], 0, self.success)]
                yield root, manifest, run

    def denied(self, code, run):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaises(guard.AdmissionError) as caught:
                guard.run_diagnostic_unit("a" * 40)
        self.assertEqual(str(caught.exception), code)
        self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        self.assertNotIn("TURSO_DIAGNOSTIC_UNIT_PASS", output.getvalue())

    def test_exact_current_unit_runs_without_credentials_before_secret_step(self):
        with self.frozen() as (root, manifest, run):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                guard.run_diagnostic_unit("a" * 40)
            self.assertEqual(output.getvalue(), "TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN\n")
            self.assertEqual(run.call_count, 2)
            self.assertEqual(run.call_args_list[0].args[0], [str(root / "turso-connection-libtest"), guard.DIAGNOSTIC_UNIT_NAME, "--list", "--exact"])
            self.assertEqual(run.call_args_list[1].args[0], [str(root / "turso-connection-libtest"), guard.DIAGNOSTIC_UNIT_NAME, "--exact", "--test-threads=1"])
            for call in run.call_args_list:
                self.assertEqual(call.kwargs["env"], {"PATH": os.environ["PATH"]})
                self.assertEqual(call.kwargs["stdout"], subprocess.PIPE)
                self.assertEqual(call.kwargs["stderr"], subprocess.STDOUT)
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/turso-test.yml").read_text()
        freeze = workflow.index("turso-test-guard.py --freeze")
        unit = workflow.index("turso-test-guard.py --diagnostic-unit")
        secret = workflow.index("      - name: Real primary selected phase")
        self.assertLess(freeze, unit)
        self.assertLess(unit, secret)
        self.assertEqual(workflow.count("turso-test-guard.py --diagnostic-unit"), 1)
        self.assertNotIn("secrets.", workflow[freeze:secret])

    def test_explicit_unit_mode_routes_before_any_secret_consumer(self):
        for event, sha, expected in (("workflow_dispatch", "a" * 40, 0),
                                     ("push", "a" * 40, 78),
                                     ("workflow_dispatch", "b" * 40, 78)):
            with self.subTest(event=event, sha=sha), tempfile.TemporaryDirectory(prefix="fvoci-unit-mode-pure-") as directory:
                event_path = Path(directory) / "event.json"
                event_path.write_text(json.dumps({"inputs": {"phase": "connection", "destructive": "false"}}))
                env = {"GITHUB_EVENT_PATH": str(event_path), "GITHUB_EVENT_NAME": event,
                       "GITHUB_REPOSITORY": guard.REPOSITORY, "GITHUB_REF": guard.REVIEWED_REF,
                       "GITHUB_SHA": sha}
                with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(guard.sys, "argv", ["guard", "--diagnostic-unit"]), mock.patch.object(guard.subprocess, "check_output", return_value="a" * 40), mock.patch.object(guard, "run_diagnostic_unit") as unit, mock.patch.object(guard, "run_primary") as consumer, mock.patch.object(guard, "freeze_compiled_test") as freeze, mock.patch.object(guard, "environment_metadata") as metadata, contextlib.redirect_stdout(io.StringIO()) as output, contextlib.redirect_stderr(io.StringIO()) as error:
                    self.assertEqual(guard.main(), expected)
                consumer.assert_not_called()
                freeze.assert_not_called()
                metadata.assert_not_called()
                if expected == 0:
                    unit.assert_called_once_with("a" * 40)
                else:
                    unit.assert_not_called()
                self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue() + error.getvalue())

    def test_no_match_duplicate_wrong_unit_benchmark_or_list_failure_refuses(self):
        for listing, status in ((b"0 tests, 0 benchmarks\n", 0), (self.listing + self.listing, 0),
                                (self.listing.replace(guard.DIAGNOSTIC_UNIT_NAME.encode(), guard.TEST_NAME.encode()), 0),
                                (self.listing.replace(b": test", b": benchmark"), 0),
                                (self.listing + b"other::test: test\n", 0), (self.listing, 1)):
            with self.subTest(listing=listing, status=status), self.frozen() as (_, _, run):
                run.side_effect = [subprocess.CompletedProcess([], status, listing + b"FAKE_PRIVATE_TOKEN\n")]
                self.denied("TURSO_DIAGNOSTIC_UNIT_SELECTION_FAILED", run)
                self.assertEqual(run.call_count, 1)

    def test_wrong_source_binary_native_cargo_binding_refuses_before_run(self):
        for changed in ("sha", "source_digest", "binary_sha256", "native_input_sha256", "cargo_output_sha256"):
            with self.subTest(field=changed), self.frozen() as (root, manifest, run):
                manifest[changed] = "wrong"
                (root / "turso-connection-build.json").write_text(json.dumps(manifest))
                self.denied("COMPILED_TEST_BINDING_FAILED", run)
                run.assert_not_called()
        with self.frozen() as (root, manifest, run):
            binary = root / "turso-connection-libtest"
            binary.write_bytes(b"not ELF")
            manifest["binary_sha256"] = guard.file_digest(binary)
            (root / "turso-connection-build.json").write_text(json.dumps(manifest))
            self.denied("COMPILED_TEST_BINDING_FAILED", run)
            run.assert_not_called()
        with self.frozen() as (root, manifest, run):
            binary = root / "turso-connection-libtest"
            original = root / "fixture-original"
            binary.rename(original)
            binary.symlink_to(original)
            self.denied("COMPILED_TEST_BINDING_FAILED", run)
            run.assert_not_called()

    def test_source_and_binary_changes_during_children_cannot_make_pass(self):
        for stage in (1, 2):
            with self.subTest(stage=stage), self.frozen() as (root, _, run):
                def mutate(*args, **kwargs):
                    if run.call_count == stage:
                        (root / "turso-connection-libtest").write_bytes(b"\x7fELFchanged after start")
                    return subprocess.CompletedProcess([], 0, self.listing if run.call_count == 1 else self.success)
                run.side_effect = mutate
                self.denied("COMPILED_TEST_BINDING_FAILED", run)
                self.assertEqual(run.call_count, stage)
        with self.frozen() as (_, _, run), mock.patch.object(guard, "source_digest", side_effect=["fixture", "changed"]):
            self.denied("COMPILED_TEST_BINDING_FAILED", run)
            self.assertEqual(run.call_count, 1)

    def test_exact_one_real_result_and_status_required_without_raw_echo(self):
        for output, status in (
            (self.success, 1), (self.success.replace(b"1 passed", b"0 passed"), 0),
            (self.success.replace(b"0 failed", b"1 failed"), 0),
            (self.success.replace(b"0 ignored", b"1 ignored"), 0),
            (self.success.replace(guard.DIAGNOSTIC_UNIT_NAME.encode(), guard.TEST_NAME.encode()), 0),
            (self.success.replace(b" ... ok", b" ... ignored"), 0),
            (self.success + self.success, 0), (b"SDK FAKE_PRIVATE_TOKEN\n", 0),
        ):
            with self.subTest(output=output, status=status), self.frozen() as (_, _, run):
                run.side_effect = [subprocess.CompletedProcess([], 0, self.listing), subprocess.CompletedProcess([], status, output + b"FAKE_PRIVATE_TOKEN\n")]
                self.denied("TURSO_DIAGNOSTIC_UNIT_FAILED", run)
                self.assertEqual(run.call_count, 2)
        with self.frozen() as (_, _, run):
            run.side_effect = [subprocess.CompletedProcess([], 0, self.listing + b"FAKE_PRIVATE_TOKEN\n"), subprocess.CompletedProcess([], 0, self.success + b"FAKE_PRIVATE_TOKEN\n")]
            with contextlib.redirect_stdout(io.StringIO()) as output:
                guard.run_diagnostic_unit("a" * 40)
            self.assertEqual(output.getvalue(), "TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN\n")


class MigrationDiagnosticSeparatorTests(unittest.TestCase):
    def test_only_lf_and_single_crlf_delimit_diagnostic_lines(self):
        receipt = MigrationDiagnosticTests.failure
        diagnostic = "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK"
        for separator in ("\r", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"):
            with self.subTest(separator=hex(ord(separator))):
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    with self.assertRaises(guard.AdmissionError) as caught:
                        guard.migration_result(subprocess.CompletedProcess([], 1, b""), receipt + diagnostic + separator + "FAKE_PRIVATE_TOKEN\n")
                self.assertEqual(str(caught.exception), "TURSO_MIGRATION_FAILED")
                self.assertNotIn("TURSO_MIGRATION_DIAGNOSTIC", output.getvalue())
                self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        for ending in ("\n", "\r\n"):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                with self.assertRaises(guard.AdmissionError):
                    guard.migration_result(subprocess.CompletedProcess([], 1, b""), receipt + diagnostic + ending)
            self.assertIn("TURSO_MIGRATION_DIAGNOSTIC primary=SCHEMA_VALIDATION_FAILED close=OK\n", output.getvalue())
        for ending in ("\r", "\r\r\n"):
            with contextlib.redirect_stdout(io.StringIO()) as output:
                with self.assertRaises(guard.AdmissionError):
                    guard.migration_result(subprocess.CompletedProcess([], 1, b""), receipt + diagnostic + ending)
            self.assertNotIn("TURSO_MIGRATION_DIAGNOSTIC", output.getvalue())


if __name__ == "__main__":
    unittest.main(verbosity=2)
