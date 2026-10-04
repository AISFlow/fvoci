#!/usr/bin/env python3
"""Unit tests for web e2e group discovery and sharding (fixture trees)."""

from __future__ import annotations

import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
GROUPS_PY = ROOT / "scripts" / "web-e2e-groups.py"


def playwright_file(*parts: str) -> Path:
    """A file of the playwright package the web app resolves (its own or a workspace ancestor's node_modules)."""
    web = ROOT / "apps" / "web"
    for directory in (web, *web.parents):
        package = directory / "node_modules" / "playwright"
        if (package / "package.json").is_file():
            return package.joinpath(*parts)
    raise RuntimeError("playwright is not installed; run bun ci")


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


def playwright_default_suffixes() -> list[str]:
    """Expand Playwright default suite suffixes without a glob parser."""
    suffixes: list[str] = []
    for kind in ("spec", "test"):
        for prefix in ("", "c", "m"):
            for lang in ("j", "t"):
                for ext_x in ("", "x"):
                    suffixes.append(f".{kind}.{prefix}{lang}s{ext_x}")
    return suffixes


def read_pinned_playwright_test_match() -> str:
    index = playwright_file("lib", "common", "index.js")
    text = index.read_text(encoding="utf-8")
    match = re.search(
        r'testMatch:\s*takeFirst\([^,]+,\s*[^,]+,\s*"([^"]+)"\)',
        text,
    )
    if match is None:
        raise RuntimeError(f"default testMatch not found in {index}")
    return match.group(1)


def playwright_match_rels(pattern: str, rels: list[str]) -> list[str]:
    payload = json.dumps({"pattern": pattern, "rels": rels})
    script = """
const { createFileMatcher } = require(process.env.PW_UTIL);
const fs = require('fs');
const { pattern, rels } = JSON.parse(fs.readFileSync(0, 'utf8'));
const matcher = createFileMatcher(pattern);
process.stdout.write(JSON.stringify(rels.filter((rel) => matcher(rel))));
"""
    proc = subprocess.run(
        ["bun", "-e", script],
        input=payload,
        capture_output=True,
        text=True,
        check=True,
        env={**os.environ, "PW_UTIL": str(playwright_file("lib", "util.js"))},
    )
    return json.loads(proc.stdout)


def minimal_pair_tree(directory: Path, extra: list[str] | None = None) -> None:
    write_spec(directory, "workspace-flow.spec.ts")
    write_spec(directory, "workspace-wiki-flow.spec.ts")
    for name in extra or []:
        write_spec(directory, name)


class WebE2eGroupsTest(unittest.TestCase):
    def test_observed_costs_reduce_concentrated_makespan(self) -> None:
        # Reproduce round-robin putting two expensive groups on the same shard.
        # Keep the cost regression independent of future production spec count.
        groups = [[f"e2e/new-{index}.spec.ts"] for index in range(16)]
        groups[0] = ["e2e/v050-task-timer.spec.ts"]
        groups[8] = ["e2e/v050-editor-modes.spec.ts"]
        balanced = GROUPS.assign_shards(groups, 8)
        old = [groups[index::8] for index in range(8)]
        def makespan(shards):
            return max(
                sum(GROUPS.group_seconds(group) for group in shard) for shard in shards
            )

        # The timer's four runtime allocations must not remain counted as one
        # cheap group. Require a material improvement over the original plan.
        self.assertLess(makespan(balanced), makespan(old) * 0.75)
        self.assertEqual(
            sorted(tuple(group) for shard in balanced for group in shard),
            sorted(map(tuple, groups)),
        )
        self.assertTrue(all(balanced))

    def test_longest_first_and_deterministic_ties(self) -> None:
        timer = ["e2e/v050-task-timer.spec.ts"]
        editor = ["e2e/v050-editor-modes.spec.ts"]
        alpha = ["e2e/alpha.spec.ts"]
        beta = ["e2e/beta.spec.ts"]
        groups = [beta, timer, alpha, editor]
        expected = [[timer], [editor, alpha, beta]]
        self.assertEqual(GROUPS.assign_shards(groups, 2), expected)
        self.assertEqual(GROUPS.assign_shards(list(reversed(groups)), 2), expected)
        self.assertEqual(GROUPS.assign_shards([beta, alpha], 2), [[alpha], [beta]])

    def test_unknown_specs_and_workspace_pair_covered_once(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(
                e2e,
                ["v050-task-timer.spec.ts"]
                + [f"new-{index}.spec.ts" for index in range(20)],
            )
            GROUPS.verify_plan(e2e, 8)
            rows = [
                row for index in range(8)
                for row in GROUPS.shard_plan_lines(e2e, index, 8)
            ]
            specs = [spec for row in rows for spec in row["specs"]]
            self.assertEqual(len(specs), len(set(specs)))
            self.assertEqual(set(specs), {f"e2e/{p.name}" for p in e2e.glob("*.spec.ts")})
            pair = [f"e2e/{GROUPS.PAIR_FIRST}", f"e2e/{GROUPS.PAIR_SECOND}"]
            self.assertEqual(sum(row["specs"] == pair for row in rows), 1)
            self.assertEqual(
                sum(row["specs"] == ["e2e/v050-task-timer.spec.ts"] for row in rows), 1
            )

    def test_assignment_missing_duplicate_and_split_groups_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["alpha.spec.ts"])
            groups = GROUPS.discover_groups(e2e)
            pair = next(group for group in groups if len(group) == 2)
            bad_plans = [
                [[groups[0]]],
                [[*groups, groups[0]]],
                [[groups[0], [pair[0]], [pair[1]]]],
                [[groups[0], ["e2e/foreign.spec.ts"]]],
            ]
            for plan in bad_plans:
                with self.subTest(plan=plan), patch.object(
                    GROUPS, "assign_shards", return_value=plan
                ):
                    with self.assertRaisesRegex(SystemExit, "shard assignment"):
                        GROUPS.verify_plan(e2e, 1)

    def test_duplicate_and_malformed_discovery_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            pair = GROUPS.discover_groups(e2e)[0]
            with patch.object(GROUPS, "discover_groups", return_value=[pair, pair]):
                with self.assertRaisesRegex(SystemExit, "duplicate spec membership"):
                    GROUPS.verify_plan(e2e, 1)
            with patch.object(GROUPS, "discover_groups", return_value=[["../escape.spec.ts"]]):
                with self.assertRaisesRegex(SystemExit, "invalid spec path"):
                    GROUPS.verify_plan(e2e, 1)

    def test_missing_wiki_and_invalid_shards_fail_closed(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            write_spec(e2e, GROUPS.PAIR_FIRST)
            with self.assertRaisesRegex(SystemExit, "missing paired spec"):
                GROUPS.verify_plan(e2e, 1)
            write_spec(e2e, GROUPS.PAIR_SECOND)
            for count in [0, -1]:
                with self.assertRaisesRegex(SystemExit, "shard_count"):
                    GROUPS.verify_plan(e2e, count)
            for index in [-1, 1]:
                with self.assertRaisesRegex(SystemExit, "out of range"):
                    GROUPS.shard_plan_lines(e2e, index, 1)

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

    def test_representative_module_suffixes_fail_closed(self) -> None:
        pattern = read_pinned_playwright_test_match()
        self.assertEqual(pattern, "**/*.@(spec|test).?(c|m)[jt]s?(x)")
        names = (
            "extra-flow.spec.mts",
            "extra-flow.test.mjs",
            "extra-flow.spec.cts",
            "extra-flow.test.cjs",
        )
        rels = [f"e2e/{name}" for name in names]
        self.assertEqual(playwright_match_rels(pattern, rels), rels)
        for name in names:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as tmp:
                e2e = Path(tmp)
                minimal_pair_tree(e2e)
                write_spec(e2e, name)
                with self.assertRaises(SystemExit) as ctx:
                    GROUPS.verify_plan(e2e, 2)
                self.assertIn(name, str(ctx.exception))
                suffix = "." + name.split(".", 1)[1]
                self.assertIn(suffix, str(ctx.exception))

    def test_all_default_discoverable_suffixes_fail_closed(self) -> None:
        pattern = read_pinned_playwright_test_match()
        names = [
            f"extra-flow{suffix}"
            for suffix in playwright_default_suffixes()
            if suffix != ".spec.ts"
        ]
        rels = [f"e2e/{name}" for name in names]
        self.assertEqual(set(playwright_match_rels(pattern, rels)), set(rels))
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e)
            for name in names:
                write_spec(e2e, name)
            with self.assertRaises(SystemExit) as ctx:
                GROUPS.verify_plan(e2e, 2)
            message = str(ctx.exception)
            for name in names:
                self.assertIn(name, message)

    def test_new_spec_ts_does_not_hide_sibling_mts(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            e2e = Path(tmp)
            minimal_pair_tree(e2e, ["brand-new-flow.spec.ts", "brand-new-flow.spec.mts"])
            with self.assertRaises(SystemExit) as ctx:
                GROUPS.verify_plan(e2e, 2)
            self.assertIn(".spec.mts", str(ctx.exception))


if __name__ == "__main__":
    unittest.main()
