#!/usr/bin/env python3
"""Render perf baseline summaries (summary-<dataset>.json) as Markdown tables.

usage: summarize-perf.py <FVOCI_PERF_OUT> [dataset ...]

Values are milliseconds. Tails of series with fewer than 30 successful samples
are marked as reference only; no p99 is computed.
"""
import json
import pathlib
import sys


def fmt(value):
    if value is None:
        return "—"
    return f"{value:.0f}" if abs(value) >= 10 else f"{value:.1f}"


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    out = pathlib.Path(sys.argv[1])
    datasets = sys.argv[2:] or ["minimal", "scaled"]
    for dataset in datasets:
        path = out / f"summary-{dataset}.json"
        if not path.exists():
            print(f"_no summary for {dataset}_\n")
            continue
        summary = json.loads(path.read_text())
        results = json.loads((out / f"results-{dataset}.json").read_text())
        print(f"### Dataset `{dataset}`\n")
        sizes = results.get("results", {}).get("dataset", {}).get("sizes", {})
        if sizes:
            print("Sizes: " + ", ".join(f"{k}={v}" for k, v in sizes.items()) + "\n")
        print("| series | boundary | n | fail | median | p95 | max |")
        print("| --- | --- | ---: | ---: | ---: | ---: | ---: |")
        for key, s in summary.items():
            ok = s["n"] - s["failures"]
            p95 = fmt(s["p95"]) + ("*" if ok < 30 and s["p95"] is not None else "")
            print(
                f"| `{key}` | {s['boundary']} | {s['n']} | {s['failures']} | "
                f"{fmt(s['median'])} | {p95} | {fmt(s['max'])} |"
            )
        print("\n\\* fewer than 30 successful samples: tail is reference only.\n")
        windows = results.get("loadWindows", [])
        if windows:
            print("| window | quiet | waited s | load1 | load5 | build procs |")
            print("| --- | --- | ---: | ---: | ---: | --- |")
            for w in windows:
                print(
                    f"| {w['label']} | {'yes' if w['quiet'] else 'no'} | {w['waitedMs'] / 1000:.0f} | "
                    f"{w['load1']} | {w['load5']} | {' '.join(w['busy']) or '—'} |"
                )
            print()
    return 0


if __name__ == "__main__":
    sys.exit(main())
