#!/usr/bin/env python3
"""Compute collab.stage percentiles from a collab-capacity-probe log."""
from __future__ import annotations

import re
import sys
from collections import defaultdict


def percentile(sorted_vals: list[float], pct: int) -> float:
    if not sorted_vals:
        return 0.0
    idx = min(len(sorted_vals) * pct // 100, len(sorted_vals) - 1)
    return sorted_vals[idx]


def main() -> int:
    if len(sys.argv) != 2:
        print(f"usage: {sys.argv[0]} <collab-capacity-probe.log>", file=sys.stderr)
        return 2
    path = sys.argv[1]
    stages: dict[str, list[float]] = defaultdict(list)
    substages: dict[str, dict[str, list[float]]] = defaultdict(lambda: defaultdict(list))
    pat = re.compile(
        r'stage="(\w+)" elapsed_us=(\d+)((?: \w+_us=\d+)*)'
    )
    field_pat = re.compile(r"(\w+_us)=(\d+)")
    with open(path, encoding="utf-8", errors="replace") as handle:
        for line in handle:
            match = pat.search(line)
            if not match:
                continue
            stage = match.group(1)
            stages[stage].append(int(match.group(2)) / 1000.0)
            for field, value in field_pat.findall(match.group(3)):
                if field != "elapsed_us":
                    substages[stage][field].append(int(value) / 1000.0)

    for stage in ["validate", "auth_tx", "append_tx", "apply", "broadcast"]:
        vals = sorted(stages.get(stage, []))
        print(
            f"{stage:10} n={len(vals):6} "
            f"p50={percentile(vals, 50):7.1f} "
            f"p95={percentile(vals, 95):7.1f} "
            f"p99={percentile(vals, 99):7.1f} "
            f"max={max(vals) if vals else 0:.1f}"
        )
        for field in sorted(substages.get(stage, {})):
            subvals = sorted(substages[stage][field])
            print(
                f"  {field:18} n={len(subvals):6} "
                f"p50={percentile(subvals, 50):7.1f} "
                f"p95={percentile(subvals, 95):7.1f} "
                f"p99={percentile(subvals, 99):7.1f}"
            )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
