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
from typing import Literal

ROOT = Path(__file__).resolve().parent.parent

Mode = Literal["full", "narrow"]
NarrowFamily = Literal["docs", "frontend_web_install"]

PLAN_VERSION = 2

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
GATE_JOB_SUFFIX = "-ci-gate"

SHA_RE = re.compile(r"^[0-9a-f]{40}$")
REASON_CODE_RE = re.compile(r"^[A-Z][A-Z0-9_]{0,63}$")

KNOWN_EVENTS = frozenset({"pull_request", "push", "merge_group", "workflow_dispatch"})

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


@dataclass(frozen=True)
class SelectionDecision:
    mode: Mode
    reason_code: str
    families: frozenset[NarrowFamily]


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
    diff_error: str | None,
    force_full: bool,
) -> dict:
    if workflow not in WORKFLOW_JOBS:
        raise SystemExit(f"unknown workflow: {workflow}")

    full_reasons: list[str] = []
    if force_full:
        full_reasons.append("FULL_FORCED")
    if event_name in ("push", "merge_group", "workflow_dispatch"):
        full_reasons.append(f"FULL_EVENT_{event_name.upper()}")
    if event_name not in KNOWN_EVENTS:
        full_reasons.append("FULL_EVENT_UNKNOWN")
    if diff_error:
        full_reasons.append("FULL_DIFF_ERROR")

    decision = SelectionDecision("full", "FULL_INITIAL", frozenset())
    if not full_reasons:
        if paths is None:
            full_reasons.append("FULL_MISSING_PATHS")
        else:
            decision = decide_from_paths(paths)
            if decision.mode != "narrow":
                full_reasons.append(decision.reason_code)

    if full_reasons:
        reason_code = full_reasons[0]
        if len(full_reasons) > 1:
            reason_code = "FULL_COMBINED"
        decision = SelectionDecision("full", reason_code, frozenset())
        mode: Mode = "full"
    else:
        mode = decision.mode
        reason_code = decision.reason_code

    sanitize_reason_code(reason_code)

    jobs: dict[str, dict] = {}
    for job in WORKFLOW_JOBS[workflow]:
        selected = workflow_job_selected(workflow, job, decision)
        jobs[job] = {
            "selected": selected,
            "reason_code": reason_code if selected else "NOT_APPLICABLE",
        }

    plan_ok = diff_error is None and event_name in KNOWN_EVENTS

    return {
        "version": PLAN_VERSION,
        "workflow": workflow,
        "mode": mode,
        "reason_code": reason_code,
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
) -> tuple[list[str] | None, str | None, str | None, str | None, str | None, str | None]:
    """Return paths, diff_error, base, head, merge_base, tested_sha."""
    tested_sha = os.environ.get("GITHUB_SHA", "").strip()
    if not validate_sha(tested_sha):
        return None, "TESTED_SHA_INVALID", None, None, None, None

    head_now, err = git_rev_parse(repo, "HEAD", require_sha_ref=False)
    if err:
        return None, "HEAD_REV_PARSE_FAILED", None, None, None, tested_sha
    if head_now != tested_sha:
        return None, "TESTED_SHA_MISMATCH", None, None, None, tested_sha

    if event_name == "workflow_dispatch":
        return None, None, None, None, None, tested_sha

    if event_name not in KNOWN_EVENTS:
        return None, "EVENT_UNKNOWN", None, None, None, tested_sha

    if event_name in ("push", "merge_group"):
        return None, None, *event_shas(event, event_name), None, tested_sha

    base_sha, head_sha = event_shas(event, event_name)
    if not base_sha or not head_sha:
        return None, "MISSING_BASE_OR_HEAD", base_sha, head_sha, None, tested_sha
    if not validate_sha(base_sha) or not validate_sha(head_sha):
        return None, "SHA_INVALID", base_sha, head_sha, None, tested_sha

    fetch_err = git_fetch_origin(repo, base_sha, head_sha)
    if fetch_err:
        return None, fetch_err, base_sha, head_sha, None, tested_sha

    paths, diff_err, merge_base = diff_paths_for_pr(repo, base_sha, head_sha)
    if diff_err:
        return None, diff_err, base_sha, head_sha, merge_base, tested_sha

    return paths, None, base_sha, head_sha, merge_base, tested_sha


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


def cmd_plan(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Compute CI selection plan for one workflow.")
    parser.add_argument("--workflow", required=True, choices=sorted(WORKFLOW_JOBS))
    parser.add_argument("--repo-root", type=Path, default=ROOT)
    parser.add_argument("--event-json", type=Path, required=True)
    parser.add_argument("--output-plan", type=Path, required=True)
    parser.add_argument("--github-output", type=Path, default=None)
    args = parser.parse_args(argv)

    event_name = os.environ.get("GITHUB_EVENT_NAME", "").strip()
    if not event_name:
        print("plan: GITHUB_EVENT_NAME required", file=sys.stderr)
        return 1

    event = load_event(args.event_json)
    paths, diff_error, base_sha, head_sha, merge_base_sha, tested_sha = resolve_selection_inputs(
        args.repo_root, event, event_name
    )

    force_full = event_name in ("push", "merge_group", "workflow_dispatch") or bool(diff_error)

    plan = build_plan(
        workflow=args.workflow,
        event_name=event_name,
        base_sha=base_sha,
        head_sha=head_sha,
        merge_base_sha=merge_base_sha,
        tested_sha=tested_sha,
        paths=paths,
        diff_error=diff_error,
        force_full=force_full,
    )
    args.output_plan.write_text(json.dumps(plan, indent=2) + "\n", encoding="utf-8")
    write_github_outputs(plan, args.github_output)
    print(json.dumps({"mode": plan["mode"], "reason_code": plan["reason_code"]}))
    return 0


_VALID_RESULTS = frozenset({"success", "failure", "cancelled", "skipped"})


def _validate_plan_schema(plan: dict, workflow: str) -> str | None:
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
    for job in WORKFLOW_JOBS[workflow]:
        entry = jobs.get(job)
        if not isinstance(entry, dict):
            return "PLAN_JOB_MISSING"
        if not isinstance(entry.get("selected"), bool):
            return "PLAN_SELECTED_TYPE"
        rc = entry.get("reason_code")
        if not isinstance(rc, str) or not REASON_CODE_RE.match(rc):
            return "PLAN_JOB_REASON"
    return None


def cmd_gate(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Fail-closed gate for one workflow.")
    parser.add_argument("--workflow", required=True, choices=sorted(WORKFLOW_JOBS))
    parser.add_argument("--plan", type=Path, default=None)
    parser.add_argument("--plan-json", type=str, default=None)
    parser.add_argument("--tested-sha", required=True)
    parser.add_argument(
        "--job-result",
        action="append",
        default=[],
        metavar="JOB=RESULT",
    )
    args = parser.parse_args(argv)

    if not validate_sha(args.tested_sha):
        print("gate: tested-sha invalid", file=sys.stderr)
        return 1

    if args.plan_json:
        try:
            plan = json.loads(args.plan_json)
        except json.JSONDecodeError:
            print("gate: malformed plan json", file=sys.stderr)
            return 1
    elif args.plan and args.plan.is_file():
        try:
            plan = json.loads(args.plan.read_text(encoding="utf-8"))
        except json.JSONDecodeError:
            print("gate: malformed plan json", file=sys.stderr)
            return 1
    else:
        print("gate: missing plan", file=sys.stderr)
        return 1

    schema_err = _validate_plan_schema(plan, args.workflow)
    if schema_err:
        print(f"gate: plan schema error {schema_err}", file=sys.stderr)
        return 1

    if plan.get("tested_sha") != args.tested_sha:
        print("gate: tested_sha mismatch", file=sys.stderr)
        return 1

    results: dict[str, str] = {}
    for item in args.job_result:
        if "=" not in item:
            print("gate: bad job-result", file=sys.stderr)
            return 1
        job, result = item.split("=", 1)
        if job in results:
            print("gate: duplicate job-result", file=sys.stderr)
            return 1
        results[job] = result

    expected_jobs = WORKFLOW_JOBS[args.workflow]
    if set(results.keys()) != set(expected_jobs):
        print("gate: job-result set mismatch", file=sys.stderr)
        return 1

    for job in expected_jobs:
        selected = plan["jobs"][job]["selected"]
        result = results[job]
        if result not in _VALID_RESULTS:
            print(f"gate: invalid result for {job}", file=sys.stderr)
            return 1
        if result == "missing":
            print(f"gate: missing result for {job}", file=sys.stderr)
            return 1
        if selected:
            if result != "success":
                print(f"gate: selected job {job} must succeed", file=sys.stderr)
                return 1
        elif result != "skipped":
            print(f"gate: unselected job {job} must be skipped", file=sys.stderr)
            return 1

    print("gate: ok")
    return 0


def discover_workflow_job_ids() -> dict[str, set[str]]:
    import yaml

    discovered: dict[str, set[str]] = {}
    workflows_dir = ROOT / ".github" / "workflows"
    for workflow, filename in WORKFLOW_YAML.items():
        path = workflows_dir / filename
        data = yaml.safe_load(path.read_text(encoding="utf-8"))
        jobs = set((data.get("jobs") or {}).keys())
        jobs.discard(PLAN_JOB_ID)
        jobs = {j for j in jobs if not j.endswith(GATE_JOB_SUFFIX) and j != "ci-gate"}
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
                errors.append(f"{workflow}: missing job id {job}")
        for job in sorted(found - set(expected)):
            errors.append(f"{workflow}: unregistered job id {job}")
        yaml_name = WORKFLOW_YAML[workflow]
        text = (ROOT / ".github" / "workflows" / yaml_name).read_text(encoding="utf-8")
        gate_name = f"{workflow}{GATE_JOB_SUFFIX}"
        if f"name: {gate_name}" not in text and f"{gate_name}:" not in text:
            errors.append(f"{workflow}: missing gate job {gate_name}")
        if f"--workflow {workflow}" not in text:
            errors.append(f"{workflow}: planner not wired in workflow yaml")
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
