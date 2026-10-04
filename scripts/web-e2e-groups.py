#!/usr/bin/env python3
"""Discover normal web Playwright groups, shard them, and verify CI coverage."""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_E2E_DIR = ROOT / "apps" / "web" / "e2e"
PAIR_FIRST = "workspace-flow.spec.ts"
PAIR_SECOND = "workspace-wiki-flow.spec.ts"
DEFAULT_SHARD_COUNT = 8
# Scheduling estimates only: completed group wall time (fresh runtime through
# cleanup), rounded up, from Web run 37194529902 at main 52129a1 (2026-10-04).
# Attempts 1/2 supply shard 0; other shards were carried forward, not rerun.
# Keep overrides only for observed groups >= 40s; the median was 17.3s, rounded
# to a 20s fallback for smaller and newly discovered groups. Build/dependency
# preparation is per-shard and excluded. Timer is ONE logical group costing the
# sum of its unchanged four fresh runs (9+7+1+5 cases), not one Playwright run.
# These estimates affect assignment only, never discovery or test selection.
DEFAULT_GROUP_SECONDS = 20
OBSERVED_GROUP_SECONDS = {
    "e2e/account-admin-vue-flow.spec.ts": 42,
    "e2e/collection-calendar-template.spec.ts": 67,
    "e2e/editor-entities-flow.spec.ts": 46,
    "e2e/main-alignment-navigation-lifetime.spec.ts": 44,
    "e2e/main-alignment-task-contract.spec.ts": 47,
    "e2e/personal-team-transfer.spec.ts": 60,
    "e2e/tb-a-dev-editor.spec.ts": 84,
    "e2e/tb-d-document-header-draft.spec.ts": 52,
    "e2e/v050-editor-modes.spec.ts": 123,
    "e2e/v050-native-archive.spec.ts": 68,
    "e2e/v050-task-timer.spec.ts": 230,
    "e2e/workspace-wiki-vue-flow.spec.ts": 42,
}
SPEC_BASENAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$")
REL_SPEC_RE = re.compile(r"^e2e/[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$")
# Playwright default testMatch '**/*.@(spec|test).?(c|m)[jt]s?(x)' (pinned
# playwright/lib/common/index.js). Supported CI grouping is only top-level
# *.spec.ts; every other default-discoverable suite must fail closed.
PLAYWRIGHT_DEFAULT_SUITE_RE = re.compile(
    r"\.(?:spec|test)\.(?:[cm])?[jt]sx?$",
    re.IGNORECASE,
)


def rel_spec(path: Path) -> str:
    rel = f"e2e/{path.name}"
    if not REL_SPEC_RE.fullmatch(rel):
        raise SystemExit(f"unsupported spec path (expected e2e/*.spec.ts): {rel}")
    return rel


def validate_spec_relpath(rel: str) -> None:
    if not REL_SPEC_RE.fullmatch(rel):
        raise SystemExit(f"invalid spec path in plan: {rel!r}")


def find_unsupported_playwright_paths(directory: Path) -> list[str]:
    """Fail closed on Playwright-like files that discovery does not cover."""
    unsupported: list[str] = []
    if not directory.is_dir():
        return unsupported

    for path in sorted(directory.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(directory).as_posix()
        name = path.name
        matched = PLAYWRIGHT_DEFAULT_SUITE_RE.search(name)
        if not matched:
            continue
        if "/" not in rel and SPEC_BASENAME_RE.fullmatch(name):
            continue
        suffix = matched.group(0).lower()
        if "/" in rel and suffix == ".spec.ts":
            unsupported.append(f"{rel} (nested spec.ts is not supported in normal e2e)")
        elif suffix == ".spec.ts":
            unsupported.append(f"{rel} (unsupported spec.ts basename)")
        else:
            unsupported.append(f"{rel} (unsupported Playwright pattern {suffix})")
    return unsupported


def discover_groups(directory: Path | None = None) -> list[list[str]]:
    directory = (directory or DEFAULT_E2E_DIR).resolve()

    if not directory.is_dir():
        raise SystemExit(f"missing e2e directory: {directory}")

    unsupported = find_unsupported_playwright_paths(directory)
    if unsupported:
        raise SystemExit(
            "unsupported Playwright paths under normal e2e; fix or move to e2e-pending:\n"
            + "\n".join(f"  - {item}" for item in unsupported)
        )

    specs = sorted(directory.glob("*.spec.ts"), key=lambda p: p.name)
    if not specs:
        raise SystemExit(f"no normal e2e specs under {directory}")

    seen: set[str] = set()
    groups: list[list[str]] = []
    paired_wiki = False

    for path in specs:
        name = path.name
        if not SPEC_BASENAME_RE.fullmatch(name):
            raise SystemExit(f"unsupported spec.ts basename: {name}")
        if name == PAIR_SECOND:
            if paired_wiki:
                continue
            raise SystemExit(
                f"{PAIR_SECOND} must be grouped with {PAIR_FIRST}, not standalone"
            )
        if name in seen:
            raise SystemExit(f"duplicate spec filename in discovery: {name}")
        if name == PAIR_FIRST:
            wiki = directory / PAIR_SECOND
            if not wiki.is_file():
                raise SystemExit(f"missing paired spec: {wiki}")
            groups.append([rel_spec(path), rel_spec(wiki)])
            seen.add(PAIR_FIRST)
            seen.add(PAIR_SECOND)
            paired_wiki = True
            continue
        groups.append([rel_spec(path)])
        seen.add(name)

    all_specs = {p.name for p in specs}
    if all_specs != seen:
        missing = sorted(all_specs - seen)
        extra = sorted(seen - all_specs)
        raise SystemExit(f"discovery mismatch missing={missing!r} extra={extra!r}")

    return groups


def group_seconds(group: list[str]) -> int:
    # A paired group has one fresh runtime. Its fallback is per logical group.
    return max(OBSERVED_GROUP_SECONDS.get(spec, DEFAULT_GROUP_SECONDS) for spec in group)


def assign_shards(groups: list[list[str]], shard_count: int) -> list[list[list[str]]]:
    if shard_count < 1:
        raise SystemExit("shard_count must be >= 1")
    shards: list[list[list[str]]] = [[] for _ in range(shard_count)]
    loads = [0] * shard_count
    # Longest first, with path and shard-index ties for reproducible plans.
    for group in sorted(groups, key=lambda g: (-group_seconds(g), tuple(g))):
        index = min(range(shard_count), key=lambda i: (loads[i], i))
        shards[index].append(group)
        loads[index] += group_seconds(group)
    return shards


def verify_plan(
    directory: Path | None,
    shard_count: int,
) -> dict[str, int | list[int]]:
    """Validate discovery and sharding for a fixture or production e2e tree."""
    root = (directory or DEFAULT_E2E_DIR).resolve()
    groups = discover_groups(root)
    shards = assign_shards(groups, shard_count)

    # Validate the assignment too: discovery alone cannot catch a scheduler
    # dropping, duplicating, splitting or altering a logical group.
    assigned_groups = [group for shard in shards for group in shard]
    if sorted(map(tuple, assigned_groups)) != sorted(map(tuple, groups)):
        raise SystemExit("shard assignment has missing, duplicate or altered groups")

    spec_paths: list[str] = []
    for group in groups:
        for spec in group:
            validate_spec_relpath(spec)
        spec_paths.extend(group)
    if len(spec_paths) != len(set(spec_paths)):
        raise SystemExit("duplicate spec membership across groups")

    expected_specs = sorted(p.name for p in root.glob("*.spec.ts"))
    discovered_specs = sorted(Path(s).name for s in spec_paths)
    if expected_specs != discovered_specs:
        raise SystemExit(
            "unregistered or missing specs: "
            f"tree={expected_specs!r} groups={discovered_specs!r}"
        )

    empty = [i for i, shard in enumerate(shards) if not shard]
    if empty:
        raise SystemExit(f"shards with no work: {empty}")

    pair = next(g for g in groups if len(g) == 2)
    if pair != [
        f"e2e/{PAIR_FIRST}",
        f"e2e/{PAIR_SECOND}",
    ]:
        raise SystemExit(f"workspace pair integrity failed: {pair!r}")

    return {
        "group_count": len(groups),
        "spec_count": len(spec_paths),
        "shard_count": shard_count,
        "groups_per_shard": [len(s) for s in shards],
    }


def shard_plan_lines(
    directory: Path | None,
    index: int,
    shard_count: int,
) -> list[dict[str, list[str]]]:
    groups = discover_groups(directory)
    shards = assign_shards(groups, shard_count)
    if index < 0 or index >= shard_count:
        raise SystemExit(f"shard index {index} out of range 0..{shard_count - 1}")
    shard_groups = shards[index]
    if not shard_groups:
        raise SystemExit(f"shard {index} has no groups")
    lines: list[dict[str, list[str]]] = []
    for group in shard_groups:
        for spec in group:
            validate_spec_relpath(spec)
        lines.append({"specs": group})
    return lines


def cmd_list_groups(_: argparse.Namespace) -> None:
    for group in discover_groups():
        print(json.dumps({"specs": group}, separators=(",", ":")))


def cmd_shard_jsonl(args: argparse.Namespace) -> None:
    for line in shard_plan_lines(None, args.index, args.shards):
        print(json.dumps(line, separators=(",", ":")))


def cmd_verify(args: argparse.Namespace) -> None:
    summary = verify_plan(None, args.shards)
    print(json.dumps(summary, separators=(",", ":")))


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    p_list = sub.add_parser("list-groups")
    p_list.set_defaults(func=cmd_list_groups)

    p_shard = sub.add_parser("shard-jsonl")
    p_shard.add_argument("--index", type=int, required=True)
    p_shard.add_argument("--shards", type=int, default=DEFAULT_SHARD_COUNT)
    p_shard.set_defaults(func=cmd_shard_jsonl)

    p_verify = sub.add_parser("verify")
    p_verify.add_argument("--shards", type=int, default=DEFAULT_SHARD_COUNT)
    p_verify.set_defaults(func=cmd_verify)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
