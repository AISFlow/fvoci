#!/usr/bin/env python3
"""Fail-closed CI job selection from cumulative PR diffs and workflow gates."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import Literal

ROOT = Path(__file__).resolve().parent.parent

Mode = Literal["full", "narrow"]
NarrowFamily = Literal["docs", "frontend_web_install"]

PLAN_VERSION = 3

WORKFLOW_JOBS: dict[str, tuple[str, ...]] = {
    "web": ("web-checks", "workspace-browser-shard", "collaboration-flow"),
    "rust": ("fast", "postgres", "collaboration"),
    "documents": ("native-extraction",),
    "collab-engine": ("native-collab-engine",),
    "install": ("install-smoke", "backup-restore-smoke"),
}

WORKFLOW_YAML: dict[str, str] = {
    "web": "web.yml",
    "rust": "rust.yml",
    "documents": "documents.yml",
    "collab-engine": "collab-engine.yml",
    "install": "install.yml",
}

PLAN_JOB_ID = "ci-plan"
PLAN_OUTPUT_KEYS = ("mode", "reason_code", "plan_ok", "plan_json")
PYYAML_PIN = "PyYAML==6.0.3"
REQUIREMENTS_FILE = "scripts/ci_selection_requirements.txt"
SELECTOR_REGRESSION_WRAPPER = "scripts/test-ci-selection.sh"

SHA_RE = re.compile(r"^[0-9a-f]{40}$")
REASON_CODE_RE = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")
JOB_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
GATE_NEEDS_JSON_EXPR = "${{ toJSON(needs) }}"
GATE_TESTED_SHA_EXPR = "${{ github.sha }}"

KNOWN_EVENTS = frozenset({"pull_request", "push", "merge_group", "workflow_dispatch"})
ALWAYS_FULL_EVENTS = frozenset({"push", "merge_group", "workflow_dispatch"})

ALLOWED_PLAN_KEYS = frozenset(
    {
        "version",
        "workflow",
        "mode",
        "reason_code",
        "plan_ok",
        "base_sha",
        "head_sha",
        "merge_base_sha",
        "tested_sha",
        "path_count",
        "jobs",
    }
)
ALLOWED_JOB_ENTRY_KEYS = frozenset({"selected"})

# Shared build, auth, DB, harness, toolchain, native crates (conservative full).
_BROADEN_PREFIXES: tuple[str, ...] = (
    ".github/",
    "migrations/",
    "src/",
    "tests/",
    "scripts/",
    "vendor/",
    "compat/",
    "infra/",
    ".agents/",
    "packages/",
    "crates/",
)

_BROADEN_EXACT: frozenset[str] = frozenset(
    {
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "Dockerfile",
        ".dockerignore",
    }
)

_MANIFEST_MARKERS: tuple[str, ...] = (
    "/package.json",
    "/package-lock.json",
    "/Cargo.toml",
    "/Cargo.lock",
    "/pnpm-lock.yaml",
    "/yarn.lock",
)

# Explicit explanatory docs only (not build inputs).
_EXPLICIT_DOCS: frozenset[str] = frozenset(
    {
        "README.md",
        "RUNNING.md",
        "docs/rewrite.md",
    }
)

# Generated / contract / config under apps/web (never narrow).
_WEB_BROADEN_PREFIXES: tuple[str, ...] = (
    "apps/web/openapi.json",
    "apps/web/src/generated/",
    "apps/web/package.json",
    "apps/web/package-lock.json",
    "apps/web/playwright.config.ts",
    "apps/web/e2e/",
    "apps/web/e2e-pending/",
)

_FRONTEND_NARROW_PREFIX = "apps/web/src/"


def _starts_with(path: str, prefix: str) -> bool:
    return path == prefix or path.startswith(prefix)


def validate_sha(ref: str) -> bool:
    return bool(SHA_RE.match(ref))


def sanitize_reason_code(code: str) -> str:
    if not REASON_CODE_RE.match(code):
        raise ValueError(f"unsafe reason code: {code!r}")
    return code


def select_output_key(job: str) -> str:
    return f"select_{job.replace('-', '_')}"


def gate_job_id(workflow: str) -> str:
    return f"{workflow}-ci-gate"


def expected_select_if(job: str) -> str:
    return f"needs.{PLAN_JOB_ID}.outputs.{select_output_key(job)} == 'true'"


def canonical_gate_run(workflow: str) -> str:
    return (
        "set -euo pipefail\n"
        "python3 scripts/ci_selection.py gate "
        f"--workflow {workflow} "
        '--needs-json "$NEEDS_JSON" '
        '--tested-sha "$TESTED_SHA"\n'
    )


def _normalize_run_script(text: str) -> str:
    return text.replace("\r\n", "\n").strip() + "\n"


def _script_lines(text: str) -> list[str]:
    return [row.strip() for row in text.replace("\r\n", "\n").split("\n") if row.strip()]


def classify_path(path: str) -> NarrowFamily | Literal["broaden"] | Literal["unknown"]:
    if path in _BROADEN_EXACT:
        return "broaden"
    for prefix in _BROADEN_PREFIXES:
        if _starts_with(path, prefix):
            return "broaden"
    for marker in _MANIFEST_MARKERS:
        if marker in path:
            return "broaden"
    if path in _EXPLICIT_DOCS:
        return "docs"
    if _starts_with(path, "docs/"):
        return "broaden"
    for prefix in _WEB_BROADEN_PREFIXES:
        if _starts_with(path, prefix):
            return "broaden"
    if _starts_with(path, _FRONTEND_NARROW_PREFIX):
        return "frontend_web_install"
    if _starts_with(path, "apps/web/"):
        return "broaden"
    return "unknown"


def parse_name_status_z(data: bytes) -> tuple[list[str], str | None]:
    """Strict git diff --name-status -z parser; rejects truncated records."""
    if data:
        if not data.endswith(b"\0"):
            return [], "DIFF_TRUNCATED"
    fields = data.split(b"\0")
    if fields and fields[-1] == b"":
        fields = fields[:-1]
    paths: list[str] = []
    i = 0
    while i < len(fields):
        status = fields[i].decode("utf-8", errors="strict")
        i += 1
        if not status:
            return [], "DIFF_EMPTY_STATUS"
        if not re.match(r"^[ACDMRTU][0-9]*$", status):
            return [], "DIFF_BAD_STATUS"
        kind = status[0]
        if kind in ("R", "C"):
            if i + 1 >= len(fields):
                return [], "DIFF_TRUNCATED_RENAME"
            old = fields[i].decode("utf-8", errors="strict")
            new = fields[i + 1].decode("utf-8", errors="strict")
            i += 2
            paths.extend([old, new])
        elif kind == "D":
            if i >= len(fields):
                return [], "DIFF_TRUNCATED_DELETE"
            paths.append(fields[i].decode("utf-8", errors="strict"))
            i += 1
        else:
            if i >= len(fields):
                return [], "DIFF_TRUNCATED_PATH"
            paths.append(fields[i].decode("utf-8", errors="strict"))
            i += 1
    if i != len(fields):
        return [], "DIFF_EXTRA_FIELDS"
    return paths, None


def _git(
    repo: Path, *args: str, text: bool = True
) -> subprocess.CompletedProcess[str] | subprocess.CompletedProcess[bytes]:
    return subprocess.run(
        ["git", *args],
        cwd=repo,
        check=False,
        capture_output=True,
        text=text,
    )


def git_rev_parse(repo: Path, ref: str, *, require_sha_ref: bool = True) -> tuple[str | None, str | None]:
    if require_sha_ref and not validate_sha(ref):
        return None, "SHA_INVALID"
    proc = _git(repo, "rev-parse", ref)
    if proc.returncode != 0:
        return None, "REV_PARSE_FAILED"
    sha = proc.stdout.strip()
    if not validate_sha(sha):
        return None, "SHA_INVALID"
    return sha, None


def git_merge_base(repo: Path, a: str, b: str) -> tuple[str | None, str | None]:
    proc = _git(repo, "merge-base", a, b)
    if proc.returncode != 0:
        return None, "MERGE_BASE_FAILED"
    sha = proc.stdout.strip()
    if not validate_sha(sha):
        return None, "SHA_INVALID"
    return sha, None


def git_diff_paths(repo: Path, base: str, head: str) -> tuple[list[str], str | None]:
    proc = _git(repo, "diff", "--name-status", "-z", "-M", base, head, text=False)
    if proc.returncode != 0:
        return [], "GIT_DIFF_FAILED"
    return parse_name_status_z(proc.stdout)


def diff_paths_for_pr(
    repo: Path, base_sha: str, head_sha: str
) -> tuple[list[str] | None, str | None, str | None]:
    resolved_base, err = git_rev_parse(repo, base_sha)
    if err:
        return None, err, None
    resolved_head, err = git_rev_parse(repo, head_sha)
    if err:
        return None, err, None
    merge_base, err = git_merge_base(repo, resolved_base, resolved_head)
    if err:
        return None, err, None
    paths, parse_err = git_diff_paths(repo, merge_base, resolved_head)
    if parse_err:
        return None, parse_err, merge_base
    return paths, None, merge_base


def git_object_exists(repo: Path, sha: str) -> bool:
    proc = _git(repo, "cat-file", "-e", f"{sha}^{{commit}}")
    return proc.returncode == 0


def git_fetch_origin(repo: Path, *refs: str) -> str | None:
    validated: list[str] = []
    for ref in refs:
        if not validate_sha(ref):
            return "SHA_INVALID"
        validated.append(ref)
    proc = _git(repo, "fetch", "--no-tags", "origin", *validated)
    if proc.returncode != 0:
        return "FETCH_FAILED"
    return None


def ensure_commit_shas(repo: Path, *shas: str) -> str | None:
    for sha in shas:
        if not validate_sha(sha):
            return "SHA_INVALID"
    missing = [sha for sha in shas if not git_object_exists(repo, sha)]
    if not missing:
        return None
    return git_fetch_origin(repo, *missing)


def git_commit_parents(repo: Path, sha: str) -> tuple[list[str] | None, str | None]:
    if not validate_sha(sha):
        return None, "SHA_INVALID"
    proc = _git(repo, "rev-list", "--parents", "-n", "1", sha)
    if proc.returncode != 0:
        return None, "REV_LIST_PARENTS_FAILED"
    parts = proc.stdout.strip().split()
    if not parts:
        return None, "REV_LIST_PARENTS_EMPTY"
    commit = parts[0]
    if commit != sha:
        return None, "REV_LIST_COMMIT_MISMATCH"
    parents = parts[1:]
    for parent in parents:
        if not validate_sha(parent):
            return None, "SHA_INVALID"
    return parents, None


def pr_checkout_narrow_block(
    repo: Path, tested_sha: str, base_sha: str, head_sha: str
) -> str | None:
    """Return a force-full reason when the tested commit is not the event merge."""
    parents, err = git_commit_parents(repo, tested_sha)
    if err:
        return err
    if len(parents) != 2:
        return "FULL_PR_CHECKOUT_NOT_MERGE"
    if parents[0] != base_sha or parents[1] != head_sha:
        return "FULL_PR_MERGE_PARENTS_MISMATCH"
    return None


@dataclass(frozen=True)
class SelectionDecision:
    mode: Mode
    reason_code: str
    families: frozenset[NarrowFamily]


@dataclass(frozen=True)
class ResolvedInputs:
    paths: list[str] | None
    fatal_error: str | None
    force_full_reason: str | None
    base_sha: str | None
    head_sha: str | None
    merge_base_sha: str | None
    tested_sha: str | None


def decide_from_paths(paths: list[str]) -> SelectionDecision:
    if not paths:
        return SelectionDecision("full", "FULL_EMPTY_DIFF", frozenset())
    families: set[NarrowFamily] = set()
    for path in paths:
        kind = classify_path(path)
        if kind == "broaden":
            return SelectionDecision("full", "FULL_PATH_BROADEN", frozenset())
        if kind == "unknown":
            return SelectionDecision("full", "FULL_UNKNOWN_PATH", frozenset())
        families.add(kind)
    if len(families) != 1:
        return SelectionDecision("full", "FULL_MIXED_NARROW", frozenset())
    family = next(iter(families))
    if family == "docs":
        return SelectionDecision("narrow", "NARROW_DOCS", frozenset({family}))
    return SelectionDecision("narrow", "NARROW_FRONTEND_WEB_INSTALL", frozenset({family}))


def workflow_job_selected(workflow: str, job: str, decision: SelectionDecision) -> bool:
    if decision.mode == "full":
        return True
    family = next(iter(decision.families))
    if family == "docs":
        return False
    if family == "frontend_web_install":
        if workflow in ("web", "install"):
            return job in WORKFLOW_JOBS[workflow]
        return False
    return False


def build_plan(
    *,
    workflow: str,
    event_name: str,
    base_sha: str | None,
    head_sha: str | None,
    merge_base_sha: str | None,
    tested_sha: str | None,
    paths: list[str] | None,
    fatal_error: str | None = None,
    force_full_reason: str | None = None,
) -> dict:
    if workflow not in WORKFLOW_JOBS:
        raise SystemExit(f"unknown workflow: {workflow}")

    plan_ok = True
    if fatal_error:
        decision = SelectionDecision("full", sanitize_reason_code(fatal_error), frozenset())
        plan_ok = False
    elif event_name in ALWAYS_FULL_EVENTS:
        decision = SelectionDecision(
            "full", sanitize_reason_code(f"FULL_EVENT_{event_name.upper()}"), frozenset()
        )
    elif event_name not in KNOWN_EVENTS:
        decision = SelectionDecision("full", "FULL_EVENT_UNKNOWN", frozenset())
        plan_ok = False
    elif force_full_reason:
        decision = SelectionDecision("full", sanitize_reason_code(force_full_reason), frozenset())
    elif paths is None:
        decision = SelectionDecision("full", "FULL_MISSING_PATHS", frozenset())
        plan_ok = False
    else:
        decision = decide_from_paths(paths)

    jobs = {
        job: {"selected": workflow_job_selected(workflow, job, decision)}
        for job in WORKFLOW_JOBS[workflow]
    }

    return {
        "version": PLAN_VERSION,
        "workflow": workflow,
        "mode": decision.mode,
        "reason_code": decision.reason_code,
        "plan_ok": plan_ok,
        "base_sha": base_sha,
        "head_sha": head_sha,
        "merge_base_sha": merge_base_sha,
        "tested_sha": tested_sha,
        "path_count": len(paths) if paths is not None else 0,
        "jobs": jobs,
    }


def event_shas(event: dict, event_name: str) -> tuple[str | None, str | None]:
    if event_name == "pull_request":
        pr = event.get("pull_request") or {}
        return pr.get("base", {}).get("sha"), pr.get("head", {}).get("sha")
    if event_name == "merge_group":
        mg = event.get("merge_group") or {}
        return mg.get("base_sha"), mg.get("head_sha")
    if event_name == "push":
        return event.get("before"), event.get("after")
    return None, None


def load_event(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def resolve_selection_inputs(
    repo: Path,
    event: dict,
    event_name: str,
) -> ResolvedInputs:
    tested_sha = os.environ.get("GITHUB_SHA", "").strip()
    if not validate_sha(tested_sha):
        return ResolvedInputs(None, "TESTED_SHA_INVALID", None, None, None, None, None)

    head_now, err = git_rev_parse(repo, "HEAD", require_sha_ref=False)
    if err:
        return ResolvedInputs(None, "HEAD_REV_PARSE_FAILED", None, None, None, None, tested_sha)
    if head_now != tested_sha:
        return ResolvedInputs(None, "TESTED_SHA_MISMATCH", None, None, None, None, tested_sha)

    if event_name == "workflow_dispatch":
        return ResolvedInputs(None, None, None, None, None, None, tested_sha)

    if event_name not in KNOWN_EVENTS:
        return ResolvedInputs(None, "EVENT_UNKNOWN", None, None, None, None, tested_sha)

    if event_name in ("push", "merge_group"):
        base_sha, head_sha = event_shas(event, event_name)
        return ResolvedInputs(None, None, None, base_sha, head_sha, None, tested_sha)

    base_sha, head_sha = event_shas(event, event_name)
    if not base_sha or not head_sha:
        return ResolvedInputs(None, "MISSING_BASE_OR_HEAD", None, base_sha, head_sha, None, tested_sha)
    if not validate_sha(base_sha) or not validate_sha(head_sha):
        return ResolvedInputs(None, "SHA_INVALID", None, base_sha, head_sha, None, tested_sha)

    fetch_err = ensure_commit_shas(repo, base_sha, head_sha)
    if fetch_err:
        return ResolvedInputs(None, fetch_err, None, base_sha, head_sha, None, tested_sha)

    checkout_block = pr_checkout_narrow_block(repo, tested_sha, base_sha, head_sha)
    if checkout_block:
        return ResolvedInputs(None, None, checkout_block, base_sha, head_sha, None, tested_sha)

    paths, diff_err, merge_base = diff_paths_for_pr(repo, base_sha, head_sha)
    if diff_err:
        return ResolvedInputs(None, diff_err, None, base_sha, head_sha, merge_base, tested_sha)

    return ResolvedInputs(paths, None, None, base_sha, head_sha, merge_base, tested_sha)


def write_github_outputs(plan: dict, output_path: Path | None) -> None:
    if output_path is None:
        return
    reason_code = plan["reason_code"]
    sanitize_reason_code(reason_code)
    lines = [
        f"mode={plan['mode']}",
        f"reason_code={reason_code}",
        f"plan_ok={'true' if plan['plan_ok'] else 'false'}",
    ]
    for job, meta in plan["jobs"].items():
        key = job.replace("-", "_")
        selected = meta["selected"]
        if not isinstance(selected, bool):
            raise ValueError("job.selected must be bool")
        lines.append(f"select_{key}={'true' if selected else 'false'}")
    payload = json.dumps(plan, separators=(",", ":"), sort_keys=True)
    with output_path.open("w", encoding="utf-8") as handle:
        handle.write("\n".join(lines) + "\n")
        handle.write("plan_json<<PLAN_EOF\n")
        handle.write(payload + "\n")
        handle.write("PLAN_EOF\n")


def _load_yaml_mapping(path: Path) -> tuple[dict | None, str | None]:
    try:
        import yaml
    except ImportError as exc:
        return None, (
            "PyYAML is required for workflow registry validation. "
            f"Install the pinned dependency from {REQUIREMENTS_FILE} ({PYYAML_PIN}). "
            f"Import error: {exc}"
        )
    try:
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
    except Exception as exc:  # noqa: BLE001
        return None, f"{path.name}: YAML parse failed: {exc}"
    if not isinstance(data, dict):
        return None, f"{path.name}: workflow YAML must be a mapping"
    return data, None


def _needs_list(job: dict) -> tuple[list[str] | None, str | None]:
    needs = job.get("needs")
    if needs is None:
        return [], None
    if isinstance(needs, str):
        return [needs], None
    if isinstance(needs, list) and all(isinstance(item, str) for item in needs):
        return list(needs), None
    return None, "needs must be a string or list of strings"


def _run_scripts(job: dict) -> list[str]:
    steps = job.get("steps")
    if not isinstance(steps, list):
        return []
    scripts: list[str] = []
    for step in steps:
        if not isinstance(step, dict):
            continue
        run = step.get("run")
        if isinstance(run, str):
            scripts.append(run)
    return scripts


def _run_steps(job: dict) -> list[dict]:
    steps = job.get("steps")
    if not isinstance(steps, list):
        return []
    return [
        step
        for step in steps
        if isinstance(step, dict) and isinstance(step.get("run"), str)
    ]


def list_workflow_files(repo_root: Path) -> list[Path]:
    directory = repo_root / ".github" / "workflows"
    if not directory.is_dir():
        return []
    return sorted(
        path
        for path in directory.iterdir()
        if path.is_file() and path.suffix in {".yml", ".yaml"}
    )


RUST_WORKFLOW_FILE = "rust.yml"
RUST_COLLAB_CI_SCRIPT = Path("scripts/run-rust-collaboration-ci-tests.sh")
RUST_CAPACITY_PROBE_SCRIPT = Path("scripts/collab-capacity-probe.sh")
RUST_POSTGRES_RUNNER_ARCH: dict[str, str] = {
    "ubuntu-24.04": "x64",
    "ubuntu-24.04-arm": "arm64",
}
RUST_INTEGRATION_MANUAL_TARGETS: frozenset[str] = frozenset({"collab_capacity_probe"})
RUST_DB_TESTS_FEATURE = "db-tests"
RUST_POSTGRES_INTEGRATION_STEP = "PostgreSQL integration tests"
RUST_S3_INTEGRATION_STEP = "S3-compatible storage integration tests (pinned test server)"
RUST_COLLAB_INTEGRATION_STEP = "WebSocket, PostgreSQL and native helper integration tests"
RUST_COLLAB_INTEGRATION_RUN = "bash scripts/run-rust-collaboration-ci-tests.sh"
RUST_COLLAB_MATRIX_RUNNERS = frozenset({"ubuntu-24.04", "ubuntu-24.04-arm"})
RUST_S3_INTEGRATION_STEP_IF = "matrix.shard == 'b'"
RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS: frozenset[str] = frozenset(
    {
        "collab_wire",
        "markdown_process",
        "docx_export_process",
        "pdf_export_process",
        "pptx_export_process",
        "doctor_conversion",
        "office_extract_process",
        "static_api",
    }
)
CARGO_TEST_FLAG_RE = re.compile(r"(?:^|\s)--test\s+([A-Za-z0-9_-]+)")


def _package_autotests_enabled(cargo_data: dict) -> bool:
    package = cargo_data.get("package")
    if isinstance(package, dict) and "autotests" in package:
        return bool(package["autotests"])
    return True


def _autotest_crate_attributes(path: Path) -> list[str]:
    attrs: list[str] = []
    for line in path.read_text(encoding="utf-8").splitlines():
        stripped = line.strip()
        if stripped.startswith("#!["):
            attrs.append(stripped)
    return attrs


def _autotest_declares_db_tests(path: Path) -> bool:
    for attr in _autotest_crate_attributes(path):
        if "extract-native-tests" in attr:
            return False
        if 'feature = "db-tests"' in attr or 'feature="db-tests"' in attr:
            return True
    return False


def root_db_integration_registry_targets(repo_root: Path) -> tuple[set[str] | None, str | None]:
    cargo_path = repo_root / "Cargo.toml"
    if not cargo_path.is_file():
        return None, "rust: missing root Cargo.toml"
    try:
        data = tomllib.loads(cargo_path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        return None, f"rust: Cargo.toml parse failed: {exc}"
    targets: set[str] = set()
    entries = data.get("test")
    if isinstance(entries, list):
        for entry in entries:
            if not isinstance(entry, dict):
                return None, "rust: Cargo.toml [[test]] entry must be a table"
            name = entry.get("name")
            if not isinstance(name, str) or not name:
                return None, "rust: Cargo.toml [[test]] missing name"
            features = entry.get("required-features", [])
            if features is None:
                features = []
            if not isinstance(features, list) or not all(isinstance(item, str) for item in features):
                return None, f"rust: Cargo.toml [[test]] {name} required-features must be a string list"
            if RUST_DB_TESTS_FEATURE in features:
                targets.add(name)
    if _package_autotests_enabled(data):
        tests_dir = repo_root / "tests"
        if tests_dir.is_dir():
            for path in sorted(tests_dir.glob("*.rs")):
                stem = path.stem
                if _autotest_declares_db_tests(path):
                    targets.add(stem)
                    continue
                attrs = _autotest_crate_attributes(path)
                if any("extract-native-tests" in attr for attr in attrs):
                    continue
                if stem in RUST_AUTOTEST_FAST_NATIVE_EXCLUSIONS:
                    continue
                if attrs or path.read_text(encoding="utf-8").strip():
                    return None, (
                        f"rust: tests/{stem}.rs is not registered and has no crate "
                        "#![cfg(feature = \"db-tests\")]; add CI inventory or an explicit fast/native exclusion"
                    )
    return targets, None


def cargo_test_flags_in_text(text: str) -> set[str]:
    return set(CARGO_TEST_FLAG_RE.findall(text))


def _rust_workflow_jobs(repo_root: Path) -> tuple[dict | None, str | None]:
    path = repo_root / ".github" / "workflows" / RUST_WORKFLOW_FILE
    if not path.is_file():
        return None, f"rust: missing workflow file {RUST_WORKFLOW_FILE}"
    data, parse_err = _load_yaml_mapping(path)
    if parse_err:
        return None, f"rust: {parse_err}"
    jobs = data.get("jobs")
    if not isinstance(jobs, dict):
        return None, "rust: jobs mapping missing"
    return jobs, None


def _postgres_matrix_rows(postgres_job: dict) -> tuple[list[dict] | None, str | None]:
    strategy = postgres_job.get("strategy")
    if not isinstance(strategy, dict):
        return None, "rust: postgres job strategy missing"
    matrix = strategy.get("matrix")
    if not isinstance(matrix, dict):
        return None, "rust: postgres job matrix missing"
    include = matrix.get("include")
    if not isinstance(include, list) or not include:
        return None, "rust: postgres job matrix.include missing"
    rows: list[dict] = []
    for row in include:
        if not isinstance(row, dict):
            return None, "rust: postgres matrix.include row must be a mapping"
        rows.append(row)
    return rows, None


def postgres_matrix_inventory(jobs: dict) -> tuple[dict[str, set[str]], str | None]:
    postgres_job = jobs.get("postgres")
    if not isinstance(postgres_job, dict):
        return {}, "rust: postgres job missing"
    rows, err = _postgres_matrix_rows(postgres_job)
    if err:
        return {}, err
    per_arch: dict[str, set[str]] = {"x64": set(), "arm64": set()}
    for row in rows:
        runner = row.get("runner")
        tests_field = row.get("tests")
        if not isinstance(runner, str) or runner not in RUST_POSTGRES_RUNNER_ARCH:
            return {}, f"rust: postgres matrix row has unknown runner {runner!r}"
        if not isinstance(tests_field, str) or not tests_field.strip():
            return {}, "rust: postgres matrix row missing tests command fragment"
        arch = RUST_POSTGRES_RUNNER_ARCH[runner]
        per_arch[arch].update(cargo_test_flags_in_text(tests_field))
    return per_arch, None


def _postgres_job_steps(jobs: dict) -> tuple[list[dict] | None, str | None]:
    postgres_job = jobs.get("postgres")
    if not isinstance(postgres_job, dict):
        return None, "rust: postgres job missing"
    steps = postgres_job.get("steps")
    if not isinstance(steps, list):
        return None, "rust: postgres job steps missing"
    return steps, None


def _unique_named_step(
    steps: list[dict], step_name: str, *, job: str
) -> tuple[dict | None, str | None]:
    matches = 0
    found: dict | None = None
    for step in steps:
        if not isinstance(step, dict):
            continue
        if step.get("name") != step_name:
            continue
        matches += 1
        found = step
    if matches == 0:
        return None, f"rust: missing {job} step {step_name!r}"
    if matches != 1:
        return None, f"rust: {job} step {step_name!r} must appear exactly once"
    return found, None


def _execution_step_masked(step: dict, *, job: str, step_name: str) -> str | None:
    if step.get("continue-on-error") is True:
        return f"rust: {job} step {step_name!r} must not use continue-on-error"
    return None


def _verify_postgres_integration_run(run: str) -> str | None:
    norm = _normalize_run_script(run).strip()
    if norm.startswith("echo "):
        return (
            "rust: PostgreSQL integration step must execute "
            "cargo test with --features db-tests and ${{ matrix.tests }}"
        )
    if not norm.startswith("cargo test"):
        return (
            "rust: PostgreSQL integration step must execute "
            "cargo test with --features db-tests and ${{ matrix.tests }}"
        )
    if "--features db-tests" not in norm:
        return (
            "rust: PostgreSQL integration step must execute "
            "cargo test with --features db-tests and ${{ matrix.tests }}"
        )
    if "${{ matrix.tests }}" not in norm:
        return (
            "rust: PostgreSQL integration step must execute "
            "cargo test with --features db-tests and ${{ matrix.tests }}"
        )
    return None


def _verify_s3_integration_run(run: str) -> str | None:
    norm = _normalize_run_script(run).strip()
    if norm.startswith("echo "):
        return (
            "rust: S3 integration step must invoke start-test-minio.sh with a db-tests cargo test"
        )
    if not norm.startswith("bash scripts/start-test-minio.sh cargo test"):
        return (
            "rust: S3 integration step must invoke start-test-minio.sh with a db-tests cargo test"
        )
    if "--features db-tests" not in norm:
        return (
            "rust: S3 integration step must invoke start-test-minio.sh with a db-tests cargo test"
        )
    if "--test attachment_s3_integration" not in norm:
        return (
            "rust: S3 integration step must invoke start-test-minio.sh with a db-tests cargo test"
        )
    return None


def verify_postgres_integration_execution(jobs: dict) -> list[str]:
    steps, err = _postgres_job_steps(jobs)
    if err:
        return [err]
    assert steps is not None
    step, step_err = _unique_named_step(steps, RUST_POSTGRES_INTEGRATION_STEP, job="postgres")
    if step_err:
        return [step_err]
    assert step is not None
    masked = _execution_step_masked(step, job="postgres", step_name=RUST_POSTGRES_INTEGRATION_STEP)
    if masked:
        return [masked]
    if "if" in step:
        return [
            f"rust: postgres step {RUST_POSTGRES_INTEGRATION_STEP!r} must not have an if condition"
        ]
    run = step.get("run")
    if not isinstance(run, str):
        return [
            f"rust: postgres step {RUST_POSTGRES_INTEGRATION_STEP!r} must have a string run command"
        ]
    run_err = _verify_postgres_integration_run(run)
    return [run_err] if run_err else []


def postgres_s3_inventory(jobs: dict) -> tuple[set[str], str | None]:
    steps, err = _postgres_job_steps(jobs)
    if err:
        return set(), err
    assert steps is not None
    step, step_err = _unique_named_step(steps, RUST_S3_INTEGRATION_STEP, job="postgres")
    if step_err:
        return set(), step_err
    assert step is not None
    masked = _execution_step_masked(step, job="postgres", step_name=RUST_S3_INTEGRATION_STEP)
    if masked:
        return set(), masked
    step_if = step.get("if")
    if step_if != RUST_S3_INTEGRATION_STEP_IF:
        return set(), (
            f"rust: S3 integration step if must be {RUST_S3_INTEGRATION_STEP_IF!r}, "
            f"got {step_if!r}"
        )
    run = step.get("run")
    if not isinstance(run, str):
        return set(), f"rust: postgres step {RUST_S3_INTEGRATION_STEP!r} must have a string run command"
    run_err = _verify_s3_integration_run(run)
    if run_err:
        return set(), run_err
    return cargo_test_flags_in_text(run), None


def verify_collaboration_workflow_execution(jobs: dict) -> list[str]:
    collab_job = jobs.get("collaboration")
    if not isinstance(collab_job, dict):
        return ["rust: collaboration job missing"]
    strategy = collab_job.get("strategy")
    if not isinstance(strategy, dict):
        return ["rust: collaboration job strategy missing"]
    matrix = strategy.get("matrix")
    if not isinstance(matrix, dict):
        return ["rust: collaboration job matrix missing"]
    include = matrix.get("include")
    if not isinstance(include, list) or not include:
        return ["rust: collaboration job matrix.include missing"]
    runners: set[str] = set()
    for row in include:
        if not isinstance(row, dict):
            return ["rust: collaboration matrix.include row must be a mapping"]
        runner = row.get("runner")
        if not isinstance(runner, str):
            return ["rust: collaboration matrix row missing runner"]
        runners.add(runner)
    missing_runners = sorted(RUST_COLLAB_MATRIX_RUNNERS - runners)
    if missing_runners:
        return [
            "rust: collaboration matrix missing runners: " + ", ".join(missing_runners)
        ]
    steps = collab_job.get("steps")
    if not isinstance(steps, list):
        return ["rust: collaboration job steps missing"]
    step, step_err = _unique_named_step(
        steps, RUST_COLLAB_INTEGRATION_STEP, job="collaboration"
    )
    if step_err:
        return [step_err]
    assert step is not None
    masked = _execution_step_masked(
        step, job="collaboration", step_name=RUST_COLLAB_INTEGRATION_STEP
    )
    if masked:
        return [masked]
    if "if" in step:
        return [
            "rust: collaboration integration step must not have an if condition"
        ]
    run_value = step.get("run")
    if not isinstance(run_value, str):
        return [
            f"rust: collaboration step {RUST_COLLAB_INTEGRATION_STEP!r} must have a string run command"
        ]
    if _normalize_run_script(run_value) != _normalize_run_script(RUST_COLLAB_INTEGRATION_RUN):
        return [
            "rust: collaboration integration step must execute "
            f"{RUST_COLLAB_INTEGRATION_RUN}"
        ]
    return []


def _collaboration_script_cargo_command(text: str) -> str | None:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        stripped = line.strip()
        if not stripped.startswith("cargo test "):
            continue
        parts = [stripped.rstrip("\\").strip()]
        next_index = index + 1
        while next_index < len(lines):
            continuation = lines[next_index].strip()
            if continuation.startswith("--test "):
                parts.append(continuation.rstrip("\\").strip())
                next_index += 1
                continue
            break
        return " ".join(parts)
    return None


def collaboration_script_inventory(repo_root: Path) -> tuple[set[str], str | None]:
    script_path = repo_root / RUST_COLLAB_CI_SCRIPT
    if not script_path.is_file():
        return set(), f"rust: missing collaboration CI script {RUST_COLLAB_CI_SCRIPT}"
    text = script_path.read_text(encoding="utf-8")
    cargo_command = _collaboration_script_cargo_command(text)
    if not cargo_command:
        return set(), "rust: collaboration CI script missing cargo test invocation"
    tests = cargo_test_flags_in_text(cargo_command)
    if not tests:
        return set(), "rust: collaboration CI script declares no --test targets"
    if "--features db-tests" not in cargo_command:
        return set(), "rust: collaboration CI script cargo test must pass --features db-tests"
    return tests, None


def verify_rust_suite_registry(repo_root: Path = ROOT) -> list[str]:
    """Ensure explicit root [[test]] db-tests targets map to rust.yml execution rows."""
    errors: list[str] = []
    rust_workflow = repo_root / ".github" / "workflows" / RUST_WORKFLOW_FILE
    if not rust_workflow.is_file():
        errors.append(f"rust: missing workflow file {RUST_WORKFLOW_FILE}")
        return errors
    if not (repo_root / "Cargo.toml").is_file():
        errors.append("rust: missing root Cargo.toml")
        return errors

    cargo_targets, cargo_err = root_db_integration_registry_targets(repo_root)
    if cargo_err:
        errors.append(cargo_err)
        return errors
    assert cargo_targets is not None

    jobs, jobs_err = _rust_workflow_jobs(repo_root)
    if jobs_err:
        errors.append(jobs_err)
        return errors
    assert jobs is not None

    errors.extend(verify_postgres_integration_execution(jobs))

    per_arch, matrix_err = postgres_matrix_inventory(jobs)
    if matrix_err:
        errors.append(matrix_err)
        return errors

    s3_tests, s3_err = postgres_s3_inventory(jobs)
    if s3_err:
        errors.append(s3_err)
        return errors

    errors.extend(verify_collaboration_workflow_execution(jobs))

    collab_tests, collab_err = collaboration_script_inventory(repo_root)
    if collab_err:
        errors.append(collab_err)
        return errors

    if per_arch["x64"] != per_arch["arm64"]:
        only_x64 = sorted(per_arch["x64"] - per_arch["arm64"])
        only_arm = sorted(per_arch["arm64"] - per_arch["x64"])
        if only_x64:
            errors.append(
                "rust: postgres matrix missing on arm64: " + ", ".join(only_x64)
            )
        if only_arm:
            errors.append(
                "rust: postgres matrix missing on x64: " + ", ".join(only_arm)
            )

    postgres_union = per_arch["x64"]
    overlap = (postgres_union & collab_tests) | (postgres_union & s3_tests) | (collab_tests & s3_tests)
    if overlap:
        errors.append(
            "rust: integration target assigned to multiple CI buckets: "
            + ", ".join(sorted(overlap))
        )

    if RUST_INTEGRATION_MANUAL_TARGETS & cargo_targets:
        probe_script = repo_root / RUST_CAPACITY_PROBE_SCRIPT
        if not probe_script.is_file():
            errors.append(f"rust: missing manual probe script {RUST_CAPACITY_PROBE_SCRIPT}")

    assigned = postgres_union | collab_tests | s3_tests | RUST_INTEGRATION_MANUAL_TARGETS
    required = cargo_targets - RUST_INTEGRATION_MANUAL_TARGETS
    missing = sorted(required - assigned)
    if missing:
        errors.append(
            "rust: Cargo.toml db-tests integration targets missing from rust.yml inventory: "
            + ", ".join(missing)
        )

    return errors


def verify_workflow_registry(repo_root: Path = ROOT) -> list[str]:
    errors: list[str] = []
    workflows_dir = repo_root / ".github" / "workflows"
    allowed_files = set(WORKFLOW_YAML.values())
    discovered_files = list_workflow_files(repo_root)
    if not workflows_dir.is_dir():
        errors.append("missing .github/workflows directory")
        return errors

    for path in discovered_files:
        if path.name not in allowed_files:
            errors.append(f"unknown workflow file {path.name}")

    for workflow, filename in WORKFLOW_YAML.items():
        path = workflows_dir / filename
        if not path.is_file():
            errors.append(f"{workflow}: missing workflow file {filename}")
            continue
        data, parse_err = _load_yaml_mapping(path)
        if parse_err:
            errors.append(f"{workflow}: {parse_err}")
            continue
        jobs = data.get("jobs")
        if not isinstance(jobs, dict) or not jobs:
            errors.append(f"{workflow}: jobs mapping missing")
            continue
        if any(not isinstance(job_id, str) or not JOB_ID_RE.match(job_id) for job_id in jobs):
            errors.append(f"{workflow}: invalid job id")
            continue

        reserved_gate = gate_job_id(workflow)
        if PLAN_JOB_ID not in jobs:
            errors.append(f"{workflow}: missing reserved plan job {PLAN_JOB_ID}")
        if reserved_gate not in jobs:
            errors.append(f"{workflow}: missing reserved gate job {reserved_gate}")

        product_jobs = [
            job_id for job_id in jobs if job_id not in {PLAN_JOB_ID, reserved_gate}
        ]
        expected = list(WORKFLOW_JOBS[workflow])
        for job in expected:
            if job not in jobs:
                errors.append(f"{workflow}: missing registered job id {job}")
            elif job in {PLAN_JOB_ID, reserved_gate}:
                errors.append(f"{workflow}: registered job collides with reserved id {job}")
        for job in product_jobs:
            if job not in expected:
                errors.append(f"{workflow}: unregistered job id {job}")

        plan_job = jobs.get(PLAN_JOB_ID)
        if isinstance(plan_job, dict):
            if "if" in plan_job:
                errors.append(f"{workflow}: {PLAN_JOB_ID} must not have an if condition")
            outputs = plan_job.get("outputs")
            if not isinstance(outputs, dict):
                errors.append(f"{workflow}: {PLAN_JOB_ID} outputs mapping missing")
            else:
                for key in PLAN_OUTPUT_KEYS:
                    if key not in outputs:
                        errors.append(f"{workflow}: {PLAN_JOB_ID} missing output {key}")
                for job in expected:
                    key = select_output_key(job)
                    if key not in outputs:
                        errors.append(f"{workflow}: missing selector output {key}")
            plan_runs = "\n".join(_run_scripts(plan_job))
            if REQUIREMENTS_FILE not in plan_runs:
                errors.append(
                    f"{workflow}: {PLAN_JOB_ID} must install pinned {REQUIREMENTS_FILE}"
                )
            if "scripts/ci_selection.py plan" not in plan_runs:
                errors.append(f"{workflow}: {PLAN_JOB_ID} must invoke ci_selection.py plan")
            if f"--workflow {workflow}" not in plan_runs and f"--workflow={workflow}" not in plan_runs:
                errors.append(f"{workflow}: {PLAN_JOB_ID} must pass --workflow {workflow}")
            wrapper_line = f"bash {SELECTOR_REGRESSION_WRAPPER}"
            plan_lines = _script_lines(plan_runs)
            if workflow == "rust":
                try:
                    wrapper_at = plan_lines.index(wrapper_line)
                except ValueError:
                    errors.append(
                        f"{workflow}: {PLAN_JOB_ID} must run {SELECTOR_REGRESSION_WRAPPER} "
                        "before plan output"
                    )
                else:
                    plan_at = next(
                        (
                            index
                            for index, row in enumerate(plan_lines)
                            if "scripts/ci_selection.py plan" in row
                        ),
                        None,
                    )
                    if plan_at is None or wrapper_at > plan_at:
                        errors.append(
                            f"{workflow}: {PLAN_JOB_ID} must run {SELECTOR_REGRESSION_WRAPPER} "
                            "before plan output"
                        )
            elif SELECTOR_REGRESSION_WRAPPER in plan_runs:
                errors.append(
                    f"{workflow}: {PLAN_JOB_ID} must not duplicate {SELECTOR_REGRESSION_WRAPPER}"
                )
        elif PLAN_JOB_ID in jobs:
            errors.append(f"{workflow}: {PLAN_JOB_ID} must be a mapping")

        for job in expected:
            spec = jobs.get(job)
            if not isinstance(spec, dict):
                continue
            needs, needs_err = _needs_list(spec)
            if needs_err:
                errors.append(f"{workflow}: {job} {needs_err}")
            elif PLAN_JOB_ID not in (needs or []):
                errors.append(f"{workflow}: {job} must need {PLAN_JOB_ID}")
            if_value = spec.get("if")
            if if_value != expected_select_if(job):
                errors.append(
                    f"{workflow}: {job} if must be {expected_select_if(job)!r}"
                )

        gate_job = jobs.get(reserved_gate)
        if isinstance(gate_job, dict):
            if gate_job.get("if") != "always()":
                errors.append(f"{workflow}: {reserved_gate} must use if: always()")
            needs, needs_err = _needs_list(gate_job)
            expected_needs = {PLAN_JOB_ID, *expected}
            if needs_err:
                errors.append(f"{workflow}: {reserved_gate} {needs_err}")
            elif set(needs or []) != expected_needs:
                errors.append(
                    f"{workflow}: {reserved_gate} needs must be {PLAN_JOB_ID} and every registered job"
                )
            run_steps = _run_steps(gate_job)
            if len(run_steps) != 1:
                errors.append(f"{workflow}: {reserved_gate} must have exactly one run step")
            else:
                gate_step = run_steps[0]
                env = gate_step.get("env")
                expected_env = {
                    "NEEDS_JSON": GATE_NEEDS_JSON_EXPR,
                    "TESTED_SHA": GATE_TESTED_SHA_EXPR,
                }
                if env != expected_env:
                    errors.append(
                        f"{workflow}: {reserved_gate} env must be exactly "
                        f"NEEDS_JSON={GATE_NEEDS_JSON_EXPR} and TESTED_SHA={GATE_TESTED_SHA_EXPR}"
                    )
                if _normalize_run_script(gate_step["run"]) != canonical_gate_run(workflow):
                    errors.append(
                        f"{workflow}: {reserved_gate} must use the canonical gate invocation"
                    )
        elif reserved_gate in jobs:
            errors.append(f"{workflow}: {reserved_gate} must be a mapping")

    errors.extend(verify_rust_suite_registry(repo_root))
    return errors


def cmd_plan(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Compute CI selection plan for one workflow.")
    parser.add_argument("--workflow", required=True, choices=sorted(WORKFLOW_JOBS))
    parser.add_argument("--repo-root", type=Path, default=ROOT)
    parser.add_argument("--event-json", type=Path, required=True)
    parser.add_argument("--output-plan", type=Path, required=True)
    parser.add_argument("--github-output", type=Path, default=None)
    args = parser.parse_args(argv)

    errors = verify_workflow_registry(args.repo_root)
    if errors:
        print("plan: workflow registry validation failed", file=sys.stderr)
        for err in errors:
            print(err, file=sys.stderr)
        return 1

    event_name = os.environ.get("GITHUB_EVENT_NAME", "").strip()
    if not event_name:
        print("plan: GITHUB_EVENT_NAME required", file=sys.stderr)
        return 1

    event = load_event(args.event_json)
    resolved = resolve_selection_inputs(args.repo_root, event, event_name)

    plan = build_plan(
        workflow=args.workflow,
        event_name=event_name,
        base_sha=resolved.base_sha,
        head_sha=resolved.head_sha,
        merge_base_sha=resolved.merge_base_sha,
        tested_sha=resolved.tested_sha,
        paths=resolved.paths,
        fatal_error=resolved.fatal_error,
        force_full_reason=resolved.force_full_reason,
    )
    args.output_plan.write_text(json.dumps(plan, indent=2) + "\n", encoding="utf-8")
    write_github_outputs(plan, args.github_output)
    print(json.dumps({"mode": plan["mode"], "reason_code": plan["reason_code"]}))
    return 0


_VALID_RESULTS = frozenset({"success", "failure", "cancelled", "skipped"})


def _validate_plan_schema(plan: object, workflow: str) -> str | None:
    if not isinstance(plan, dict):
        return "PLAN_TOP_TYPE"
    unknown = set(plan) - ALLOWED_PLAN_KEYS
    if unknown:
        return "PLAN_UNKNOWN_KEYS"
    if plan.get("version") != PLAN_VERSION:
        return "PLAN_VERSION"
    if plan.get("workflow") != workflow:
        return "PLAN_WORKFLOW"
    if plan.get("mode") not in ("full", "narrow"):
        return "PLAN_MODE"
    reason = plan.get("reason_code")
    if not isinstance(reason, str) or not REASON_CODE_RE.match(reason):
        return "PLAN_REASON_CODE"
    if not isinstance(plan.get("plan_ok"), bool):
        return "PLAN_OK_TYPE"
    if not plan.get("plan_ok"):
        return "PLAN_NOT_OK"
    tested = plan.get("tested_sha")
    if not isinstance(tested, str) or not validate_sha(tested):
        return "PLAN_TESTED_SHA"
    jobs = plan.get("jobs")
    if not isinstance(jobs, dict):
        return "PLAN_JOBS"
    expected = list(WORKFLOW_JOBS[workflow])
    if set(jobs) != set(expected):
        return "PLAN_JOB_SET"
    for job in expected:
        entry = jobs.get(job)
        if not isinstance(entry, dict):
            return "PLAN_JOB_MISSING"
        extra = set(entry) - ALLOWED_JOB_ENTRY_KEYS
        if extra:
            return "PLAN_JOB_UNKNOWN_KEYS"
        selected = entry.get("selected")
        if type(selected) is not bool:
            return "PLAN_SELECTED_TYPE"
    return None


def _need_result(entry: object) -> tuple[str | None, str | None]:
    if not isinstance(entry, dict):
        return None, "NEED_ENTRY_TYPE"
    if "result" not in entry:
        return None, "NEED_RESULT_MISSING"
    result = entry.get("result")
    if not isinstance(result, str):
        return None, "NEED_RESULT_TYPE"
    if result not in _VALID_RESULTS:
        return None, "NEED_RESULT_INVALID"
    return result, None


def _need_outputs(entry: dict) -> tuple[dict[str, str] | None, str | None]:
    if "outputs" not in entry:
        return {}, None
    outputs = entry.get("outputs")
    if outputs is None:
        return {}, None
    if not isinstance(outputs, dict):
        return None, "NEED_OUTPUTS_TYPE"
    parsed: dict[str, str] = {}
    for key, value in outputs.items():
        if not isinstance(key, str) or not isinstance(value, str):
            return None, "NEED_OUTPUTS_TYPE"
        parsed[key] = value
    return parsed, None


def _load_needs_context(raw: str, workflow: str) -> tuple[dict | None, dict[str, str] | None, str | None]:
    try:
        needs = json.loads(raw)
    except json.JSONDecodeError:
        return None, None, "NEEDS_MALFORMED"
    if not isinstance(needs, dict):
        return None, None, "NEEDS_TYPE"
    expected_jobs = list(WORKFLOW_JOBS[workflow])
    expected_keys = {PLAN_JOB_ID, *expected_jobs}
    if set(needs.keys()) != expected_keys:
        return None, None, "NEEDS_KEY_SET"

    plan_entry = needs[PLAN_JOB_ID]
    plan_result, result_err = _need_result(plan_entry)
    if result_err:
        return None, None, f"PLAN_{result_err}"
    if plan_result != "success":
        return None, None, "PLAN_RESULT"
    assert isinstance(plan_entry, dict)
    outputs, outputs_err = _need_outputs(plan_entry)
    if outputs_err:
        return None, None, f"PLAN_{outputs_err}"
    assert outputs is not None
    plan_json = outputs.get("plan_json")
    if not isinstance(plan_json, str) or not plan_json.strip():
        return None, None, "PLAN_JSON_MISSING"
    try:
        plan = json.loads(plan_json)
    except json.JSONDecodeError:
        return None, None, "PLAN_JSON_MALFORMED"

    results: dict[str, str] = {}
    for job in expected_jobs:
        job_result, job_err = _need_result(needs[job])
        if job_err:
            return None, None, f"JOB_{job_err}"
        assert job_result is not None
        results[job] = job_result
    return plan, results, None


def cmd_gate(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Fail-closed gate for one workflow.")
    parser.add_argument("--workflow", required=True, choices=sorted(WORKFLOW_JOBS))
    parser.add_argument("--needs-json", default=None)
    parser.add_argument("--tested-sha", required=True)
    args = parser.parse_args(argv)

    if not validate_sha(args.tested_sha):
        print("gate: tested-sha invalid", file=sys.stderr)
        return 1

    needs_raw = args.needs_json if args.needs_json is not None else os.environ.get("NEEDS_JSON")
    if not isinstance(needs_raw, str) or not needs_raw.strip():
        print("gate: needs json missing", file=sys.stderr)
        return 1

    plan, results, needs_err = _load_needs_context(needs_raw, args.workflow)
    if needs_err:
        print(f"gate: needs error {needs_err}", file=sys.stderr)
        return 1
    assert plan is not None and results is not None

    schema_err = _validate_plan_schema(plan, args.workflow)
    if schema_err:
        print(f"gate: plan schema error {schema_err}", file=sys.stderr)
        return 1

    if plan.get("tested_sha") != args.tested_sha:
        print("gate: tested_sha mismatch", file=sys.stderr)
        return 1

    expected_jobs = WORKFLOW_JOBS[args.workflow]
    for job in expected_jobs:
        selected = plan["jobs"][job]["selected"]
        result = results[job]
        if selected:
            if result != "success":
                print(f"gate: selected job {job} must succeed, got {result}", file=sys.stderr)
                return 1
        elif result != "skipped":
            print(f"gate: unselected job {job} must be skipped, got {result}", file=sys.stderr)
            return 1

    print("gate: ok")
    return 0


def cmd_verify_workflows(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Validate workflow registry wiring.")
    parser.add_argument("--repo-root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    errors = verify_workflow_registry(args.repo_root)
    for err in errors:
        print(err, file=sys.stderr)
    return 1 if errors else 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv:
        print("usage: ci_selection.py {plan,gate,verify-workflows} ...", file=sys.stderr)
        return 2
    command = argv[0]
    rest = argv[1:]
    if command == "plan":
        return cmd_plan(rest)
    if command == "gate":
        return cmd_gate(rest)
    if command == "verify-workflows":
        return cmd_verify_workflows(rest)
    print(f"unknown command: {command}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
