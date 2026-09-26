#!/usr/bin/env python3
"""Tests for fail-closed CI selection (production planner module)."""

from __future__ import annotations

import importlib.util
import json
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


class GitRepoFixture:
    def __init__(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        self._run("init")
        self._run("config", "user.email", "ci@test")
        self._run("config", "user.name", "ci")

    def _run(self, *args: str) -> str:
        subprocess.run(
            ["git", *args],
            cwd=self.repo,
            check=True,
            capture_output=True,
            text=True,
        )
        if args[0] == "rev-parse":
            return subprocess.check_output(["git", "rev-parse", args[1]], cwd=self.repo, text=True).strip()
        return ""

    def commit_file(self, rel: str, content: str = "x\n") -> str:
        path = self.repo / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content, encoding="utf-8")
        self._run("add", rel)
        self._run("commit", "-m", f"add {rel}")
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.repo, text=True).strip()

    def rename_file(self, old: str, new: str) -> str:
        self._run("mv", old, new)
        self._run("commit", "-m", f"rename {old}")
        return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=self.repo, text=True).strip()


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
                diff_error=None,
                force_full=False,
            )
            self.assertEqual(plan["jobs"][job]["selected"], expected)

    def test_docs_skips_product_jobs(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha="c" * 40,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
            diff_error=None,
            force_full=False,
        )
        self.assertEqual(plan["mode"], "narrow")
        self.assertFalse(plan["jobs"]["web-checks"]["selected"])

    def test_crate_change_is_full(self) -> None:
        decision = SEL.decide_from_paths(["crates/document-extract/src/lib.rs"])
        self.assertEqual(decision.mode, "full")


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
        mid = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=fx.repo, text=True).strip()
        fx.rename_file("apps/web/src/a.ts", "apps/web/src/b.ts")
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=fx.repo, text=True).strip()
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, mid, head)
        self.assertIsNone(err)
        self.assertTrue(any("b.ts" in p for p in paths or []))

    def test_base_advance_second_pr_commit(self) -> None:
        fx = GitRepoFixture()
        base = fx.commit_file("README.md")
        fx.commit_file("apps/web/src/z.ts")
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=fx.repo, text=True).strip()
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertIn("apps/web/src/z.ts", paths or [])


class GateSchemaTest(unittest.TestCase):
    def _plan(self, workflow: str, selected: dict[str, bool], plan_ok: bool = True) -> dict:
        jobs = {
            job: {"selected": selected.get(job, False), "reason_code": "NARROW_DOCS"}
            for job in SEL.WORKFLOW_JOBS[workflow]
        }
        return {
            "version": SEL.PLAN_VERSION,
            "workflow": workflow,
            "mode": "narrow",
            "reason_code": "NARROW_DOCS",
            "plan_ok": plan_ok,
            "tested_sha": "a" * 40,
            "jobs": jobs,
        }

    def test_unselected_must_be_skipped(self) -> None:
        plan = self._plan("web", {"web-checks": False})
        path = Path(tempfile.mkstemp(suffix=".json")[1])
        path.write_text(json.dumps(plan), encoding="utf-8")
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "web",
                "--plan",
                str(path),
                "--tested-sha",
                "a" * 40,
                "--job-result",
                "web-checks=missing",
                "--job-result",
                "workspace-browser-shard=skipped",
                "--job-result",
                "collaboration-flow=skipped",
            ]
        )
        self.assertEqual(rc, 1)

    def test_duplicate_results_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        path = Path(tempfile.mkstemp(suffix=".json")[1])
        path.write_text(json.dumps(plan), encoding="utf-8")
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "documents",
                "--plan",
                str(path),
                "--tested-sha",
                "a" * 40,
                "--job-result",
                "native-extraction=skipped",
                "--job-result",
                "native-extraction=skipped",
            ]
        )
        self.assertEqual(rc, 1)

    def test_plan_not_ok_rejected(self) -> None:
        plan = self._plan("web", {"web-checks": True}, plan_ok=False)
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "web",
                "--plan-json",
                json.dumps(plan),
                "--tested-sha",
                "a" * 40,
                "--job-result",
                "web-checks=success",
                "--job-result",
                "workspace-browser-shard=skipped",
                "--job-result",
                "collaboration-flow=skipped",
            ]
        )
        self.assertEqual(rc, 1)


class WorkflowRegistryTest(unittest.TestCase):
    def test_workflows_match_planner(self) -> None:
        errors = SEL.verify_workflow_registry()
        self.assertEqual(errors, [], msg="\n".join(errors))


if __name__ == "__main__":
    unittest.main()
