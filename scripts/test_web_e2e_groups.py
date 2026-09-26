#!/usr/bin/env python3
"""Unit tests for web e2e group discovery and sharding (fixture trees)."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GROUPS_PY = ROOT / "scripts" / "web-e2e-groups.py"
BASELINE_COVERAGE = ROOT / "scripts" / "fixtures" / "web-e2e-groups" / "baseline-coverage-6479332.json"


def run_groups(command: list[str], e2e_dir: Path) -> subprocess.CompletedProcess[str]:
    env = {**os.environ, "FVOCI_WEB_E2E_DIR": str(e2e_dir)}
    return subprocess.run(
        [sys.executable, str(GROUPS_PY), *command],
        cwd=ROOT,
        env=env,
        text=True,
        capture_output=True,
        check=False,
    )


def write_spec(directory: Path, name: str, content: str = "// fixture\n") -> None:
    if "/" in name:
        raise ValueError(name)
    (directory / name).write_text(content, encoding="utf-8")


def minimal_pair_tree(directory: Path, extra: list[str] | None = None) -> None:
    write_spec(directory, "workspace-flow.spec.ts")
    write_spec(directory, "workspace-wiki-flow.spec.ts")
    for name in extra or []:
        write_spec(directory, name)


class WebE2eGroupsTest(unittest.TestCase):
    def test_workspace_pair_required(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            write_spec(e2e, "workspace-wiki-flow.spec.ts")
            result = run_groups(["verify", "--shards", "2"], e2e)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("workspace-wiki-flow.spec.ts", result.stderr)

    def test_new_spec_auto_included(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["brand-new-flow.spec.ts"])
            result = run_groups(["verify", "--shards", "2"], e2e)
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            listed = run_groups(["list-groups"], e2e)
            self.assertEqual(listed.returncode, 0)
            specs = []
            for line in listed.stdout.splitlines():
                specs.extend(json.loads(line)["specs"])
            self.assertIn("e2e/brand-new-flow.spec.ts", specs)

    def test_nested_spec_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            nested = e2e / "nested"
            nested.mkdir()
            write_spec(nested, "hidden-flow.spec.ts")
            result = run_groups(["verify", "--shards", "2"], e2e)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("nested spec.ts", result.stderr)

    def test_unsupported_test_suffix(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            write_spec(e2e, "collab-wire.test.ts")
            result = run_groups(["verify", "--shards", "2"], e2e)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn(".test.ts", result.stderr)

    def test_empty_shard_fails_verify(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            result = run_groups(["verify", "--shards", "8"], e2e)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("shards with no work", result.stderr)

    def test_shard_jsonl_pair_on_one_line(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["alpha-flow.spec.ts", "beta-flow.spec.ts"])
            result = run_groups(["shard-jsonl", "--index", "0", "--shards", "2"], e2e)
            self.assertEqual(result.returncode, 0, msg=result.stderr)
            groups = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
            pair = next(g for g in groups if len(g["specs"]) == 2)
            self.assertEqual(
                pair["specs"],
                ["e2e/workspace-flow.spec.ts", "e2e/workspace-wiki-flow.spec.ts"],
            )

    def test_unsafe_basename_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            write_spec(e2e, "bad spec.spec.ts")
            result = run_groups(["verify", "--shards", "2"], e2e)
            self.assertNotEqual(result.returncode, 0)

    def test_baseline_coverage_snapshot(self) -> None:
        self.assertTrue(BASELINE_COVERAGE.is_file(), "missing baseline coverage fixture")
        baseline = json.loads(BASELINE_COVERAGE.read_text(encoding="utf-8"))
        result = run_groups(["verify", "--shards", "8"], e2e_dir())
        self.assertEqual(result.returncode, 0, msg=result.stderr)
        current = json.loads(result.stdout.strip())
        self.assertEqual(current["group_count"], baseline["group_count"])
        self.assertEqual(current["spec_count"], baseline["spec_count"])
        self.assertEqual(current["groups_per_shard"], baseline["groups_per_shard"])


def e2e_dir() -> Path:
    return ROOT / "apps" / "web" / "e2e"


if __name__ == "__main__":
    unittest.main()
