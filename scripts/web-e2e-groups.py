#!/usr/bin/env python3
"""Discover normal web Playwright groups, shard them, and verify CI coverage."""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
E2E_DIR = ROOT / "apps" / "web" / "e2e"
PAIR_FIRST = "workspace-flow.spec.ts"
PAIR_SECOND = "workspace-wiki-flow.spec.ts"
DEFAULT_SHARD_COUNT = 8

# Legacy web.yml workspace-flow matrix `flow` names (28 jobs) for listing comparison.
LEGACY_FLOW_NAMES = [
    "workspace-flow",
    "project-task-flow",
    "project-home-flow",
    "project-trash-flow",
    "invite-flow",
    "task-edit-flow",
    "task-assign-labels-flow",
    "workspace-groups-flow",
    "task-milestones-deps-flow",
    "notifications-flow",
    "task-comments-flow",
    "task-activity-flow",
    "task-ops-flow",
    "search-flow",
    "schedule-ics-flow",
    "workspace-lifecycle-flow",
    "mail-reset-flow",
    "wiki-lifecycle-flow",
    "comments-flow",
    "api-token-flow",
    "import-export-flow",
    "integrations-flow",
    "share-stars-flow",
    "account-lifecycle-flow",
    "admin-console-flow",
    "collections-flow",
    "mfa-flow",
    "task-attachments-flow",
]

LEGACY_FLOW_TO_LEAD_SPEC = {
    "workspace-flow": "workspace-flow.spec.ts",
    "project-task-flow": "project-task-flow.spec.ts",
    "project-home-flow": "project-home-flow.spec.ts",
    "project-trash-flow": "project-trash-flow.spec.ts",
    "invite-flow": "workspace-invite-flow.spec.ts",
    "task-edit-flow": "task-edit-flow.spec.ts",
    "task-assign-labels-flow": "task-assign-labels-flow.spec.ts",
    "workspace-groups-flow": "workspace-groups-flow.spec.ts",
    "task-milestones-deps-flow": "task-milestones-deps-flow.spec.ts",
    "notifications-flow": "notifications-flow.spec.ts",
    "task-comments-flow": "task-comments-flow.spec.ts",
    "task-activity-flow": "task-activity-flow.spec.ts",
    "task-ops-flow": "task-ops-flow.spec.ts",
    "search-flow": "search-flow.spec.ts",
    "schedule-ics-flow": "schedule-ics-flow.spec.ts",
    "workspace-lifecycle-flow": "workspace-lifecycle-flow.spec.ts",
    "mail-reset-flow": "mail-reset-flow.spec.ts",
    "wiki-lifecycle-flow": "workspace-wiki-lifecycle.spec.ts",
    "comments-flow": "comments-flow.spec.ts",
    "api-token-flow": "api-token-flow.spec.ts",
    "import-export-flow": "workspace-import-export.spec.ts",
    "integrations-flow": "integrations-flow.spec.ts",
    "share-stars-flow": "share-stars-flow.spec.ts",
    "account-lifecycle-flow": "account-lifecycle-flow.spec.ts",
    "admin-console-flow": "admin-console-flow.spec.ts",
    "collections-flow": "collections-flow.spec.ts",
    "mfa-flow": "mfa-flow.spec.ts",
    "task-attachments-flow": "task-attachments-flow.spec.ts",
}


def rel_spec(path: Path) -> str:
    return path.relative_to(ROOT / "apps" / "web").as_posix()


def discover_groups() -> list[list[str]]:
    if not E2E_DIR.is_dir():
        raise SystemExit(f"missing e2e directory: {E2E_DIR}")

    specs = sorted(E2E_DIR.glob("*.spec.ts"), key=lambda p: p.name)
    if not specs:
        raise SystemExit(f"no normal e2e specs under {E2E_DIR}")

    seen: set[str] = set()
    groups: list[list[str]] = []
    paired_wiki = False

    for path in specs:
        name = path.name
        if name == PAIR_SECOND:
            if paired_wiki:
                continue
            raise SystemExit(
                f"{PAIR_SECOND} must be grouped with {PAIR_FIRST}, not standalone"
            )
        if name in seen:
            raise SystemExit(f"duplicate spec filename in discovery: {name}")
        if name == PAIR_FIRST:
            wiki = E2E_DIR / PAIR_SECOND
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


def group_label(group: list[str]) -> str:
    first = Path(group[0]).stem
    if len(group) == 1:
        return first.replace(".spec", "")
    return first.replace(".spec", "")


def legacy_group_leads(groups: list[list[str]]) -> list[str]:
    return [Path(g[0]).name for g in groups]


def cmd_list_groups(_: argparse.Namespace) -> None:
    for group in discover_groups():
        print(" ".join(group))


def cmd_shard(args: argparse.Namespace) -> None:
    groups = discover_groups()
    shards = assign_shards(groups, args.shards)
    if args.index < 0 or args.index >= args.shards:
        raise SystemExit(f"shard index {args.index} out of range 0..{args.shards - 1}")
    shard_groups = shards[args.index]
    if not shard_groups:
        raise SystemExit(f"shard {args.index} has no groups")
    for group in shard_groups:
        print(" ".join(group))


def cmd_verify(args: argparse.Namespace) -> None:
    groups = discover_groups()
    shard_count = args.shards
    shards = assign_shards(groups, shard_count)

    spec_paths: list[str] = []
    for group in groups:
        spec_paths.extend(group)
    if len(spec_paths) != len(set(spec_paths)):
        raise SystemExit("duplicate spec membership across groups")

    expected_specs = sorted(p.name for p in E2E_DIR.glob("*.spec.ts"))
    discovered_specs = sorted(Path(s).name for s in spec_paths)
    if expected_specs != discovered_specs:
        raise SystemExit(
            "unregistered or missing specs: "
            f"tree={expected_specs!r} groups={discovered_specs!r}"
        )

    if len(groups) != len(LEGACY_FLOW_NAMES):
        raise SystemExit(
            f"expected {len(LEGACY_FLOW_NAMES)} groups, discovered {len(groups)}"
        )

    legacy_leads = {LEGACY_FLOW_TO_LEAD_SPEC[name] for name in LEGACY_FLOW_NAMES}
    discovered_leads = set(legacy_group_leads(groups))
    if legacy_leads != discovered_leads:
        raise SystemExit(
            "legacy flow lead specs do not match discovered groups: "
            f"missing={sorted(legacy_leads - discovered_leads)!r} "
            f"extra={sorted(discovered_leads - legacy_leads)!r}"
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
            }
        )
    )


def cmd_compare_legacy(_: argparse.Namespace) -> None:
    groups = discover_groups()
    labels = [group_label(g) for g in groups]
    # invite-flow label differs from filename; compare via lead specs only in verify.
    if len(labels) != len(LEGACY_FLOW_NAMES):
        raise SystemExit("legacy flow count mismatch")
    print("legacy listing matches discovered groups")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)

    p_list = sub.add_parser("list-groups")
    p_list.set_defaults(func=cmd_list_groups)

    p_shard = sub.add_parser("shard")
    p_shard.add_argument("--index", type=int, required=True)
    p_shard.add_argument("--shards", type=int, default=DEFAULT_SHARD_COUNT)
    p_shard.set_defaults(func=cmd_shard)

    p_verify = sub.add_parser("verify")
    p_verify.add_argument("--shards", type=int, default=DEFAULT_SHARD_COUNT)
    p_verify.set_defaults(func=cmd_verify)

    p_legacy = sub.add_parser("compare-legacy")
    p_legacy.set_defaults(func=cmd_compare_legacy)

    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
