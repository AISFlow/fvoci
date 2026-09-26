#!/usr/bin/env python3
"""Unit tests for web e2e group discovery and sharding (fixture trees)."""

from __future__ import annotations

import importlib.util
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
GROUPS_PY = ROOT / "scripts" / "web-e2e-groups.py"


def load_groups_module():
    spec = importlib.util.spec_from_file_location("web_e2e_groups", GROUPS_PY)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {GROUPS_PY}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["web_e2e_groups"] = module
    spec.loader.exec_module(module)
    return module


GROUPS = load_groups_module()


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
            with self.assertRaises(SystemExit):
                GROUPS.verify_plan(e2e, 2)

    def test_new_spec_auto_included(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["brand-new-flow.spec.ts"])
            GROUPS.verify_plan(e2e, 2)
            specs: list[str] = []
            for idx in range(2):
                for line in GROUPS.shard_plan_lines(e2e, idx, 2):
                    specs.extend(line["specs"])
            self.assertIn("e2e/brand-new-flow.spec.ts", specs)

    def test_nested_spec_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            nested = e2e / "nested"
            nested.mkdir()
            write_spec(nested, "hidden-flow.spec.ts")
            with self.assertRaises(SystemExit) as ctx:
                GROUPS.verify_plan(e2e, 2)
            self.assertIn("nested spec.ts", str(ctx.exception))

    def test_unsupported_test_suffix(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            write_spec(e2e, "collab-wire.test.ts")
            with self.assertRaises(SystemExit) as ctx:
                GROUPS.verify_plan(e2e, 2)
            self.assertIn(".test.ts", str(ctx.exception))

    def test_empty_shard_fails_verify(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            with self.assertRaises(SystemExit) as ctx:
                GROUPS.verify_plan(e2e, 8)
            self.assertIn("shards with no work", str(ctx.exception))

    def test_shard_jsonl_pair_on_one_line(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["alpha-flow.spec.ts", "beta-flow.spec.ts"])
            lines = GROUPS.shard_plan_lines(e2e, 0, 2)
            pair = next(g for g in lines if len(g["specs"]) == 2)
            self.assertEqual(
                pair["specs"],
                ["e2e/workspace-flow.spec.ts", "e2e/workspace-wiki-flow.spec.ts"],
            )

    def test_unsafe_basename_rejected(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            write_spec(e2e, "bad spec.spec.ts")
            with self.assertRaises(SystemExit):
                GROUPS.verify_plan(e2e, 2)


if __name__ == "__main__":
    unittest.main()
