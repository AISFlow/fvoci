#!/usr/bin/env python3
"""Unit tests for web e2e group discovery and sharding (fixture trees)."""

from __future__ import annotations

import importlib.util
import json
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


def run_verify(e2e_dir: Path, shards: int) -> tuple[int, str, str]:
    """Run verify logic against a fixture tree (no subprocess / env overrides)."""
    import io
    from contextlib import redirect_stderr, redirect_stdout

    stdout = io.StringIO()
    stderr = io.StringIO()
    code = 0
    try:
        with redirect_stdout(stdout), redirect_stderr(stderr):
            groups = GROUPS.discover_groups(e2e_dir)
            shards_list = GROUPS.assign_shards(groups, shards)
            spec_paths: list[str] = []
            for group in groups:
                for spec in group:
                    GROUPS.validate_spec_relpath(spec)
                spec_paths.extend(group)
            if len(spec_paths) != len(set(spec_paths)):
                raise SystemExit("duplicate spec membership across groups")
            expected_specs = sorted(p.name for p in e2e_dir.glob("*.spec.ts"))
            discovered_specs = sorted(Path(s).name for s in spec_paths)
            if expected_specs != discovered_specs:
                raise SystemExit(
                    "unregistered or missing specs: "
                    f"tree={expected_specs!r} groups={discovered_specs!r}"
                )
            empty = [i for i, shard in enumerate(shards_list) if not shard]
            if empty:
                raise SystemExit(f"shards with no work: {empty}")
            pair = next(g for g in groups if len(g) == 2)
            if pair != [
                f"e2e/{GROUPS.PAIR_FIRST}",
                f"e2e/{GROUPS.PAIR_SECOND}",
            ]:
                raise SystemExit(f"workspace pair integrity failed: {pair!r}")
            print(
                json.dumps(
                    {
                        "group_count": len(groups),
                        "spec_count": len(spec_paths),
                        "shard_count": shards,
                        "groups_per_shard": [len(s) for s in shards_list],
                    },
                    separators=(",", ":"),
                )
            )
    except SystemExit as exc:
        code = int(exc.code) if isinstance(exc.code, int) else 1
        if str(exc):
            stderr.write(f"{exc}\n")
    return code, stdout.getvalue(), stderr.getvalue()


def run_shard_jsonl(e2e_dir: Path, index: int, shards: int) -> tuple[int, str, str]:
    import io
    from contextlib import redirect_stderr, redirect_stdout

    stdout = io.StringIO()
    stderr = io.StringIO()
    code = 0
    try:
        with redirect_stdout(stdout), redirect_stderr(stderr):
            groups = GROUPS.discover_groups(e2e_dir)
            shards_list = GROUPS.assign_shards(groups, shards)
            if index < 0 or index >= shards:
                raise SystemExit(f"shard index {index} out of range 0..{shards - 1}")
            shard_groups = shards_list[index]
            if not shard_groups:
                raise SystemExit(f"shard {index} has no groups")
            for group in shard_groups:
                for spec in group:
                    GROUPS.validate_spec_relpath(spec)
                print(json.dumps({"specs": group}, separators=(",", ":")))
    except SystemExit as exc:
        code = int(exc.code) if isinstance(exc.code, int) else 1
        if str(exc):
            stderr.write(f"{exc}\n")
    return code, stdout.getvalue(), stderr.getvalue()


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
            code, _, stderr = run_verify(e2e, 2)
            self.assertNotEqual(code, 0)
            self.assertIn("workspace-wiki-flow.spec.ts", stderr)

    def test_new_spec_auto_included(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["brand-new-flow.spec.ts"])
            code, _, stderr = run_verify(e2e, 2)
            self.assertEqual(code, 0, msg=stderr)
            all_specs: list[str] = []
            for idx in range(2):
                c, out, _ = run_shard_jsonl(e2e, idx, 2)
                self.assertEqual(c, 0)
                for line in out.splitlines():
                    all_specs.extend(json.loads(line)["specs"])
            self.assertIn("e2e/brand-new-flow.spec.ts", all_specs)

    def test_nested_spec_fails_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            nested = e2e / "nested"
            nested.mkdir()
            write_spec(nested, "hidden-flow.spec.ts")
            code, _, stderr = run_verify(e2e, 2)
            self.assertNotEqual(code, 0)
            self.assertIn("nested spec.ts", stderr)

    def test_unsupported_test_suffix(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            write_spec(e2e, "collab-wire.test.ts")
            code, _, stderr = run_verify(e2e, 2)
            self.assertNotEqual(code, 0)
            self.assertIn(".test.ts", stderr)

    def test_empty_shard_fails_verify(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            code, _, stderr = run_verify(e2e, 8)
            self.assertNotEqual(code, 0)
            self.assertIn("shards with no work", stderr)

    def test_shard_jsonl_pair_on_one_line(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["alpha-flow.spec.ts", "beta-flow.spec.ts"])
            code, stdout, stderr = run_shard_jsonl(e2e, 0, 2)
            self.assertEqual(code, 0, msg=stderr)
            groups = [json.loads(line) for line in stdout.splitlines() if line.strip()]
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
            code, _, _ = run_verify(e2e, 2)
            self.assertNotEqual(code, 0)


if __name__ == "__main__":
    unittest.main()
