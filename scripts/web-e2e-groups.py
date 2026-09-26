#!/usr/bin/env python3
"""Discover normal web Playwright groups, shard them, and verify CI coverage."""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
DEFAULT_E2E_DIR = ROOT / "apps" / "web" / "e2e"
PAIR_FIRST = "workspace-flow.spec.ts"
PAIR_SECOND = "workspace-wiki-flow.spec.ts"
DEFAULT_SHARD_COUNT = 8
SPEC_BASENAME_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$")
REL_SPEC_RE = re.compile(r"^e2e/[A-Za-z0-9][A-Za-z0-9._-]*\.spec\.ts$")

UNSUPPORTED_SUFFIXES = (
    ".spec.tsx",
    ".spec.js",
    ".spec.jsx",
    ".test.ts",
    ".test.tsx",
    ".test.js",
    ".test.jsx",
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
        if path.suffix == ".ts" and name.endswith(".spec.ts"):
            if "/" in rel:
                unsupported.append(f"{rel} (nested spec.ts is not supported in normal e2e)")
            elif not SPEC_BASENAME_RE.fullmatch(name):
                unsupported.append(f"{rel} (unsupported spec.ts basename)")
            continue
        for suffix in UNSUPPORTED_SUFFIXES:
            if name.endswith(suffix):
                unsupported.append(f"{rel} (unsupported Playwright pattern {suffix})")
                break
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


def assign_shards(groups: list[list[str]], shard_count: int) -> list[list[list[str]]]:
    if shard_count < 1:
        raise SystemExit("shard_count must be >= 1")
    shards: list[list[list[str]]] = [[] for _ in range(shard_count)]
    for index, group in enumerate(groups):
        shards[index % shard_count].append(group)
    return shards


def cmd_list_groups(_: argparse.Namespace) -> None:
    for group in discover_groups():
        print(json.dumps({"specs": group}, separators=(",", ":")))


def cmd_shard_jsonl(args: argparse.Namespace) -> None:
    groups = discover_groups()
    shards = assign_shards(groups, args.shards)
    if args.index < 0 or args.index >= args.shards:
        raise SystemExit(f"shard index {args.index} out of range 0..{args.shards - 1}")
    shard_groups = shards[args.index]
    if not shard_groups:
        raise SystemExit(f"shard {args.index} has no groups")
    for group in shard_groups:
        for spec in group:
            validate_spec_relpath(spec)
        print(json.dumps({"specs": group}, separators=(",", ":")))


def cmd_verify(args: argparse.Namespace) -> None:
    directory = DEFAULT_E2E_DIR.resolve()
    groups = discover_groups()
    shard_count = args.shards
    shards = assign_shards(groups, shard_count)

    spec_paths: list[str] = []
    for group in groups:
        for spec in group:
            validate_spec_relpath(spec)
        spec_paths.extend(group)
    if len(spec_paths) != len(set(spec_paths)):
        raise SystemExit("duplicate spec membership across groups")

    expected_specs = sorted(p.name for p in directory.glob("*.spec.ts"))
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

    print(
        json.dumps(
            {
                "group_count": len(groups),
                "spec_count": len(spec_paths),
                "shard_count": shard_count,
                "groups_per_shard": [len(s) for s in shards],
            },
            separators=(",", ":"),
        )
    )


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
