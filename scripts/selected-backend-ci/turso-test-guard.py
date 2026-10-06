#!/usr/bin/env python3
"""Trusted manual admission and one explicitly selected real primary consumer.

Pure fixtures do not call metadata APIs or the probe. Only --admit fetches public
Environment metadata; only --consume reads the two runtime credential variables.
Configuration admission is not proof of server identity or later CRUD support.
"""

import json
import hashlib
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
from urllib.request import HTTPRedirectHandler, ProxyHandler, Request, build_opener
from urllib.parse import urlsplit

PHASES = (
    "connection",
    "crud",
    "transactions",
    "migration",
    "inventory",
    "persistence",
    "restore",
    "ui-ack",
)
REPOSITORY = "AISFlow/fvoci"
ENVIRONMENT = "fvoci-turso-test"
REVIEWED_REF = "refs/heads/fvoci/v060-turso-verified-connection"
TEST_NAME = "db::turso_test::turso_primary_connection"
MIGRATION_TEST_NAME = "db::turso_test::turso_primary_current12_install_resume"
INVENTORY_TEST_NAME = "db::turso_test::turso_primary_migration_target_inventory"
API_ROOT = "https://api.github.com/repos/AISFlow/fvoci/environments/fvoci-turso-test"


class AdmissionError(Exception):
    """Only fixed codes may reach output; never include an input or SDK error."""


def reject(code):
    raise AdmissionError(code)


def boolean(value):
    if type(value) is not bool:
        reject("INVALID_BOOLEAN")
    return value


def validate_dispatch(context, inputs, checkout_sha):
    manual = context.get("event_name") == "workflow_dispatch" and context.get("ref") in ("refs/heads/main", REVIEWED_REF)
    bootstrap = context.get("event_name") == "push" and context.get("ref") == REVIEWED_REF
    if context.get("repository") != REPOSITORY or not (manual or bootstrap):
        reject("UNTRUSTED_DISPATCH")
    sha = context.get("sha", "")
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or checkout_sha != sha:
        reject("CHECKOUT_MISMATCH")
    phase = inputs.get("phase", "connection")
    if phase not in PHASES:
        reject("UNKNOWN_PHASE")
    destructive = boolean(inputs.get("destructive", False))
    if bootstrap and phase != "connection":
        reject("SECRET_MODE_REQUIRES_MANUAL")
    if phase == "connection" and destructive:
        reject("CONNECTION_MUST_BE_READ_ONLY")
    if phase == "inventory" and destructive:
        reject("INVENTORY_MUST_BE_READ_ONLY")
    if phase not in ("connection", "inventory") and not destructive:
        reject("DESTRUCTIVE_CONFIRMATION_REQUIRED")
    return phase


def validate_target(inputs, settings, secrets):
    """Validate a future consuming step's explicit configuration, without I/O.

    The URL is the user-designated isolated target. Shape admission is not a
    server identity check. A future mutating driver still needs owned markers
    and target metadata; connection performs no schema/data mutation.
    """
    phase = inputs.get("phase", "connection")
    if phase not in PHASES:
        reject("UNKNOWN_PHASE")
    destructive = boolean(inputs.get("destructive", False))
    if phase == "connection" and destructive:
        reject("CONNECTION_MUST_BE_READ_ONLY")
    if phase == "inventory" and (
        destructive or settings.get("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE") != "false"
    ):
        reject("INVENTORY_MUST_BE_READ_ONLY")
    if phase not in ("connection", "inventory") and (
        not destructive or settings.get("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE") != "true"
    ):
        reject("DESTRUCTIVE_NOT_ALLOWED")
    url = secrets.get("FVOCI_TEST_TURSO_DATABASE_URL", "")
    token = secrets.get("FVOCI_TEST_TURSO_AUTH_TOKEN", "")
    if not isinstance(url, str) or not url or not isinstance(token, str) or not token:
        reject("MISSING_SECRET")
    if len(url) > 2048 or len(token) > 16384 or any(
        ord(char) <= 32 or ord(char) == 127 for char in url + token
    ):
        reject("INVALID_SECRET_FORMAT")
    try:
        parsed = urlsplit(url)
        host = parsed.hostname or ""
        valid = (
            parsed.scheme in ("libsql", "https")
            and bool(re.fullmatch(r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+", host))
            and host.endswith(".turso.io")
            and len(host) <= 253
            and parsed.netloc == host
            and parsed.username is None
            and parsed.password is None
            and parsed.port is None
            and parsed.path in ("", "/")
            and not parsed.query
            and not parsed.fragment
            and "?" not in url
            and "#" not in url
        )
    except ValueError:
        reject("INVALID_PRIMARY_URL")
    if not valid:
        reject("INVALID_PRIMARY_URL")
    return phase


def require_implemented(phase):
    if phase not in PHASES:
        reject("UNKNOWN_PHASE")
    if phase not in ("connection", "migration", "inventory"):
        reject("NOT_IMPLEMENTED")


def validate_environment(environment):
    if (
        environment.get("name") != ENVIRONMENT
        or type(environment.get("id")) is not int
        or environment["id"] <= 0
    ):
        reject("ENVIRONMENT_POLICY_DENIED")
    # Current user configuration is null. Main-only admission is enforced in
    # both jobs and validate_dispatch, not fabricated from Environment metadata.


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def environment_metadata():
    # Public resources support anonymous read. No GitHub/admin/org credential,
    # environment creation, configuration write or redirect fallback.
    opener = build_opener(ProxyHandler({}), NoRedirect())
    responses = []
    for suffix in ("",):
        request = Request(
            API_ROOT + suffix,
            headers={"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2026-03-10"},
        )
        try:
            with opener.open(request, timeout=15) as response:
                if response.status != 200 or response.geturl() != API_ROOT + suffix:
                    reject("ENVIRONMENT_METADATA_UNAVAILABLE")
                body = response.read(262145)
                if len(body) > 262144:
                    reject("ENVIRONMENT_METADATA_UNAVAILABLE")
                value = json.loads(body)
                if not isinstance(value, dict):
                    reject("ENVIRONMENT_METADATA_UNAVAILABLE")
                responses.append(value)
        except AdmissionError:
            raise
        except Exception:
            reject("ENVIRONMENT_METADATA_UNAVAILABLE")
    validate_environment(responses[0])
    return responses[0]["id"]


def source_digest():
    if subprocess.run(["git", "diff", "--quiet", "HEAD"], env=git_environment(), check=False).returncode != 0:
        reject("SOURCE_CHANGED")
    files = subprocess.check_output(["git", "ls-files", "-z"], env=git_environment()).split(b"\0")
    hashes = {
        name.decode(): hashlib.sha256(Path(name.decode()).read_bytes()).hexdigest()
        for name in files if name
    }
    return hashlib.sha256(json.dumps(hashes, sort_keys=True).encode()).hexdigest()


def git_environment():
    # Process-local filtering, not a change to user/global Git configuration.
    return {"PATH": os.environ.get("PATH", ""), "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}


def file_digest(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1048576), b""):
            digest.update(block)
    return digest.hexdigest()


def freeze_compiled_test(checkout_sha):
    root = Path(os.environ["RUNNER_TEMP"]).resolve()
    target = root / "turso-target"
    artifact_file = root / "turso-compile.json"
    artifacts = [json.loads(line) for line in artifact_file.read_text().splitlines()]
    matches = [a for a in artifacts if a.get("reason") == "compiler-artifact"
               and a.get("target", {}).get("kind") == ["lib"]
               and a.get("target", {}).get("name") == "fvoci_server"
               and a.get("target", {}).get("src_path") == str(Path.cwd() / "src" / "lib.rs")
               and a.get("profile", {}).get("test") is True
               and a.get("features") == ["db-tests"] and a.get("executable")]
    if len(matches) != 1 or not any(a.get("reason") == "build-finished" and a.get("success") is True for a in artifacts):
        reject("COMPILED_TEST_BINDING_FAILED")
    artifact = matches[0]
    executable = Path(artifact["executable"])
    if executable.is_symlink() or not executable.resolve().is_relative_to(target / "debug" / "deps"):
        reject("COMPILED_TEST_BINDING_FAILED")
    with executable.open("rb") as source:
        if source.read(4) != b"\x7fELF":
            reject("COMPILED_TEST_BINDING_FAILED")
    frozen = root / "turso-connection-libtest"
    if frozen.exists():
        reject("COMPILED_TEST_BINDING_FAILED")
    shutil.copyfile(executable, frozen)
    frozen.chmod(0o700)
    manifest = {
        "sha": checkout_sha, "source_digest": source_digest(),
        "binary_sha256": file_digest(frozen),
        "cargo_output_sha256": hashlib.sha256(artifact_file.read_bytes()).hexdigest(),
        "native_input_sha256": file_digest(root / "fvoci-sqlite" / "consumer-inputs.json"),
        "artifact": artifact,
    }
    (root / "turso-connection-build.json").write_text(json.dumps(manifest))


DIAGNOSTIC_UNIT_NAME = "db::turso_test::migration_diagnostics_disclose_only_known_static_failures"


def diagnostic_unit_binding(checkout_sha):
    # Revalidate the existing current frozen-lib receipt before/after each child.
    root = Path(os.environ["RUNNER_TEMP"]).resolve()
    manifest_path = root / "turso-connection-build.json"
    manifest = json.loads(manifest_path.read_text())
    executable = root / "turso-connection-libtest"
    source = source_digest()
    binary = file_digest(executable)
    native = file_digest(root / "fvoci-sqlite" / "consumer-inputs.json")
    cargo_output = file_digest(root / "turso-compile.json")
    if (manifest.get("sha") != checkout_sha or manifest.get("source_digest") != source
            or executable.is_symlink() or manifest.get("binary_sha256") != binary
            or manifest.get("native_input_sha256") != native
            or manifest.get("cargo_output_sha256") != cargo_output):
        reject("COMPILED_TEST_BINDING_FAILED")
    with executable.open("rb") as frozen:
        if frozen.read(4) != b"\x7fELF":
            reject("COMPILED_TEST_BINDING_FAILED")
    return executable, (source, binary, native, cargo_output, file_digest(manifest_path))


def run_diagnostic_unit(checkout_sha):
    executable, binding = diagnostic_unit_binding(checkout_sha)
    # This exact pure unit receives no DB/provider/GitHub credential or selector.
    child_env = {key: os.environ[key] for key in ("PATH", "LD_LIBRARY_PATH", "TZ")
                 if key in os.environ}
    listed = subprocess.run(
        [str(executable), DIAGNOSTIC_UNIT_NAME, "--list", "--exact"],
        env=child_env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False,
    )
    listing = listed.stdout.decode("utf-8", errors="replace")
    matches = re.findall(r"^([^\r\n]+): (test|benchmark)\r?$", listing, re.MULTILINE)
    if listed.returncode != 0 or matches != [(DIAGNOSTIC_UNIT_NAME, "test")]:
        reject("TURSO_DIAGNOSTIC_UNIT_SELECTION_FAILED")
    if diagnostic_unit_binding(checkout_sha) != (executable, binding):
        reject("COMPILED_TEST_BINDING_FAILED")
    result = subprocess.run(
        [str(executable), DIAGNOSTIC_UNIT_NAME, "--exact", "--test-threads=1"],
        env=child_env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False,
    )
    if diagnostic_unit_binding(checkout_sha) != (executable, binding):
        reject("COMPILED_TEST_BINDING_FAILED")
    output = result.stdout.decode("utf-8", errors="replace")
    cases = re.findall(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\r?$", output, re.MULTILINE)
    summaries = [line for line in output.splitlines() if line.startswith("test result:")]
    if (result.returncode != 0 or cases != [(DIAGNOSTIC_UNIT_NAME, "ok")]
            or len(summaries) != 1 or re.fullmatch(
                r"test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out;[^\r\n]*",
                summaries[0],
            ) is None):
        reject("TURSO_DIAGNOSTIC_UNIT_FAILED")
    # Raw listing/test/panic output is discarded, even on success.
    print("TURSO_DIAGNOSTIC_UNIT_PASS tests=1 ignored=0 consumer=NOTRUN")


def run_connection(checkout_sha, inputs):
    if inputs.get("phase", "connection") != "connection":
        reject("WRONG_CONSUMER_PHASE")
    run_primary(checkout_sha, inputs)


def run_migration(checkout_sha, inputs):
    if inputs.get("phase") != "migration":
        reject("WRONG_CONSUMER_PHASE")
    run_primary(checkout_sha, inputs)


def run_inventory(checkout_sha, inputs):
    if inputs.get("phase") != "inventory":
        reject("WRONG_CONSUMER_PHASE")
    run_primary(checkout_sha, inputs)


INVENTORY_PRIMARY_CODES = frozenset({
    "BEGIN_FAILED", "WRONG_PRODUCT_BACKEND", "WRONG_BACKEND", "FK_QUERY_FAILED",
    "FK_DECODE_FAILED", "FOREIGN_KEYS_NOT_ONE", "LITERAL_QUERY_FAILED",
    "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH", "CURRENT_LINEAGE_CHANGED",
    "INVENTORY_QUERY_FAILED", "INVENTORY_DECODE_FAILED", "INVENTORY_PREFIX_REFUSED",
    "INVENTORY_SCHEMA_REFUSED", "INVENTORY_SNAPSHOT_MISMATCH", "INVENTORY_HASH_INVALID",
})


def inventory_failure_diagnostic(result, output):
    # Same LF/single-CRLF separator policy as the migration diagnostic. A
    # static producer RETURN boundary separates facts from untrusted libtest
    # returned-Error bytes. No returned Error is parsed as a code or ACK.
    if result.returncode == 0 or len(output) > 32768 or any(output.count(marker) != 1 for marker in (
        "FVOCI_TURSO_INVENTORY_RECEIPT", "FVOCI_TURSO_INVENTORY_DIAGNOSTIC",
        "FVOCI_TURSO_INVENTORY_RETURN",
    )):
        return
    match = re.fullmatch(
        r"(?:\r?\n)*running 1 test\r?\n"
        r"test " + re.escape(INVENTORY_TEST_NAME) + r" \.\.\. "
        r"FVOCI_TURSO_INVENTORY_RECEIPT classification=REFUSED prefix=NONE schema_sha256=NONE "
        r"rollback=(OK|FAILED|NOT_STARTED) close=(OK|FAILED) leases=(ZERO|FAILED)\r?\n\r?\n"
        r"FVOCI_TURSO_INVENTORY_DIAGNOSTIC primary=([A-Z_]+) rollback=([A-Z_]+) "
        r"close=([A-Z_]+) leases=(ZERO|FAILED)\r?\n"
        r"FVOCI_TURSO_INVENTORY_RETURN\r?\n(?P<harness>(?:[^\n]*\n)*?)"
        r"FAILED\r?\n(?:\r?\n)*failures:\r?\n(?:\r?\n)*failures:\r?\n"
        r"    " + re.escape(INVENTORY_TEST_NAME) + r"\r?\n(?:\r?\n)*"
        r"test result: FAILED\. 0 passed; 1 failed; 0 ignored; 0 measured; "
        r"[0-9]+ filtered out; finished in [0-9]+\.[0-9]+s\r?\n(?:\r?\n)*",
        output,
    )
    if match is None:
        return
    rollback_receipt, close_receipt, lease_receipt, primary, rollback, close, leases, harness = match.groups()
    # Opaque harness output may contain private errors; never reflect it or
    # adopt its framing/meaning. Extra producer markers/tests/results refuse.
    if len(harness) > 16384 or any(marker in harness for marker in (
        "FVOCI_TURSO_", "test ", "test result:", "running ", "failures:",
    )) or re.search(r"(?:^|\n)FAILED\r?(?:\n|$)", harness):
        return
    if (primary not in INVENTORY_PRIMARY_CODES | {"OK"}
            or rollback not in ("OK", "NOT_STARTED", "ROLLBACK_UNCONFIRMED")
            or close not in ("OK", "CLOSE_FAILED", "LEASES_NOT_ZERO")
            or leases != lease_receipt
            or rollback_receipt != {"OK": "OK", "NOT_STARTED": "NOT_STARTED", "ROLLBACK_UNCONFIRMED": "FAILED"}[rollback]
            or (close == "OK") != (close_receipt == "OK")
            or (primary in ("BEGIN_FAILED", "WRONG_PRODUCT_BACKEND")) != (rollback == "NOT_STARTED")
            or (primary == "OK" and rollback == "OK" and close == "OK" and leases == "ZERO")):
        return
    print("TURSO_INVENTORY_FAILURE classification=REFUSED prefix=NONE schema_sha256=NONE"
          + " rollback=" + rollback_receipt + " close=" + close_receipt + " leases=" + lease_receipt)
    print("TURSO_INVENTORY_DIAGNOSTIC primary=" + primary + " rollback=" + rollback
          + " close=" + close + " leases=" + leases)


def inventory_result(result, output):
    # Recognize the maintained serial libtest --nocapture framing literally:
    # test NAME ... RECEIPT\nok, plus one complete PASS summary. No stripping
    # arbitrary lines, summary-only admission or raw failure reflection.
    match = re.fullmatch(
        r"(?:\r?\n)*running 1 test\r?\n"
        r"test " + re.escape(INVENTORY_TEST_NAME) + r" \.\.\. "
        r"FVOCI_TURSO_INVENTORY_RECEIPT classification=(BLANK|PREFIX|CURRENT) "
        r"prefix=(0|[1-9]|1[0-2]) schema_sha256=([0-9a-f]{64}) "
        r"rollback=OK close=OK leases=ZERO\r?\nok\r?\n"
        r"(?:\r?\n)*test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; "
        r"[0-9]+ filtered out; finished in [0-9]+\.[0-9]+s\r?\n(?:\r?\n)*",
        output,
    )
    if result.returncode != 0 or match is None:
        inventory_failure_diagnostic(result, output)
        reject("TURSO_INVENTORY_FAILED")
    classification, prefix, schema_hash = match.groups()
    if not ((classification == "BLANK" and prefix == "0")
            or (classification == "PREFIX" and 1 <= int(prefix) <= 11)
            or (classification == "CURRENT" and prefix == "12")):
        reject("TURSO_INVENTORY_FAILED")
    print("TURSO_INVENTORY_RECEIPT classification=" + classification + " prefix=" + prefix
          + " schema_sha256=" + schema_hash + " rollback=OK close=OK leases=ZERO")
    print("TURSO_INVENTORY_PASS tests=1 ignored=0")


# Literal consumer codes only: no arbitrary SDK/test output may be echoed.
MIGRATION_PRIMARY_CODES = frozenset({
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
})
MIGRATION_CLOSE_CODES = frozenset({"CLOSE_FAILED", "LEASES_NOT_ZERO"})


def migration_result(result, output):
    receipt = re.findall(
        r"FVOCI_TURSO_MIGRATION_RECEIPT primary=(OK|FAILED) prefix=(OK|NOT_CONFIRMED) fk_rollback=(OK|NOT_CONFIRMED) current=(OK|NOT_CONFIRMED) restart=(OK|NOT_CONFIRMED) close=(OK|FAILED) leases=(ZERO|FAILED)(?:\r?\n|$)",
        output,
    )
    if len(receipt) != 1:
        reject("TURSO_MIGRATION_RECEIPT_MISSING")
    print("TURSO_MIGRATION_RECEIPT " + " ".join(receipt[0]))
    # Inspect every occurrence, including malformed/private injected lines.
    # A diagnostic is never a success receipt and never authorizes a retry.
    lines = output.split("\n")
    diagnostics = [line[:-1] if index < len(lines) - 1 and line.endswith("\r") else line
                   for index, line in enumerate(lines)
                   if "FVOCI_TURSO_MIGRATION_DIAGNOSTIC" in line]
    if diagnostics:
        if len(diagnostics) != 1 or receipt[0][0] != "FAILED":
            reject("TURSO_MIGRATION_FAILED")
        diagnostic = re.fullmatch(
            r"FVOCI_TURSO_MIGRATION_DIAGNOSTIC primary=([A-Z_]+) close=([A-Z_]+)",
            diagnostics[0],
        )
        if diagnostic is None:
            reject("TURSO_MIGRATION_FAILED")
        primary, close = diagnostic.groups()
        if (primary not in MIGRATION_PRIMARY_CODES | {"OK"}
                or close not in MIGRATION_CLOSE_CODES | {"OK"}
                or (primary == "OK" and close == "OK")
                or (close == "OK") != (receipt[0][5] == "OK")):
            reject("TURSO_MIGRATION_FAILED")
        # The complete, anchored line and both closed sets were validated.
        print("TURSO_MIGRATION_DIAGNOSTIC primary=" + primary + " close=" + close)
    if (result.returncode != 0 or receipt[0] != ("OK", "OK", "OK", "OK", "OK", "OK", "ZERO")
            or not re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out;", output)
            or not re.search(r"test " + re.escape(MIGRATION_TEST_NAME) + r" \.\.\. ", output)):
        reject("TURSO_MIGRATION_FAILED")
    print("TURSO_MIGRATION_PASS tests=1 ignored=0")


def run_primary(checkout_sha, inputs):
    phase = inputs.get("phase", "connection")
    require_implemented(phase)
    if os.environ.get("FVOCI_DATABASE_BACKEND") != "libsql-remote":
        reject("BACKEND_SELECTOR_REQUIRED")
    settings = {"FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": os.environ.get("FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE", "")}
    credentials = {"FVOCI_TEST_TURSO_DATABASE_URL": os.environ.get("FVOCI_LIBSQL_URL", ""), "FVOCI_TEST_TURSO_AUTH_TOKEN": os.environ.get("FVOCI_LIBSQL_AUTH_TOKEN", "")}
    validate_target(inputs, settings, credentials)
    # Inventory also binds the full maintained freeze receipt before/after its
    # one child. Original connection/migration binding and parsers stay intact.
    inventory_binding = diagnostic_unit_binding(checkout_sha) if phase == "inventory" else None
    root = Path(os.environ["RUNNER_TEMP"]).resolve()
    manifest = json.loads((root / "turso-connection-build.json").read_text())
    executable = root / "turso-connection-libtest"
    if (manifest.get("sha") != checkout_sha or manifest.get("source_digest") != source_digest()
            or executable.is_symlink()
            or manifest.get("binary_sha256") != file_digest(executable)
            or manifest.get("native_input_sha256") != file_digest(root / "fvoci-sqlite" / "consumer-inputs.json")):
        reject("COMPILED_TEST_BINDING_FAILED")
    with executable.open("rb") as binary:
        if binary.read(4) != b"\x7fELF":
            reject("COMPILED_TEST_BINDING_FAILED")
    # Restrict the child environment; no unrelated service/GitHub credentials.
    child_env = {key: os.environ[key] for key in (
        "PATH", "LD_LIBRARY_PATH", "SSL_CERT_FILE", "SSL_CERT_DIR", "TZ"
    ) if key in os.environ}
    child_env.update({"FVOCI_DATABASE_BACKEND": "libsql-remote", "FVOCI_LIBSQL_URL": credentials["FVOCI_TEST_TURSO_DATABASE_URL"], "FVOCI_LIBSQL_AUTH_TOKEN": credentials["FVOCI_TEST_TURSO_AUTH_TOKEN"]})
    if phase == "connection":
        child_env["FVOCI_TEST_TURSO_CONNECTION_SELECTED"] = "1"
        test_name = TEST_NAME
    elif phase == "inventory":
        # The read-only body gets its own exact mode, never migration authority.
        child_env.update({
            "FVOCI_TEST_TURSO_MIGRATION_SELECTED": "1",
            "FVOCI_TEST_TURSO_PHASE": "inventory",
            "FVOCI_TEST_TURSO_DESTRUCTIVE": "false",
            "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "false",
        })
        test_name = INVENTORY_TEST_NAME
    else:
        child_env.update({
            "FVOCI_TEST_TURSO_MIGRATION_SELECTED": "1",
            "FVOCI_TEST_TURSO_PHASE": "migration",
            "FVOCI_TEST_TURSO_DESTRUCTIVE": "true",
            "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "true",
        })
        test_name = MIGRATION_TEST_NAME
    # Raw SDK/test errors can contain endpoint/query/token values. Capture only
    # in memory; never write/upload/reflect them. Do not retry a failed probe.
    result = subprocess.run(
        [str(executable), test_name, "--ignored", "--exact", "--test-threads=1", "--nocapture"],
        env=child_env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, check=False,
    )
    if phase == "inventory":
        if diagnostic_unit_binding(checkout_sha) != inventory_binding:
            reject("COMPILED_TEST_BINDING_FAILED")
        inventory_result(result, result.stdout.decode("utf-8", errors="replace"))
        return
    output = result.stdout.decode("utf-8", errors="replace")
    if phase == "migration":
        migration_result(result, output)
        return
    receipt = re.findall(
        r"FVOCI_TURSO_RECEIPT primary=([A-Z_]+) rollback=(OK|FAILED|NOT_STARTED) close=(OK|FAILED|NOT_STARTED) leases=(ZERO|FAILED|NOT_OBSERVED)",
        output,
    )
    known_primary = {"OK", "CONNECT_FAILED", "BEGIN_FAILED", "WRONG_BACKEND", "FK_QUERY_FAILED", "FK_DECODE_FAILED", "FOREIGN_KEYS_NOT_ONE", "LITERAL_QUERY_FAILED", "LITERAL_DECODE_FAILED", "LITERAL_MISMATCH"}
    if len(receipt) == 1 and receipt[0][0] in known_primary:
        print("TURSO_CONNECTION_RECEIPT " + " ".join(receipt[0]))
    else:
        reject("TURSO_CONNECTION_RECEIPT_MISSING")
    if (result.returncode != 0 or receipt[0] != ("OK", "OK", "OK", "ZERO")
            or not re.search(r"test result: ok\. 1 passed; 0 failed; 0 ignored; 0 measured; \d+ filtered out;", output)
            or not re.search(r"test " + re.escape(TEST_NAME) + r" \.\.\. ", output)):
        reject("TURSO_CONNECTION_FAILED")
    print("TURSO_CONNECTION_PASS tests=1 ignored=0")


def main():
    try:
        if sys.argv[1:] not in (["--admit"], ["--freeze"], ["--diagnostic-unit"], ["--consume"]):
            reject("EXPLICIT_MODE_REQUIRED")
        with open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8") as stream:
            event = json.load(stream)
        inputs = dict(event.get("inputs", {}))
        # GitHub event inputs encode booleans as strings. Reject other strings.
        flag = inputs.get("destructive", "false")
        if flag not in ("true", "false"):
            reject("INVALID_BOOLEAN")
        inputs["destructive"] = flag == "true"
        checkout_sha = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], env=git_environment(), text=True, stderr=subprocess.DEVNULL
        ).strip()
        phase = validate_dispatch(
            {
                "event_name": os.environ.get("GITHUB_EVENT_NAME"),
                "repository": os.environ.get("GITHUB_REPOSITORY"),
                "ref": os.environ.get("GITHUB_REF"),
                "sha": os.environ.get("GITHUB_SHA"),
            },
            inputs,
            checkout_sha,
        )
        require_implemented(phase)
        if os.environ.get("GITHUB_EVENT_NAME") == "push":
            if sys.argv[1] != "--admit":
                reject("SECRET_MODE_REQUIRES_MANUAL")
            print("BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN")
            return 0
        if sys.argv[1] == "--admit":
            environment_id = environment_metadata()
            with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
                output.write(f"environment_id={environment_id}\n")
            print("ENVIRONMENT_ADMISSION_OK_RUNTIME_NOT_RUN")
        elif sys.argv[1] == "--freeze":
            freeze_compiled_test(checkout_sha)
            print("COMPILED_TEST_FROZEN_RUNTIME_NOT_RUN")
        elif sys.argv[1] == "--diagnostic-unit":
            run_diagnostic_unit(checkout_sha)
        else:
            if phase == "connection":
                run_connection(checkout_sha, inputs)
            elif phase == "inventory":
                run_inventory(checkout_sha, inputs)
            else:
                run_migration(checkout_sha, inputs)
        return 0
    except AdmissionError as error:
        print(str(error), file=sys.stderr)
        return 78
    except Exception:
        # No traceback: even parsing errors must not reflect hostile inputs.
        print("ADMISSION_FAILED", file=sys.stderr)
        return 78
    print("ADMISSION_FAILED", file=sys.stderr)
    return 78


if __name__ == "__main__":
    sys.exit(main())
