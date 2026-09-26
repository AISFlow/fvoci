#!/usr/bin/env python3
"""Tests for fail-closed CI selection (production planner module)."""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CI_SELECTION_PY = ROOT / "scripts" / "ci_selection.py"


def load_ci_selection():
    spec = importlib.util.spec_from_file_location("ci_selection", CI_SELECTION_PY)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {CI_SELECTION_PY}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["ci_selection"] = module
    spec.loader.exec_module(module)
    return module


SEL = load_ci_selection()


def git(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=check,
        capture_output=True,
        text=True,
    )


def git_sha(cwd: Path, ref: str = "HEAD") -> str:
    return git(cwd, "rev-parse", ref).stdout.strip()


def write_file(repo: Path, rel: str, content: str = "x\n") -> None:
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def copy_workflows(dst: Path) -> None:
    src = ROOT / ".github" / "workflows"
    target = dst / ".github" / "workflows"
    target.mkdir(parents=True, exist_ok=True)
    for path in src.iterdir():
        if path.is_file() and path.suffix in {".yml", ".yaml"}:
            shutil.copy2(path, target / path.name)


def configure_git(repo: Path) -> None:
    git(repo, "config", "user.email", "ci@test")
    git(repo, "config", "user.name", "ci")
    git(repo, "config", "commit.gpgsign", "false")


class GitRepoFixture:
    def __init__(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        git(self.repo, "init", "-b", "main")
        configure_git(self.repo)

    def close(self) -> None:
        self.tmp.cleanup()

    def __enter__(self) -> GitRepoFixture:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def commit_file(self, rel: str, content: str = "x\n") -> str:
        write_file(self.repo, rel, content)
        git(self.repo, "add", rel)
        git(self.repo, "commit", "-m", f"add {rel}")
        return git_sha(self.repo)

    def rename_file(self, old: str, new: str) -> str:
        git(self.repo, "mv", old, new)
        git(self.repo, "commit", "-m", f"rename {old}")
        return git_sha(self.repo)

    def delete_file(self, rel: str) -> str:
        git(self.repo, "rm", rel)
        git(self.repo, "commit", "-m", f"delete {rel}")
        return git_sha(self.repo)


def run_cli(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    return subprocess.run(
        [sys.executable, str(CI_SELECTION_PY), *args],
        cwd=cwd or ROOT,
        env=merged,
        capture_output=True,
        text=True,
    )


def dummy_event_path(directory: Path) -> Path:
    path = directory / "event.json"
    path.write_text("{}", encoding="utf-8")
    return path


class ClassifyPathsTest(unittest.TestCase):
    def test_explicit_docs_only(self) -> None:
        self.assertEqual(SEL.classify_path("docs/rewrite.md"), "docs")
        self.assertEqual(SEL.classify_path("docs/other.md"), "broaden")

    def test_frontend_src_narrow(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/src/foo.ts"), "frontend_web_install")

    def test_generated_broadens(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/src/generated/api.ts"), "broaden")

    def test_e2e_broadens(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/e2e/foo.spec.ts"), "broaden")

    def test_packages_broaden(self) -> None:
        self.assertEqual(SEL.classify_path("packages/editor/x.ts"), "broaden")

    def test_native_crate_broadens(self) -> None:
        self.assertEqual(SEL.classify_path("crates/collab-engine/src/x.rs"), "broaden")

    def test_compat_fixtures_broaden(self) -> None:
        self.assertEqual(SEL.classify_path("compat/fixtures/x"), "broaden")


class DiffParseTest(unittest.TestCase):
    def test_valid_rename_delete(self) -> None:
        raw = b"A\0src/new.ts\0D\0src/old.ts\0R100\0old-name\0new-name\0"
        paths, err = SEL.parse_name_status_z(raw)
        self.assertIsNone(err)
        self.assertEqual(paths, ["src/new.ts", "src/old.ts", "old-name", "new-name"])

    def test_truncated_rename_fails(self) -> None:
        raw = b"R100\0only-old\0"
        paths, err = SEL.parse_name_status_z(raw)
        self.assertEqual(paths, [])
        self.assertEqual(err, "DIFF_TRUNCATED_RENAME")

    def test_missing_trailing_nul_fails(self) -> None:
        raw = b"A\0file.ts"
        paths, err = SEL.parse_name_status_z(raw)
        self.assertEqual(err, "DIFF_TRUNCATED")


class PlanSelectionTest(unittest.TestCase):
    def test_frontend_selects_web_and_install(self) -> None:
        for workflow, job, expected in (
            ("web", "web-checks", True),
            ("install", "install-smoke", True),
            ("rust", "fast", False),
        ):
            plan = SEL.build_plan(
                workflow=workflow,
                event_name="pull_request",
                base_sha="a" * 40,
                head_sha="b" * 40,
                merge_base_sha="c" * 40,
                tested_sha="b" * 40,
                paths=["apps/web/src/x.ts"],
            )
            self.assertEqual(plan["jobs"][job]["selected"], expected)
            self.assertEqual(set(plan["jobs"][job]), {"selected"})

    def test_docs_skips_product_jobs(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha="c" * 40,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
        )
        self.assertEqual(plan["mode"], "narrow")
        self.assertFalse(plan["jobs"]["web-checks"]["selected"])

    def test_crate_change_is_full(self) -> None:
        decision = SEL.decide_from_paths(["crates/document-extract/src/lib.rs"])
        self.assertEqual(decision.mode, "full")

    def test_main_push_is_full(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="push",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha=None,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
        )
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_EVENT_PUSH")
        self.assertTrue(plan["plan_ok"])
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])

    def test_parent_mismatch_cannot_narrow(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha=None,
            tested_sha="c" * 40,
            paths=["docs/rewrite.md"],
            force_full_reason="FULL_PR_MERGE_PARENTS_MISMATCH",
        )
        self.assertEqual(plan["mode"], "full")
        self.assertTrue(plan["plan_ok"])
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])


class GitIntegrationTest(unittest.TestCase):
    def test_multi_commit_and_merge_base(self) -> None:
        fx = GitRepoFixture()
        base = fx.commit_file("docs/rewrite.md", "a\n")
        head = fx.commit_file("docs/rewrite.md", "b\n")
        paths, err, merge_base = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertTrue(merge_base)
        self.assertIn("docs/rewrite.md", paths or [])

    def test_rename_in_diff(self) -> None:
        fx = GitRepoFixture()
        fx.commit_file("apps/web/src/a.ts")
        mid = git_sha(fx.repo)
        fx.rename_file("apps/web/src/a.ts", "apps/web/src/b.ts")
        head = git_sha(fx.repo)
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, mid, head)
        self.assertIsNone(err)
        self.assertTrue(any("b.ts" in p for p in paths or []))

    def test_base_advance_second_pr_commit(self) -> None:
        fx = GitRepoFixture()
        base = fx.commit_file("README.md")
        fx.commit_file("apps/web/src/z.ts")
        head = git_sha(fx.repo)
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertIn("apps/web/src/z.ts", paths or [])

    def test_multicommit_rename_and_delete(self) -> None:
        fx = GitRepoFixture()
        fx.commit_file("keep.md", "keep\n")
        fx.commit_file("apps/web/src/old.ts", "old\n")
        base = fx.commit_file("gone.ts", "gone\n")
        fx.rename_file("apps/web/src/old.ts", "apps/web/src/new.ts")
        fx.delete_file("gone.ts")
        head = git_sha(fx.repo)
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertIn("apps/web/src/old.ts", paths or [])
        self.assertIn("apps/web/src/new.ts", paths or [])
        self.assertIn("gone.ts", paths or [])


class PrCheckoutFixture:
    """Origin + work clone with GitHub-like PR event/checkout trees."""

    def __init__(self) -> None:
        self.origin_tmp = tempfile.TemporaryDirectory()
        self.work_tmp = tempfile.TemporaryDirectory()
        self.origin = Path(self.origin_tmp.name)
        self.work = Path(self.work_tmp.name)
        git(self.origin, "init", "-b", "main")
        configure_git(self.origin)
        git(self.origin, "config", "uploadpack.allowReachableSHA1InWant", "true")
        copy_workflows(self.origin)
        write_file(self.origin, "README.md", "base docs\n")
        git(self.origin, "add", ".")
        git(self.origin, "commit", "-m", "base")
        self.base_sha = git_sha(self.origin)

    def commit_on_branch(self, branch: str, rel: str, content: str) -> str:
        git(self.origin, "checkout", "-B", branch, "main")
        write_file(self.origin, rel, content)
        git(self.origin, "add", rel)
        git(self.origin, "commit", "-m", f"{branch} {rel}")
        sha = git_sha(self.origin)
        git(self.origin, "checkout", "main")
        return sha

    def close(self) -> None:
        self.work_tmp.cleanup()
        self.origin_tmp.cleanup()

    def __enter__(self) -> PrCheckoutFixture:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def clone_work(self) -> None:
        git(self.origin, "clone", str(self.origin), str(self.work))
        configure_git(self.work)

    def merge_checkout(self, first_parent: str, second_parent: str) -> str:
        git(self.work, "checkout", "-B", "tested", first_parent)
        git(self.work, "merge", "--no-ff", "-m", "github merge", second_parent)
        return git_sha(self.work)

    def write_pr_event(self, base_sha: str, head_sha: str) -> Path:
        path = self.work / "event.json"
        path.write_text(
            json.dumps({"pull_request": {"base": {"sha": base_sha}, "head": {"sha": head_sha}}}),
            encoding="utf-8",
        )
        return path

    def plan_cli(
        self, *, tested_sha: str, event_path: Path, output: Path, github_output: Path | None = None
    ) -> subprocess.CompletedProcess[str]:
        args = [
            "plan",
            "--workflow",
            "web",
            "--repo-root",
            str(self.work),
            "--event-json",
            str(event_path),
            "--output-plan",
            str(output),
        ]
        if github_output is not None:
            args.extend(["--github-output", str(github_output)])
        return run_cli(
            args,
            env={"GITHUB_EVENT_NAME": "pull_request", "GITHUB_SHA": tested_sha},
            cwd=self.work,
        )


class PrCheckoutBindingTest(unittest.TestCase):
    def test_valid_merge_tree_can_narrow_docs(self) -> None:
        fx = PrCheckoutFixture()
        head = fx.commit_on_branch("pr", "README.md", "docs only\n")
        fx.clone_work()
        tested = fx.merge_checkout(fx.base_sha, head)
        event = fx.write_pr_event(fx.base_sha, head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "narrow")
        self.assertEqual(plan["reason_code"], "NARROW_DOCS")
        self.assertFalse(plan["jobs"]["web-checks"]["selected"])

    def test_valid_merge_frontend_selects_web(self) -> None:
        fx = PrCheckoutFixture()
        head = fx.commit_on_branch("pr", "apps/web/src/x.ts", "export {}\n")
        fx.clone_work()
        tested = fx.merge_checkout(fx.base_sha, head)
        event = fx.write_pr_event(fx.base_sha, head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "narrow")
        self.assertEqual(plan["reason_code"], "NARROW_FRONTEND_WEB_INSTALL")
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])

    def test_unrelated_code_merge_vs_docs_event_cannot_narrow(self) -> None:
        fx = PrCheckoutFixture()
        docs_head = fx.commit_on_branch("docs-pr", "README.md", "docs only\n")
        code_head = fx.commit_on_branch("code-pr", "src/lib.rs", "fn x() {}\n")
        fx.clone_work()
        tested = fx.merge_checkout(fx.base_sha, code_head)
        event = fx.write_pr_event(fx.base_sha, docs_head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_PR_MERGE_PARENTS_MISMATCH")
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])

    def test_base_advance_mismatch_cannot_narrow(self) -> None:
        fx = PrCheckoutFixture()
        docs_head = fx.commit_on_branch("docs-pr", "README.md", "docs only\n")
        git(fx.origin, "checkout", "main")
        write_file(fx.origin, "src/lib.rs", "fn advanced() {}\n")
        git(fx.origin, "add", "src/lib.rs")
        git(fx.origin, "commit", "-m", "advance base")
        advanced_base = git_sha(fx.origin)
        fx.clone_work()
        tested = fx.merge_checkout(advanced_base, docs_head)
        event = fx.write_pr_event(fx.base_sha, docs_head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_PR_MERGE_PARENTS_MISMATCH")

    def test_direct_head_checkout_cannot_narrow(self) -> None:
        fx = PrCheckoutFixture()
        docs_head = fx.commit_on_branch("docs-pr", "README.md", "docs only\n")
        fx.clone_work()
        git(fx.work, "checkout", "--detach", docs_head)
        event = fx.write_pr_event(fx.base_sha, docs_head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=docs_head, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_PR_CHECKOUT_NOT_MERGE")


class GateSchemaTest(unittest.TestCase):
    def _plan(self, workflow: str, selected: dict[str, bool], plan_ok: bool = True) -> dict:
        jobs = {job: {"selected": selected.get(job, False)} for job in SEL.WORKFLOW_JOBS[workflow]}
        return {
            "version": SEL.PLAN_VERSION,
            "workflow": workflow,
            "mode": "narrow",
            "reason_code": "NARROW_DOCS",
            "plan_ok": plan_ok,
            "tested_sha": "a" * 40,
            "jobs": jobs,
        }

    def _gate(self, plan: dict, workflow: str, results: list[str], tested: str = "a" * 40) -> int:
        args = [
            "--workflow",
            workflow,
            "--plan-json",
            json.dumps(plan),
            "--tested-sha",
            tested,
        ]
        for item in results:
            args.extend(["--job-result", item])
        return SEL.cmd_gate(args)

    def test_unselected_must_be_skipped(self) -> None:
        plan = self._plan("web", {"web-checks": False})
        rc = self._gate(
            plan,
            "web",
            [
                "web-checks=missing",
                "workspace-browser-shard=skipped",
                "collaboration-flow=skipped",
            ],
        )
        self.assertEqual(rc, 1)

    def test_duplicate_results_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        rc = self._gate(
            plan,
            "documents",
            ["native-extraction=skipped", "native-extraction=skipped"],
        )
        self.assertEqual(rc, 1)

    def test_plan_not_ok_rejected(self) -> None:
        plan = self._plan("web", {"web-checks": True}, plan_ok=False)
        rc = self._gate(
            plan,
            "web",
            [
                "web-checks=success",
                "workspace-browser-shard=skipped",
                "collaboration-flow=skipped",
            ],
        )
        self.assertEqual(rc, 1)

    def test_selected_failure_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", ["native-extraction=failure"])
        self.assertEqual(rc, 1)

    def test_selected_cancelled_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", ["native-extraction=cancelled"])
        self.assertEqual(rc, 1)

    def test_selected_skip_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", ["native-extraction=skipped"])
        self.assertEqual(rc, 1)

    def test_selected_success_ok(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", ["native-extraction=success"])
        self.assertEqual(rc, 0)

    def test_invalid_json_top_type_rejected(self) -> None:
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "documents",
                "--plan-json",
                json.dumps(["not", "an", "object"]),
                "--tested-sha",
                "a" * 40,
                "--job-result",
                "native-extraction=success",
            ]
        )
        self.assertEqual(rc, 1)

    def test_unknown_plan_keys_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["extra"] = "nope"
        rc = self._gate(plan, "documents", ["native-extraction=skipped"])
        self.assertEqual(rc, 1)

    def test_unknown_job_keys_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["jobs"]["native-extraction"]["reason_code"] = "NARROW_DOCS"
        rc = self._gate(plan, "documents", ["native-extraction=skipped"])
        self.assertEqual(rc, 1)

    def test_strict_bool_rejects_string_true(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["jobs"]["native-extraction"]["selected"] = "true"
        rc = self._gate(plan, "documents", ["native-extraction=skipped"])
        self.assertEqual(rc, 1)

    def test_strict_bool_rejects_integer(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["jobs"]["native-extraction"]["selected"] = 1
        rc = self._gate(plan, "documents", ["native-extraction=success"])
        self.assertEqual(rc, 1)


class WorkflowRegistryTest(unittest.TestCase):
    def test_workflows_match_planner(self) -> None:
        errors = SEL.verify_workflow_registry()
        self.assertEqual(errors, [], msg="\n".join(errors))

    def test_requirements_pin_pyyaml(self) -> None:
        text = (ROOT / "scripts" / "ci_selection_requirements.txt").read_text(encoding="utf-8")
        self.assertIn("PyYAML==6.0.3", text)


class RegistryMutationCliTest(unittest.TestCase):
    def _mutated_root(self) -> Path:
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, True)
        copy_workflows(tmp)
        return tmp

    def _plan_against(self, repo_root: Path) -> tuple[subprocess.CompletedProcess[str], Path]:
        event = dummy_event_path(repo_root)
        output = repo_root / "green-plan.json"
        github_output = repo_root / "github-output.txt"
        proc = run_cli(
            [
                "plan",
                "--workflow",
                "web",
                "--repo-root",
                str(repo_root),
                "--event-json",
                str(event),
                "--output-plan",
                str(output),
                "--github-output",
                str(github_output),
            ],
            env={"GITHUB_EVENT_NAME": "pull_request", "GITHUB_SHA": "a" * 40},
        )
        return proc, output

    def _assert_no_green_outputs(self, proc: subprocess.CompletedProcess[str], output: Path, needle: str) -> None:
        self.assertEqual(proc.returncode, 1, proc.stderr)
        self.assertIn("workflow registry validation failed", proc.stderr)
        self.assertIn(needle, proc.stderr)
        self.assertFalse(output.exists(), "plan must not emit outputs after registry failure")
        github_output = output.parent / "github-output.txt"
        self.assertFalse(github_output.exists(), "GITHUB_OUTPUT must stay empty after registry failure")

    def test_new_job_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        injected = """
  new-suite:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_new_suite == 'true'
    runs-on: ubuntu-24.04
    steps:
      - run: echo new
"""
        web.write_text(text.replace("  web-ci-gate:", injected + "  web-ci-gate:"), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "unregistered job id new-suite")

    def test_gate_suffixed_product_job_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        injected = """
  sneaky-ci-gate:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_sneaky_ci_gate == 'true'
    runs-on: ubuntu-24.04
    steps:
      - run: echo sneaky
"""
        web.write_text(text.replace("  web-ci-gate:", injected + "  web-ci-gate:"), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "unregistered job id sneaky-ci-gate")

    def test_new_workflow_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        extra = root / ".github" / "workflows" / "extra.yml"
        extra.write_text("name: Extra\non: push\njobs:\n  extra-job:\n    runs-on: ubuntu-24.04\n    steps:\n      - run: echo x\n", encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "unknown workflow file extra.yml")

    def test_missing_selector_output_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        web.write_text(text.replace("      select_web_checks: ${{ steps.plan.outputs.select_web_checks }}\n", ""), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "missing selector output select_web_checks")

    def test_gate_needs_mismatch_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        web.write_text(
            text.replace(
                "    needs: [ci-plan, web-checks, workspace-browser-shard, collaboration-flow]",
                "    needs: [ci-plan, web-checks]",
            ),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "needs must be ci-plan and every registered job")

    def test_gate_result_args_mismatch_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        web.write_text(
            text.replace('            --job-result web-checks="$JOB_WEB_CHECKS" \\\n', ""),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "--job-result arguments must match registered jobs")


if __name__ == "__main__":
    unittest.main()
