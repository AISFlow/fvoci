#!/usr/bin/env python3
"""Fail-closed CI job selection from cumulative PR diffs and workflow gates."""

from __future__ import annotations

import argparse
import hashlib
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
NarrowFamily = Literal["docs", "frontend_web_install", "web_tests"]

PLAN_VERSION = 3

WORKFLOW_JOBS: dict[str, tuple[str, ...]] = {
    "web": ("web-static", "web-checks", "web-native-checks", "workspace-browser-shard", "collaboration-build", "collaboration-flow"),
    "rust": ("fast", "native-arm64", "postgres", "collaboration"),
    "documents": ("native-extraction",),
    "collab-engine": ("native-collab-engine",),
    "install": ("install-smoke", "backup-restore-smoke", "upgrade-smoke-arm64"),
}

# Manual opt-in jobs: no path or event policy selects them (not even full mode or
# a fatal plan). Only a workflow_dispatch whose boolean input of the same name is
# exactly true selects the job; the gate re-derives that from the event file.
OPT_IN_JOBS: dict[str, dict[str, str]] = {
    "install": {"upgrade-smoke-arm64": "run_upgrade_smoke_arm"},
}
OPT_IN_RUNNER: dict[str, str] = {"upgrade-smoke-arm64": "ubuntu-26.04-arm"}

WORKFLOW_YAML: dict[str, str] = {
    "web": "web.yml",
    "rust": "rust.yml",
    "documents": "documents.yml",
    "collab-engine": "collab-engine.yml",
    "install": "install.yml",
}

# Tag-driven release workflow (docs/RELEASING.md). It is not a PR/merge
# selection workflow, so it has no ci-plan/gate; it is allowed only while it
# cannot run for untrusted refs and write scopes stay in the listed jobs.
RELEASE_WORKFLOW_FILE = "release.yml"
TURSO_MANUAL_WORKFLOW_FILE = "turso-test.yml"
RELEASE_WRITE_SCOPES: dict[str, frozenset[str]] = {
    "build": frozenset({"packages"}),
    "index": frozenset({"packages"}),
    "publish": frozenset({"packages"}),
    "release": frozenset({"contents"}),
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
    # Bun `patchedDependencies` (package.json), applied by every install.
    "patches/",
)

_BROADEN_EXACT: frozenset[str] = frozenset(
    {
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
        "Dockerfile",
        ".dockerignore",
        # The Bun workspace root: apps/web, packages/* and scripts/document-convert.
        "package.json",
        "bun.lock",
        "eslint.config.mjs",
        ".prettierrc.json",
        ".prettierignore",
        "bunfig.toml",
        ".bun-version",
    }
)

_MANIFEST_MARKERS: tuple[str, ...] = (
    "/package.json",
    "/package-lock.json",
    "/bun.lock",
    "/Cargo.toml",
    "/Cargo.lock",
    "/pnpm-lock.yaml",
    "/yarn.lock",
)

# Explicit explanatory docs only (not build inputs). Exact paths are checked
# before the broaden prefixes so the agent role/environment records stay docs
# while every other `.agents/` path (skills, references) remains full.
_EXPLICIT_DOCS: frozenset[str] = frozenset(
    {
        "README.md",
        "RUNNING.md",
        "docs/rewrite.md",
        "docs/RELEASING.md",
        "docs/collab-engine-comparison.md",
        "AGENTS.md",
        ".agents/environment.md",
    }
)

# Generated / contract / config under apps/web (never narrow).
_WEB_BROADEN_PREFIXES: tuple[str, ...] = (
    "apps/web/openapi.json",
    "apps/web/src/generated/",
    "apps/web/package.json",
    "apps/web/playwright.config.ts",
)

_FRONTEND_NARROW_PREFIX = "apps/web/src/"

# Browser UI code consumed by Web unit/type checks, production browser builds,
# and the install image. Other editor paths retain full validation: schema,
# serialization, CRDT adapters and exports mirror Rust contracts; fonts are also
# read by the native export child. Do not narrow the entire packages/ workspace
# (i18n/ko.json is include_str! input to a Rust test).
_EDITOR_UI_PREFIXES = ("packages/editor/src/react/", "packages/editor/src/vue/")
_EDITOR_UI_EXACT = frozenset({
    "packages/editor/src/clipboard.ts",
    "packages/editor/src/gutter-actions.ts",
    "packages/editor/src/menu-roving.ts",
})
_UI_SUFFIXES = (".ts", ".tsx", ".vue", ".css")

# The browser suites exercise API/DB/CRDT behavior with the actual Rust server.
# Only flat specs and reviewed UI helpers narrow. Fixtures, server lifecycle,
# wire codecs/oracles, configs and arbitrary new harness files remain full.
_BROWSER_SPEC_RE = re.compile(r"^apps/web/(?:e2e|e2e-pending)/[^/]+\.spec\.ts$")
_BROWSER_UI_HELPERS = frozenset({
    "apps/web/e2e/helpers.ts",
    "apps/web/e2e/mfa-helpers.ts",
    "apps/web/e2e/workspace-wiki-vue-editor.ts",
    "apps/web/e2e-pending/collab-helpers.ts",
    "apps/web/e2e-pending/collab-helpers.test.ts",
})


def _starts_with(path: str, prefix: str) -> bool:
    return path == prefix or path.startswith(prefix)


def validate_sha(ref: str) -> bool:
    return bool(SHA_RE.match(ref))


def sanitize_reason_code(code: str) -> str:
    if not REASON_CODE_RE.match(code):
        raise ValueError(f"unsafe reason code: {code!r}")
    return code


def select_output_key(job: str) -> str:
    # Both mandatory web budget lanes use the same existing selection output.
    if job == "web-native-checks":
        job = "web-checks"
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
    # Git paths are relative and canonical. Reject unexpected separators or
    # traversal before any allowlist/prefix match, including synthetic inputs.
    if not path or "\\" in path or any(part in {"", ".", ".."} for part in path.split("/")):
        return "unknown"
    if path in _EXPLICIT_DOCS:
        return "docs"
    if _BROWSER_SPEC_RE.fullmatch(path) or path in _BROWSER_UI_HELPERS:
        return "web_tests"
    if path == "packages/editor/src/react/schema.tsx":
        return "broaden"
    if path in _EDITOR_UI_EXACT or (
        path.startswith(_EDITOR_UI_PREFIXES) and path.endswith(_UI_SUFFIXES)
    ):
        return "frontend_web_install"
    if re.fullmatch(r"packages/editor/test/[^/]+\.test\.ts", path):
        return "web_tests"
    if path in _BROADEN_EXACT:
        return "broaden"
    for prefix in _BROADEN_PREFIXES:
        if _starts_with(path, prefix):
            return "broaden"
    for marker in _MANIFEST_MARKERS:
        if marker in path:
            return "broaden"
    if _starts_with(path, "docs/"):
        return "broaden"
    for prefix in _WEB_BROADEN_PREFIXES:
        if _starts_with(path, prefix):
            return "broaden"
    if _starts_with(path, _FRONTEND_NARROW_PREFIX):
        if not path.endswith(_UI_SUFFIXES):
            return "broaden"
        if path.endswith((".test.ts", ".test.tsx")):
            return "web_tests"
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
    """Bind the tested merge to the exact PR head and a trusted base lineage.

    GitHub can regenerate refs/pull/N/merge after the event base advanced. Only
    a descendant of the event's trusted base may replace the first parent. The
    caller must still classify both the cumulative PR and actual merge diffs.
    """
    parents, err = git_commit_parents(repo, tested_sha)
    if err:
        return err
    if len(parents) != 2:
        return "FULL_PR_CHECKOUT_NOT_MERGE"
    if parents[1] != head_sha:
        return "FULL_PR_MERGE_PARENTS_MISMATCH"
    if parents[0] != base_sha:
        ancestry = _git(repo, "merge-base", "--is-ancestor", base_sha, parents[0])
        if ancestry.returncode != 0:
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
    # Known impact families compose by union; explanatory docs add no jobs.
    if "frontend_web_install" in families:
        reason = "NARROW_FRONTEND_WEB_INSTALL"
    elif "web_tests" in families:
        reason = "NARROW_WEB_TESTS"
    else:
        reason = "NARROW_DOCS"
    return SelectionDecision("narrow", reason, frozenset(families))


def workflow_job_selected(workflow: str, job: str, decision: SelectionDecision) -> bool:
    if job in OPT_IN_JOBS.get(workflow, {}):
        return False
    if decision.mode == "full":
        return True
    return (
        workflow in ("web", "install") and "frontend_web_install" in decision.families
    ) or (workflow == "web" and "web_tests" in decision.families)


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
    opt_in_inputs: frozenset[str] = frozenset(),
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
    if plan_ok and event_name == "workflow_dispatch":
        for job, input_name in OPT_IN_JOBS.get(workflow, {}).items():
            if input_name in opt_in_inputs:
                jobs[job]["selected"] = True

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


def dispatch_opt_ins(workflow: str, event_name: str, event: object) -> tuple[frozenset[str], str | None]:
    """Boolean opt-in inputs chosen by a workflow_dispatch event, fail closed.

    Every other event selects nothing, whatever its payload carries. GitHub writes
    dispatch booleans into the event file as the strings "true"/"false"; a missing
    input is not chosen, while an unknown input or any other value is an error.
    """
    if event_name != "workflow_dispatch":
        return frozenset(), None
    if not isinstance(event, dict):
        return frozenset(), "DISPATCH_EVENT_INVALID"
    raw = event.get("inputs")
    if raw is None:
        return frozenset(), None
    if not isinstance(raw, dict):
        return frozenset(), "DISPATCH_INPUTS_INVALID"
    allowed = set(OPT_IN_JOBS.get(workflow, {}).values())
    if set(raw) - allowed:
        return frozenset(), "DISPATCH_INPUTS_UNKNOWN"
    chosen: set[str] = set()
    for name, value in raw.items():
        if value is True or value == "true":
            chosen.add(name)
        elif not (value is False or value == "false"):
            return frozenset(), "DISPATCH_INPUT_VALUE_INVALID"
    return frozenset(chosen), None


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

    # A merge can contain conflict resolutions or injected files absent from the
    # PR head. Inspect the exact tested tree relative to its trusted first parent
    # even when both parents equal the event. Never accept an external path list.
    parents, parent_err = git_commit_parents(repo, tested_sha)
    if parent_err:
        return ResolvedInputs(None, parent_err, None, base_sha, head_sha, merge_base, tested_sha)
    merged_paths, diff_err = git_diff_paths(repo, parents[0], tested_sha)
    if diff_err:
        return ResolvedInputs(None, diff_err, None, base_sha, head_sha, merge_base, tested_sha)
    paths = sorted(set(paths or []) | set(merged_paths))

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
    "ubuntu-26.04": "x64",
    "ubuntu-26.04-arm": "arm64",
}
RUST_INTEGRATION_MANUAL_TARGETS: frozenset[str] = frozenset({"collab_capacity_probe"})
RUST_NATIVE_ARM64_STEP = "Native server build and policy tests (ARM64)"
RUST_NATIVE_ARM64_RUN = "cargo build --locked --offline --bins\ncargo test --locked --offline --lib"
RUST_DB_TESTS_FEATURE = "db-tests"
RUST_SELECTED_INSTALL_STEP = "Selected SQLite install lifetime controls"
RUST_SELECTED_INSTALL_TARGET = "selected_install_lifetime"
RUST_SELECTED_INSTALL_IF = "matrix.shard == 'b' && matrix.pg_major == '18'"
# Bind the actual owned setup, compiler artifact selection and execution/count gate.
RUST_SELECTED_INSTALL_RUN_SHA256 = "52996bd6cfcfc23baa6066cb096464f931e538895f7aee5b7c42650b47c77bad"
RUST_POSTGRES_INTEGRATION_STEP = "PostgreSQL integration tests"
RUST_S3_INTEGRATION_STEP = "S3-compatible storage integration tests (pinned test server)"
RUST_COLLAB_INTEGRATION_STEP = "WebSocket, PostgreSQL and native helper integration tests"
RUST_COLLAB_INTEGRATION_RUN = "bash scripts/run-rust-collaboration-ci-tests.sh"
RUST_COLLAB_MATRIX_RUNNERS = frozenset({"ubuntu-26.04", "ubuntu-26.04-arm"})
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
CARGO_TEST_NAME_RE = re.compile(r"^[A-Za-z0-9_-]+$")
RUST_POSTGRES_INTEGRATION_RUN_CANONICAL = (
    "cargo test --locked --offline --no-fail-fast --features db-tests ${{ matrix.tests }}"
)
RUST_S3_INTEGRATION_RUN_CANONICAL = (
    "bash scripts/start-test-minio.sh cargo test --locked --offline --no-fail-fast "
    "--features db-tests --test attachment_s3_integration"
)


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
        if not isinstance(tests_field, str):
            return {}, "rust: postgres matrix row missing tests command fragment"
        fragment_err = _validate_matrix_tests_fragment(tests_field)
        if fragment_err:
            return {}, fragment_err
        arch = RUST_POSTGRES_RUNNER_ARCH[runner]
        per_arch[arch].update(cargo_test_flags_in_text(tests_field))
    return per_arch, None


# Finite PG16 A budget split; these two complete targets alone move to C.
RUST_POSTGRES_C_TARGETS = frozenset({"task_integration", "comment_integration"})
RUST_POSTGRES_BUDGET = "${{ matrix.shard == 'b' && 20 || 15 }}"
RUST_POSTGRES_IMAGES = {
    "16": "postgres:16.15@sha256:1a6ab3f5345eb6dbe04a1349529caabdb0ab09293a09590fad07b2246bfa4b54",
    "17": "postgres:17.11@sha256:d74eeac9a635390a49bc21bd49fccd973de707e2a53a76ac49b552b8712ec46f",
    "18": "postgres:18.3@sha256:7e32e9833a6fb1c92c32552794cb6ed569d51b445a54907d35fc112ef39684db",
}
RUST_POSTGRES_BUILD_CACHE_KEY = (
    "v2-server-ubuntu-26.04-${{ runner.arch }}-1.98.1-db-db-tests-nodebug-"
    "${{ hashFiles('Cargo.lock', 'Cargo.toml', 'rust-toolchain.toml') }}-"
    "${{ hashFiles('src/**', 'tests/**', 'migrations/**', 'scripts/**', 'vendor/**', 'crates/collab-engine/src/**') }}-"
    "${{ steps.sqlite.outputs.cache_identity }}"
)


def verify_postgres_budget_matrix(jobs: dict) -> list[str]:
    """Keep twelve isolated A/B/C rows and equal complete coverage per pair."""
    job = jobs.get("postgres")
    if not isinstance(job, dict):
        return ["rust: PostgreSQL budget job missing"]
    errors: list[str] = []
    if job.get("timeout-minutes") != RUST_POSTGRES_BUDGET:
        errors.append("rust: PostgreSQL budget must retain A/C15m and B20m")
    if job.get("runs-on") != "${{ matrix.runner }}" or "continue-on-error" in job:
        errors.append("rust: PostgreSQL budget requires isolated matrix runners without error masking")
    strategy = job.get("strategy", {})
    if not isinstance(strategy, dict) or strategy.get("fail-fast") is not False:
        errors.append("rust: PostgreSQL budget must run every selected matrix row")
    matrix = strategy.get("matrix", {}) if isinstance(strategy, dict) else {}
    if not isinstance(matrix, dict) or set(matrix) != {"include"}:
        errors.append("rust: PostgreSQL budget matrix must use only explicit include rows")
    rows, err = _postgres_matrix_rows(job)
    if err:
        return errors + [err]
    expected = {
        (runner, major, shard): "postgres" + suffix + ("" if shard == "a" else "-" + shard)
        for runner, major, suffix in (
            ("ubuntu-26.04", "18", ""),
            ("ubuntu-26.04-arm", "18", "-arm64"),
            ("ubuntu-26.04", "16", "-pg16"),
            ("ubuntu-26.04", "17", "-pg17"),
        )
        for shard in ("a", "b", "c")
    }
    seen: set[tuple[str, str, str]] = set()
    per_pair: dict[tuple[str, str], set[str]] = {}
    for row in rows:
        key = (row.get("runner"), row.get("pg_major"), row.get("shard"))
        if any(not isinstance(value, str) for value in key) or key not in expected:
            errors.append("rust: PostgreSQL budget has an unsupported platform/major/shard row")
            continue
        if key in seen:
            errors.append("rust: PostgreSQL budget has a duplicate matrix row")
        seen.add(key)
        if set(row) != {"runner", "pg_major", "postgres_image", "check", "shard", "tests"}:
            errors.append("rust: PostgreSQL budget row must keep exact execution fields")
        if row.get("check") != expected[key] or row.get("postgres_image") != RUST_POSTGRES_IMAGES[key[1]]:
            errors.append("rust: PostgreSQL budget must retain check names and signed image pins")
        fragment = row.get("tests")
        if not isinstance(fragment, str) or _validate_matrix_tests_fragment(fragment):
            errors.append("rust: PostgreSQL budget requires complete --test target pairs")
            continue
        names = fragment.split()[1::2]
        targets = set(names)
        pair = key[:2]
        assigned = per_pair.setdefault(pair, set())
        if len(names) != len(targets) or assigned & targets:
            errors.append("rust: PostgreSQL budget duplicates a target within a platform/major pair")
        assigned.update(targets)
        if key[2] == "c" and targets != RUST_POSTGRES_C_TARGETS:
            errors.append("rust: PostgreSQL budget C must run exactly task/comment targets")
    if seen != set(expected):
        errors.append("rust: PostgreSQL budget requires all twelve A/B/C rows")
    baseline = per_pair.get(("ubuntu-26.04", "18"), set())
    if any(targets != baseline for targets in per_pair.values()):
        errors.append("rust: PostgreSQL budget must retain equal target coverage on every platform/major pair")
    cache = [step for step in job.get("steps", []) if isinstance(step, dict) and step.get("name") == "Restore server build outputs"]
    if len(cache) != 1 or cache[0].get("with") != {"path": "target", "key": RUST_POSTGRES_BUILD_CACHE_KEY}:
        errors.append("rust: PostgreSQL budget must retain strict complete-input server cache without restore fallback")
    return errors


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
    if "continue-on-error" in step and step.get("continue-on-error") is not False:
        return f"rust: {job} step {step_name!r} must not use continue-on-error"
    return None


def _collapse_shell_words(text: str) -> str:
    return " ".join(text.split())


def _cargo_command_suppression_error(
    norm: str, context: str, allowed_libtest_args: frozenset[str] = frozenset({"--nocapture"})
) -> str | None:
    if "--no-run" in norm:
        return f"rust: {context} must not use --no-run"
    if "--exclude" in norm:
        return f"rust: {context} must not use --exclude"
    for operator in ("||", "&&", "|", ";", "&"):
        if operator in norm:
            return f"rust: {context} must not contain shell operator {operator!r}"
    separator = " -- "
    if separator in norm:
        suffix = norm.split(separator, 1)[1].strip()
        if suffix not in allowed_libtest_args:
            return f"rust: {context} must not use libtest filter after --"
    return None


def _validate_matrix_tests_fragment(tests_field: str) -> str | None:
    trimmed = tests_field.strip()
    if not trimmed:
        return "rust: postgres matrix row missing tests command fragment"
    suppression = _cargo_command_suppression_error(trimmed, "postgres matrix tests")
    if suppression:
        return suppression
    tokens = trimmed.split()
    if not tokens or len(tokens) % 2 != 0:
        return "rust: postgres matrix tests must be --test NAME pairs only"
    for index in range(0, len(tokens), 2):
        if tokens[index] != "--test":
            return "rust: postgres matrix tests must be --test NAME pairs only"
        name = tokens[index + 1]
        if not CARGO_TEST_NAME_RE.match(name):
            return "rust: postgres matrix tests must be --test NAME pairs only"
    return None


def _validate_cargo_test_invocation(tokens: list[str], *, context: str, require_tests: bool) -> str | None:
    if len(tokens) < 2 or tokens[0] != "cargo" or tokens[1] != "test":
        return f"rust: {context} must invoke cargo test with --features db-tests"
    index = 2
    saw_locked = saw_offline = saw_no_fail_fast = saw_features = False
    saw_test = False
    while index < len(tokens):
        token = tokens[index]
        if token == "--locked":
            saw_locked = True
            index += 1
            continue
        if token == "--offline":
            saw_offline = True
            index += 1
            continue
        if token == "--no-fail-fast":
            saw_no_fail_fast = True
            index += 1
            continue
        if token == "--features":
            if index + 1 >= len(tokens) or tokens[index + 1] != RUST_DB_TESTS_FEATURE:
                return f"rust: {context} must invoke cargo test with --features db-tests"
            saw_features = True
            index += 2
            continue
        if token == "--test":
            if index + 1 >= len(tokens) or not CARGO_TEST_NAME_RE.match(tokens[index + 1]):
                return f"rust: {context} must use --test NAME pairs only"
            saw_test = True
            index += 2
            continue
        if token == "${{":
            if (
                index + 2 < len(tokens)
                and tokens[index + 1] == "matrix.tests"
                and tokens[index + 2] == "}}"
            ):
                index += 3
                continue
        return f"rust: {context} must not use unknown cargo test flag {token!r}"
    if not (saw_locked and saw_offline and saw_no_fail_fast and saw_features):
        return f"rust: {context} must invoke cargo test with --features db-tests"
    if require_tests and not saw_test:
        return f"rust: {context} must declare at least one --test target"
    return None


def _verify_postgres_integration_run(run: str) -> str | None:
    norm = _normalize_run_script(run).strip()
    if norm.startswith("echo "):
        return (
            "rust: PostgreSQL integration step must execute "
            "cargo test with --features db-tests and ${{ matrix.tests }}"
        )
    suppression = _cargo_command_suppression_error(norm, "PostgreSQL integration step")
    if suppression:
        return suppression
    collapsed = _collapse_shell_words(norm)
    if collapsed != RUST_POSTGRES_INTEGRATION_RUN_CANONICAL:
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
    suppression = _cargo_command_suppression_error(norm, "S3 integration step")
    if suppression:
        return suppression
    collapsed = _collapse_shell_words(norm)
    if collapsed != RUST_S3_INTEGRATION_RUN_CANONICAL:
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


# libtest scheduling the collaboration script may pass after `--`: never a
# filter, skip or ignored selection.
RUST_COLLAB_LIBTEST_ARGS = frozenset({"--nocapture", "--test-threads=1"})


def _collaboration_script_cargo_commands(text: str) -> list[str]:
    """Every `cargo test` invocation with its `--test` continuations and an
    optional final `-- ...` libtest line."""
    lines = text.splitlines()
    commands: list[str] = []
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
            if continuation.startswith("-- "):
                parts.append(continuation.rstrip("\\").strip())
            break
        commands.append(" ".join(parts))
    return commands


def collaboration_script_inventory(repo_root: Path) -> tuple[set[str], str | None]:
    script_path = repo_root / RUST_COLLAB_CI_SCRIPT
    if not script_path.is_file():
        return set(), f"rust: missing collaboration CI script {RUST_COLLAB_CI_SCRIPT}"
    text = script_path.read_text(encoding="utf-8")
    cargo_commands = _collaboration_script_cargo_commands(text)
    if not cargo_commands:
        return set(), "rust: collaboration CI script missing cargo test invocation"
    tests: set[str] = set()
    for cargo_command in cargo_commands:
        suppression = _cargo_command_suppression_error(
            cargo_command, "collaboration CI script cargo test", RUST_COLLAB_LIBTEST_ARGS
        )
        if suppression:
            return set(), suppression
        cargo_args = cargo_command.split(" -- ", 1)[0]
        shape_err = _validate_cargo_test_invocation(
            cargo_args.split(),
            context="collaboration CI script cargo test",
            require_tests=True,
        )
        if shape_err:
            return set(), shape_err
        invocation_tests = cargo_test_flags_in_text(cargo_args)
        repeated = sorted(tests & invocation_tests)
        if repeated:
            return set(), (
                "rust: collaboration CI script runs --test targets more than once: "
                + ", ".join(repeated)
            )
        tests |= invocation_tests
    if not tests:
        return set(), "rust: collaboration CI script declares no --test targets"
    return tests, None


def verify_native_arm64_execution(jobs: dict) -> list[str]:
    """Keep the former ARM A default-feature check mandatory after the job split."""
    job = jobs.get("native-arm64")
    if not isinstance(job, dict):
        return ["rust: native-arm64 job missing"]
    errors: list[str] = []
    if job.get("runs-on") != "ubuntu-26.04-arm" or "strategy" in job:
        errors.append("rust: native-arm64 must run once on ubuntu-26.04-arm")
    if job.get("timeout-minutes") != 15:
        errors.append("rust: native-arm64 must keep the 15 minute budget")
    if "continue-on-error" in job:
        errors.append("rust: native-arm64 must fail on build/policy errors")
    steps = [step for step in _run_steps(job) if step.get("name") == RUST_NATIVE_ARM64_STEP]
    if len(steps) != 1:
        errors.append("rust: native-arm64 must execute the default build/policy step exactly once")
    elif (
        steps[0]["run"].strip() != RUST_NATIVE_ARM64_RUN
        or "if" in steps[0]
        or "continue-on-error" in steps[0]
        or "env" in steps[0]
    ):
        errors.append("rust: native-arm64 must execute the exact unconditional default build/policy commands")
    if any(step.get("name") == RUST_NATIVE_ARM64_STEP for step in _run_steps(jobs.get("postgres", {}))):
        errors.append("rust: default ARM build/policy must not share the PostgreSQL job budget")
    return errors


def selected_install_inventory(jobs: dict) -> tuple[set[str], str | None]:
    """Require the privileged SQLite fixture on both actual PG18 B runners."""
    steps, err = _postgres_job_steps(jobs)
    if err:
        return set(), err
    assert steps is not None
    step, err = _unique_named_step(steps, RUST_SELECTED_INSTALL_STEP, job="postgres")
    if err:
        return set(), err
    assert step is not None
    if "continue-on-error" in step or step.get("if") != RUST_SELECTED_INSTALL_IF:
        return set(), "rust: selected install step must execute on PG18 B without error masking"
    expected_env = {
        "FVOCI_COLLAB_ENGINE": "${{ github.workspace }}/crates/collab-engine/target/debug/collab-engine"
    }
    run = step.get("run")
    if (
        step.get("env") != expected_env
        or not isinstance(run, str)
        or hashlib.sha256(run.strip().encode()).hexdigest() != RUST_SELECTED_INSTALL_RUN_SHA256
    ):
        return set(), (
            "rust: selected install step must keep exact db-tests build, root inputs, "
            "unfiltered execution and count gate"
        )
    helper, err = _unique_named_step(
        steps, "Build production helper for PostgreSQL B native fixtures", job="postgres"
    )
    if err:
        return set(), err
    assert helper is not None
    expected_helper = (
        "cargo fetch --locked --manifest-path crates/collab-engine/Cargo.toml\n"
        "cargo build --locked --offline --manifest-path crates/collab-engine/Cargo.toml "
        "--features worker --bin collab-engine"
    )
    if (
        helper.get("if") != "matrix.shard == 'b'"
        or "continue-on-error" in helper
        or helper.get("env") != {"CARGO_TARGET_DIR": "${{ github.workspace }}/crates/collab-engine/target"}
        or str(helper.get("run", "")).strip() != expected_helper
        or steps.index(helper) >= steps.index(step)
    ):
        return set(), "rust: selected install requires the preceding mandatory production worker helper build"
    rows, err = _postgres_matrix_rows(jobs["postgres"])
    if err:
        return set(), err
    assert rows is not None
    runners = [
        row.get("runner") for row in rows
        if row.get("shard") == "b" and row.get("pg_major") == "18"
    ]
    if set(runners) != set(RUST_POSTGRES_RUNNER_ARCH) or len(runners) != 2:
        return set(), "rust: selected install requires exactly one PG18 B execution on x64 and arm64"
    return {RUST_SELECTED_INSTALL_TARGET}, None


RUST_SCHEMA_BASELINE_STEP = "Schema baseline SQLite controls and prepared PostgreSQL catalog"
RUST_SCHEMA_BASELINE_RUN_SHA256 = "ee031ee20abc5405b838fe9f2b4073f0a392ca5649ebff3eb9ced9d866850c91"


def schema_baseline_inventory(jobs: dict) -> tuple[set[str], str | None]:
    """Two actual local SDK controls and the configured catalog inspection tool.
    The catalog uses an owned prepared DB/owner, not a normal-server credential.
    """
    steps, err = _postgres_job_steps(jobs)
    if err:
        return set(), err
    step, err = _unique_named_step(steps, RUST_SCHEMA_BASELINE_STEP, job="postgres")
    if err:
        return set(), err
    expected_env = {
        "PG_CONTAINER": "${{ job.services.postgres.id }}",
        "PREPARATION_DATABASE_URL": "postgres://postgres:ci-ephemeral-only@127.0.0.1:${{ job.services.postgres.ports['5432'] }}/postgres",
    }
    if (step.get("if") != "matrix.shard == 'a'" or "continue-on-error" in step
            or step.get("env") != expected_env
            or hashlib.sha256(str(step.get("run", "")).strip().encode()).hexdigest() != RUST_SCHEMA_BASELINE_RUN_SHA256):
        return set(), "rust: schema baseline requires exact configured extraction, 2+1 actual controls and owned cleanup"
    rows, err = _postgres_matrix_rows(jobs["postgres"])
    if err:
        return set(), err
    actual = [(row.get("runner"), row.get("pg_major")) for row in rows if row.get("shard") == "a"]
    expected = [("ubuntu-26.04", "16"), ("ubuntu-26.04", "17"),
                ("ubuntu-26.04", "18"), ("ubuntu-26.04-arm", "18")]
    if sorted(actual) != sorted(expected):
        return set(), "rust: schema baseline requires PG16/17/18 x64 and PG18 arm64 A execution"
    return {"schema_baseline_integration"}, None


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

    errors.extend(verify_native_arm64_execution(jobs))
    errors.extend(verify_postgres_budget_matrix(jobs))
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

    install_tests, install_err = selected_install_inventory(jobs)
    if install_err:
        errors.append(install_err)
        return errors

    schema_tests, schema_err = schema_baseline_inventory(jobs)
    if schema_err:
        errors.append(schema_err)
        return errors

    postgres_union = per_arch["x64"]
    overlap = (
        (postgres_union & collab_tests) | (postgres_union & s3_tests)
        | (collab_tests & s3_tests)
        | (install_tests & (postgres_union | collab_tests | s3_tests))
        | (schema_tests & (postgres_union | collab_tests | s3_tests | install_tests))
    )
    if overlap:
        errors.append(
            "rust: integration target assigned to multiple CI buckets: "
            + ", ".join(sorted(overlap))
        )

    if RUST_INTEGRATION_MANUAL_TARGETS & cargo_targets:
        probe_script = repo_root / RUST_CAPACITY_PROBE_SCRIPT
        if not probe_script.is_file():
            errors.append(f"rust: missing manual probe script {RUST_CAPACITY_PROBE_SCRIPT}")

    assigned = postgres_union | collab_tests | s3_tests | install_tests | schema_tests | RUST_INTEGRATION_MANUAL_TARGETS
    required = cargo_targets - RUST_INTEGRATION_MANUAL_TARGETS
    missing = sorted(required - assigned)
    if missing:
        errors.append(
            "rust: Cargo.toml db-tests integration targets missing from rust.yml inventory: "
            + ", ".join(missing)
        )

    return errors


def _verify_web_build_handoff(jobs: dict) -> list[str]:
    """The only admitted cross-job native consumer, bound to this run's producer."""
    errors = []
    def require(ok, message):
        if not ok: errors.append("web: current build handoff " + message)
    producer = jobs.get("collaboration-build", {})
    consumer = jobs.get("collaboration-flow", {})
    require(producer.get("needs") == "ci-plan", "producer needs ci-plan")
    require(consumer.get("needs") == ["ci-plan", "collaboration-build"], "consumer needs successful registered producer")
    for name, job in (("collaboration-build", producer), ("collaboration-flow", consumer)):
        require(job.get("runs-on") == "ubuntu-26.04" and job.get("timeout-minutes") == 15, "fixed runner/budget")
        require(not any(k in job for k in ("continue-on-error", "strategy", "env", "permissions")), "no masked/alternate authority")
        checkout = [step for step in job.get("steps", []) if str(step.get("uses", "")).startswith("actions/checkout@")]
        require(len(checkout) == 1 and checkout[0].get("with") == {"persist-credentials": False}, "default exact checkout without stored credentials")
        require(job.get("if") == "needs.ci-plan.outputs.select_" + name.replace("-", "_") + " == 'true'", "selection only by registered plan")
    # Pending registration controls are DB-free, but not part of apps/web's
    # default src-only unit discovery. Require their explicit mandatory consumer.
    unit = [step for step in jobs.get("web-checks", {}).get("steps", [])
            if step.get("name") == "Web and editor unit regressions"]
    required = ("(cd apps/web && bun run test)", "(cd packages/editor && bun run test)",
                "(cd apps/web && bun test e2e-pending/collab-playwright.config.test.ts --timeout 60000)",
                "python3 scripts/selected-backend-ci/test_off_registration.py")
    require(len(unit) == 1 and unit[0].get("run", "").splitlines() == list(required)
            and not any(k in unit[0] for k in ("if", "continue-on-error")),
            "mandatory complete web/editor and selected registration fixtures")
    steps = producer.get("steps", [])
    prepare = [step for step in steps if step.get("id") == "prepare"]
    require(len(prepare) == 1 and prepare[0].get("env") == {"FVOCI_E2E_PENDING": "1"}
            and "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-prepare-selected" in prepare[0].get("run", "")
            and not any(k in prepare[0] for k in ("if", "continue-on-error")), "unconditional qualified producer")
    publish = [step for step in steps if step.get("id") == "publish"]
    require(len(publish) == 1 and publish[0].get("uses") == "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02"
            and publish[0].get("with") == {"name": "web-current-build-${{ github.run_attempt }}",
                "path": "${{ runner.temp }}/fvoci-web-build-handoff/handoff.json\n${{ runner.temp }}/fvoci-web-build-handoff/payload.tar\n",
                "if-no-files-found": "error", "retention-days": 1}
            and not any(k in publish[0] for k in ("if", "continue-on-error")), "publish only successful complete packet")
    require(producer.get("outputs") == {"artifact_id": "${{ steps.publish.outputs.artifact-id }}",
            "handoff_sha256": "${{ steps.prepare.outputs.handoff_sha256 }}"}, "producer artifact identity and digest outputs")
    steps = consumer.get("steps", [])
    download = [step for step in steps if str(step.get("uses", "")).startswith("actions/download-artifact@")]
    require(len(download) == 1 and download[0].get("uses") == "actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093"
            and download[0].get("with") == {"artifact-ids": "${{ needs.collaboration-build.outputs.artifact_id }}", "merge-multiple": True,
                "path": "${{ runner.temp }}/fvoci-web-build-handoff"}
            and not any(k in download[0] for k in ("if", "continue-on-error")), "current-run exact artifact ID without foreign token/ref/run")
    runtime = [step for step in steps if step.get("id") == "browser"]
    require(len(runtime) == 1 and runtime[0].get("env") == {"FVOCI_E2E_PENDING": "1",
                "FVOCI_WEB_BUILD_HANDOFF_SHA256": "${{ needs.collaboration-build.outputs.handoff_sha256 }}"}
            and "bash scripts/run-web-e2e.sh --ci-use-committed-api --ci-consume-selected" in runtime[0].get("run", "")
            and not any(k in runtime[0] for k in ("if", "continue-on-error")), "mandatory full original runtime after qualification")
    require(not any(step.get("with", {}).get("path") in ("target", "crates/collab-engine/target")
            for step in steps if str(step.get("uses", "")).startswith("actions/cache@")), "consumer cannot borrow target cache")
    return errors


def verify_workflow_registry(repo_root: Path = ROOT) -> list[str]:
    errors: list[str] = []
    workflows_dir = repo_root / ".github" / "workflows"
    allowed_files = {*WORKFLOW_YAML.values(), RELEASE_WORKFLOW_FILE, TURSO_MANUAL_WORKFLOW_FILE}
    discovered_files = list_workflow_files(repo_root)
    if not workflows_dir.is_dir():
        errors.append("missing .github/workflows directory")
        return errors

    for path in discovered_files:
        if path.name not in allowed_files:
            errors.append(f"unknown workflow file {path.name}")
        if path.name in allowed_files:
            data, parse_err = _load_yaml_mapping(path)
            if parse_err:
                continue  # The workflow-specific validator reports parse errors.
            jobs = data.get("jobs")
            if not isinstance(jobs, dict):
                continue
            for job_id, job in jobs.items():
                if not isinstance(job, dict):
                    continue
                runner = job.get("runs-on")
                runners = [runner]
                if runner == "${{ matrix.runner }}":
                    strategy = job.get("strategy")
                    matrix = strategy.get("matrix") if isinstance(strategy, dict) else None
                    rows = matrix.get("include", []) if isinstance(matrix, dict) else []
                    runners = [row.get("runner") for row in rows if isinstance(row, dict)]
                if not runners or any(not isinstance(label, str) or label not in RUST_POSTGRES_RUNNER_ARCH for label in runners):
                    errors.append(f"{path.name}: {job_id} requires explicit Ubuntu 26.04 runners")
                for step in job.get("steps", []):
                    if not isinstance(step, dict) or not str(step.get("uses", "")).startswith("actions/cache@"):
                        continue
                    cache = step.get("with", {})
                    # This source-only cache is revalidated by fetch-rhwp.sh.
                    if cache.get("path") == "crates/document-extract/.vendor-src/rhwp":
                        continue
                    for field in ("key", "restore-keys"):
                        if field in cache and any("ubuntu-26.04-${{ runner.arch }}-1.98.1-" not in line for line in str(cache[field]).splitlines()):
                            errors.append(f"{path.name}: {job_id} cache {field} must bind Ubuntu 26.04, architecture and toolchain")

    for workflow, filename in WORKFLOW_YAML.items():
        path = workflows_dir / filename
        if not path.is_file():
            errors.append(f"{workflow}: missing workflow file {filename}")
            continue
        data, parse_err = _load_yaml_mapping(path)
        if parse_err:
            errors.append(f"{workflow}: {parse_err}")
            continue
        triggers = data.get("on", data.get(True))
        if not isinstance(triggers, dict) or "pull_request" not in triggers:
            errors.append(f"{workflow}: pull_request trigger is required for the stable gate")
        elif triggers["pull_request"] is not None:
            errors.append(f"{workflow}: pull_request must be unfiltered so required gates always run")
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
            # The ancestry exception trusts only GitHub's merge SHA for this
            # event. Pin that assumption to normal checkout (no alternate ref
            # or repository) and prevent YAML from replacing the runner SHA.
            plan_steps = plan_job.get("steps", [])
            checkouts = [
                step for step in plan_steps
                if isinstance(step, dict) and str(step.get("uses", "")).startswith("actions/checkout@")
            ] if isinstance(plan_steps, list) else []
            if len(checkouts) != 1 or checkouts[0].get("with") != {"fetch-depth": 0}:
                errors.append(f"{workflow}: ci-plan must checkout the event merge with fetch-depth: 0 and no ref override")
            envs = [data.get("env"), plan_job.get("env")]
            if isinstance(plan_steps, list):
                envs.extend(step.get("env") for step in plan_steps if isinstance(step, dict))
            if any(isinstance(env, dict) and "GITHUB_SHA" in env for env in envs):
                errors.append(f"{workflow}: ci-plan must not override trusted GITHUB_SHA")
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

        errors.extend(_verify_opt_in_wiring(workflow, data, jobs))
        if workflow == "web":
            errors.extend(_verify_web_build_handoff(jobs))

    release_path = workflows_dir / RELEASE_WORKFLOW_FILE
    if release_path.is_file():
        errors.extend(verify_release_workflow(release_path))

    turso_path = workflows_dir / TURSO_MANUAL_WORKFLOW_FILE
    if turso_path.is_file():
        errors.extend(verify_turso_workflow(turso_path))

    errors.extend(verify_rust_suite_registry(repo_root))
    return errors


def verify_turso_workflow(path: Path) -> list[str]:
    """Named manual-only exception; never changes PR selection or stable gates."""
    data, parse_err = _load_yaml_mapping(path)
    if parse_err:
        return [f"{path.name}: {parse_err}"]
    errors: list[str] = []

    def require(condition: bool, boundary: str) -> None:
        if not condition:
            errors.append(f"{path.name}: {boundary}")

    triggers = data.get("on", data.get(True))
    require(isinstance(triggers, dict) and set(triggers) == {"push", "workflow_dispatch"} and triggers.get("push") == {"branches": ["fvoci/v060-turso-verified-connection"]}, "manual dispatch only with fixed credential-free bootstrap")
    dispatch = triggers.get("workflow_dispatch", {}) if isinstance(triggers, dict) else {}
    require(dispatch.get("inputs") == {
        "phase": {"description": "Connection read-only; migration requires both destructive gates; others NOT IMPLEMENTED", "type": "choice", "default": "connection", "options": ["connection", "crud", "transactions", "migration", "persistence", "restore", "ui-ack"]},
        "destructive": {"description": "Explicit isolated test DB mutation confirmation (connection must be false)", "type": "boolean", "default": False},
    } if isinstance(dispatch, dict) else False, "fixed phase inputs and non-destructive default")
    require(data.get("permissions") == {"contents": "read"}, "contents read only")
    require("env" not in data, "no global credential environment")
    require(data.get("concurrency") == {"group": "fvoci-turso-test-database", "cancel-in-progress": False}, "fixed database concurrency without cancellation")
    jobs = data.get("jobs")
    if not isinstance(jobs, dict) or set(jobs) != {"admission", "turso-connection"}:
        return [*errors, f"{path.name}: exactly admission and turso-connection jobs required"]
    trusted = "github.event_name == 'workflow_dispatch' && github.repository == 'AISFlow/fvoci' && (github.ref == 'refs/heads/main' || github.ref == 'refs/heads/fvoci/v060-turso-verified-connection')"
    bootstrap_admission = "github.repository == 'AISFlow/fvoci' && ((github.event_name == 'push' && github.ref == 'refs/heads/fvoci/v060-turso-verified-connection') || (github.event_name == 'workflow_dispatch' && (github.ref == 'refs/heads/main' || github.ref == 'refs/heads/fvoci/v060-turso-verified-connection')))"
    checkout = {"uses": "actions/checkout@11d5960a326750d5838078e36cf38b85af677262", "with": {"ref": "${{ github.sha }}", "persist-credentials": False}}
    admission = jobs["admission"]
    runtime = jobs["turso-connection"]
    if not isinstance(admission, dict) or not isinstance(runtime, dict):
        return [*errors, f"{path.name}: job mappings required"]
    require(set(admission) == {"if", "runs-on", "timeout-minutes", "outputs", "steps"}, "admission has no Environment or credentials")
    require(admission.get("if") == bootstrap_admission and admission.get("outputs") == {"environment_id": "${{ steps.admit.outputs.environment_id }}"}, "trusted admission and existence output")
    require(admission.get("steps") == [checkout,
        {"name": "Pure admission fixtures (no credentials or network)", "run": "python3 scripts/selected-backend-ci/turso-test-fixtures.py"},
        {"name": "Verify preexisting Environment (no configuration writes)", "id": "admit", "run": "python3 scripts/selected-backend-ci/turso-test-guard.py --admit"}], "pre-Environment admission steps")
    require(set(runtime) == {"needs", "if", "environment", "runs-on", "timeout-minutes", "env", "steps"}, "runtime job cannot add unchecked execution or permissions")
    require(runtime.get("needs") == "admission" and runtime.get("if") == trusted + " && needs.admission.result == 'success' && needs.admission.outputs.environment_id != ''", "runtime needs successful trusted admission")
    require(runtime.get("environment") == "fvoci-turso-test", "fixed Environment")
    require(runtime.get("env") == {"LIBCLANG_PATH": "/usr/lib/llvm-18/lib", "CARGO_BUILD_JOBS": 4, "CARGO_INCREMENTAL": 0, "CARGO_PROFILE_DEV_DEBUG": 0, "CARGO_PROFILE_TEST_DEBUG": 0}, "credential-free compiler environment")
    require(admission.get("runs-on") == runtime.get("runs-on") == "ubuntu-26.04" and admission.get("timeout-minutes") == 5 and runtime.get("timeout-minutes") == 15, "fixed runner and budgets")
    steps = runtime.get("steps")
    if not isinstance(steps, list) or len(steps) != 5 or not all(isinstance(step, dict) for step in steps):
        return [*errors, f"{path.name}: fixed credential-free build then single consuming step"]
    require(steps[0] == checkout, "exact SHA checkout with stripped credentials")
    require(set(steps[1]) == set(steps[2]) == {"name", "run"}, "no compilation credentials")
    require(steps[1].get("run") == "set -euo pipefail\nprintf 'CARGO_TARGET_DIR=%s/turso-target\\n' \"$RUNNER_TEMP\" >> \"$GITHUB_ENV\"\nrustup toolchain install 1.98.1 --profile minimal\nsudo apt-get update\nsudo apt-get install -y --no-install-recommends python3 gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8\nmkdir \"$RUNNER_TEMP/fvoci-sqlite\"\ndpkg-query -W > \"$RUNNER_TEMP/fvoci-sqlite/build-packages.txt\"\nbash scripts/prepare-sqlite-ci.sh --parent \"$RUNNER_TEMP/fvoci-sqlite\" \\\n  --github-env \"$GITHUB_ENV\" --github-output \"$GITHUB_OUTPUT\"\ncargo fetch --locked\n", "maintained pinned compiler/native preparation")
    require(steps[2].get("run") == "set -euo pipefail\ncargo test --locked --offline --lib --features db-tests --jobs 4 --no-run --message-format=json > \"$RUNNER_TEMP/turso-compile.json\"\npython3 scripts/selected-backend-ci/turso-test-guard.py --freeze\n", "fixed fresh compilation and ELF binding")
    require(steps[3] == {"name": "Credential-free frozen diagnostic unit (exactly one test)",
        "run": "python3 scripts/selected-backend-ci/turso-test-guard.py --diagnostic-unit"}, "credential-free exact frozen diagnostic unit before secret consumption")
    require(steps[4] == {"name": "Real primary selected phase (exactly one test)", "env": {
        "FVOCI_DATABASE_BACKEND": "libsql-remote",
        "FVOCI_LIBSQL_URL": "${{ secrets.FVOCI_TEST_TURSO_DATABASE_URL }}",
        "FVOCI_LIBSQL_AUTH_TOKEN": "${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}",
        "FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE": "${{ vars.FVOCI_TEST_TURSO_ALLOW_DESTRUCTIVE }}",
    }, "run": "python3 scripts/selected-backend-ci/turso-test-guard.py --consume"}, "only one sanitized runtime step consumes two secrets")
    return errors


def verify_release_workflow(path: Path) -> list[str]:
    """Only tag pushes and manual dispatch; read-only default token; scoped writes."""
    name = path.name
    data, parse_err = _load_yaml_mapping(path)
    if parse_err:
        return [f"{name}: {parse_err}"]
    errors: list[str] = []
    triggers = data.get("on", data.get(True))
    if not isinstance(triggers, dict) or set(triggers) != {"push", "workflow_dispatch"}:
        errors.append(f"{name}: triggers must be exactly push (tags) and workflow_dispatch")
    else:
        push = triggers["push"]
        tags = push.get("tags") if isinstance(push, dict) else None
        if (
            not isinstance(push, dict)
            or set(push) != {"tags"}
            or not isinstance(tags, list)
            or not tags
            or not all(isinstance(tag, str) and tag.startswith("v0.") for tag in tags)
        ):
            errors.append(f"{name}: push must list only v0.* tags")
    if data.get("permissions") != {"contents": "read"}:
        errors.append(f"{name}: top-level permissions must be exactly contents: read")
    # One queue for every tag: runs for two patch tags must not race on :0.y.
    concurrency = data.get("concurrency")
    if (
        not isinstance(concurrency, dict)
        or not isinstance(concurrency.get("group"), str)
        or "${{" in concurrency["group"]
        or concurrency.get("cancel-in-progress") is not False
    ):
        errors.append(
            f"{name}: concurrency must be one fixed group with cancel-in-progress: false"
        )
    jobs = data.get("jobs")
    if not isinstance(jobs, dict) or not jobs:
        return [*errors, f"{name}: jobs mapping missing"]
    for job_id, spec in jobs.items():
        if not isinstance(spec, dict):
            errors.append(f"{name}: {job_id} must be a mapping")
            continue
        permissions = spec.get("permissions", {})
        if not isinstance(permissions, dict):
            errors.append(f"{name}: {job_id} permissions must be a scope mapping")
            continue
        writes = {scope for scope, level in permissions.items() if level == "write"}
        allowed = RELEASE_WRITE_SCOPES.get(job_id, frozenset())
        if not writes <= allowed:
            errors.append(
                f"{name}: {job_id} may not write {sorted(writes - allowed)}"
            )
    return errors


def _verify_opt_in_wiring(workflow: str, data: dict, jobs: dict) -> list[str]:
    """workflow_dispatch declares exactly the opt-in booleans (default false)."""
    errors: list[str] = []
    triggers = data.get("on", data.get(True))
    dispatch = triggers.get("workflow_dispatch") if isinstance(triggers, dict) else None
    if not isinstance(triggers, dict) or "workflow_dispatch" not in triggers:
        errors.append(f"{workflow}: workflow_dispatch trigger missing")
        return errors
    if dispatch is not None and not isinstance(dispatch, dict):
        errors.append(f"{workflow}: workflow_dispatch must be a mapping")
        return errors
    inputs = (dispatch or {}).get("inputs")
    if inputs is not None and not isinstance(inputs, dict):
        errors.append(f"{workflow}: workflow_dispatch inputs must be a mapping")
        return errors
    opt_ins = OPT_IN_JOBS.get(workflow, {})
    expected_inputs = set(opt_ins.values())
    if set(inputs or {}) != expected_inputs:
        errors.append(
            f"{workflow}: workflow_dispatch inputs must be exactly {sorted(expected_inputs)}"
        )
        return errors
    for name in expected_inputs:
        spec = inputs[name]
        if not isinstance(spec, dict) or spec.get("type") != "boolean" or spec.get("default") is not False:
            errors.append(f"{workflow}: input {name} must be type boolean with default false")
    for job in opt_ins:
        spec = jobs.get(job)
        if not isinstance(spec, dict):
            continue
        if spec.get("runs-on") != OPT_IN_RUNNER[job]:
            errors.append(f"{workflow}: {job} runs-on must be {OPT_IN_RUNNER[job]}")
        if "strategy" in spec:
            errors.append(f"{workflow}: {job} must be a single job without a matrix")
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
    opt_ins, opt_in_err = dispatch_opt_ins(args.workflow, event_name, event)

    plan = build_plan(
        workflow=args.workflow,
        event_name=event_name,
        base_sha=resolved.base_sha,
        head_sha=resolved.head_sha,
        merge_base_sha=resolved.merge_base_sha,
        tested_sha=resolved.tested_sha,
        paths=resolved.paths,
        fatal_error=resolved.fatal_error or opt_in_err,
        force_full_reason=resolved.force_full_reason,
        opt_in_inputs=opt_ins,
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


def _gate_opt_in_error(workflow: str, plan: dict) -> str | None:
    """Re-derive manual opt-ins from this run's event file; the plan cannot override them."""
    opt_ins = OPT_IN_JOBS.get(workflow)
    if not opt_ins:
        return None
    event_name = os.environ.get("GITHUB_EVENT_NAME", "").strip()
    if event_name not in KNOWN_EVENTS:
        return "EVENT_NAME"
    event_path = os.environ.get("GITHUB_EVENT_PATH", "").strip()
    if not event_path:
        return "EVENT_PATH_MISSING"
    try:
        event = load_event(Path(event_path))
    except (OSError, ValueError):
        return "EVENT_MALFORMED"
    chosen, err = dispatch_opt_ins(workflow, event_name, event)
    if err:
        return err
    for job, input_name in opt_ins.items():
        if plan["jobs"][job]["selected"] is not (input_name in chosen):
            return f"OPT_IN_MISMATCH {job}"
    return None


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

    opt_in_err = _gate_opt_in_error(args.workflow, plan)
    if opt_in_err:
        print(f"gate: opt-in error {opt_in_err}", file=sys.stderr)
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
