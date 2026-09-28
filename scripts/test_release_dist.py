#!/usr/bin/env python3
"""Dry runs of scripts/release-dist.sh and scripts/release-preflight.sh.

Each case copies the scripts into a scratch root with a user compose (the
testdata copy of infra/rust/compose.user.yml, and the real file when the tree
has one) and runs them as the release workflow does. No registry, no network;
the preflight cases need the docker CLI with the compose plugin (no daemon).

  python3 scripts/test_release_dist.py
"""
from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "scripts/testdata/release/compose.user.yml"
REAL = ROOT / "infra/rust/compose.user.yml"
IMAGE = "ghcr.io/aisflow/fvoci"
VERSION = "0.1.0"
INDEX = "sha256:" + "1" * 64
AMD64 = "sha256:" + "2" * 64
ARM64 = "sha256:" + "3" * 64
SHA = "a" * 40
PINNED = f"{IMAGE}:{VERSION}@{INDEX}"
DOCKERFILE = """FROM rust:1 AS rust-sources
FROM rust-sources AS rust-build
ARG FVOCI_BUILD_SHA=
ENV FVOCI_BUILD_SHA=${FVOCI_BUILD_SHA}
FROM debian:bookworm-slim AS runtime
"""


def compose_sources() -> list[Path]:
    return [FIXTURE] + ([REAL] if REAL.is_file() else [])


class Scratch:
    def __init__(self, compose: str) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        (self.root / "scripts").mkdir()
        (self.root / "infra/rust").mkdir(parents=True)
        for name in ("release-dist.sh", "release-preflight.sh", "release-notes-template.md"):
            shutil.copy(ROOT / "scripts" / name, self.root / "scripts" / name)
        (self.root / "infra/rust/compose.user.yml").write_text(compose, encoding="utf-8")
        (self.root / "infra/rust/Dockerfile").write_text(DOCKERFILE, encoding="utf-8")

    def write(self, rel: str, text: str) -> None:
        (self.root / rel).write_text(text, encoding="utf-8")

    def read(self, rel: str) -> str:
        return (self.root / rel).read_text(encoding="utf-8")

    def dist(self) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["bash", str(self.root / "scripts/release-dist.sh"), "--version", VERSION, "--sha", SHA,
             "--repository", "AISFlow/fvoci", "--image", IMAGE, "--index-digest", INDEX,
             "--amd64-digest", AMD64, "--arm64-digest", ARM64,
             "--run-url", "https://github.com/AISFlow/fvoci/actions/runs/1", "--out", str(self.root / "dist")],
            capture_output=True, text=True, check=False)

    def preflight(self) -> subprocess.CompletedProcess:
        return subprocess.run(["bash", str(self.root / "scripts/release-preflight.sh"), "--version", VERSION],
                              capture_output=True, text=True, check=False)

    def ready_notes(self) -> None:
        notes = self.read("scripts/release-notes-template.md")
        notes = "\n".join(
            f"<!-- notes-for: {VERSION} -->" if "notes-for:" in line else
            "- written for this release" if "TODO(release)" in line else line
            for line in notes.splitlines()) + "\n"
        self.write("scripts/release-notes-template.md", notes)


class ReleaseDistTest(unittest.TestCase):
    def scratch(self, compose: str) -> Scratch:
        s = Scratch(compose)
        self.addCleanup(s.tmp.cleanup)
        return s

    def assert_rejected(self, compose: str, needle: str) -> None:
        proc = self.scratch(compose).dist()
        self.assertNotEqual(proc.returncode, 0, proc.stdout)
        self.assertIn(needle, proc.stderr)

    def test_renders_the_user_compose_anchor(self) -> None:
        for source in compose_sources():
            with self.subTest(source=str(source)):
                s = self.scratch(source.read_text(encoding="utf-8"))
                proc = s.dist()
                self.assertEqual(proc.returncode, 0, proc.stderr)
                rendered = s.read("dist/compose.yml")
                self.assertIn(f"x-fvoci-image: &fvoci-image {PINNED}\n", rendered)
                self.assertEqual(rendered.count(IMAGE), 1)
                self.assertNotIn("FVOCI_IMAGE", rendered)
                self.assertGreaterEqual(rendered.count("image: *fvoci-image"), 3)
                record = json.loads(s.read("dist/release.json"))
                self.assertEqual(record["image"], PINNED)
                self.assertEqual(record["platforms"], {"linux/amd64": AMD64, "linux/arm64": ARM64})
                self.assertEqual(record["composeSource"], "infra/rust/compose.user.yml")
                self.assertEqual(record["tags"], {"immutable": "0.1.0", "floating": "0.1"})
                self.assertEqual(record["publishOrder"][:3], ["index-by-digest", "smoke-linux/amd64", "smoke-linux/arm64"])
                self.assertEqual(record["publishOrder"][-1], "github-release")
                sums = dict(reversed(line.split("  ")) for line in s.read("dist/SHA256SUMS").splitlines())
                for name in ("compose.yml", "release.json", "RELEASE-NOTES.md"):
                    self.assertEqual(sums[name], hashlib.sha256((s.root / "dist" / name).read_bytes()).hexdigest())

    def test_rejects_other_image_forms(self) -> None:
        base = FIXTURE.read_text(encoding="utf-8")
        anchor = "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}"
        self.assertIn(anchor, base)
        cases = {
            "no default": (base.replace(anchor, "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE}"), "expected exactly one"),
            "required form": (base.replace(anchor, "x-fvoci-image: &fvoci-image ${FVOCI_IMAGE:?set it}"), "expected exactly one"),
            "no anchor": (base.replace(anchor, "x-other: &fvoci-image ${FVOCI_IMAGE:-ghcr.io/aisflow/fvoci:0.1.0}"), "expected exactly one"),
            "second anchor": (base + "\n" + anchor + "\n", "expected exactly one"),
            "variable elsewhere": (base.replace("    mem_limit: 4g", "    mem_limit: 4g\n    labels: [\"${FVOCI_IMAGE}\"]"), "only in the x-fvoci-image anchor"),
            "hard-coded image": (base.replace("  server:\n    image: *fvoci-image", "  server:\n    image: ghcr.io/aisflow/fvoci:latest"), "only through the x-fvoci-image anchor"),
            "no alias": (base.replace("image: *fvoci-image", "image: busybox"), "no service uses"),
            "environment needed": (base.replace('FVOCI_COOKIE_SECURE: "false"', 'FVOCI_COOKIE_SECURE: "${COOKIE_SECURE}"'), "must need no environment"),
            "env file": (base.replace("    mem_limit: 4g", "    mem_limit: 4g\n    env_file: .env"), "env_file"),
        }
        for name, (compose, needle) in cases.items():
            with self.subTest(name):
                self.assert_rejected(compose, needle)


class ReleasePreflightTest(unittest.TestCase):
    def scratch(self) -> Scratch:
        s = Scratch(FIXTURE.read_text(encoding="utf-8"))
        self.addCleanup(s.tmp.cleanup)
        return s

    def test_passes_when_ready(self) -> None:
        for source in compose_sources():
            with self.subTest(source=str(source)):
                s = self.scratch()
                s.write("infra/rust/compose.user.yml", source.read_text(encoding="utf-8"))
                s.ready_notes()
                proc = s.preflight()
                self.assertEqual(proc.returncode, 0, proc.stderr)
                self.assertIn("one-shot: ['bootstrap', 'init']", proc.stdout)

    def test_refuses_unwritten_notes(self) -> None:
        proc = self.scratch().preflight()
        self.assertEqual(proc.returncode, 1)
        self.assertIn("TODO(release) markers", proc.stderr)

    def test_refuses_notes_for_another_version(self) -> None:
        s = self.scratch()
        s.ready_notes()
        s.write("scripts/release-notes-template.md",
                s.read("scripts/release-notes-template.md").replace(f"notes-for: {VERSION}", "notes-for: 0.0.9"))
        proc = s.preflight()
        self.assertEqual(proc.returncode, 1)
        self.assertIn(f"notes-for: {VERSION}", proc.stderr)

    def test_refuses_dockerfile_without_build_sha(self) -> None:
        for dockerfile in (DOCKERFILE.replace("ARG FVOCI_BUILD_SHA=\n", ""),
                           DOCKERFILE.replace("ARG FVOCI_BUILD_SHA=\n", "") + "ARG FVOCI_BUILD_SHA=\n"):
            with self.subTest(dockerfile=dockerfile):
                s = self.scratch()
                s.ready_notes()
                s.write("infra/rust/Dockerfile", dockerfile)
                proc = s.preflight()
                self.assertEqual(proc.returncode, 1)
                self.assertIn("ARG FVOCI_BUILD_SHA", proc.stderr)

    def test_refuses_compose_without_one_shot_bootstrap(self) -> None:
        s = self.scratch()
        s.ready_notes()
        compose = s.read("infra/rust/compose.user.yml").replace("condition: service_completed_successfully",
                                                                  "condition: service_started")
        s.write("infra/rust/compose.user.yml", compose)
        proc = s.preflight()
        self.assertEqual(proc.returncode, 1)
        self.assertIn("one-shot services []", proc.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
