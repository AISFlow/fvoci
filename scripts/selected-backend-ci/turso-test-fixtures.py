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
        for phase in (phase for phase in guard.PHASES if phase not in ("connection", "inventory")):
            self.denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", guard.validate_dispatch, self.context, dict(self.inputs, phase=phase), "a" * 40)

    def test_inventory_readonly_admission_is_exact_and_other_phases_stay_destructive(self):
        inventory = {"phase": "inventory", "destructive": False}
        original = copy.deepcopy((inventory, self.settings, self.secrets))
        self.assertEqual(guard.validate_dispatch(self.context, inventory, "a" * 40), "inventory")
        self.assertEqual(guard.validate_target(inventory, self.settings, self.secrets), "inventory")
        self.assertEqual((inventory, self.settings, self.secrets), original)
        for value in ("false", "true", 0, 1, None):
            self.denied("INVALID_BOOLEAN", guard.validate_dispatch, self.context,
                        dict(inventory, destructive=value), "a" * 40)
            self.denied("INVALID_BOOLEAN", guard.validate_target,
                        dict(inventory, destructive=value), self.settings, self.secrets)
        for phase in guard.PHASES:
            if phase in ("connection", "inventory"):
                continue
            inputs = {"phase": phase, "destructive": False}
            self.denied("DESTRUCTIVE_CONFIRMATION_REQUIRED", guard.validate_dispatch,
                        self.context, inputs, "a" * 40)
            for flag, allow in ((False, "false"), (False, "true"), (True, "false"), (True, "")):
                self.denied("DESTRUCTIVE_NOT_ALLOWED", guard.validate_target,
                            dict(inputs, destructive=flag),
                            {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": allow}, self.secrets)
            confirmed = dict(inputs, destructive=True)
            self.assertEqual(guard.validate_dispatch(self.context, confirmed, "a" * 40), phase)
            self.assertEqual(guard.validate_target(confirmed,
                             {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true"}, self.secrets), phase)
            if phase not in ("migration", "reset"):
                self.denied("NOT_IMPLEMENTED", guard.require_implemented, phase)

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
        valid = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\n"
                 "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n")
        success = subprocess.CompletedProcess([], 0, b"")
        with contextlib.redirect_stdout(io.StringIO()) as output:
            guard.migration_result(success, valid)
        self.assertIn("TURSO_MIGRATION_FK_PROOF kind=EXTENDED\n", output.getvalue())
        self.assertIn("TURSO_MIGRATION_PASS tests=1 ignored=0", output.getvalue())
        # The same-writer proof kind is the only other confirmed kind; it is echoed closed.
        with contextlib.redirect_stdout(io.StringIO()) as output:
            guard.migration_result(success, valid.replace("fk_proof=EXTENDED", "fk_proof=SAME_WRITER_PRIMARY_HRANA"))
        self.assertIn("TURSO_MIGRATION_FK_PROOF kind=SAME_WRITER_PRIMARY_HRANA\n", output.getvalue())
        self.assertIn("TURSO_MIGRATION_PASS tests=1 ignored=0", output.getvalue())
        for changed, code in (
            (valid.replace("1 passed", "0 passed"), "TURSO_MIGRATION_FAILED"),
            # Proof field: NOT_CONFIRMED never passes; missing, extra, unknown, generic,
            # lower-case or injected kinds are not a receipt at all.
            (valid.replace("fk_proof=EXTENDED", "fk_proof=NOT_CONFIRMED"), "TURSO_MIGRATION_FAILED"),
            (valid.replace(" fk_proof=EXTENDED", ""), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid.replace("fk_proof=EXTENDED", "fk_proof=EXTENDED witness=PROVEN"), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid.replace("fk_proof=EXTENDED", "fk_proof=GENERIC_19"), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid.replace("fk_proof=EXTENDED", "fk_proof=SQLITE_CONSTRAINT"), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid.replace("fk_proof=EXTENDED", "fk_proof=extended"), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid.replace("fk_proof=EXTENDED", "fk_proof=EXTENDED_FAKE_PRIVATE_TOKEN"), "TURSO_MIGRATION_RECEIPT_MISSING"),
            (valid.replace("fk_proof=EXTENDED", "fk_proof="), "TURSO_MIGRATION_RECEIPT_MISSING"),
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
            valid = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\n"
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
    success = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=OK prefix=OK fk_rollback=OK fk_proof=EXTENDED current=OK restart=OK close=OK leases=ZERO\nok\n"
               "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out;\n")
    failure = ("test " + guard.MIGRATION_TEST_NAME + " ... FVOCI_TURSO_MIGRATION_RECEIPT primary=FAILED prefix=NOT_CONFIRMED fk_rollback=NOT_CONFIRMED fk_proof=NOT_CONFIRMED current=NOT_CONFIRMED restart=NOT_CONFIRMED close=OK leases=ZERO\nFAILED\n"
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
            "DEFER_PRAGMA_REFUSED",
            "FENCE_BASELINE_NOT_EMPTY",
            "FENCE_ROW_UNBOUND",
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
            "NOT_FK_ONLY",
            "PARENT_PRESENT",
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
            "WITNESS_DECODE_FAILED",
            "WITNESS_MISMATCH",
            "WITNESS_QUERY_FAILED",
            "WRONG_BACKEND",
            "WRONG_FK_FAILURE",
        }
        self.assertEqual(guard.MIGRATION_PRIMARY_CODES, expected)
        self.assertEqual(guard.MIGRATION_CLOSE_CODES, {"CLOSE_FAILED", "LEASES_NOT_ZERO"})
        self.assertEqual(guard.MIGRATION_FK_PROOF_KINDS, {"EXTENDED", "SAME_WRITER_PRIMARY_HRANA"})
        # A failed receipt never carries a confirmed proof kind into a pass.
        for kind in ("EXTENDED", "SAME_WRITER_PRIMARY_HRANA"):
            output = self.failed_output(self.failure.replace("fk_proof=NOT_CONFIRMED", "fk_proof=" + kind))
            self.assertNotIn("TURSO_MIGRATION_FK_PROOF", output)
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


class InventoryTests(unittest.TestCase):
    hash = "a" * 64
    inputs = {"phase": "inventory", "destructive": False}

    def success(self, classification="CURRENT", prefix=12):
        return ("\nrunning 1 test\ntest " + guard.INVENTORY_TEST_NAME
                + " ... FVOCI_TURSO_INVENTORY_RECEIPT classification=" + classification
                + " prefix=" + str(prefix) + " schema_sha256=" + self.hash
                + " rollback=OK close=OK leases=ZERO\nok\n\n"
                "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n")

    def denied_result(self, text, status=0):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaises(guard.AdmissionError) as error:
                guard.inventory_result(subprocess.CompletedProcess([], status, b""), text)
        self.assertEqual(str(error.exception), "TURSO_INVENTORY_FAILED")
        self.assertEqual(output.getvalue(), "")

    @contextlib.contextmanager
    def frozen(self):
        # Reuse the existing file-only freeze fixture. No native producer or
        # ELF/SDK is executed; the sole runtime invocation is always mocked.
        with DiagnosticUnitRegistrationTests().frozen() as (root, manifest, run):
            env = {
                "FVOCI_DATABASE_BACKEND": "libsql-remote",
                "FVOCI_LIBSQL_URL": "libsql://isolated-owner.aws-us-east-1.turso.io",
                "FVOCI_LIBSQL_AUTH_TOKEN": "FAKE_PRIVATE_TOKEN",
                "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "false",
                "LD_LIBRARY_PATH": "/fixture/lib",
                "SSL_CERT_FILE": "/fixture/cert",
                "SSL_CERT_DIR": "/fixture/certs",
                "TZ": "UTC",
                "FVOCI_TEST_TURSO_CONNECTION_SELECTED": "1",
                "FVOCI_DATABASE_APP_URL": "FAKE_PRIVATE_TOKEN",
                "UNRELATED_FAKE_CREDENTIAL": "FAKE_PRIVATE_TOKEN",
            }
            with mock.patch.dict(os.environ, env):
                run.side_effect = None
                run.return_value = subprocess.CompletedProcess([], 0, self.success().encode())
                yield root, manifest, run

    def denied_run(self, code, inputs=None):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaises(guard.AdmissionError) as error:
                guard.run_inventory("a" * 40, self.inputs if inputs is None else inputs)
        self.assertEqual(str(error.exception), code)
        self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        self.assertNotIn("TURSO_INVENTORY_PASS", output.getvalue())

    def test_exact_class_prefix_receipt_and_nocapture_framing(self):
        for prefix in range(13):
            classification = "BLANK" if prefix == 0 else "CURRENT" if prefix == 12 else "PREFIX"
            for ending in ("\n", "\r\n"):
                with self.subTest(prefix=prefix, ending=ending), contextlib.redirect_stdout(io.StringIO()) as output:
                    guard.inventory_result(subprocess.CompletedProcess([], 0, b""), self.success(classification, prefix).replace("\n", ending))
                self.assertEqual(output.getvalue(),
                                 "TURSO_INVENTORY_RECEIPT classification=" + classification
                                 + " prefix=" + str(prefix) + " schema_sha256=" + self.hash
                                 + " rollback=OK close=OK leases=ZERO\nTURSO_INVENTORY_PASS tests=1 ignored=0\n")

    def test_missing_duplicate_extra_wrong_test_count_or_partial_summary_refuses(self):
        valid = self.success()
        receipt = valid.split(" ... ", 1)[1].split("\n", 1)[0]
        for changed in (
            "", receipt + "\n", valid[valid.index("test result:"):],
            valid.replace(receipt, ""), valid.replace(receipt, receipt + "\n" + receipt),
            valid + valid, valid.replace(guard.INVENTORY_TEST_NAME, guard.MIGRATION_TEST_NAME),
            valid.replace("running 1 test", "running 0 tests"),
            valid.replace("running 1 test", "running 2 tests"),
            valid.replace("1 passed", "0 passed"), valid.replace("1 passed", "2 passed"),
            valid.replace("0 failed", "1 failed"), valid.replace("0 ignored", "1 ignored"),
            valid.replace("0 measured", "1 measured"), valid.replace("100 filtered out", "100 filtered"),
            valid.replace(" finished in 0.00s", ""), valid.replace("\nok\n", "\nignored\n"),
            valid.replace("\nok\n", "\nFAILED\n"), valid.replace("running 1 test\n", ""),
            valid.replace("\nok\n", "\ntest other::case ... ok\nok\n"),
            valid + "test other::case ... ignored\n",
            valid.replace("test result: ok.", "test result: FAILED."),
            valid + "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s\n",
            "FAKE_PRIVATE_TOKEN " + valid, valid + "SDK FAKE_PRIVATE_TOKEN\n",
            valid.replace(" ... ", " ... FAKE_PRIVATE_TOKEN "),
            valid.replace("\nok\n", "\nSDK FAKE_PRIVATE_TOKEN\nok\n"),
            valid.replace("\n", "\r"), valid.replace("\n", "\u2028"),
        ):
            with self.subTest(changed=changed):
                self.denied_result(changed)

    def test_class_prefix_hash_and_each_cleanup_field_are_not_summary_oracles(self):
        valid = self.success()
        for changed in (
            valid.replace("CURRENT", "UNKNOWN"), valid.replace("CURRENT", "current"),
            valid.replace("CURRENT", "BLANK"), valid.replace("CURRENT", "PREFIX"),
            valid.replace("prefix=12", "prefix=0"), valid.replace("prefix=12", "prefix=11"),
            valid.replace("prefix=12", "prefix=13"), valid.replace("prefix=12", "prefix=012"),
            valid.replace("prefix=12", "prefix=-1"), valid.replace("prefix=12", "prefix=NONE"),
            valid.replace(self.hash, "a" * 63), valid.replace(self.hash, "a" * 65),
            valid.replace(self.hash, "A" * 64), valid.replace(self.hash, "g" * 64),
            valid.replace(self.hash, "libsql://FAKE_PRIVATE_TOKEN"),
            valid.replace(self.hash, "FAKE_PRIVATE_TOKEN\n" + self.hash),
            valid.replace("rollback=OK", "rollback=FAILED"),
            valid.replace("rollback=OK", "rollback=NOT_STARTED"),
            valid.replace("close=OK", "close=FAILED"), valid.replace("close=OK", "close=NOT_STARTED"),
            valid.replace("leases=ZERO", "leases=FAILED"), valid.replace("leases=ZERO", "leases=NOT_OBSERVED"),
            valid.replace("FVOCI_TURSO_INVENTORY_RECEIPT", "FVOCI_TURSO_MIGRATION_RECEIPT"),
            valid.replace("leases=ZERO", "leases=ZERO FAKE_PRIVATE_TOKEN"),
            valid.replace(" schema_sha256=", "\tschema_sha256="),
            valid.replace("CURRENT prefix=12 schema_sha256=" + self.hash, "REFUSED prefix=NONE schema_sha256=NONE"),
            self.success("BLANK", 1), self.success("PREFIX", 0), self.success("PREFIX", 12),
        ):
            with self.subTest(changed=changed):
                self.denied_result(changed)
        self.denied_result(valid, 1)
        self.denied_result(valid, -9)
        for separator in ("\r", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"):
            self.denied_result(valid.replace("leases=ZERO\n", "leases=ZERO" + separator + "FAKE_PRIVATE_TOKEN\n"))

    def test_single_exact_inventory_child_has_only_maintained_environment(self):
        with self.frozen() as (root, _, run), contextlib.redirect_stdout(io.StringIO()) as output:
            guard.run_inventory("a" * 40, self.inputs)
            self.assertIn("TURSO_INVENTORY_PASS tests=1 ignored=0", output.getvalue())
            run.assert_called_once()
            self.assertEqual(run.call_args.args[0], [str(root / "turso-connection-libtest"), guard.INVENTORY_TEST_NAME,
                                                     "--ignored", "--exact", "--test-threads=1", "--nocapture"])
            expected = {key: os.environ[key] for key in ("PATH", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ")}
            expected.update({"FVOCI_DATABASE_BACKEND": "libsql-remote", "FVOCI_LIBSQL_URL": os.environ["FVOCI_LIBSQL_URL"],
                             "FVOCI_LIBSQL_AUTH_TOKEN": "FAKE_PRIVATE_TOKEN", "FVOCI_TEST_TURSO_MIGRATION_SELECTED": "1",
                             "FVOCI_TEST_TURSO_PHASE": "inventory", "FVOCI_TEST_TURSO_DESTRUCTIVE": "false",
                             "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "false"})
            self.assertEqual(run.call_args.kwargs, {"env": expected, "stdout": subprocess.PIPE, "stderr": subprocess.STDOUT, "check": False})
            self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())

    def test_false_flags_backend_and_wrong_phase_refuse_before_child(self):
        with self.frozen() as (_, _, run):
            for value in ("true", "FALSE", "", "FAKE_PRIVATE_TOKEN"):
                with mock.patch.dict(os.environ, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": value}):
                    self.denied_run("INVENTORY_MUST_BE_READ_ONLY")
                run.assert_not_called()
            self.denied_run("INVENTORY_MUST_BE_READ_ONLY", dict(self.inputs, destructive=True))
            for value in ("true", 1, None):
                self.denied_run("INVALID_BOOLEAN", dict(self.inputs, destructive=value))
            with mock.patch.dict(os.environ, {"FVOCI_DATABASE_BACKEND": "sqlite"}):
                self.denied_run("BACKEND_SELECTOR_REQUIRED")
            for phase in ("migration", "connection", "unknown"):
                self.denied_run("WRONG_CONSUMER_PHASE", dict(self.inputs, phase=phase))
            run.assert_not_called()

    def test_inventory_mixed_or_missing_allow_gate_refuses_before_binding_or_child(self):
        for destructive in (False, True):
            for allow in (None, "false", "true", "FALSE", "", 0, False):
                if destructive is False and allow == "false":
                    continue  # The positive exact tuple is exercised by the real wrapper fixture.
                with self.subTest(destructive=destructive, allow=allow), self.frozen() as (_, _, run):
                    with mock.patch.dict(os.environ, {}):
                        if allow is None:
                            os.environ.pop("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE")
                        else:
                            os.environ["FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE"] = str(allow)
                        with mock.patch.object(guard, "diagnostic_unit_binding") as binding:
                            self.denied_run("INVENTORY_MUST_BE_READ_ONLY",
                                            dict(self.inputs, destructive=destructive))
                            binding.assert_not_called()
                    run.assert_not_called()

    def test_inventory_child_does_not_inherit_ambient_migration_authority(self):
        with self.frozen() as (_, _, run):
            with mock.patch.dict(os.environ, {
                "FVOCI_TEST_TURSO_MIGRATION_SELECTED": "0",
                "FVOCI_TEST_TURSO_PHASE": "migration",
                "FVOCI_TEST_TURSO_DESTRUCTIVE": "true",
            }):
                before = dict(os.environ)
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    guard.run_inventory("a" * 40, self.inputs)
                run.assert_called_once()
                child = run.call_args.kwargs["env"]
                for name, expected in (("FVOCI_TEST_TURSO_MIGRATION_SELECTED", "1"),
                                       ("FVOCI_TEST_TURSO_PHASE", "inventory"),
                                       ("FVOCI_TEST_TURSO_DESTRUCTIVE", "false"),
                                       ("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "false")):
                    self.assertEqual(child[name], expected)
                self.assertEqual(dict(os.environ), before)
                self.assertNotIn("FVOCI_TEST_TURSO_CONNECTION_SELECTED", child)
                self.assertNotIn("FVOCI_DATABASE_APP_URL", child)
                self.assertNotIn("UNRELATED_FAKE_CREDENTIAL", child)
                self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())

    def test_complete_frozen_binding_and_elf_controls_before_inventory(self):
        for key in ("sha", "source_digest", "binary_sha256", "native_input_sha256", "cargo_output_sha256"):
            with self.subTest(key=key), self.frozen() as (root, manifest, run):
                manifest[key] = "FAKE_PRIVATE_TOKEN"
                (root / "turso-connection-build.json").write_text(json.dumps(manifest))
                self.denied_run("COMPILED_TEST_BINDING_FAILED")
                run.assert_not_called()
        for mutation in ("elf", "symlink"):
            with self.subTest(mutation=mutation), self.frozen() as (root, manifest, run):
                binary = root / "turso-connection-libtest"
                if mutation == "elf":
                    binary.write_bytes(b"not ELF FAKE_PRIVATE_TOKEN")
                    manifest["binary_sha256"] = guard.file_digest(binary)
                    (root / "turso-connection-build.json").write_text(json.dumps(manifest))
                else:
                    original = root / "fixture-original"
                    binary.rename(original)
                    binary.symlink_to(original)
                self.denied_run("COMPILED_TEST_BINDING_FAILED")
                run.assert_not_called()

    def test_binding_mutation_or_failed_execution_never_emits_inventory_pass(self):
        for mutation in ("source", "binary", "native", "cargo", "manifest"):
            with self.subTest(mutation=mutation), self.frozen() as (root, _, run):
                def change(*args, **kwargs):
                    if mutation == "source":
                        guard.source_digest.return_value = "changed"
                    elif mutation == "binary":
                        (root / "turso-connection-libtest").write_bytes(b"\x7fELFchanged")
                    elif mutation == "native":
                        (root / "fvoci-sqlite/consumer-inputs.json").write_text("changed")
                    elif mutation == "cargo":
                        (root / "turso-compile.json").write_text("changed")
                    else:
                        with (root / "turso-connection-build.json").open("a") as stream:
                            stream.write(" ")  # semantic JSON unchanged, exact receipt changed
                    return subprocess.CompletedProcess([], 0, self.success().encode())
                run.side_effect = change
                self.denied_run("COMPILED_TEST_BINDING_FAILED")
                run.assert_called_once()
        with self.frozen() as (_, _, run):
            run.return_value = subprocess.CompletedProcess([], 1, self.success().encode() + b"FAKE_PRIVATE_TOKEN")
            self.denied_run("TURSO_INVENTORY_FAILED")
            run.assert_called_once()

    def test_inventory_manual_dispatch_routes_without_untrusted_or_false_escape(self):
        context = {"event_name": "workflow_dispatch", "repository": guard.REPOSITORY, "ref": "refs/heads/main", "sha": "a" * 40}
        self.assertIsNone(guard.require_implemented("inventory"))
        for ref in ("refs/heads/main", guard.REVIEWED_REF):
            self.assertEqual(guard.validate_dispatch(dict(context, ref=ref), self.inputs, "a" * 40), "inventory")
        for changed, inputs, code in (
            (dict(context, repository="attacker/fvoci"), self.inputs, "UNTRUSTED_DISPATCH"),
            (dict(context, event_name="pull_request"), self.inputs, "UNTRUSTED_DISPATCH"),
            (dict(context, event_name="pull_request_target"), self.inputs, "UNTRUSTED_DISPATCH"),
            (dict(context, ref="refs/heads/topic"), self.inputs, "UNTRUSTED_DISPATCH"),
            (dict(context, event_name="push", ref=guard.REVIEWED_REF), self.inputs, "SECRET_MODE_REQUIRES_MANUAL"),
            (dict(context, sha="b" * 40), self.inputs, "CHECKOUT_MISMATCH"),
            (context, dict(self.inputs, destructive=True), "INVENTORY_MUST_BE_READ_ONLY"),
        ):
            with self.assertRaises(guard.AdmissionError) as error:
                guard.validate_dispatch(changed, inputs, "a" * 40)
            self.assertEqual(str(error.exception), code)
        for event, repository, ref, sha, destructive, expected in (
            ("workflow_dispatch", guard.REPOSITORY, "refs/heads/main", "a" * 40, "false", 0),
            ("workflow_dispatch", guard.REPOSITORY, guard.REVIEWED_REF, "a" * 40, "false", 0),
            ("push", guard.REPOSITORY, guard.REVIEWED_REF, "a" * 40, "false", 78),
            ("pull_request", guard.REPOSITORY, "refs/heads/main", "a" * 40, "false", 78),
            ("workflow_dispatch", "attacker/fvoci", "refs/heads/main", "a" * 40, "false", 78),
            ("workflow_dispatch", guard.REPOSITORY, "refs/heads/topic", "a" * 40, "false", 78),
            ("workflow_dispatch", guard.REPOSITORY, "refs/heads/main", "b" * 40, "false", 78),
            ("workflow_dispatch", guard.REPOSITORY, "refs/heads/main", "a" * 40, "true", 78),
        ):
            with tempfile.TemporaryDirectory(prefix="fvoci-inventory-route-pure-") as directory:
                event_path = Path(directory) / "event.json"
                event_path.write_text(json.dumps({"inputs": {"phase": "inventory", "destructive": destructive}}))
                env = {"GITHUB_EVENT_PATH": str(event_path), "GITHUB_EVENT_NAME": event,
                       "GITHUB_REPOSITORY": repository, "GITHUB_REF": ref, "GITHUB_SHA": sha}
                with mock.patch.dict(os.environ, env, clear=True), mock.patch.object(guard.sys, "argv", ["guard", "--consume"]), mock.patch.object(guard.subprocess, "check_output", return_value="a" * 40), mock.patch.object(guard, "run_inventory") as inventory, mock.patch.object(guard, "run_migration") as migration, mock.patch.object(guard, "run_connection") as connection, mock.patch.object(guard, "environment_metadata") as metadata, contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
                    self.assertEqual(guard.main(), expected)
                metadata.assert_not_called()
                migration.assert_not_called()
                connection.assert_not_called()
                if expected == 0:
                    inventory.assert_called_once_with("a" * 40, self.inputs)
                else:
                    inventory.assert_not_called()

    def test_workflow_retains_false_default_trust_serialization_and_presecret_pipeline(self):
        workflow = (Path(__file__).resolve().parents[2] / ".github/workflows/turso-test.yml").read_text()
        self.assertIn("options: [connection, crud, transactions, migration, inventory, reset, persistence, restore, ui-ack]", workflow)
        self.assertIn("        default: connection\n", workflow)
        self.assertIn("        type: boolean\n        default: false\n", workflow)
        self.assertIn("  group: fvoci-turso-test-database\n  cancel-in-progress: false\n", workflow)
        self.assertIn("permissions:\n  contents: read\n", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 2)
        self.assertEqual(workflow.count("ref: ${{ github.sha }}"), 2)
        self.assertEqual(workflow.count("github.repository == 'AISFlow/fvoci'"), 2)
        self.assertEqual(workflow.count("refs/heads/fvoci/v060-turso-verified-connection"), 3)
        self.assertEqual(workflow.count("timeout-minutes: 5"), 1)
        self.assertEqual(workflow.count("timeout-minutes: 15"), 1)
        self.assertEqual(workflow.count("environment: fvoci-turso-test"), 1)
        self.assertLess(workflow.index("--freeze"), workflow.index("--diagnostic-unit"))
        self.assertLess(workflow.index("--diagnostic-unit"), workflow.index("      - name: Real primary selected phase"))
        self.assertNotIn("secrets.", workflow[:workflow.index("      - name: Real primary selected phase")])


class InventoryFailureTests(unittest.TestCase):
    primary_codes = {
        "BEGIN_FAILED", "WRONG_PRODUCT_BACKEND", "WRONG_BACKEND", "FK_QUERY_FAILED",
        "FK_DECODE_FAILED", "FOREIGN_KEYS_NOT_ONE", "LITERAL_QUERY_FAILED",
        "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH", "CURRENT_LINEAGE_CHANGED",
        "INVENTORY_QUERY_FAILED", "INVENTORY_DECODE_FAILED", "INVENTORY_PREFIX_REFUSED",
        "INVENTORY_SCHEMA_REFUSED", "INVENTORY_SNAPSHOT_MISMATCH", "INVENTORY_HASH_INVALID",
    }

    def failure(self, primary="INVENTORY_SCHEMA_REFUSED", rollback="OK", close="OK", leases="ZERO", harness=None):
        rb = {"OK": "OK", "NOT_STARTED": "NOT_STARTED", "ROLLBACK_UNCONFIRMED": "FAILED"}[rollback]
        if harness is None:
            # Illustrative libtest returned Error, never the cause oracle. Use
            # each original priority-selected code so *_FAILED stays opaque.
            returned = primary if primary != "OK" else rollback if rollback != "OK" else close if close != "OK" else "LEASES_NOT_ZERO" if leases == "FAILED" else "INVENTORY_DISCLOSURE_REFUSED"
            harness = 'Error: "' + returned + '"\n'
        return ("\nrunning 1 test\ntest " + guard.INVENTORY_TEST_NAME
                + " ... FVOCI_TURSO_INVENTORY_RECEIPT classification=REFUSED prefix=NONE schema_sha256=NONE"
                + " rollback=" + rb + " close=" + ("OK" if close == "OK" else "FAILED") + " leases=" + leases
                + "\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC primary=" + primary + " rollback=" + rollback
                + " close=" + close + " leases=" + leases + "\nFVOCI_TURSO_INVENTORY_RETURN\n"
                + harness + "FAILED\n\nfailures:\n\nfailures:\n    " + guard.INVENTORY_TEST_NAME
                + "\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n")

    def failed_output(self, text, status=1):
        with contextlib.redirect_stdout(io.StringIO()) as output:
            with self.assertRaises(guard.AdmissionError) as caught:
                guard.inventory_result(subprocess.CompletedProcess([], status, b""), text)
        self.assertEqual(str(caught.exception), "TURSO_INVENTORY_FAILED")
        self.assertNotIn("TURSO_INVENTORY_PASS", output.getvalue())
        self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        self.assertNotIn("opaque returned error", output.getvalue())
        return output.getvalue()

    def test_current_codes_and_exact_refused_settled_cleanup_disclose_only_failure(self):
        self.assertEqual(guard.INVENTORY_PRIMARY_CODES, self.primary_codes)
        for code in sorted(self.primary_codes):
            rollbacks = ("NOT_STARTED",) if code in ("BEGIN_FAILED", "WRONG_PRODUCT_BACKEND") else ("OK", "ROLLBACK_UNCONFIRMED")
            for rollback in rollbacks:
                for close in ("OK", "CLOSE_FAILED", "LEASES_NOT_ZERO"):
                    for leases in ("ZERO", "FAILED"):
                        for ending in ("\n", "\r\n"):
                            output = self.failed_output(self.failure(code, rollback, close, leases).replace("\n", ending))
                            self.assertIn("TURSO_INVENTORY_DIAGNOSTIC primary=" + code + " rollback=" + rollback
                                          + " close=" + close + " leases=" + leases + "\n", output)
                            self.assertIn("classification=REFUSED prefix=NONE schema_sha256=NONE", output)

    def test_primary_ok_records_only_real_finish_or_local_lease_failure_not_all_healthy(self):
        for rollback, close, leases in (
            ("ROLLBACK_UNCONFIRMED", "OK", "ZERO"), ("ROLLBACK_UNCONFIRMED", "CLOSE_FAILED", "FAILED"),
            ("OK", "CLOSE_FAILED", "ZERO"), ("OK", "LEASES_NOT_ZERO", "FAILED"), ("OK", "OK", "FAILED"),
        ):
            output = self.failed_output(self.failure("OK", rollback, close, leases))
            self.assertIn("primary=OK rollback=" + rollback + " close=" + close + " leases=" + leases, output)
        for primary, rollback in (("OK", "OK"), ("OK", "NOT_STARTED"),
                                  ("BEGIN_FAILED", "OK"), ("INVENTORY_QUERY_FAILED", "NOT_STARTED")):
            self.assertEqual(self.failed_output(self.failure(primary, rollback)), "")

    def test_unknown_private_malformed_and_duplicate_codes_keep_cause_unknown(self):
        valid = self.failure()
        diagnostic = valid.split("\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC", 1)[1].split("\n", 1)[0]
        for code in ("UNKNOWN", "CONNECT_FAILED", "DDL_FAILED", "COMMIT_UNCONFIRMED", "SCHEMA_VALIDATION_FAILED", "",
                     "inventory_schema_refused", "INVENTORY_SCHEMA_REFUSED_EXTRA", "FAKE_PRIVATE_TOKEN", "libsql://FAKE_PRIVATE_TOKEN"):
            self.assertEqual(self.failed_output(valid.replace("primary=INVENTORY_SCHEMA_REFUSED", "primary=" + code)), "")
        for changed in (
            valid.replace("primary=INVENTORY_SCHEMA_REFUSED", "primary=INVENTORY_SCHEMA_REFUSED\nFAKE_PRIVATE_TOKEN"),
            valid.replace("rollback=OK close=OK", "rollback=UNKNOWN close=OK"),
            valid.replace("close=OK leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN", "close=BEGIN_FAILED leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN"),
            valid.replace("FVOCI_TURSO_INVENTORY_DIAGNOSTIC" + diagnostic, "FVOCI_TURSO_INVENTORY_DIAGNOSTIC" + diagnostic + "\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC" + diagnostic),
            valid.replace("FVOCI_TURSO_INVENTORY_RETURN", "FVOCI_TURSO_INVENTORY_RETURN\nFVOCI_TURSO_INVENTORY_RETURN"),
            valid.replace("\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC", "\n\nFAKE_PRIVATE_TOKEN FVOCI_TURSO_INVENTORY_DIAGNOSTIC"),
            valid.replace("\nFVOCI_TURSO_INVENTORY_RETURN", " FAKE_PRIVATE_TOKEN\nFVOCI_TURSO_INVENTORY_RETURN"),
            valid.replace("primary=INVENTORY_SCHEMA_REFUSED", "private=INVENTORY_SCHEMA_REFUSED"),
            valid.replace("\n\nFVOCI_TURSO_INVENTORY_DIAGNOSTIC" + diagnostic + "\n", "\n"),
            valid.replace("FVOCI_TURSO_INVENTORY_RETURN", "FAKE_RETURN"),
        ):
            self.assertEqual(self.failed_output(changed), "")

    def test_failure_cannot_forge_pass_counts_status_wrong_case_or_cleanup_tuple(self):
        valid = self.failure()
        for changed, status in (
            (valid, 0), (valid.replace("0 passed", "1 passed"), 1),
            (valid.replace("1 failed", "0 failed"), 1), (valid.replace("0 ignored", "1 ignored"), 1),
            (valid.replace("0 measured", "1 measured"), 1), (valid.replace("running 1 test", "running 2 tests"), 1),
            (valid.replace(guard.INVENTORY_TEST_NAME, guard.MIGRATION_TEST_NAME), 1),
            (valid.replace("classification=REFUSED", "classification=CURRENT"), 1),
            (valid.replace("prefix=NONE", "prefix=12"), 1), (valid.replace("schema_sha256=NONE", "schema_sha256=" + "a" * 64), 1),
            (valid.replace("rollback=OK close=OK leases=ZERO\n\n", "rollback=FAILED close=OK leases=ZERO\n\n"), 1),
            (valid.replace("rollback=OK close=OK leases=ZERO\n\n", "rollback=OK close=FAILED leases=ZERO\n\n"), 1),
            (valid.replace("close=OK leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN", "close=OK leases=FAILED\nFVOCI_TURSO_INVENTORY_RETURN"), 1),
            (valid + valid, 1), (valid[valid.index("test result:"):], 1),
            (valid.replace("test result: FAILED.", "test result: ok."), 1),
            (valid.replace("\nFAILED\n", "\nok\n"), 1),
            (InventoryTests().success() + valid, 1), (valid + "test other::case ... ok\n", 1),
        ):
            self.assertEqual(self.failed_output(changed, status), "")

    def test_static_return_boundary_never_interprets_or_exposes_error_text(self):
        for private in ('Error: "FAKE_PRIVATE_TOKEN"\n', "libsql://FAKE_PRIVATE_TOKEN\n",
                        'Error: "CLOSE_FAILED"\n', "unqualified future harness framing FAKE_PRIVATE_TOKEN\n", ""):
            output = self.failed_output(self.failure(harness=private))
            self.assertIn("primary=INVENTORY_SCHEMA_REFUSED rollback=OK close=OK leases=ZERO", output)
            self.assertNotIn("primary=CLOSE_FAILED", output)
        for forged in ("test other::case ... ok\n", "test result: ok. 1 passed\n", "running 2 tests\n", "FAILED\n",
                       "FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=DDL_FAILED close=OK\n", "x" * 16385 + "\n"):
            self.assertEqual(self.failed_output(self.failure(harness=forged)), "")
        self.assertEqual(self.failed_output(self.failure() + "x" * 32769), "")

    def test_only_lf_and_single_crlf_delimit_diagnostic_producer_lines(self):
        valid = self.failure()
        for separator in ("\r", "\v", "\f", "\x1c", "\x1d", "\x1e", "\x85", "\u2028", "\u2029"):
            self.assertEqual(self.failed_output(valid.replace("leases=ZERO\nFVOCI_TURSO_INVENTORY_RETURN",
                                                            "leases=ZERO" + separator + "FAKE_PRIVATE_TOKEN\nFVOCI_TURSO_INVENTORY_RETURN")), "")
        for ending in ("\r", "\r\r\n"):
            self.assertEqual(self.failed_output(valid.replace("\nFVOCI_TURSO_INVENTORY_RETURN", ending + "FVOCI_TURSO_INVENTORY_RETURN")), "")

    def test_failed_inventory_still_rejects_and_mutated_whole_binding_cannot_disclose(self):
        with InventoryTests().frozen() as (_, _, run):
            run.return_value = subprocess.CompletedProcess([], 1, self.failure(harness='Error: "FAKE_PRIVATE_TOKEN"\n').encode())
            with contextlib.redirect_stdout(io.StringIO()) as output:
                with self.assertRaises(guard.AdmissionError) as caught:
                    guard.run_inventory("a" * 40, InventoryTests.inputs)
            self.assertEqual(str(caught.exception), "TURSO_INVENTORY_FAILED")
            self.assertIn("primary=INVENTORY_SCHEMA_REFUSED", output.getvalue())
            self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
            self.assertNotIn("TURSO_INVENTORY_PASS", output.getvalue())
            run.assert_called_once()
        for mutation in ("binary", "native", "cargo", "manifest", "source"):
            with InventoryTests().frozen() as (root, _, run):
                def change(*args, **kwargs):
                    paths = {"binary": "turso-connection-libtest", "native": "fvoci-sqlite/consumer-inputs.json",
                             "cargo": "turso-compile.json", "manifest": "turso-connection-build.json"}
                    if mutation == "source":
                        guard.source_digest.return_value = "changed"
                    elif mutation == "manifest":
                        with (root / paths[mutation]).open("a") as stream:
                            stream.write(" ")
                    else:
                        (root / paths[mutation]).write_bytes(b"FAKE_PRIVATE_TOKEN")
                    return subprocess.CompletedProcess([], 1, self.failure().encode())
                run.side_effect = change
                with contextlib.redirect_stdout(io.StringIO()) as output:
                    with self.assertRaises(guard.AdmissionError) as caught:
                        guard.run_inventory("a" * 40, InventoryTests.inputs)
                self.assertEqual(str(caught.exception), "COMPILED_TEST_BINDING_FAILED")
                self.assertEqual(output.getvalue(), "")
                run.assert_called_once()


class ResetTests(unittest.TestCase):
    inputs = {"phase": "reset", "destructive": True}

    @staticmethod
    def success():
        return ("\nrunning 1 test\ntest " + guard.RESET_TEST_NAME
                + " ... FVOCI_TURSO_RESET_RECEIPT primary=OK rollback=NOT_STARTED commit=RETURNED_OK "
                  "blank=CONFIRMED steps=126 close=OK drain=LOCAL_OK leases=ZERO\nok\n\n"
                  "test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n\n")

    def test_reset_requires_manual_confirmation_and_allow_before_child(self):
        admission = AdmissionTests(); admission.setUp()
        self.assertEqual(guard.validate_dispatch(admission.context, self.inputs, "a" * 40), "reset")
        for context in (dict(admission.context, event_name="push", ref=guard.REVIEWED_REF),
                        dict(admission.context, repository="attacker/fvoci"),
                        dict(admission.context, ref="refs/heads/topic")):
            with self.assertRaises(guard.AdmissionError):
                guard.validate_dispatch(context, self.inputs, "a" * 40)
        for destructive, allow in ((False, "false"), (False, "true"), (True, "false"), (True, "TRUE"), (True, "")):
            with InventoryTests().frozen() as (_, _, run), mock.patch.dict(os.environ, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": allow}):
                with self.assertRaises(guard.AdmissionError) as error:
                    guard.run_reset("a" * 40, dict(self.inputs, destructive=destructive))
                self.assertEqual(str(error.exception), "DESTRUCTIVE_NOT_ALLOWED")
                run.assert_not_called()
        for phase in ("connection", "migration", "inventory", "unknown"):
            with self.assertRaises(guard.AdmissionError) as error:
                guard.run_reset("a" * 40, dict(self.inputs, phase=phase))
            self.assertEqual(str(error.exception), "WRONG_CONSUMER_PHASE")

    def test_reset_child_is_separate_exact_body_and_current_binding_no_secret_echo(self):
        with InventoryTests().frozen() as (root, _, run), mock.patch.dict(os.environ, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true"}):
            run.return_value = subprocess.CompletedProcess([], 0, self.success().encode())
            with contextlib.redirect_stdout(io.StringIO()) as output:
                guard.run_reset("a" * 40, self.inputs)
            run.assert_called_once()
            self.assertEqual(run.call_args.args[0], [str(root / "turso-connection-libtest"), guard.RESET_TEST_NAME,
                "--ignored", "--exact", "--test-threads=1", "--nocapture"])
            child = run.call_args.kwargs["env"]
            expected = {key: os.environ[key] for key in ("PATH", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ")}
            expected.update({"FVOCI_DATABASE_BACKEND": "libsql-remote", "FVOCI_LIBSQL_URL": os.environ["FVOCI_LIBSQL_URL"],
                "FVOCI_LIBSQL_AUTH_TOKEN": "FAKE_PRIVATE_TOKEN", "FVOCI_TEST_TURSO_RESET_SELECTED": "1",
                "FVOCI_TEST_TURSO_MIGRATION_SELECTED": "1", "FVOCI_TEST_TURSO_PHASE": "migration",
                "FVOCI_TEST_TURSO_DESTRUCTIVE": "true", "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true"})
            self.assertEqual(child, expected)
            self.assertIn("TURSO_RESET_PASS tests=1 ignored=0", output.getvalue())
            self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
        for mutation in ("source", "binary", "native", "cargo", "manifest"):
            with self.subTest(mutation=mutation), InventoryTests().frozen() as (root, _, run), mock.patch.dict(os.environ, {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true"}):
                def changed(*args, **kwargs):
                    if mutation == "source":
                        guard.source_digest.return_value = "changed"
                    else:
                        target = {"binary": "turso-connection-libtest", "native": "fvoci-sqlite/consumer-inputs.json",
                                  "cargo": "turso-compile.json", "manifest": "turso-connection-build.json"}[mutation]
                        with (root / target).open("ab") as stream:
                            stream.write(b" ")
                    return subprocess.CompletedProcess([], 0, self.success().encode())
                run.side_effect = changed
                with contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(guard.AdmissionError) as error:
                    guard.run_reset("a" * 40, self.inputs)
                self.assertEqual(str(error.exception), "COMPILED_TEST_BINDING_FAILED")
                self.assertEqual(output.getvalue(), "")

    def test_reset_success_requires_original_complete_execution_and_settlement(self):
        valid = self.success()
        for text, status in [(valid, 1), (valid.replace("1 passed", "0 passed"), 0),
            (valid.replace(guard.RESET_TEST_NAME, guard.MIGRATION_TEST_NAME), 0),
            (valid.replace("commit=RETURNED_OK", "commit=UNCONFIRMED"), 0),
            (valid.replace("blank=CONFIRMED", "blank=NOT_RUN"), 0),
            (valid.replace("steps=126", "steps=125"), 0),
            (valid.replace("close=OK", "close=FAILED"), 0),
            (valid.replace("drain=LOCAL_OK", "drain=UNCONFIRMED"), 0),
            (valid.replace("leases=ZERO", "leases=FAILED"), 0),
            (valid + valid, 0), (valid + "FAKE_PRIVATE_TOKEN\n", 0),
            (valid[valid.index("test result:"):], 0)]:
            with self.subTest(text=text, status=status), contextlib.redirect_stdout(io.StringIO()) as output:
                with self.assertRaises(guard.AdmissionError) as error:
                    guard.reset_result(subprocess.CompletedProcess([], status, b""), text)
                self.assertEqual(str(error.exception), "TURSO_RESET_FAILED")
                self.assertEqual(output.getvalue(), "")

    def test_reset_failed_primary_unknown_commit_or_cleanup_never_becomes_pass(self):
        template = ("\nrunning 1 test\ntest " + guard.RESET_TEST_NAME + " ... FVOCI_TURSO_RESET_RECEIPT {}\n\n"
                    "FVOCI_TURSO_RESET_RETURN\nError: FAKE_PRIVATE_TOKEN\nFAILED\n\nfailures:\n\nfailures:\n    "
                    + guard.RESET_TEST_NAME + "\n\ntest result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 100 filtered out; finished in 0.00s\n")
        for fields in (
            "primary=RESET_SCHEMA_REFUSED rollback=RETURNED_OK commit=NOT_STARTED blank=NOT_RUN steps=0 close=OK drain=LOCAL_OK leases=ZERO",
            "primary=RESET_DDL_FAILED rollback=UNCONFIRMED commit=NOT_STARTED blank=NOT_RUN steps=23 close=FAILED drain=UNCONFIRMED leases=ZERO",
            "primary=COMMIT_UNCONFIRMED rollback=NOT_STARTED commit=UNCONFIRMED blank=NOT_RUN steps=126 close=FAILED drain=UNCONFIRMED leases=ZERO",
            "primary=RESET_FRESH_BLANK_FAILED rollback=NOT_STARTED commit=RETURNED_OK blank=FAILED steps=126 close=OK drain=LOCAL_OK leases=ZERO",
            "primary=OK rollback=NOT_STARTED commit=RETURNED_OK blank=CONFIRMED steps=126 close=FAILED drain=UNCONFIRMED leases=ZERO",
        ):
            text = template.format(fields)
            with contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(guard.AdmissionError):
                guard.reset_result(subprocess.CompletedProcess([], 1, b""), text)
            self.assertEqual(output.getvalue(), "TURSO_RESET_FAILURE " + fields + "\n")
            self.assertNotIn("FAKE_PRIVATE_TOKEN", output.getvalue())
            for mutated in (text.replace("primary=", "primary=PRIVATE_"),
                            text.replace("0 passed; 1 failed", "1 passed; 0 failed"),
                            text.replace("steps=126", "steps=127").replace("steps=23", "steps=127").replace("steps=0", "steps=127"),
                            text.replace("FVOCI_TURSO_RESET_RETURN", "FAKE_PRIVATE_TOKEN"), text + text):
                with contextlib.redirect_stdout(io.StringIO()) as output, self.assertRaises(guard.AdmissionError):
                    guard.reset_result(subprocess.CompletedProcess([], 1, b""), mutated)
                self.assertEqual(output.getvalue(), "")

    def test_reset_plan_matches_current_literal_schema_and_child_before_parent_FK_order(self):
        import re
        root = Path(__file__).resolve().parents[2]
        source = (root / "src/db/turso_test.rs").read_text()
        literal = source.split("const RESET_DROP_STATEMENTS: [&str; 126] = [", 1)[1].split("];", 1)[0]
        statements = [json.loads(line.strip().removesuffix(",")) for line in literal.splitlines() if line.strip()]
        tables = {}; triggers = set()
        # Source fixture for the maintained literal DDL grammar, never a
        # production parser or runtime catalog validation substitute.
        for path in sorted((root / "migrations/sqlite/060").glob("*.sql")):
            sql = path.read_text()
            for match in re.finditer(r"CREATE TABLE (\w+)\s*\(", sql):
                body = sql[match.start():sql.find(";", match.end())]
                tables[match[1]] = set(re.findall(r"REFERENCES (\w+)", body)) - {match[1]}
            triggers.update(re.findall(r"CREATE TRIGGER (\w+)", sql))
        drop_triggers = [statement.removeprefix('DROP TRIGGER IF EXISTS "').removesuffix('";') for statement in statements[:27]]
        drop_tables = [statement.removeprefix('DROP TABLE IF EXISTS "').removesuffix('";') for statement in statements[27:]]
        self.assertEqual(len(statements), 126)
        self.assertEqual(len(set(drop_triggers)), 27); self.assertEqual(set(drop_triggers), triggers)
        self.assertEqual(len(set(drop_tables)), 99); self.assertEqual(set(drop_tables), set(tables))
        self.assertEqual(drop_tables[-1], "schema_migrations")
        for child, parents in tables.items():
            for parent in parents:
                self.assertLess(drop_tables.index(child), drop_tables.index(parent))
        self.assertNotIn("PRAGMA", "".join(statements))


if __name__ == "__main__":
    unittest.main(verbosity=2)
