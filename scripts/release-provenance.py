#!/usr/bin/env python3
"""Stamp a rendered release directory with the tooling commit (docs/RELEASING.md).

The product (image, compose.yml, env.example, INSTALL.md, notes text, version,
labels) comes from the tagged commit, rendered by that commit's
scripts/release-dist.sh. The smoke that exercises it is test tooling and comes
from the ref the workflow run started from (the tag itself on a tag push, the
dispatching branch on workflow_dispatch). This records the second commit next
to the first so both are explicit:

  release.json      "toolingSha" and "toolingRef" added; "sourceSha" unchanged
  RELEASE-NOTES.md  a provenance section naming both commits
  SHA256SUMS        recomputed over the same five files

It runs from the workflow ref, so it also stamps directories rendered by the
release-dist.sh of an older tag. A directory is stamped once.

  scripts/release-provenance.py --dist DIR --tooling-sha <40-hex> --tooling-ref REF
Standard library only.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

SUMMED = ("compose.yml", "env.example", "INSTALL.md", "release.json", "RELEASE-NOTES.md")
SHA_RE = re.compile(r"[0-9a-f]{40}")
REF_RE = re.compile(r"refs/(heads|tags)/[A-Za-z0-9._/-]+")


def fail(message: str) -> None:
    sys.exit(f"release-provenance: {message}")


def check_sums(dist: Path) -> None:
    lines = (dist / "SHA256SUMS").read_text(encoding="utf-8").splitlines()
    sums = {}
    for line in lines:
        digest, _, name = line.partition("  ")
        sums[name] = digest
    if set(sums) != set(SUMMED):
        fail(f"SHA256SUMS lists {sorted(sums)}, expected {sorted(SUMMED)}")
    for name, digest in sums.items():
        if hashlib.sha256((dist / name).read_bytes()).hexdigest() != digest:
            fail(f"SHA256SUMS does not match {name}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--dist", type=Path, required=True)
    parser.add_argument("--tooling-sha", required=True)
    parser.add_argument("--tooling-ref", required=True)
    args = parser.parse_args(argv)
    if not SHA_RE.fullmatch(args.tooling_sha):
        fail("--tooling-sha must be a full commit SHA")
    if not REF_RE.fullmatch(args.tooling_ref):
        fail("--tooling-ref must be refs/heads/<name> or refs/tags/<name>")

    dist = args.dist
    check_sums(dist)
    record_path = dist / "release.json"
    record = json.loads(record_path.read_text(encoding="utf-8"))
    source_sha = record.get("sourceSha")
    if not isinstance(source_sha, str) or not SHA_RE.fullmatch(source_sha):
        fail("release.json has no sourceSha")
    if "toolingSha" in record or "toolingRef" in record:
        fail("release.json is already stamped")
    record["toolingSha"] = args.tooling_sha
    record["toolingRef"] = args.tooling_ref
    record_path.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")

    notes_path = dist / "RELEASE-NOTES.md"
    notes = notes_path.read_text(encoding="utf-8").rstrip("\n")
    notes += (
        "\n\n## Provenance\n\n"
        f"- Product (image build, compose.yml, env.example, INSTALL.md, these notes): "
        f"`{source_sha}` (tag v{record['version']})\n"
        f"- Release smoke tooling: `{args.tooling_sha}` ({args.tooling_ref})\n"
    )
    notes_path.write_text(notes, encoding="utf-8")

    (dist / "SHA256SUMS").write_text(
        "".join(f"{hashlib.sha256((dist / name).read_bytes()).hexdigest()}  {name}\n" for name in SUMMED),
        encoding="utf-8",
    )
    print(f"stamped {dist}: product {source_sha}, tooling {args.tooling_sha} ({args.tooling_ref})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
