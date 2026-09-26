#!/usr/bin/env python3
"""Tests for fail-closed CI selection (production planner module)."""

from __future__ import annotations

import importlib.util
import json
import os
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


class ClassifyPathsTest(unittest.TestCase):
    def test_docs_narrow(self) -> None:
        self.assertEqual(SEL.classify_path("docs/rewrite.md"), "docs")
        self.assertEqual(SEL.classify_path("README.md"), "docs")

    def test_frontend_narrow(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/src/foo.ts"), "frontend")
        self.assertEqual(SEL.classify_path("packages/editor/x.ts"), "frontend")

    def test_native_documents(self) -> None:
        self.assertEqual(SEL.classify_path("crates/document-extract/src/lib.rs"), "native_documents")

    def test_native_collab(self) -> None:
        self.assertEqual(SEL.classify_path("crates/collab-engine/src/x.rs"), "native_collab")

    def test_compat_fixtures_broadens(self) -> None:
        decision = SEL.decide_from_paths(["compat/fixtures/markdown-oracle/x.md"])
        self.assertEqual(decision.mode, "full")

    def test_build_input_broadens(self) -> None:
        self.assertEqual(SEL.classify_path("compat/fixtures/x"), "broaden")
        self.assertEqual(SEL.classify_path("scripts/ci_selection.py"), "broaden")
        self.assertEqual(SEL.classify_path("migrations/001.sql"), "broaden")
        self.assertEqual(SEL.classify_path("src/main.rs"), "broaden")
        self.assertEqual(SEL.classify_path("tests/foo.rs"), "broaden")
        self.assertEqual(SEL.classify_path("Cargo.toml"), "broaden")

    def test_unknown_broadens_via_decision(self) -> None:
        decision = SEL.decide_from_paths(["random/unknown.bin"])
        self.assertEqual(decision.mode, "full")

    def test_mixed_narrow_is_full(self) -> None:
        decision = SEL.decide_from_paths(["docs/a.md", "apps/web/x.ts"])
        self.assertEqual(decision.mode, "full")

    def test_empty_diff_is_full(self) -> None:
        decision = SEL.decide_from_paths([])
        self.assertEqual(decision.mode, "full")


class DiffParseTest(unittest.TestCase):
    def test_rename_and_delete(self) -> None:
        raw = b"A\0src/new.ts\0D\0src/old.ts\0R100\0old-name\0new-name\0"
        paths = SEL.parse_name_status_z(raw)
        self.assertEqual(
            paths,
            ["src/new.ts", "src/old.ts", "old-name", "new-name"],
        )


class PlanNarrowTest(unittest.TestCase):
    def test_docs_skips_web_jobs(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a",
            head_sha="b",
            tested_sha="b",
            repo=ROOT,
            paths=["docs/rewrite.md"],
            diff_error=None,
            force_full=False,
        )
        self.assertEqual(plan["mode"], "narrow")
        self.assertFalse(plan["jobs"]["web-checks"]["selected"])

    def test_frontend_selects_web(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a",
            head_sha="b",
            tested_sha="b",
            repo=ROOT,
            paths=["apps/web/foo.ts"],
            diff_error=None,
            force_full=False,
        )
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])
        self.assertTrue(plan["jobs"]["workspace-browser-shard"]["selected"])

    def test_backend_path_full_rust(self) -> None:
        plan = SEL.build_plan(
            workflow="rust",
            event_name="pull_request",
            base_sha="a",
            head_sha="b",
            tested_sha="b",
            repo=ROOT,
            paths=["src/api.rs"],
            diff_error=None,
            force_full=False,
        )
        self.assertEqual(plan["mode"], "full")
        self.assertTrue(plan["jobs"]["fast"]["selected"])

    def test_diff_error_forces_full(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a",
            head_sha="b",
            tested_sha="b",
            repo=ROOT,
            paths=None,
            diff_error="fetch_failed",
            force_full=False,
        )
        self.assertEqual(plan["mode"], "full")
        self.assertIn("diff_error", plan["reason"])


class GateTest(unittest.TestCase):
    def _plan(self, workflow: str, selected: dict[str, bool]) -> Path:
        jobs = {
            job: {"selected": selected.get(job, False), "reason": "test"}
            for job in SEL.WORKFLOW_JOBS[workflow]
        }
        payload = {
            "version": SEL.PLAN_VERSION,
            "workflow": workflow,
            "mode": "narrow",
            "reason": "test",
            "base_sha": "a",
            "head_sha": "b",
            "tested_sha": "b",
            "diff_error": None,
            "paths": [],
            "jobs": jobs,
        }
        tmp = Path(tempfile.mkstemp(suffix=".json")[1])
        tmp.write_text(json.dumps(payload), encoding="utf-8")
        return tmp

    def test_selected_cancel_fails(self) -> None:
        plan = self._plan("web", {"web-checks": True})
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "web",
                "--plan",
                str(plan),
                "--job-result",
                "web-checks=cancelled",
                "--job-result",
                "workspace-browser-shard=skipped",
                "--job-result",
                "collaboration-flow=skipped",
            ]
        )
        self.assertEqual(rc, 1)

    def test_selected_skip_fails(self) -> None:
        plan = self._plan("web", {"web-checks": True, "workspace-browser-shard": False})
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "web",
                "--plan",
                str(plan),
                "--job-result",
                "web-checks=skipped",
                "--job-result",
                "workspace-browser-shard=skipped",
                "--job-result",
                "collaboration-flow=skipped",
            ]
        )
        self.assertEqual(rc, 1)

    def test_unselected_skipped_ok(self) -> None:
        plan = self._plan(
            "web",
            {"web-checks": False, "workspace-browser-shard": False, "collaboration-flow": False},
        )
        rc = SEL.cmd_gate(
            [
                "--workflow",
                "web",
                "--plan",
                str(plan),
                "--job-result",
                "web-checks=skipped",
                "--job-result",
                "workspace-browser-shard=skipped",
                "--job-result",
                "collaboration-flow=skipped",
            ]
        )
        self.assertEqual(rc, 0)

    def test_malformed_plan(self) -> None:
        bad = Path(tempfile.mkstemp(suffix=".json")[1])
        bad.write_text("{not json", encoding="utf-8")
        rc = SEL.cmd_gate(["--workflow", "web", "--plan", str(bad), "--job-result", "web-checks=skipped"])
        self.assertEqual(rc, 1)


class GitFixtureTest(unittest.TestCase):
    def _git(self, repo: Path, *args: str) -> None:
        subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True)

    def test_cumulative_diff_fixture(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            self._git(repo, "init")
            self._git(repo, "config", "user.email", "ci@test")
            self._git(repo, "config", "user.name", "ci")
            (repo / "docs").mkdir()
            (repo / "docs" / "a.md").write_text("a\n", encoding="utf-8")
            self._git(repo, "add", "docs/a.md")
            self._git(repo, "commit", "-m", "base")
            base = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
            (repo / "docs" / "b.md").write_text("b\n", encoding="utf-8")
            self._git(repo, "add", "docs/b.md")
            self._git(repo, "commit", "-m", "head")
            head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
            paths, err = SEL.git_diff_paths(repo, base, head)
            self.assertIsNone(err)
            self.assertIn("docs/b.md", paths)
            plan_path = repo / "plan.json"
            env = os.environ.copy()
            env["GITHUB_EVENT_NAME"] = "pull_request"
            event = {
                "pull_request": {
                    "base": {"sha": base},
                    "head": {"sha": head},
                }
            }
            event_path = repo / "event.json"
            event_path.write_text(json.dumps(event), encoding="utf-8")
            rc = subprocess.call(
                [
                    sys.executable,
                    str(CI_SELECTION_PY),
                    "plan",
                    "--workflow",
                    "web",
                    "--repo-root",
                    str(repo),
                    "--event-json",
                    str(event_path),
                    "--skip-fetch",
                    "--output-plan",
                    str(plan_path),
                ],
                env=env,
            )
            self.assertEqual(rc, 0)
            plan = json.loads(plan_path.read_text(encoding="utf-8"))
            self.assertEqual(plan["mode"], "narrow")


class WorkflowRegistryTest(unittest.TestCase):
    def test_workflows_match_planner(self) -> None:
        errors = SEL.verify_workflow_registry()
        self.assertEqual(errors, [], msg="\n".join(errors))


if __name__ == "__main__":
    unittest.main()
