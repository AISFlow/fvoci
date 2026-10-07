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

    def test_product_integration_ref_is_manual_ui_only(self):
        ui = dict(self.context, ref=guard.UI_REVIEWED_REF)
        self.assertEqual(guard.validate_dispatch(ui, {"phase": "ui-baseline", "destructive": False}, "a" * 40), "ui-baseline")
        self.assertEqual(guard.validate_dispatch(ui, {"phase": "ui-ack", "destructive": True}, "a" * 40), "ui-ack")
        self.denied("UNTRUSTED_DISPATCH", guard.validate_dispatch, dict(ui, event_name="push"), self.inputs, "a" * 40)
        self.denied("UNTRUSTED_DISPATCH", guard.validate_dispatch, dict(ui, event_name="pull_request"), self.inputs, "a" * 40)
        self.denied("UNTRUSTED_DISPATCH", guard.validate_dispatch, dict(ui, repository="attacker/fvoci"), {"phase": "ui-baseline", "destructive": False}, "a" * 40)
        self.denied("CHECKOUT_MISMATCH", guard.validate_dispatch, ui, {"phase": "ui-baseline", "destructive": False}, "b" * 40)
        for phase in guard.PHASES:
            if phase in ("ui-baseline", "ui-ack"):
                continue
            destructive = phase not in ("connection", "inventory")
            self.denied("UI_REF_PHASE_REQUIRED", guard.validate_dispatch, ui, {"phase": phase, "destructive": destructive}, "a" * 40)

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
        for phase in (phase for phase in guard.PHASES if phase not in ("connection", "inventory", "ui-baseline")):
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
            if phase in ("connection", "inventory", "ui-baseline"):
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
            if phase not in ("migration", "reset", "ui-ack"):
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
        for phase in ("crud", "transactions", "persistence", "restore"):
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
        self.assertIn("options: [connection, crud, transactions, migration, inventory, reset, persistence, restore, ui-ack, ui-baseline]", workflow)
        self.assertIn("        default: connection\n", workflow)
        self.assertIn("        type: boolean\n        default: false\n", workflow)
        self.assertIn("  group: fvoci-turso-test-database\n  cancel-in-progress: false\n", workflow)
        self.assertIn("permissions:\n  contents: read\n", workflow)
        self.assertEqual(workflow.count("persist-credentials: false"), 3)
        self.assertEqual(workflow.count("ref: ${{ github.sha }}"), 3)
        self.assertEqual(workflow.count("github.repository == 'AISFlow/fvoci'"), 3)
        self.assertEqual(workflow.count("refs/heads/fvoci/v060-turso-verified-connection"), 4)
        self.assertEqual(workflow.count(guard.UI_REVIEWED_REF), 2)
        self.assertNotIn(guard.UI_REVIEWED_REF, workflow.split("  turso-connection:", 1)[1].split("  turso-ui:", 1)[0])
        self.assertNotIn(guard.UI_REVIEWED_REF, workflow.split("jobs:", 1)[0])
        self.assertEqual(workflow.count("timeout-minutes: 5"), 1)
        self.assertEqual(workflow.count("timeout-minutes: 15"), 1)
        self.assertEqual(workflow.count("environment: fvoci-turso-test"), 2)
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


class UiAdapterTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        spec = importlib.util.spec_from_file_location('turso_ui', GUARD_PATH.with_name('turso-ui.py'))
        cls.ui = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.ui)

    def test_remote_baseline_keeps_both_readonly_gates_and_exact_source_binding(self):
        fixture = AdmissionTests()
        fixture.setUp()
        inputs = {'phase': 'ui-baseline', 'destructive': False}
        self.assertEqual(guard.validate_dispatch(fixture.context, inputs, 'a' * 40), 'ui-baseline')
        self.assertEqual(guard.validate_target(inputs, fixture.settings, fixture.secrets), 'ui-baseline')
        for flag, allow in ((True, 'false'), (False, 'true'), (True, 'true')):
            with self.assertRaises(guard.AdmissionError):
                guard.validate_target(dict(inputs, destructive=flag),
                    {'FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE': allow}, fixture.secrets)
        with mock.patch.dict(os.environ, {'FVOCI_DATABASE_BACKEND': 'libsql-remote',
                'FVOCI_LIBSQL_URL': fixture.secrets['FVOCI_TEST_TURSO_DATABASE_URL'],
                'FVOCI_LIBSQL_AUTH_TOKEN': fixture.secrets['FVOCI_TEST_TURSO_AUTH_TOKEN'],
                'FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE': 'false'}, clear=True):
            fixture.denied('UI_REVIEWED_SOURCE_REQUIRED', guard.run_ui, 'a' * 40, inputs)

    def test_preexisting_exact_rows_and_ledger_cannot_be_relaxed_to_counts(self):
        original = {'ledger': [['integer', '9007199254740993']], 'schemaSha256': 'a' * 64,
                    'lineage': 'fvoci-sqlite-060', 'fingerprints': {'users': {'b' * 64: 1}}}
        added = copy.deepcopy(original)
        added['fingerprints']['users']['c' * 64] = 1
        self.ui.assert_preserved(original, added)
        for mutated in (dict(added, ledger=[['integer', '9007199254740992']]),
                        dict(added, schemaSha256='d' * 64),
                        dict(added, fingerprints={'users': {'c' * 64: 2}}),
                        dict(added, fingerprints={'users': {'b' * 64: 1}, 'extra': {}})):
            with self.assertRaises(self.ui.UiError):
                self.ui.assert_preserved(original, mutated)

    def counter_records(self):
        number = lambda n: ['integer', str(n)]
        namespace = 'tui-' + 'a' * 20
        workspace, actor = ['blob', 'b' * 32], ['blob', 'c' * 32]
        original = {'ledger': [], 'schemaSha256': 'a' * 64, 'lineage': 'fvoci-sqlite-060',
            'startupHazards': 0, 'liveOutboxLeases': 0,
            'fingerprints': {table: {} for table in self.ui.BACKGROUND_TABLES},
            'operations': {'event_sequence': [[number(1), number(0)]],
                'collab_fence_counter': [[number(1), number(1)]],
                'maintenance_job_claims': [[number(k), ['null'], number(0), ['null']] for k in range(1, 10)],
                'events': [], 'users': [], 'workspaces': [], 'collab_room_fences': [],
                'task_collab_room_fences': [], 'outbox_consumers': []}}
        original['fingerprints'].update({t: {'d' * 64: 1} for t in self.ui.AUDITED_COUNTERS})
        original['fingerprints']['users'] = {'e' * 64: 1}
        after = copy.deepcopy(original)
        after['fingerprints'].update({t: {'f' * 64: 1} for t in self.ui.AUDITED_COUNTERS})
        op = after['operations']
        op['event_sequence'][0][1] = number(1)
        op['events'] = [[number(1), workspace, actor]]
        op['users'] = [[actor, ['text', namespace + '-owner@example.invalid'], ['null']]]
        op['workspaces'] = [[workspace, ['text', namespace], ['text', 'team']]]
        op['collab_fence_counter'][0][1] = number(3)
        fence = [workspace, ['blob', '1' * 32], ['blob', '2' * 32], number(2), number(1000)]
        op['collab_room_fences'] = [fence]
        for row in op['maintenance_job_claims']:
            if int(row[0][1]) in (1, 8, 9): row[2] = number(2)
        op['outbox_consumers'] = [[['text', name], number(1), ['null'], ['null']]
                                for name in ('notifications', 'mail', 'push', 'webhooks', 'github')]
        first_fence = copy.deepcopy(fence)
        first_fence[3] = number(1)
        original['fingerprints']['workspaces'] = {}
        after['fingerprints']['workspaces'] = {'1' * 64: 1}
        after['fingerprints']['users']['2' * 64] = 1
        after['fingerprints']['events']['3' * 64] = 1
        original['allocations'] = {t: {} for t in original['fingerprints']}
        after['allocations'] = {t: {} for t in after['fingerprints']}
        refs = lambda own=None, spaces=None, actors=None: {'workspaces': spaces or [], 'actors': actors or [], 'events': [], 'self': own, 'consumer': None}
        after['allocations']['users']['2' * 64] = refs(actor[1])
        after['allocations']['workspaces']['1' * 64] = refs(workspace[1])
        after['allocations']['events']['3' * 64] = refs('4' * 32, [workspace[1]], [actor[1]])
        servers = []
        for generation in (1, 2):
            process = {'pid': 100 + generation, 'startTicks': str(200 + generation)}
            records = []
            for key in (8, 9, 1):
                owner = str(generation) + str(key) + 'a' * 62
                for outcome in ('prepared', 'acquired', 'released'):
                    records.append({'schema': 1, 'pid': process['pid'], 'key': key, 'ownerSha256': owner,
                                    'generation': None if outcome == 'prepared' else str(generation), 'outcome': outcome})
            servers.append({'identity': process, 'targetSha256': 'a' * 64, 'receipts': records})
        audit = {'namespaces': [namespace], 'actors': {actor[1]: namespace + '-owner@example.invalid'},
                 'workspaces': {workspace[1]: namespace}, 'targetSha256': 'a' * 64, 'servers': servers,
                 'serverStarts': 2, 'observedFences': [first_fence, fence]}
        return original, after, audit

    def test_only_correlated_counter_deltas_preserve_existing_business_rows(self):
        original, after, audit = self.counter_records()
        self.ui.assert_preserved(original, after, audit)
        mutations = (
            lambda a: a['fingerprints']['users'].clear(),
            lambda a: a['operations']['event_sequence'][0].__setitem__(1, ['integer', '2']),
            lambda a: a['operations']['events'][0].__setitem__(2, ['null']),
            lambda a: a['operations']['events'][0].__setitem__(1, ['blob', '9' * 32]),
            lambda a: a['operations']['collab_fence_counter'][0].__setitem__(1, ['integer', '4']),
            lambda a: a['operations']['collab_fence_counter'][0].__setitem__(1, ['integer', '9223372036854775807']),
            lambda a: a['operations']['maintenance_job_claims'][0].__setitem__(2, ['integer', '3']),
            lambda a: a['operations']['maintenance_job_claims'][1].__setitem__(2, ['integer', '1']),
            lambda a: a['operations']['maintenance_job_claims'][0].__setitem__(1, ['blob', '3' * 32]),
            lambda a: a['operations']['maintenance_job_claims'].pop(),
            lambda a: a['operations']['outbox_consumers'][0].__setitem__(2, ['blob', '3' * 32]),
            lambda a: a.__setitem__('startupHazards', 1),
        )
        for mutate in mutations:
            wrong = copy.deepcopy(after)
            mutate(wrong)
            with self.assertRaises(self.ui.UiError): self.ui.assert_preserved(original, wrong, audit)
        with self.assertRaises(self.ui.UiError): self.ui.assert_preserved(original, after)
        missing = dict(audit, observedFences=[])
        with self.assertRaises(self.ui.UiError): self.ui.assert_preserved(original, after, missing)

    def test_background_preflight_keeps_populated_users_but_refuses_old_work(self):
        original, _, _ = self.counter_records()
        self.assertEqual(self.ui.startup_blockers(original), [])
        for table in self.ui.BACKGROUND_TABLES:
            wrong = copy.deepcopy(original)
            wrong['fingerprints'][table]['f' * 64] = 1
            self.assertEqual(self.ui.startup_blockers(wrong), [table])
        for key in ('startupHazards', 'liveOutboxLeases'):
            wrong = dict(original, **{key: 1})
            self.assertIn('existing-owner-or-deletion', self.ui.startup_blockers(wrong))

    def test_private_actor_capsule_rejects_symlink_permissions_and_oversize(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'capsule'
            self.ui.write(path, {'synthetic': True})
            self.assertEqual(self.ui.private_read(path), {'synthetic': True})
            alias = Path(tmp) / 'alias'
            alias.symlink_to(path)
            with self.assertRaises(self.ui.UiError): self.ui.private_read(alias)
            path.chmod(0o644)
            with self.assertRaises(self.ui.UiError): self.ui.private_read(path)
            path.chmod(0o600)
            with self.assertRaises(self.ui.UiError): self.ui.private_read(path, cap=1)

    def test_browser_receipts_require_every_actual_case_once_without_skips_or_retries(self):
        titles = ['case ' + str(i) for i in range(8)]
        report = {'config': {'workers': 1, 'metadata': {'selectedBackend': 'libsql-remote'}},
                  'errors': [], 'stats': {'expected': 8, 'unexpected': 0, 'flaky': 0, 'skipped': 0},
                  'suites': [{'specs': [{'file': self.ui.OFF, 'ok': True, 'title': title,
                    'tests': [{'expectedStatus': 'passed', 'results': [{'status': 'passed',
                        'retry': 0, 'errors': [], 'attachments': []}]}]} for title in titles]}]}
        self.assertEqual(len(self.ui.report_cases(report, self.ui.OFF, titles)), 8)
        for mutate in ('retry', 'skip', 'missing', 'duplicate'):
            wrong = copy.deepcopy(report)
            if mutate == 'retry': wrong['suites'][0]['specs'][0]['tests'][0]['results'][0]['retry'] = 1
            if mutate == 'skip': wrong['stats']['skipped'] = 1
            if mutate == 'missing': wrong['suites'][0]['specs'].pop()
            if mutate == 'duplicate': wrong['suites'][0]['specs'][1] = wrong['suites'][0]['specs'][0]
            with self.assertRaises(self.ui.UiError): self.ui.report_cases(wrong, self.ui.OFF, titles)

    def test_each_foreign_new_actor_workspace_unobserved_room_and_process_claim_is_refused(self):
        before, after, audit = self.counter_records()
        self.ui.assert_preserved(before, after, audit)
        for defect in ('user', 'workspace', 'room-owner', 'owner-hash', 'process', 'target', 'missing', 'generation', 'key', 'finish'):
            a, proof = copy.deepcopy(after), copy.deepcopy(audit)
            if defect == 'user': a['operations']['users'].append([['blob', '9' * 32], ['text', 'foreign@example.invalid'], ['null']])
            elif defect == 'workspace': a['operations']['workspaces'].append([['blob', '9' * 32], ['text', 'foreign'], ['text', 'team']])
            elif defect == 'room-owner': a['operations']['collab_room_fences'][0][2] = ['blob', '9' * 32]
            elif defect == 'owner-hash': proof['servers'][0]['receipts'][1]['ownerSha256'] = '9' * 64
            elif defect == 'process': proof['servers'][0]['receipts'][1]['pid'] = 999
            elif defect == 'target': proof['servers'][0]['targetSha256'] = '9' * 64
            elif defect == 'missing': proof['servers'][0]['receipts'].pop()
            elif defect == 'generation': proof['servers'][0]['receipts'][1]['generation'] = '9'
            elif defect == 'key': proof['servers'][0]['receipts'][1]['key'] = 2
            elif defect == 'finish': proof['servers'][0]['receipts'][2]['outcome'] = 'release-error'
            with self.subTest(defect=defect), self.assertRaises(self.ui.UiError): self.ui.assert_preserved(before, a, proof)

    def test_identical_dataset_on_changed_target_is_refused_before_mutation_and_vapid_keyring_absent(self):
        import hashlib
        baseline = {'rows': 1, 'setupNeeded': False}
        manifest = {'sourceInputs': {'source': 'a' * 40}}
        target = 'libsql://pure-original.invalid'
        inputs = {'ui_source_sha': 'a' * 40, 'ui_baseline_sha256': self.ui.value_digest(baseline),
                  'ui_target_sha256': hashlib.sha256(target.encode()).hexdigest()}
        for endpoint, accepted in ((target, True), ('libsql://pure-changed.invalid', False)):
            with tempfile.TemporaryDirectory() as directory, mock.patch.dict(os.environ, {'FVOCI_LIBSQL_URL': endpoint, 'FVOCI_LIBSQL_AUTH_TOKEN': 'invented'}, clear=True), mock.patch.object(self.ui, 'root', return_value=Path(directory)), mock.patch.object(self.ui, 'current_build', return_value=manifest), mock.patch.object(self.ui, 'fixture', return_value=baseline), mock.patch.object(self.ui, 'execute_ui', return_value={'cleanupErrors': []}) as body, contextlib.redirect_stdout(io.StringIO()):
                if accepted:
                    self.ui._consume('ui-ack', inputs)
                    env = body.call_args.args[2]
                    self.assertNotIn('ENCRYPTION_KEYS', env)
                    self.assertNotIn('ENCRYPTION_ACTIVE_KEY_ID', env)
                else:
                    with self.assertRaisesRegex(self.ui.UiError, 'UI_CURRENT_TARGET_BINDING_REQUIRED'): self.ui._consume('ui-ack', inputs)
                    body.assert_not_called()

    def test_physical_file_mutation_refuses_even_when_metadata_is_identical(self):
        import hashlib
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            paths = [root / name for name in ('libsqlite3.a', 'sqlite3.h', 'rustc', 'sysroot', 'registry', 'config', 'libclang', 'cc', 'ar')]
            for path in paths: path.write_bytes(b'original physical bytes')
            def collect(): return {'files': {str(path): hashlib.sha256(path.read_bytes()).hexdigest() for path in paths}, 'buildEnvironment': {}}
            before = collect()
            with mock.patch.object(self.ui, 'physical_inputs', side_effect=collect):
                self.ui.recheck_physical(before)
                for path in paths:
                    path.write_bytes(b'changed physical bytes')
                    with self.subTest(path=path.name), self.assertRaisesRegex(self.ui.UiError, 'UI_PHYSICAL_BUILD_INPUTS_CHANGED'): self.ui.recheck_physical(before)
                    path.write_bytes(b'original physical bytes')


    def test_failed_browser_still_observes_primary_and_retains_original_when_receipt_or_log_fails(self):
        import hashlib
        import types
        before, _, _ = self.counter_records()
        before['setupNeeded'] = False
        owner = {'namespace': 'tui-' + 'a' * 20, 'userId': 'cccccccc-cccc-cccc-cccc-cccccccccccc',
                 'workspaceId': 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'email': 'tui-' + 'a' * 20 + '-owner@example.invalid',
                 'commit': 'confirmed', 'freshPrimaryReadback': True, 'lifecycleDrain': 'confirmed', 'leases': 0}
        manifest = {'sourceInputs': {'source': 'a' * 40, 'tree': 'b' * 40},
                    'binaries': {'collab-engine': {'path': '/never-executed/engine'}}}
        for fault in ('none', 'receipt', 'log', 'closure'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                root = Path(directory); (root / 'current-build.json').write_text('pure pinned metadata')
                server = types.SimpleNamespace(pid=123, poll=lambda: 0)
                log = mock.Mock()
                if fault == 'log': log.close.side_effect = OSError('PRIVATE_INVENTED_TOKEN')
                scope = types.SimpleNamespace(closure=lambda: fault != 'closure', finish=lambda *args: 0)
                modes = []
                def fixture(manifest, mode, environment, input=None):
                    modes.append(mode)
                    return owner if mode == 'owner' else before
                original_write = self.ui.write
                def write(path, value):
                    if fault == 'receipt' and path.name == 'ui-result.private.json': raise OSError('PRIVATE_INVENTED_TOKEN')
                    return original_write(path, value)
                with mock.patch.object(self.ui, '_PROCESSES', scope), mock.patch.object(self.ui, 'root', return_value=root), mock.patch.object(self.ui.secrets, 'token_hex', return_value='a' * 20), mock.patch.object(self.ui, 'fixture', side_effect=fixture), mock.patch.object(self.ui, 'write', side_effect=write), mock.patch.object(self.ui, 'start', return_value=(server, 'http://127.0.0.1:12345', log)), mock.patch.object(self.ui, 'stop', return_value={'serverExit': 0, 'portClosed': True, 'recordedIdentitiesRetired': True}), mock.patch.object(self.ui, 'identity', return_value={'pid': 123, 'startTicks': '200'}), mock.patch.object(self.ui, 'maintenance_receipts', return_value={'identity': {'pid':123,'startTicks':'200'}, 'targetSha256':'a'*64, 'receipts':[]}), mock.patch.object(self.ui, 'browser', side_effect=self.ui.UiError('UI_ACTUAL_BROWSER_FAILED')), mock.patch.object(self.ui, 'current_build', return_value=manifest), contextlib.redirect_stderr(io.StringIO()) as diagnostic:
                    with self.assertRaisesRegex(self.ui.UiError, '^UI_ACTUAL_BROWSER_FAILED$'):
                        self.ui.execute_ui(manifest, before, {'FVOCI_LIBSQL_URL': 'libsql://invented.invalid'})
                self.assertEqual(modes, ['owner'] if fault == 'closure' else ['owner', 'baseline'])
                self.assertEqual((root / 'preservation.private.json').exists(), fault != 'closure')
                if fault == 'receipt':
                    self.assertIn('UI_ACTUAL_BROWSER_FAILED', diagnostic.getvalue())
                    self.assertIn('"receiptWrite": "failed"', diagnostic.getvalue())
                else:
                    result = json.loads((root / 'ui-result.private.json').read_text())
                    self.assertEqual(result['originalFailure'], 'UI_ACTUAL_BROWSER_FAILED')
                    self.assertEqual(result['uiResult'], 'FAIL')
                    if fault == 'closure': self.assertEqual(result['preservation']['result'], 'NOTRUN')
                self.assertNotIn('PRIVATE_INVENTED_TOKEN', diagnostic.getvalue())

    def test_process_history_covers_detach_adoption_and_pid_reuse_excludes_foreign_pids(self):
        import threading
        import types
        tracker = self.ui.UiProcesses.__new__(self.ui.UiProcesses)
        tracker.pid = 999
        tracker.entries, tracker.errors = {}, []
        tracker.lock = threading.RLock()
        process = types.SimpleNamespace(pid=10)
        tracker.allocations = [{'process': process, 'label': 'server', 'closed': False, 'forced': False, 'key': (10, '100')}]
        rows = {10: {'pid':10,'parentPid':999,'startTicks':'100','state':'S'},
                11: {'pid':11,'parentPid':10,'startTicks':'101','state':'S'},
                12: {'pid':12,'parentPid':11,'startTicks':'102','state':'S'},
                77: {'pid':77,'parentPid':88,'startTicks':'777','state':'S'}}
        def current(pid):
            if pid not in rows: raise FileNotFoundError()
            return dict(rows[pid])
        tracker.proc_rows = lambda: list(rows.values())
        with mock.patch.object(self.ui, 'proc_identity', side_effect=current), mock.patch.object(self.ui.os, 'pidfd_open', side_effect=lambda pid, flags: pid + 1000), mock.patch.object(self.ui.signal, 'pidfd_send_signal') as signal, mock.patch.object(self.ui.os, 'close'):
            tracker.capture(rows[10], 'server', 0)
            tracker.snapshot()
            self.assertEqual({pid for pid, ticks in tracker.entries}, {10, 11, 12})
            rows[12]['parentPid'] = 999  # detached grandchild reparented to the actual subreaper
            rows[13] = {'pid':13,'parentPid':999,'startTicks':'103','state':'S'}  # previously unobserved adopted descendant
            tracker.snapshot()
            self.assertEqual(tracker.entries[(12,'102')]['allocation'], 0)
            self.assertIsNone(tracker.entries[(13,'103')]['allocation'])
            old = tracker.entries[(11,'101')]
            rows[11] = {'pid':11,'parentPid':88,'startTicks':'500','state':'S'}  # unrelated reuse
            tracker.send(old, 15)
            signal.assert_not_called()
            tracker.send(tracker.entries[(12,'102')], 15)
            signal.assert_called_once_with(1012, 15, None, 0)
            self.assertNotIn((77,'777'), tracker.entries)
            self.assertFalse(tracker.closure())
            tracker.allocations[0]['closed'] = True
            rows.clear()
            self.assertTrue(tracker.closure())

    def test_pidfd_capture_race_and_signal_fault_cannot_qualify_retirement(self):
        import threading
        import types
        tracker = self.ui.UiProcesses.__new__(self.ui.UiProcesses)
        tracker.pid, tracker.entries, tracker.errors = 999, {}, []
        tracker.lock = threading.RLock()
        tracker.allocations = [{'process': types.SimpleNamespace(pid=10), 'closed':False, 'forced':False}]
        row = {'pid':10,'parentPid':999,'startTicks':'100','state':'S'}
        tracker.proc_rows = lambda: [row]
        with mock.patch.object(self.ui.os, 'pidfd_open', return_value=1010), mock.patch.object(self.ui.os, 'close') as close, mock.patch.object(self.ui, 'proc_identity', return_value=dict(row, startTicks='200')):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_PROCESS_IDENTITY_RACE'): tracker.capture(row, 'server', 0)
            close.assert_called_once_with(1010)
        with mock.patch.object(self.ui.os, 'pidfd_open', return_value=1010), mock.patch.object(self.ui, 'proc_identity', return_value=row), mock.patch.object(self.ui.signal, 'pidfd_send_signal', side_effect=OSError('private control')):
            tracker.capture(row, 'server', 0)
            with self.assertRaises(OSError): tracker.send(tracker.entries[(10,'100')], 15)
            self.assertFalse(tracker.closure())


    def test_canonical_restart_projection_keeps_detailed_identities_private(self):
        import types
        with tempfile.TemporaryDirectory() as directory:
            process = types.SimpleNamespace(pid=10)
            row = {'pid':10, 'startTicks':'100'}
            scope = types.SimpleNamespace(allocations=[{'process':process}], entries={(10,'100'):{'identity':row,'allocation':0}}, finish=lambda server, normal: 0)
            with mock.patch.object(self.ui, '_PROCESSES', scope), mock.patch.object(self.ui, 'retired', return_value=True), mock.patch.object(self.ui, 'port_closed', return_value=True):
                result = self.ui.stop(process, 'http://127.0.0.1:12345', Path(directory))
            self.assertEqual(result, {'serverExit':0, 'portClosed':True, 'recordedIdentitiesRetired':True})
            packet = json.loads(next(Path(directory).glob('server-identities-*.private.json')).read_text())
            self.assertEqual(packet['identities'], [row])
            self.assertNotIn('identities', result)

    def test_subreaper_capability_verify_restore_and_failure_preserve_original_without_real_prctl(self):
        import ctypes
        import threading
        import types
        for prior in (0, 1):
            with self.subTest(prior=prior), tempfile.TemporaryDirectory() as directory:
                state = {'flag':prior, 'calls':[]}
                def prctl(option, value, zero1, zero2, zero3):
                    state['calls'].append(option)
                    self.assertEqual((zero1,zero2,zero3), (0,0,0))
                    if option == 37: ctypes.c_int.from_address(value).value = state['flag']
                    elif option == 36: state['flag'] = value
                    return 0
                function = mock.Mock(side_effect=prctl)
                thread = mock.Mock(); thread.is_alive.return_value = False
                with mock.patch.object(self.ui.ctypes, 'CDLL', return_value=types.SimpleNamespace(prctl=function)), mock.patch.object(self.ui.threading, 'Thread', return_value=thread), mock.patch.object(self.ui.UiProcesses, 'proc_rows', return_value=[]), mock.patch.object(self.ui, 'root', return_value=Path(directory)), mock.patch.object(self.ui, '_PROCESSES', None):
                    scope = self.ui.UiProcesses()
                    scope.__enter__()
                    self.assertEqual(state['flag'], 1)
                    scope.__exit__(None, None, None)
                    self.assertEqual(state['flag'], prior)
                    self.assertEqual(state['calls'], [37,36,37,36,37])
                    self.assertEqual(function.argtypes, [ctypes.c_int,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_ulong,ctypes.c_ulong])
                    thread.start.assert_called_once()
                    thread.join.assert_called_once_with(timeout=1)
                    self.assertIsNone(self.ui._PROCESSES)
        with mock.patch.object(self.ui.os, 'pidfd_open', None), mock.patch.object(self.ui, '_PROCESSES', None):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_PROCESS_CAPABILITY_REQUIRED'): self.ui.UiProcesses().__enter__()
        scope = self.ui.UiProcesses.__new__(self.ui.UiProcesses)
        scope.allocations, scope.entries, scope.errors = [], {}, ['UI_PROCESS_OBSERVATION_FAILED']
        scope.halt, scope.thread = threading.Event(), mock.Mock()
        scope.thread.is_alive.return_value = False
        scope.closure = lambda: False
        scope.prctl = mock.Mock()
        with mock.patch.object(self.ui, 'write', side_effect=OSError('PRIVATE_FAKE')), contextlib.redirect_stderr(io.StringIO()) as diagnostics:
            scope.__exit__(self.ui.UiError, self.ui.UiError('UI_ACTUAL_BROWSER_FAILED'), None)
        scope.prctl.assert_not_called()  # unknown child closure cannot restore the flag
        self.assertIn('UI_ACTUAL_BROWSER_FAILED', diagnostics.getvalue())
        self.assertIn('UI_PROCESS_RECEIPT_WRITE_FAILED', diagnostics.getvalue())
        self.assertNotIn('PRIVATE_FAKE', diagnostics.getvalue())

    def test_native_nonzero_precedes_finish_and_packet_write_failures(self):
        import types
        packet = {'originalFailure': 'TURSO_UI_ACTOR_FAILED',
            'nativeOutcome': {'operation': 'failed', 'rollback': 'unknown', 'commit': 'not-attempted'},
            'lifecycleDrain': 'unconfirmed', 'leases': 1}
        for fault in ('none', 'finish', 'write', 'both', 'malformed', 'invalid-code'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                payload = dict(packet, originalFailure='PRIVATE_SDK_DETAIL') if fault == 'invalid-code' else packet
                process = types.SimpleNamespace(returncode=7,
                    communicate=lambda **kw: (b'not-json' if fault == 'malformed' else json.dumps(payload).encode(), b'PRIVATE_STDERR'))
                scope = types.SimpleNamespace(spawn=mock.Mock(return_value=process), finish=mock.Mock())
                if fault in ('finish', 'both'):
                    scope.finish.side_effect = self.ui.UiError('UI_PROCESS_CLOSURE_FAILED')
                original_write = self.ui.write
                def write(path, value):
                    if fault in ('write', 'both'): raise OSError('PRIVATE_WRITE_DETAIL')
                    original_write(path, value)
                with mock.patch.object(self.ui, '_PROCESSES', scope), mock.patch.object(self.ui, 'clean_env', return_value={}), mock.patch.object(self.ui, 'root', return_value=Path(directory)), mock.patch.object(self.ui, 'write', side_effect=write), contextlib.redirect_stderr(io.StringIO()) as diagnostics:
                    try:
                        self.ui.fixture({'binaries': {'fvoci-e2e-fixture': {'path': '/never/executed'}}}, 'owner', {})
                    except BaseException as error:
                        actual = self.ui.failure_code(error)
                    else:
                        self.fail('nonzero native helper was accepted')
                self.assertEqual(actual, 'UI_CONSUMER_FAILED' if fault == 'malformed' else 'UI_NATIVE_FAILURE_CODE_REFUSED' if fault == 'invalid-code' else 'TURSO_UI_ACTOR_FAILED')
                scope.finish.assert_called_once_with(process)
                receipts = list(Path(directory).glob('fixture-failure-*.private.json'))
                self.assertEqual(len(receipts), 1 if fault in ('none', 'finish') else 0)
                if receipts:
                    self.assertEqual(json.loads(receipts[0].read_text())['receipt'], packet)
                if fault in ('finish', 'write', 'both'):
                    diagnostic = json.loads(diagnostics.getvalue())
                    self.assertEqual(diagnostic['originalFailure'], 'TURSO_UI_ACTOR_FAILED')
                    self.assertEqual('UI_NATIVE_FAILURE_RECEIPT_WRITE_FAILED' in diagnostic['nativeCleanupErrors'], fault in ('write', 'both'))
                for private in ('PRIVATE_STDERR', 'PRIVATE_WRITE_DETAIL', 'PRIVATE_SDK_DETAIL'):
                    self.assertNotIn(private, diagnostics.getvalue())

    def test_final_scope_observation_and_cleanup_faults_keep_original_and_unknown_closure(self):
        import threading
        import types
        for fault in ('snapshot', 'stop', 'join', 'state', 'descriptor', 'receipt'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as directory:
                scope = self.ui.UiProcesses.__new__(self.ui.UiProcesses)
                scope.pid, scope.lock = 999, threading.RLock()
                scope.allocations = [{'process': types.SimpleNamespace(pid=10), 'closed': False, 'forced': False}]
                scope.entries = {'synthetic': {'pidfd': 123, 'identity': {'pid': 10, 'startTicks': '1'}, 'allocation': 0, 'reaped': False}}
                scope.errors = []
                scope.finish = mock.Mock(side_effect=self.ui.UiError('UI_PROCESS_CLOSURE_FAILED'))
                scope.proc_rows = mock.Mock(side_effect=self.ui.UiError('UI_PROCESS_SNAPSHOT_CAP_REFUSED'))
                scope.halt, scope.thread, scope.prctl = mock.Mock(), mock.Mock(), mock.Mock()
                scope.thread.is_alive.return_value = False
                if fault != 'snapshot': scope.closure = lambda: False
                if fault == 'stop': scope.halt.set.side_effect = OSError('PRIVATE_STOP')
                if fault == 'join': scope.thread.join.side_effect = OSError('PRIVATE_JOIN')
                if fault == 'state': scope.thread.is_alive.side_effect = OSError('PRIVATE_STATE')
                original_write, attempted = self.ui.write, []
                def write(path, value):
                    attempted.append(value)
                    if fault == 'receipt': raise OSError('PRIVATE_RECEIPT')
                    original_write(path, value)
                with mock.patch.object(self.ui, 'root', return_value=Path(directory)), mock.patch.object(self.ui, 'write', side_effect=write), mock.patch.object(self.ui.os, 'close', side_effect=OSError('PRIVATE_FD') if fault == 'descriptor' else None) as close, contextlib.redirect_stderr(io.StringIO()) as diagnostics:
                    self.assertIsNone(scope.__exit__(self.ui.UiError, self.ui.UiError('UI_ACTUAL_BROWSER_FAILED'), None))
                scope.halt.set.assert_called_once()
                scope.thread.join.assert_called_once_with(timeout=1)
                scope.thread.is_alive.assert_called_once()
                close.assert_called_once_with(123)
                scope.prctl.assert_not_called()
                self.assertEqual(len(attempted), 1)
                self.assertFalse(attempted[0]['confirmed'])
                self.assertIn('UI_ACTUAL_BROWSER_FAILED', diagnostics.getvalue())
                self.assertNotIn('PRIVATE_', diagnostics.getvalue())
                expected = {'snapshot': 'UI_PROCESS_FINAL_OBSERVATION_FAILED', 'stop': 'UI_PROCESS_OBSERVER_STOP_FAILED', 'join': 'UI_PROCESS_OBSERVER_JOIN_FAILED', 'state': 'UI_PROCESS_OBSERVER_STATE_FAILED', 'descriptor': 'UI_PIDFD_CLOSE_FAILED', 'receipt': 'UI_PROCESS_RECEIPT_WRITE_FAILED'}[fault]
                self.assertIn(expected, diagnostics.getvalue())


    def test_shell_proof_comes_only_from_the_grant_and_config_env_stays_clean(self):
        environment = {'FVOCI_LIBSQL_URL': 'libsql://invented.invalid', 'FVOCI_LIBSQL_AUTH_TOKEN': "invented'token"}
        text = self.ui.capsule_text(environment)
        self.assertIn('FVOCI_LIBSQL_AUTH_TOKEN=' + __import__('shlex').quote("invented'token"), text)
        argv = self.ui.container_argv('fvoci-tui-aa', '/host/launcher.sh', '/host/native-env.sh',
                                      '/fvoci-current/bin/fvoci-migrate', [], ['--start'])
        for flag in ('--cpus', '--cpu-quota', '--cpu-period', '--cpuset-cpus'):
            self.assertNotIn(flag, argv)
        self.assertEqual(argv[argv.index('--memory') + 1], '12884901888')
        self.assertEqual(argv[argv.index('--memory-swap') + 1], '12884901888')
        self.assertEqual(argv[argv.index('--pids-limit') + 1], '128')
        self.assertEqual(self.ui.DAEMON_CAPS['cpu.max'], 'max 100000')
        self.ui.admit_published_config(argv, ['PATH=/usr/bin:/bin', 'FVOCI_COLLAB_ENGINE=/opt/fvoci/bin/collab-engine'],
                                       ["invented'token", 'libsql://invented.invalid'])
        self.assertNotIn('--env-file', argv)
        self.assertNotIn("invented'token", '\n'.join(argv))
        with self.assertRaisesRegex(self.ui.UiError, 'UI_DOCKER_ENV_SECRET_REFUSED'):
            self.ui.admit_published_config(argv, ['FVOCI_LIBSQL_AUTH_TOKEN=invented'], ['invented'])
        with self.assertRaisesRegex(self.ui.UiError, 'UI_CANONICAL_SHELL_UNAVAILABLE'):
            self.ui.read_shell_proof({'imageId': self.ui.QUALIFIED_CANONICAL_IMAGE})
        with tempfile.TemporaryDirectory() as directory:
            proof = Path(directory) / 'shell-proof.json'
            proof.write_text(json.dumps({'Config': {'Image': self.ui.QUALIFIED_CANONICAL_IMAGE,
                                                    'Cmd': ['/bin/sh', '/acceptance/run.sh'],
                                                    'Env': ['PATH=/usr/bin:/bin']}}))
            digest = self.ui.digest(proof)
            self.assertEqual(self.ui.read_shell_proof({'imageId': self.ui.QUALIFIED_CANONICAL_IMAGE,
                                                       'shellProof': {'path': str(proof), 'sha256': digest}}), '/bin/sh')
            with self.assertRaisesRegex(self.ui.UiError, 'UI_CANONICAL_SHELL_UNAVAILABLE'):
                self.ui.read_shell_proof({'imageId': self.ui.QUALIFIED_CANONICAL_IMAGE,
                                          'shellProof': {'path': str(proof), 'sha256': '0' * 64}})

    def test_one_shot_without_cgroup_sample_stays_blocked(self):
        created = {'State': {'Pid': 0, 'Running': False},
                   'Config': {'Image': self.ui.QUALIFIED_CANONICAL_IMAGE, 'Entrypoint': ['/bin/sh'],
                              'Cmd': ['/fvoci-current/launcher.sh'], 'Env': ['PATH=/usr/bin:/bin'], 'User': '1000:1000'},
                   'HostConfig': {'ReadonlyRootfs': True, 'CapDrop': ['ALL'], 'Privileged': False, 'Memory': 12884901888}}
        argv = ['docker', 'create', '--entrypoint', '/bin/sh', self.ui.QUALIFIED_CANONICAL_IMAGE, '/fvoci-current/launcher.sh']
        creation = self.ui.creation_identity(created, argv, ['invented-token'], 'fixture')
        self.assertEqual(creation['qualification'], 'BLOCKED')
        self.assertEqual(creation['cgroupCaps'], 'not-observed')
        self.assertNotIn('cpuUnlimited', creation)
        unlimited = {'State': created['State'], 'Config': created['Config'],
                     'HostConfig': dict(created['HostConfig'], CpuQuota=0, NanoCpus=0, CpusetCpus='')}
        admitted = self.ui.creation_identity(unlimited, argv, ['invented-token'], 'fixture')
        self.assertEqual(admitted['cgroupCaps'], 'not-observed')
        self.assertNotIn('cpuUnlimited', admitted)
        for restricted in ({'CpuQuota': 200000}, {'NanoCpus': 2000000000}, {'CpusetCpus': '0-1'}):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_CAP_REFUSED'):
                self.ui.creation_identity({'State': created['State'], 'Config': created['Config'],
                                          'HostConfig': dict(created['HostConfig'], **restricted)}, argv, ['invented-token'], 'fixture')
        self.assertNotIn('caps', creation)
        done = self.ui.finished_one_shot(creation, 0)
        self.assertEqual(done, {'productExit': 0, 'liveDaemon': 'unsupported-before-execution', 'qualification': 'BLOCKED'})
        self.assertNotIn('accepted', json.dumps(done))
        proc = {'pid': 50, 'startTicks': '100', 'comm': 'fvoci-server', 'exeInspection': 'UNAVAILABLE'}
        running = {'State': {'Pid': 50, 'Running': True, 'OOMKilled': False}}
        with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_CAP_REFUSED'):
            self.ui.running_daemon_sample(running, created['HostConfig'], proc, 1, 77, '/fvoci-current/bin/fvoci-server', True)
        sample = self.ui.running_daemon_sample(running, dict(self.ui.DAEMON_CAPS), proc, 1, 77,
                                               '/fvoci-current/bin/fvoci-server', True)
        self.assertEqual(sample['qualification'], 'daemon-observed')
        old_quota = dict(self.ui.DAEMON_CAPS, **{'cpu.max': '200000 100000'})
        with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_CAP_REFUSED'):
            self.ui.running_daemon_sample(running, old_quota, proc, 1, 77, '/fvoci-current/bin/fvoci-server', True)
        self.assertEqual(sample['startTicks'], '100')
        with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_WAIT_MISSING'):
            self.ui.running_daemon_sample(running, dict(self.ui.DAEMON_CAPS), proc, 1, 77,
                                          '/fvoci-current/bin/fvoci-server', False)

    def test_open_grant_keeps_local_guard_and_does_not_borrow_run(self):
        with mock.patch.object(self.ui, 'load_existing_local_lease') as lease:
            with self.assertRaisesRegex(self.ui.UiError, 'UI_EXECUTION_MODE_REFUSED'):
                self.ui.open_referenced_grant('fixture')
            lease.assert_not_called()
        with mock.patch.dict(os.environ, {'FVOCI_SELECTED_EXECUTION_MODE': 'orca-local'}), mock.patch.object(self.ui, 'load_existing_local_lease', return_value={'canonicalRuntime': {}}) as lease:
            with self.assertRaisesRegex(self.ui.UiError, 'UI_LOCAL_NETWORK_NOT_GRANTED'):
                self.ui.open_referenced_grant('fixture')
            lease.assert_called_once_with('fixture')

    def test_publish_without_grant_does_not_call_docker_and_mocked_create_hides_token(self):
        token = "invented'token"
        with mock.patch.dict(os.environ, {'FVOCI_SELECTED_EXECUTION_MODE': 'github-ci'}, clear=True), mock.patch.object(self.ui, 'docker_client', side_effect=AssertionError('docker')):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_EXECUTION_MODE_REFUSED'):
                self.ui.publish_container(Path('/unused'), {'FVOCI_LIBSQL_URL': 'libsql://invented.invalid',
                    'FVOCI_LIBSQL_AUTH_TOKEN': token, 'FVOCI_STORAGE_DIR': '/work/storage'},
                    {'binaries': {'fvoci-e2e-fixture': {'path': '/host/bin/fvoci-e2e-fixture'}}},
                    'fvoci-e2e-fixture', ['baseline'], 'fixture')
        calls = []
        created = {'State': {'Pid': 0, 'Running': False, 'OOMKilled': False},
                   'Config': {'Image': self.ui.QUALIFIED_CANONICAL_IMAGE, 'Entrypoint': ['/bin/sh'],
                              'Cmd': ['/fvoci-current/launcher.sh'], 'Env': ['PATH=/usr/bin:/bin'], 'User': '1000:1000'},
                   'HostConfig': {'ReadonlyRootfs': True, 'CapDrop': ['ALL'], 'Privileged': False,
                                  'CpuQuota': 0, 'NanoCpus': 0, 'CpusetCpus': ''}}
        def fake_docker(args, timeout):
            calls.append(list(args))
            if args[1] == 'create':
                return b'a' * 64 + b'\n'
            return json.dumps([created]).encode()
        environment = {'FVOCI_LIBSQL_URL': 'libsql://invented.invalid', 'FVOCI_LIBSQL_AUTH_TOKEN': token,
                       'FVOCI_STORAGE_DIR': '/work/storage'}
        with tempfile.TemporaryDirectory() as directory, mock.patch.dict(os.environ, {'FVOCI_SELECTED_EXECUTION_MODE': 'orca-local'}), mock.patch.object(self.ui, 'load_existing_local_lease', return_value={'source': 'a'*40, 'tree': 'b'*40, 'runId': 'run_a1', 'dispatchId': 'ctx_b2', 'canonicalRuntime': {'imageId': self.ui.QUALIFIED_CANONICAL_IMAGE, 'networkAuthorized': True, 'shellProof': {'path': '/granted/proof.json', 'sha256': 'ab'*32}}}), mock.patch.object(self.ui, 'read_shell_proof', return_value='/bin/sh'), mock.patch.object(self.ui, 'docker_client', side_effect=fake_docker), mock.patch.object(self.ui, '_PROCESSES', object()):
            self.ui.publish_container(Path(directory), environment, {'binaries': {'fvoci-e2e-fixture': {'path': '/host/bin/fvoci-e2e-fixture'}}},
                                      'fvoci-e2e-fixture', ['baseline'], 'fixture')
        blob = '\n'.join(str(part) for args in calls for part in args)
        self.assertNotIn(token, blob)
        self.assertNotIn(token, json.dumps(created['Config']['Env']))
        self.assertIn('type=bind,src=/work/storage,dst=/work/storage\n', blob + '\n')
        self.assertNotIn('--env-file', blob)

    def ids(self):
        return [1000, 1000, 1000, 1000]

    def paused(self, client_pid=77):
        stat = '1 (sh) S 0 0 0 0 -1 0 0 0 0 0 0 0 0 0 0 20 0 1 999'
        proc = {'pid': 50, 'startTicks': '999', 'comm': 'sh'}
        inspected = {'State': {'Pid': 50, 'Running': True, 'OOMKilled': False}}
        return self.ui.bind_paused_shell(stat, inspected, proc, [50, 1], dict(self.ui.DAEMON_CAPS), client_pid,
                                          {'uid': self.ids(), 'gid': self.ids()})

    def test_launcher_bounds_and_exact_go(self):
        text = self.ui.FIXED_LAUNCHER
        self.assertLess(text.index('. "$1"'), text.index('/proc/$$/stat'))
        self.assertLess(text.index('/fvoci-private/stat.ready'), text.index('/fvoci-private/exec.go'))
        self.assertIn('[ "${#fvoci_stat}" -le 511 ]', text)
        self.assertIn('[ "$fvoci_go" = GO ]', text)
        self.assertNotIn('/proc/self', text)
        self.assertEqual(self.ui.GO_MARKER, b'GO\n')
        self.assertEqual((self.ui.STAT_BODY_MAX, self.ui.STAT_READ_BOUND, self.ui.FIXTURE_BUDGET, self.ui.SERVER_BUDGET), (511, 512, 120, 10))
        self.ui.accept_stat_payload(b'a' * 511 + b'\n')
        for refused in (b'', b'\n', b'a' * 512 + b'\n', b'GO\nextra\n'):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_PID_REFUSED'):
                self.ui.accept_stat_payload(refused)
        self.assertEqual(self.ui.stat_open_flags() & (os.O_WRONLY | os.O_RDWR), 0)
        self.assertEqual(self.ui.go_open_flags() & os.O_RDWR, os.O_RDWR)
        with mock.patch.object(self.ui.os, 'write', return_value=3) as write, mock.patch.object(self.ui.os, 'read', side_effect=AssertionError('read')):
            self.ui.write_go_once(4)
        write.assert_called_once_with(4, b'GO\n')

    def test_owned_capture_index_is_per_client_and_sample_keeps_caps_and_ids(self):
        first, second = mock.Mock(), mock.Mock()
        recorded = []
        processes = mock.Mock()
        processes.allocations = [{'process': first}, {'process': second}]
        processes.capture.side_effect = lambda row, label, allocation=None: recorded.append(allocation)
        with mock.patch.object(self.ui, '_PROCESSES', processes):
            self.assertEqual(self.ui.capture_owned_daemon(first, {'pid': 50, 'startTicks': '9'}), 0)
            self.assertEqual(self.ui.capture_owned_daemon(second, {'pid': 60, 'startTicks': '8'}), 1)
        self.assertEqual(recorded, [0, 1])
        bound = self.paused()
        self.assertEqual(bound['caps'], dict(self.ui.DAEMON_CAPS))
        self.assertEqual(bound['uid'], self.ids())
        self.assertEqual(bound['gid'], self.ids())
        self.assertEqual(bound['maintenance']['pid'], 1)
        self.assertEqual(bound['retirement']['pid'], 50)
        with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_PID_REFUSED'):
            self.ui.bind_paused_shell('1 (sh) S 0 0 0 0 -1 0 0 0 0 0 0 0 0 0 0 20 0 1 999',
                                      {'State': {'Pid': 50, 'Running': True, 'OOMKilled': False}},
                                      {'pid': 50, 'startTicks': '999', 'comm': 'sh'}, [50, 1], dict(self.ui.DAEMON_CAPS), 77,
                                      {'uid': self.ids(), 'gid': [0, 0, 0, 0]})
        status = 'NSpid:\t50\t1\nUid:\t1000\t1000\t1000\t1000\nGid:\t1000\t1000\t1000\t1000\n'
        self.assertEqual(self.ui.parse_status_fields(status, 'Uid:'), self.ids())
        self.assertEqual(self.ui.parse_nspid(status), [50, 1])
        self.assertIn('retired', self.ui.prove_normal_daemon.__code__.co_consts)
        self.assertNotIn('BLOCKED', self.ui.prove_normal_daemon.__code__.co_consts)
        self.assertIn('daemon-completion.private.json', self.ui.local_fixture.__code__.co_consts)
        self.assertNotIn('one-shot-blocked.private.json', self.ui.local_fixture.__code__.co_consts)
        self.assertIn('cleanup_owned', self.ui.local_server_start.__code__.co_names)
        self.assertNotIn('running_daemon_sample', self.ui.local_server_start.__code__.co_names)

    def test_normal_proof_needs_client_exit_retirement_and_is_not_blocked(self):
        import types
        client = types.SimpleNamespace(pid=77)
        sample = self.paused()
        sample['allocation'] = 0
        allocation = {'process': client, 'closed': False, 'forced': False}
        daemon = {'allocation': 0, 'identity': {'pid': 50, 'startTicks': '999'}}
        client_row = {'allocation': 0, 'identity': {'pid': 77, 'startTicks': '3'}}
        other = {'process': mock.Mock(), 'closed': False, 'forced': False}
        processes = mock.Mock()
        processes.allocations = [allocation, other]
        processes.entries = {(50, '999'): daemon, (77, '3'): client_row}
        def finish(process):
            allocation['closed'] = True
            return finish.code
        finish.code = 0
        state = {'Pid': 0, 'ExitCode': 0, 'OOMKilled': False}
        with mock.patch.object(self.ui, '_PROCESSES', processes), mock.patch.object(self.ui, 'retired', return_value=True), mock.patch.object(self.ui, '_PROCESSES', processes):
            processes.finish.side_effect = finish
            record = self.ui.prove_normal_daemon(client, sample, state, False)
        self.assertEqual(record['qualification'], 'retired')
        self.assertEqual(record['clientExit'], 0)
        self.assertEqual(record['productExit'], 0)
        self.assertNotEqual(record['qualification'], 'BLOCKED')
        self.assertEqual(record['caps'], dict(self.ui.DAEMON_CAPS))
        self.assertFalse(other['closed'])
        finish.code = 9
        allocation['closed'] = False
        with mock.patch.object(self.ui, '_PROCESSES', processes), mock.patch.object(self.ui, 'retired', return_value=True):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_PROCESS_CLOSURE_FAILED'):
                self.ui.prove_normal_daemon(client, sample, state, False)
        with mock.patch.object(self.ui, '_PROCESSES', processes), mock.patch.object(self.ui, 'retired', return_value=False):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_PROCESS_CLOSURE_FAILED'):
                self.ui.prove_normal_daemon(client, sample, state, False)
        processes.finish.reset_mock()
        with mock.patch.object(self.ui, '_PROCESSES', processes):
            with self.assertRaisesRegex(self.ui.UiError, 'UI_DAEMON_OBSERVATION_UNSUPPORTED'):
                self.ui.prove_normal_daemon(client, sample, state, True)
        processes.finish.assert_not_called()
        self.assertEqual(self.ui.qualify_daemon_exit(state, True), (False, 0))
        self.assertEqual(self.ui.qualify_daemon_exit({'Pid': 0, 'ExitCode': 137, 'OOMKilled': False}, False), (False, 137))

    def test_cleanup_removes_only_the_owned_container_and_keeps_the_original(self):
        import io
        import types
        process = types.SimpleNamespace(pid=77, poll=lambda: None)
        owned = {'process': process, 'closed': True, 'forced': False}
        other = {'process': mock.Mock(), 'closed': False, 'forced': False}
        processes = mock.Mock()
        processes.allocations = [owned, other]
        processes.entries = {(50, '1'): {'allocation': 0, 'identity': {'pid': 50, 'startTicks': '1'}}}
        calls = []
        def finish(target):
            self.assertIs(target, process)
            owned['closed'] = True
            process.poll = lambda: 0
        processes.finish.side_effect = finish
        def client(args, timeout):
            calls.append(list(args))
            return b''
        err = self.ui.UiError('UI_SERVER_START_FAILED')
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(self.ui, '_PROCESSES', processes), mock.patch.object(self.ui, 'docker_client', side_effect=client), mock.patch.object(self.ui, 'retired', return_value=True):
            errors = self.ui.cleanup_owned('a' * 64, process, directory, [])
            buffer = io.StringIO()
            with contextlib.redirect_stderr(buffer):
                self.assertIs(self.ui.report_cleanup(err, ['UI_DOCKER_CLIENT_FAILED']), err)
            self.assertIn('UI_SERVER_START_FAILED', buffer.getvalue())
        self.assertEqual(errors, [])
        self.assertEqual([item[1] for item in calls], ['stop', 'rm'])
        self.assertFalse(other['closed'])
        self.assertIn('cleanup_owned', self.ui.local_server_start.__code__.co_names)

    def test_nonzero_native_exit_keeps_the_allowlisted_cause(self):
        import inspect
        import io
        self.assertEqual(self.ui.native_failure_code({'originalFailure': 'TURSO_UI_ROWS'}, 3), 'TURSO_UI_ROWS')
        self.assertEqual(self.ui.native_failure_code({}, 2), 'UI_NATIVE_FIXTURE_FAILED')
        self.assertIsNone(self.ui.native_failure_code({'originalFailure': 'TURSO_UI_ROWS'}, 0))
        with self.assertRaisesRegex(self.ui.UiError, 'UI_NATIVE_FAILURE_CODE_REFUSED'):
            self.ui.native_failure_code({'originalFailure': 'not-a-code'}, 4)
        names = self.ui.local_fixture.__code__.co_names
        self.assertLess(names.index('native_failure_code'), names.index('prove_normal_daemon'))
        cause = self.ui.UiError('TURSO_UI_ROWS')
        with contextlib.redirect_stderr(io.StringIO()) as buffer:
            self.assertIs(self.ui.report_cleanup(cause, ['UI_PROCESS_CLOSURE_FAILED']), cause)
        self.assertIn('TURSO_UI_ROWS', buffer.getvalue())
        self.assertIn('UI_PROCESS_CLOSURE_FAILED', buffer.getvalue())
        server_source = inspect.getsource(self.ui.local_server_start)
        self.assertLess(server_source.index('try:'), server_source.index('logpath.open'))
        publish_source = inspect.getsource(self.ui.publish_container)
        self.assertLess(publish_source.index('try:'), publish_source.index('docker_client'))
        self.assertIn('release_failed_publish', publish_source)

    def test_failed_publish_removes_its_container_and_fifos(self):
        cause = self.ui.UiError('UI_DAEMON_OBSERVATION_UNSUPPORTED')
        calls = []
        with tempfile.TemporaryDirectory() as directory, mock.patch.object(self.ui, 'docker_client', side_effect=lambda args, timeout: calls.append(list(args)) or b''), contextlib.redirect_stderr(io.StringIO()):
            self.ui.prepare_private_fifos(directory)
            self.assertIs(self.ui.release_failed_publish(directory, 'ab' * 32, cause), cause)
        self.assertEqual(calls, [['docker', 'rm', 'ab' * 32]])
        self.assertFalse((Path(directory) / 'stat.ready').exists())
        self.assertFalse((Path(directory) / 'exec.go').exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
