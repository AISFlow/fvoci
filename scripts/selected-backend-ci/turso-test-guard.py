#!/usr/bin/env python3
"""Pure admission contract; no SDK, credential lookup, or network execution.

The CLI deliberately rejects every runtime phase until a reviewed real primary
driver exists. Target validation is available to pure fixtures and a later
driver; it is configuration admission, not proof of the server's identity.
"""

import json
import os
import re
import subprocess
import sys
from urllib.parse import urlsplit

PHASES = (
    "connection",
    "crud",
    "transactions",
    "migration",
    "persistence",
    "restore",
    "ui-ack",
)
REPOSITORY = "AISFlow/fvoci"
ENVIRONMENT = "fvoci-turso-test"


class AdmissionError(Exception):
    """Only fixed codes may reach output; never include an input or SDK error."""


def reject(code):
    raise AdmissionError(code)


def boolean(value):
    if type(value) is not bool:
        reject("INVALID_BOOLEAN")
    return value


def validate_dispatch(context, inputs, checkout_sha):
    if (
        context.get("event_name") != "workflow_dispatch"
        or context.get("repository") != REPOSITORY
        or context.get("ref") != "refs/heads/main"
    ):
        reject("UNTRUSTED_DISPATCH")
    sha = context.get("sha", "")
    if not re.fullmatch(r"[0-9a-f]{40}", sha) or checkout_sha != sha:
        reject("CHECKOUT_MISMATCH")
    phase = inputs.get("phase", "connection")
    if phase not in PHASES:
        reject("UNKNOWN_PHASE")
    destructive = boolean(inputs.get("destructive", False))
    if phase == "connection" and destructive:
        reject("CONNECTION_MUST_BE_READ_ONLY")
    if phase != "connection" and not destructive:
        reject("DESTRUCTIVE_CONFIRMATION_REQUIRED")
    for key in ("expected_host", "test_database_id"):
        if not isinstance(inputs.get(key), str) or not inputs[key].strip():
            reject("TARGET_CONFIRMATION_REQUIRED")
    return phase


def validate_target(inputs, settings, secrets):
    """Validate a future consuming step's explicit configuration, without I/O.

    settings are dedicated Environment variables, not workflow defaults. A
    future driver must additionally verify the real target's owned marker
    before any mutations and assess SDK follow-up endpoint behavior.
    """
    host = settings.get("FVOCI_TEST_TURSO_EXPECTED_HOST", "")
    db_id = settings.get("FVOCI_TEST_TURSO_DATABASE_ID", "")
    if not isinstance(host, str) or not re.fullmatch(
        r"[a-z0-9](?:[a-z0-9-]*[a-z0-9])?(?:\.[a-z0-9](?:[a-z0-9-]*[a-z0-9])?)+",
        host,
    ):
        reject("INVALID_EXPECTED_HOST")
    if not host.endswith(".turso.io") or len(host) > 253:
        reject("INVALID_EXPECTED_HOST")
    if not isinstance(db_id, str) or not re.fullmatch(r"[a-z0-9][a-z0-9-]{0,62}", db_id):
        reject("INVALID_DATABASE_ID")
    if inputs.get("expected_host") != host or inputs.get("test_database_id") != db_id:
        reject("TARGET_CONFIRMATION_MISMATCH")
    phase = inputs.get("phase", "connection")
    if phase not in PHASES:
        reject("UNKNOWN_PHASE")
    destructive = boolean(inputs.get("destructive", False))
    if phase == "connection" and destructive:
        reject("CONNECTION_MUST_BE_READ_ONLY")
    if phase != "connection" and (
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
        valid = (
            parsed.scheme in ("libsql", "https")
            and parsed.hostname == host
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
    # Deliberately no implemented phases. Adding one requires an independently
    # reviewed real maintained-SDK driver, not an override or shell input.
    if phase not in PHASES:
        reject("UNKNOWN_PHASE")
    reject("NOT_IMPLEMENTED")


def main():
    try:
        with open(os.environ["GITHUB_EVENT_PATH"], encoding="utf-8") as stream:
            event = json.load(stream)
        inputs = dict(event.get("inputs", {}))
        # GitHub event inputs encode booleans as strings. Reject other strings.
        flag = inputs.get("destructive", "false")
        if flag not in ("true", "false"):
            reject("INVALID_BOOLEAN")
        inputs["destructive"] = flag == "true"
        checkout_sha = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], text=True, stderr=subprocess.DEVNULL
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
