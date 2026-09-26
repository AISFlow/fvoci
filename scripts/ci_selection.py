#!/usr/bin/env python3
"""Fail-closed CI job selection from cumulative PR diffs and workflow gates."""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Iterable, Literal

ROOT = Path(__file__).resolve().parent.parent

Mode = Literal["full", "narrow"]
NarrowFamily = Literal["docs", "frontend", "native_documents", "native_collab"]

PLAN_VERSION = 1

# Product jobs per workflow (stable ids; matrix jobs are one logical job each).
WORKFLOW_JOBS: dict[str, tuple[str, ...]] = {
    "web": ("web-checks", "workspace-browser-shard", "collaboration-flow"),
    "rust": ("fast", "postgres", "collaboration"),
    "documents": ("native-extraction",),
    "collab-engine": ("native-collab-engine",),
    "install": ("install-smoke", "backup-restore-smoke"),
}

WORKFLOW_FILE = ROOT / ".github" / "workflows"

# Paths that always require the full CI battery (shared build, auth, DB, harness, toolchain).
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

_DOCS_PREFIXES: tuple[str, ...] = ("docs/",)
_DOCS_EXACT: frozenset[str] = frozenset({"README.md", "RUNNING.md", "CONTRIBUTING.md"})
_DOCS_SUFFIX_PATHS: tuple[str, ...] = (
    "crates/document-extract/VERIFY.md",
    "crates/collab-engine/README.md",
)

_FRONTEND_PREFIXES: tuple[str, ...] = ("apps/web/", "packages/")

_NATIVE_DOCUMENTS_PREFIXES: tuple[str, ...] = (
    "crates/document-extract/",
    "crates/document-extract-client/",
)
_NATIVE_COLLAB_PREFIXES: tuple[str, ...] = ("crates/collab-engine/",)

_CRATE_README_RE = re.compile(r"^crates/[^/]+/README\.md$")


def _starts_with(path: str, prefix: str) -> bool:
    return path == prefix or path.startswith(prefix)


def classify_path(path: str) -> NarrowFamily | Literal["broaden"] | Literal["unknown"]:
    if path in _BROADEN_EXACT:
        return "broaden"
    for prefix in _BROADEN_PREFIXES:
        if _starts_with(path, prefix):
            return "broaden"
    if path in _DOCS_EXACT or _CRATE_README_RE.match(path):
        return "docs"
    for prefix in _DOCS_PREFIXES:
        if _starts_with(path, prefix):
            return "docs"
    if path in _DOCS_SUFFIX_PATHS:
        return "docs"
    for prefix in _FRONTEND_PREFIXES:
        if _starts_with(path, prefix):
            return "frontend"
    for prefix in _NATIVE_DOCUMENTS_PREFIXES:
        if _starts_with(path, prefix):
            return "native_documents"
    for prefix in _NATIVE_COLLAB_PREFIXES:
        if _starts_with(path, prefix):
            return "native_collab"
    return "unknown"


def parse_name_status_z(data: bytes) -> list[str]:
    """Return every path touched in a cumulative diff (including rename source/dest)."""
    fields = [
        part.decode("utf-8", errors="surrogateescape")
        for part in data.split(b"\0")
        if part
    ]
    paths: list[str] = []
    i = 0
    while i < len(fields):
        status = fields[i]
        i += 1
        if not status:
            continue
        kind = status[0]
        if kind in ("R", "C"):
            if i + 1 >= len(fields):
                break
            paths.append(fields[i])
            paths.append(fields[i + 1])
            i += 2
        elif kind == "D":
            if i >= len(fields):
                break
            paths.append(fields[i])
            i += 1
        else:
            if i >= len(fields):
                break
            paths.append(fields[i])
            i += 1
    return paths


def git_diff_paths(repo: Path, base: str, head: str) -> tuple[list[str], str | None]:
    try:
        proc = subprocess.run(
            ["git", "diff", "--name-status", "-z", "-M", base, head],
            cwd=repo,
            check=False,
            capture_output=True,
        )
    except OSError as exc:
        return [], f"git diff failed to start: {exc}"
    if proc.returncode != 0:
        err = proc.stderr.decode("utf-8", errors="replace").strip()
        return [], f"git diff exit {proc.returncode}: {err}"
    paths = parse_name_status_z(proc.stdout)
    return paths, None


def git_rev_parse(repo: Path, ref: str) -> tuple[str | None, str | None]:
    try:
        proc = subprocess.run(
            ["git", "rev-parse", ref],
            cwd=repo,
            check=False,
            capture_output=True,
            text=True,
        )
    except OSError as exc:
        return None, str(exc)
    if proc.returncode != 0:
        return None, proc.stderr.strip() or f"rev-parse exit {proc.returncode}"
    return proc.stdout.strip(), None


@dataclass(frozen=True)
class SelectionDecision:
    mode: Mode
    reason: str
    families: frozenset[NarrowFamily]


def decide_from_paths(paths: list[str]) -> SelectionDecision:
    if not paths:
        return SelectionDecision("full", "empty_diff_not_allowed", frozenset())
    families: set[NarrowFamily] = set()
    for path in paths:
        kind = classify_path(path)
        if kind == "broaden" or kind == "unknown":
            return SelectionDecision(
                "full",
                f"path_requires_full:{path}",
                frozenset(),
            )
        families.add(kind)
    if len(families) != 1:
        return SelectionDecision(
            "full",
            f"mixed_narrow_families:{','.join(sorted(families))}",
            frozenset(),
        )
    family = next(iter(families))
    return SelectionDecision("narrow", f"narrow_{family}", frozenset({family}))


def workflow_job_selected(workflow: str, job: str, decision: SelectionDecision) -> bool:
    if decision.mode == "full":
        return True
    family = next(iter(decision.families))
    if workflow == "web" and family == "frontend":
        return job in WORKFLOW_JOBS["web"]
    if workflow == "documents" and family == "native_documents":
        return job == "native-extraction"
    if workflow == "collab-engine" and family == "native_collab":
        return job == "native-collab-engine"
    if family == "docs":
        return False
    return False


def build_plan(
    *,
    workflow: str,
    event_name: str,
    base_sha: str | None,
    head_sha: str | None,
    tested_sha: str | None,
    repo: Path,
    paths: list[str] | None,
    diff_error: str | None,
    force_full: bool,
) -> dict:
    if workflow not in WORKFLOW_JOBS:
        raise SystemExit(f"unknown workflow: {workflow}")

    full_reasons: list[str] = []
    if force_full:
        full_reasons.append("forced_full")
    if event_name in ("push", "merge_group"):
        full_reasons.append(f"event_{event_name}")
    if diff_error:
        full_reasons.append(f"diff_error:{diff_error}")

    decision = SelectionDecision("full", "initial", frozenset())
    if not full_reasons:
        if paths is None:
            full_reasons.append("missing_paths")
        else:
            decision = decide_from_paths(paths)
            if decision.mode == "narrow":
                pass
            else:
                full_reasons.append(decision.reason)

    if full_reasons:
        decision = SelectionDecision("full", ";".join(full_reasons), frozenset())
        mode: Mode = "full"
        reason = decision.reason
    else:
        mode = decision.mode
        reason = decision.reason

    jobs: dict[str, dict] = {}
    for job in WORKFLOW_JOBS[workflow]:
        selected = workflow_job_selected(workflow, job, decision)
        jobs[job] = {
            "selected": selected,
            "reason": reason if selected else "not_applicable",
        }

    return {
        "version": PLAN_VERSION,
        "workflow": workflow,
        "mode": mode,
        "reason": reason,
        "base_sha": base_sha,
        "head_sha": head_sha,
        "tested_sha": tested_sha,
        "diff_error": diff_error,
        "paths": paths or [],
        "jobs": jobs,
    }


def event_shas(event: dict, event_name: str) -> tuple[str | None, str | None, str | None]:
    if event_name == "pull_request":
        pr = event.get("pull_request") or {}
        return pr.get("base", {}).get("sha"), pr.get("head", {}).get("sha"), pr.get("head", {}).get("sha")
    if event_name == "merge_group":
        mg = event.get("merge_group") or {}
        return mg.get("base_sha"), mg.get("head_sha"), mg.get("head_sha")
    if event_name == "push":
        return event.get("before"), event.get("after"), event.get("after")
    if event_name == "workflow_dispatch":
        return None, None, None
    return None, None, None


def load_event(path: Path | None) -> tuple[dict, str]:
    if path is None:
        return {}, os.environ.get("GITHUB_EVENT_NAME", "workflow_dispatch")
    data = json.loads(path.read_text(encoding="utf-8"))
    name = os.environ.get("GITHUB_EVENT_NAME") or path.stem.replace(".", "_")
    if name.endswith("_json"):
        name = os.environ.get("GITHUB_EVENT_NAME", "pull_request")
    return data, os.environ.get("GITHUB_EVENT_NAME", "pull_request")


def resolve_paths_for_event(
    repo: Path,
    event: dict,
    event_name: str,
    base_sha: str | None,
    head_sha: str | None,
    tested_sha: str | None,
    paths_override: list[str] | None,
    skip_fetch: bool,
) -> tuple[list[str] | None, str | None, str | None]:
    diff_error: str | None = None
    if paths_override is not None:
        return paths_override, diff_error, tested_sha

    if event_name == "workflow_dispatch":
        return None, None, tested_sha

    if not base_sha or not head_sha:
        return None, "missing_base_or_head_sha", tested_sha

    if not skip_fetch:
        fetch = subprocess.run(
            ["git", "fetch", "--no-tags", "origin", base_sha, head_sha],
            cwd=repo,
            capture_output=True,
        )
        if fetch.returncode != 0:
            err = fetch.stderr.decode("utf-8", errors="replace").strip()
            return None, f"fetch_failed:{err}", tested_sha

    resolved_base, err = git_rev_parse(repo, base_sha)
    if err:
        return None, f"base_sha_invalid:{err}", tested_sha
    resolved_head, err = git_rev_parse(repo, head_sha)
    if err:
        return None, f"head_sha_invalid:{err}", tested_sha

    paths, diff_err = git_diff_paths(repo, resolved_base, resolved_head)
    if diff_err:
        return None, diff_err, tested_sha

    if tested_sha:
        head_now, err = git_rev_parse(repo, "HEAD")
        if err:
            return None, f"tested_sha_check:{err}", tested_sha
        tested_resolved, err = git_rev_parse(repo, tested_sha)
        if err:
            return None, f"tested_sha_invalid:{err}", tested_sha
        if head_now != tested_resolved:
            return None, f"tested_sha_mismatch:expected {tested_resolved} got {head_now}", tested_sha

    return paths, diff_error, tested_sha


def write_github_outputs(plan: dict, output_path: Path | None) -> None:
    if output_path is None:
        return
    lines: list[str] = []
    lines.append(f"mode={plan['mode']}")
    lines.append(f"reason={plan['reason']}")
    for job, meta in plan["jobs"].items():
        key = job.replace("-", "_")
        lines.append(f"select_{key}={'true' if meta['selected'] else 'false'}")
    output_path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def cmd_plan(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Compute CI selection plan for one workflow.")
    parser.add_argument("--workflow", required=True, choices=sorted(WORKFLOW_JOBS))
    parser.add_argument("--repo-root", type=Path, default=ROOT)
    parser.add_argument("--event-json", type=Path, default=None)
    parser.add_argument("--paths-file", type=Path, default=None, help="NUL-separated paths (test hook)")
    parser.add_argument("--force-full", action="store_true")
    parser.add_argument("--skip-fetch", action="store_true")
    parser.add_argument("--output-plan", type=Path, required=True)
    parser.add_argument("--github-output", type=Path, default=None)
    args = parser.parse_args(argv)

    event, event_name = load_event(args.event_json)
    if args.event_json:
        event_name = os.environ.get("GITHUB_EVENT_NAME", "pull_request")

    base_sha, head_sha, tested_sha = event_shas(event, event_name)
    paths_override: list[str] | None = None
    if args.paths_file:
        raw = args.paths_file.read_bytes()
        paths_override = [p.decode("utf-8") for p in raw.split(b"\0") if p]

    wd_input = os.environ.get("CI_SELECTION_WORKFLOW_DISPATCH_FULL", "").strip().lower()
    force_full = args.force_full or wd_input in ("1", "true", "full", "yes")
    if event_name == "workflow_dispatch" and not force_full:
        inputs = event.get("inputs") or {}
        if str(inputs.get("ci_mode", "full")).lower() != "auto":
            force_full = True

    paths, diff_error, tested_sha = resolve_paths_for_event(
        args.repo_root,
        event,
        event_name,
        base_sha,
        head_sha,
        tested_sha,
        paths_override,
        args.skip_fetch,
    )

    plan = build_plan(
        workflow=args.workflow,
        event_name=event_name,
        base_sha=base_sha,
        head_sha=head_sha,
        tested_sha=tested_sha,
        repo=args.repo_root,
        paths=paths,
        diff_error=diff_error,
        force_full=force_full,
    )
    args.output_plan.write_text(json.dumps(plan, indent=2) + "\n", encoding="utf-8")
    write_github_outputs(plan, args.github_output)
    print(json.dumps({"mode": plan["mode"], "reason": plan["reason"]}))
    return 0


_VALID_RESULTS = frozenset({"success", "failure", "cancelled", "skipped", "missing"})


def cmd_gate(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Fail-closed gate for one workflow.")
    parser.add_argument("--workflow", required=True, choices=sorted(WORKFLOW_JOBS))
    parser.add_argument("--plan", type=Path, required=True)
    parser.add_argument(
        "--job-result",
        action="append",
        default=[],
        metavar="JOB=RESULT",
        help="Repeat per product job (e.g. web-checks=success)",
    )
    args = parser.parse_args(argv)

    if not args.plan.is_file():
        print(f"gate: missing plan file {args.plan}", file=sys.stderr)
        return 1

    try:
        plan = json.loads(args.plan.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        print(f"gate: malformed plan json: {exc}", file=sys.stderr)
        return 1

    if plan.get("version") != PLAN_VERSION:
        print(f"gate: unsupported plan version {plan.get('version')}", file=sys.stderr)
        return 1
    if plan.get("workflow") != args.workflow:
        print("gate: plan workflow mismatch", file=sys.stderr)
        return 1
    if "jobs" not in plan or not isinstance(plan["jobs"], dict):
        print("gate: plan missing jobs", file=sys.stderr)
        return 1

    if plan.get("diff_error"):
        print(f"gate: plan recorded diff_error: {plan['diff_error']}", file=sys.stderr)
        return 1

    results: dict[str, str] = {}
    for item in args.job_result:
        if "=" not in item:
            print(f"gate: bad job-result {item!r}", file=sys.stderr)
            return 1
        job, result = item.split("=", 1)
        results[job] = result

    expected_jobs = WORKFLOW_JOBS[args.workflow]
    for job in expected_jobs:
        if job not in plan["jobs"]:
            print(f"gate: plan missing job {job}", file=sys.stderr)
            return 1

    for job in expected_jobs:
        selected = bool(plan["jobs"][job].get("selected"))
        result = results.get(job, "missing")
        if result not in _VALID_RESULTS:
            print(f"gate: invalid result for {job}: {result!r}", file=sys.stderr)
            return 1
        if selected:
            if result != "success":
                print(
                    f"gate: selected job {job} must succeed, got {result}",
                    file=sys.stderr,
                )
                return 1
        else:
            if result == "failure":
                print(f"gate: unselected job {job} failed unexpectedly", file=sys.stderr)
                return 1
            if result == "cancelled":
                print(f"gate: unselected job {job} cancelled unexpectedly", file=sys.stderr)
                return 1
            if result == "success":
                print(
                    f"gate: unselected job {job} ran successfully (expected skip/not run)",
                    file=sys.stderr,
                )
                return 1
            # skipped or missing are acceptable as not_applicable
    print("gate: ok")
    return 0


def discover_workflow_job_ids() -> dict[str, set[str]]:
    """Conservative static check: every product job id appears in its workflow file."""
    discovered: dict[str, set[str]] = {}
    mapping = {
        "web.yml": "web",
        "rust.yml": "rust",
        "documents.yml": "documents",
        "collab-engine.yml": "collab-engine",
        "install.yml": "install",
    }
    job_line = re.compile(r"^  ([a-z0-9][a-z0-9-]*):\s*$")
    for filename, workflow in mapping.items():
        path = WORKFLOW_FILE / filename
        jobs: set[str] = set()
        in_jobs = False
        for line in path.read_text(encoding="utf-8").splitlines():
            if line.startswith("jobs:"):
                in_jobs = True
                continue
            if not in_jobs:
                continue
            if line and not line.startswith(" "):
                break
            match = job_line.match(line)
            if match:
                jobs.add(match.group(1))
        jobs -= {"ci-plan", "ci-gate"}
        discovered[workflow] = jobs
    return discovered


def verify_workflow_registry() -> list[str]:
    errors: list[str] = []
    try:
        discovered = discover_workflow_job_ids()
    except Exception as exc:  # noqa: BLE001
        return [f"workflow parse failed: {exc}"]
    for workflow, expected in WORKFLOW_JOBS.items():
        found = discovered.get(workflow, set())
        for job in expected:
            if job not in found:
                errors.append(f"{workflow}: expected job id {job} missing from workflow yaml")
        extra = found - set(expected)
        for job in sorted(extra):
            errors.append(
                f"{workflow}: job {job} not registered in ci_selection WORKFLOW_JOBS"
            )
    return errors


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
        errors = verify_workflow_registry()
        for err in errors:
            print(err, file=sys.stderr)
        return 1 if errors else 0
    print(f"unknown command: {command}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
