#!/usr/bin/env python3
"""Tests for fail-closed CI selection (production planner module)."""

from __future__ import annotations

import ast
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
CI_SELECTION_PY = ROOT / "scripts" / "ci_selection.py"


def load_ci_selection():
    spec = importlib.util.spec_from_file_location("ci_selection", CI_SELECTION_PY)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"cannot load {CI_SELECTION_PY}")
    module = importlib.util.module_from_spec(spec)
    sys.modules["ci_selection"] = module
    spec.loader.exec_module(module)
    return module


SEL = load_ci_selection()


X64_APT_MIRROR_SETUP = r"""# Select the signed Ubuntu primary archive only on x64; preserve ARM mirrors.
. /etc/os-release
[[ "$ID" == ubuntu && "$VERSION_ID" == 26.04 ]]
case "$(dpkg --print-architecture)" in
  amd64)
    test -f /etc/apt/apt-mirrors.txt
    grep -Fq 'mirror+file:/etc/apt/apt-mirrors.txt' /etc/apt/sources.list.d/ubuntu.sources
    grep -Fq 'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' /etc/apt/sources.list.d/ubuntu.sources
    test -s /usr/share/keyrings/ubuntu-archive-keyring.gpg
    printf '%s\n' 'https://archive.ubuntu.com/ubuntu/' | sudo tee /etc/apt/apt-mirrors.txt >/dev/null
    ;;
  arm64) ;;
  *) exit 1 ;;
esac
"""


def git(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        cwd=cwd,
        check=check,
        capture_output=True,
        text=True,
    )


def git_sha(cwd: Path, ref: str = "HEAD") -> str:
    return git(cwd, "rev-parse", ref).stdout.strip()


def write_file(repo: Path, rel: str, content: str = "x\n") -> None:
    path = repo / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8")


def copy_workflows(dst: Path) -> None:
    src = ROOT / ".github" / "workflows"
    target = dst / ".github" / "workflows"
    target.mkdir(parents=True, exist_ok=True)
    for path in src.iterdir():
        if path.is_file() and path.suffix in {".yml", ".yaml"}:
            shutil.copy2(path, target / path.name)


def write_minimal_rust_registry_stub(repo: Path, extra_targets: list[str] | None = None) -> None:
    (repo / "scripts").mkdir(parents=True, exist_ok=True)
    for rel in (SEL.RUST_COLLAB_CI_SCRIPT, SEL.RUST_CAPACITY_PROBE_SCRIPT):
        dst = repo / rel
        dst.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / rel, dst)
    lines = [
        "[features]",
        'db-tests = []',
        "",
    ]
    for name in ("db_integration", *(extra_targets or [])):
        lines.extend(
            [
                "[[test]]",
                f'name = "{name}"',
                f'path = "tests/{name}.rs"',
                'required-features = ["db-tests"]',
                "",
            ]
        )
    (repo / "Cargo.toml").write_text("\n".join(lines), encoding="utf-8")


def configure_git(repo: Path) -> None:
    git(repo, "config", "user.email", "ci@test")
    git(repo, "config", "user.name", "ci")
    git(repo, "config", "commit.gpgsign", "false")


class UbuntuPrimaryMirrorTest(unittest.TestCase):
    def preparation_steps(self):
        result = {}
        for workflow in ('web', 'rust', 'documents'):
            data, error = SEL._load_yaml_mapping(ROOT / '.github/workflows' / (workflow + '.yml'))
            self.assertIsNone(error)
            for name, job in data['jobs'].items():
                for step in job.get('steps', []):
                    if step.get('id') == 'sqlite' and name != 'native-arm64':
                        result[(workflow, name)] = step
        return result

    def test_every_x64_root_preparation_selects_signed_primary_before_apt(self):
        steps = self.preparation_steps()
        self.assertEqual(set(steps), {('web', 'web-checks'), ('web', 'web-native-checks'), ('web', 'workspace-browser-shard'),
            ('web', 'collaboration-build'), ('web', 'collaboration-flow'), ('rust', 'fast'),
            ('rust', 'postgres'), ('rust', 'collaboration'), ('documents', 'native-extraction')})
        for name, step in steps.items():
            with self.subTest(consumer=name):
                run = step['run']
                self.assertEqual(run.count(X64_APT_MIRROR_SETUP), 1)
                self.assertLess(run.index(X64_APT_MIRROR_SETUP), run.index('sudo apt-get update'))
                self.assertLess(run.index('sudo apt-get update'), run.index('sudo apt-get install'))
                self.assertIn('sudo apt-get install -y --no-install-recommends python3 gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8', run)
                self.assertEqual(step['env'], {'LIBCLANG_PATH': '/usr/lib/llvm-18/lib'})

    def probe(self, arch='amd64', *, os_id='ubuntu', version='26.04', missing=None, source=None, apt_exit=0):
        # Execute the actual workflow shell prefix against owned files and
        # explicit dpkg/sudo stubs; never alter host APT or make network calls.
        with tempfile.TemporaryDirectory(prefix='fvoci-apt-mirror-control-') as directory:
            root = Path(directory); fake_bin = root / 'bin'; fake_bin.mkdir()
            release = root / 'os-release'; release.write_text(f'ID={os_id}\nVERSION_ID={version}\n')
            mirrors = root / 'apt-mirrors.txt'; mirrors.write_text('original ARM/Azure mirror\n')
            sources = root / 'ubuntu.sources'
            sources.write_text(source if source is not None else 'URIs: mirror+file:/etc/apt/apt-mirrors.txt\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n')
            keyring = root / 'ubuntu-archive-keyring.gpg'; keyring.write_bytes(b'fixture keyring, not cryptographic verification')
            foreign = root / 'foreign.sources'; foreign.write_text('foreign repository preserved\n')
            before = (sources.read_bytes(), foreign.read_bytes())
            if missing: {'mirrors':mirrors, 'sources':sources, 'keyring':keyring}[missing].unlink()
            (fake_bin / 'dpkg').write_text('#!/bin/bash\n[[ "$*" == --print-architecture ]] || exit 90\nprintf "%s\n" "$FIXTURE_ARCH"\n')
            (fake_bin / 'sudo').write_text('''#!/bin/bash
set -euo pipefail
if [[ "$1" == tee && "$2" == "$FIXTURE_MIRRORS" && $# == 2 ]]; then
  exec /usr/bin/tee "$2"
elif [[ "$1" == apt-get ]]; then
  printf '%s\n' "$*" >> "$FIXTURE_TRACE"
  exit "$FIXTURE_APT_EXIT"
else
  exit 91
fi
''')
            for path in fake_bin.iterdir(): path.chmod(0o755)
            trace = root / 'trace'
            step = self.preparation_steps()[('web', 'collaboration-build')]['run']
            prefix = step[:step.index('mkdir "$RUNNER_TEMP/fvoci-sqlite"')]
            for old, new in [('/etc/os-release', release),
                ('/etc/apt/sources.list.d/ubuntu.sources', sources),
                ('/usr/share/keyrings/ubuntu-archive-keyring.gpg', keyring),
                ('/etc/apt/apt-mirrors.txt', mirrors)]:
                # Replace only known literal filesystem paths in this harness.
                prefix = prefix.replace(old, str(new))
                if sources.exists(): sources.write_text(sources.read_text().replace(old, str(new)))
            if sources.exists(): before = (sources.read_bytes(), foreign.read_bytes())
            result = subprocess.run(['bash', '-c', prefix], env={'PATH':str(fake_bin)+':/usr/bin:/bin',
                'FIXTURE_ARCH':arch, 'FIXTURE_MIRRORS':str(mirrors), 'FIXTURE_TRACE':str(trace),
                'FIXTURE_APT_EXIT':str(apt_exit)}, capture_output=True, text=True, timeout=10)
            calls = trace.read_text().splitlines() if trace.exists() else []
            self.assertEqual(foreign.read_bytes(), before[1])
            if sources.exists(): self.assertEqual(sources.read_bytes(), before[0])
            return result.returncode, calls, mirrors.read_text() if mirrors.exists() else None

    def test_x64_primary_and_arm_mirror_preservation(self):
        calls = ['apt-get update', 'apt-get install -y --no-install-recommends python3 gcc binutils curl libclang-18-dev=1:18.1.8-20ubuntu8']
        self.assertEqual(self.probe(), (0, calls, 'https://archive.ubuntu.com/ubuntu/\n'))
        self.assertEqual(self.probe('arm64'), (0, calls, 'original ARM/Azure mirror\n'))

    def test_wrong_platform_or_unsigned_missing_sources_stop_before_apt(self):
        cases = [dict(arch='riscv64'), dict(os_id='debian'), dict(version='24.04'),
            dict(missing='mirrors'), dict(missing='sources'), dict(missing='keyring'),
            dict(source='URIs: https://foreign.example/ubuntu\nSigned-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg\n'),
            dict(source='URIs: mirror+file:/etc/apt/apt-mirrors.txt\nSigned-By: /foreign/keyring.gpg\n')]
        for case in cases:
            with self.subTest(case=case):
                code, calls, mirror = self.probe(**case)
                self.assertNotEqual(code, 0)
                self.assertEqual(calls, [])
                self.assertIn(mirror, (None, 'original ARM/Azure mirror\n'))

    def test_apt_failure_propagates_without_retry(self):
        code, calls, mirror = self.probe(apt_exit=7)
        self.assertEqual(code, 7)
        self.assertEqual(calls, ['apt-get update'])
        self.assertEqual(mirror, 'https://archive.ubuntu.com/ubuntu/\n')


class GitRepoFixture:
    def __init__(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self.tmp.name)
        git(self.repo, "init", "-b", "main")
        configure_git(self.repo)

    def close(self) -> None:
        self.tmp.cleanup()

    def __enter__(self) -> GitRepoFixture:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def commit_file(self, rel: str, content: str = "x\n") -> str:
        write_file(self.repo, rel, content)
        git(self.repo, "add", rel)
        git(self.repo, "commit", "-m", f"add {rel}")
        return git_sha(self.repo)

    def rename_file(self, old: str, new: str) -> str:
        git(self.repo, "mv", old, new)
        git(self.repo, "commit", "-m", f"rename {old}")
        return git_sha(self.repo)

    def delete_file(self, rel: str) -> str:
        git(self.repo, "rm", rel)
        git(self.repo, "commit", "-m", f"delete {rel}")
        return git_sha(self.repo)


def run_cli(
    args: list[str],
    *,
    env: dict[str, str] | None = None,
    cwd: Path | None = None,
) -> subprocess.CompletedProcess[str]:
    merged = os.environ.copy()
    if env:
        merged.update(env)
    return subprocess.run(
        [sys.executable, str(CI_SELECTION_PY), *args],
        cwd=cwd or ROOT,
        env=merged,
        capture_output=True,
        text=True,
    )


def is_opt_in(workflow: str, job: str) -> bool:
    return job in SEL.OPT_IN_JOBS.get(workflow, {})


def assert_full_selection(case: unittest.TestCase, workflow: str, plan: dict, context: object) -> None:
    """Full mode selects every job except manual opt-ins, which stay unselected."""
    for job, meta in plan["jobs"].items():
        case.assertIs(meta["selected"], not is_opt_in(workflow, job), (workflow, job, context))


def dummy_event_path(directory: Path) -> Path:
    path = directory / "event.json"
    path.write_text("{}", encoding="utf-8")
    return path


class ClassifyPathsTest(unittest.TestCase):
    def test_explicit_docs_only(self) -> None:
        self.assertEqual(SEL.classify_path("docs/rewrite.md"), "docs")
        self.assertEqual(SEL.classify_path("docs/other.md"), "broaden")

    def test_frontend_src_narrow(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/src/foo.ts"), "frontend_web_install")

    def test_generated_broadens(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/src/generated/api.ts"), "broaden")

    def test_e2e_specs_select_web(self) -> None:
        self.assertEqual(SEL.classify_path("apps/web/e2e/foo.spec.ts"), "web_tests")

    def test_packages_broaden(self) -> None:
        self.assertEqual(SEL.classify_path("packages/editor/x.ts"), "broaden")

    def test_native_crate_broadens(self) -> None:
        self.assertEqual(SEL.classify_path("crates/collab-engine/src/x.rs"), "broaden")

    def test_compat_fixtures_broaden(self) -> None:
        self.assertEqual(SEL.classify_path("compat/fixtures/x"), "broaden")


class DiffParseTest(unittest.TestCase):
    def test_valid_rename_delete(self) -> None:
        raw = b"A\0src/new.ts\0D\0src/old.ts\0R100\0old-name\0new-name\0"
        paths, err = SEL.parse_name_status_z(raw)
        self.assertIsNone(err)
        self.assertEqual(paths, ["src/new.ts", "src/old.ts", "old-name", "new-name"])

    def test_truncated_rename_fails(self) -> None:
        raw = b"R100\0only-old\0"
        paths, err = SEL.parse_name_status_z(raw)
        self.assertEqual(paths, [])
        self.assertEqual(err, "DIFF_TRUNCATED_RENAME")

    def test_missing_trailing_nul_fails(self) -> None:
        raw = b"A\0file.ts"
        paths, err = SEL.parse_name_status_z(raw)
        self.assertEqual(err, "DIFF_TRUNCATED")


class PlanSelectionTest(unittest.TestCase):
    def test_frontend_selects_web_and_install(self) -> None:
        for workflow, job, expected in (
            ("web", "web-checks", True),
            ("install", "install-smoke", True),
            ("rust", "fast", False),
        ):
            plan = SEL.build_plan(
                workflow=workflow,
                event_name="pull_request",
                base_sha="a" * 40,
                head_sha="b" * 40,
                merge_base_sha="c" * 40,
                tested_sha="b" * 40,
                paths=["apps/web/src/x.ts"],
            )
            self.assertEqual(plan["jobs"][job]["selected"], expected)
            self.assertEqual(set(plan["jobs"][job]), {"selected"})

    def test_docs_skips_product_jobs(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha="c" * 40,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
        )
        self.assertEqual(plan["mode"], "narrow")
        self.assertFalse(plan["jobs"]["web-checks"]["selected"])

    def test_crate_change_is_full(self) -> None:
        decision = SEL.decide_from_paths(["crates/document-extract/src/lib.rs"])
        self.assertEqual(decision.mode, "full")

    def test_main_push_is_full(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="push",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha=None,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
        )
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_EVENT_PUSH")
        self.assertTrue(plan["plan_ok"])
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])

    def test_manual_dispatch_is_full(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="workflow_dispatch",
            base_sha=None,
            head_sha=None,
            merge_base_sha=None,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
        )
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_EVENT_WORKFLOW_DISPATCH")
        self.assertTrue(plan["plan_ok"])

    def test_merge_group_is_full(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="merge_group",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha=None,
            tested_sha="b" * 40,
            paths=["docs/rewrite.md"],
        )
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_EVENT_MERGE_GROUP")
        self.assertTrue(plan["plan_ok"])

    def test_parent_mismatch_cannot_narrow(self) -> None:
        plan = SEL.build_plan(
            workflow="web",
            event_name="pull_request",
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha=None,
            tested_sha="c" * 40,
            paths=["docs/rewrite.md"],
            force_full_reason="FULL_PR_MERGE_PARENTS_MISMATCH",
        )
        self.assertEqual(plan["mode"], "full")
        self.assertTrue(plan["plan_ok"])
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])


class GitIntegrationTest(unittest.TestCase):
    def test_multi_commit_and_merge_base(self) -> None:
        fx = GitRepoFixture()
        base = fx.commit_file("docs/rewrite.md", "a\n")
        head = fx.commit_file("docs/rewrite.md", "b\n")
        paths, err, merge_base = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertTrue(merge_base)
        self.assertIn("docs/rewrite.md", paths or [])

    def test_rename_in_diff(self) -> None:
        fx = GitRepoFixture()
        fx.commit_file("apps/web/src/a.ts")
        mid = git_sha(fx.repo)
        fx.rename_file("apps/web/src/a.ts", "apps/web/src/b.ts")
        head = git_sha(fx.repo)
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, mid, head)
        self.assertIsNone(err)
        self.assertTrue(any("b.ts" in p for p in paths or []))

    def test_base_advance_second_pr_commit(self) -> None:
        fx = GitRepoFixture()
        base = fx.commit_file("README.md")
        fx.commit_file("apps/web/src/z.ts")
        head = git_sha(fx.repo)
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertIn("apps/web/src/z.ts", paths or [])

    def test_multicommit_rename_and_delete(self) -> None:
        fx = GitRepoFixture()
        fx.commit_file("keep.md", "keep\n")
        fx.commit_file("apps/web/src/old.ts", "old\n")
        base = fx.commit_file("gone.ts", "gone\n")
        fx.rename_file("apps/web/src/old.ts", "apps/web/src/new.ts")
        fx.delete_file("gone.ts")
        head = git_sha(fx.repo)
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, head)
        self.assertIsNone(err)
        self.assertIn("apps/web/src/old.ts", paths or [])
        self.assertIn("apps/web/src/new.ts", paths or [])
        self.assertIn("gone.ts", paths or [])


class PrCheckoutFixture:
    """Origin + work clone with GitHub-like PR event/checkout trees."""

    def __init__(self) -> None:
        self.origin_tmp = tempfile.TemporaryDirectory()
        self.work_tmp = tempfile.TemporaryDirectory()
        self.origin = Path(self.origin_tmp.name)
        self.work = Path(self.work_tmp.name)
        git(self.origin, "init", "-b", "main")
        configure_git(self.origin)
        git(self.origin, "config", "uploadpack.allowReachableSHA1InWant", "true")
        copy_workflows(self.origin)
        write_minimal_rust_registry_stub(self.origin)
        write_file(self.origin, "README.md", "base docs\n")
        git(self.origin, "add", ".")
        git(self.origin, "commit", "-m", "base")
        self.base_sha = git_sha(self.origin)

    def commit_on_branch(self, branch: str, rel: str, content: str) -> str:
        git(self.origin, "checkout", "-B", branch, "main")
        write_file(self.origin, rel, content)
        git(self.origin, "add", rel)
        git(self.origin, "commit", "-m", f"{branch} {rel}")
        sha = git_sha(self.origin)
        git(self.origin, "checkout", "main")
        return sha

    def close(self) -> None:
        self.work_tmp.cleanup()
        self.origin_tmp.cleanup()

    def __enter__(self) -> PrCheckoutFixture:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def clone_work(self) -> None:
        git(self.origin, "clone", str(self.origin), str(self.work))
        configure_git(self.work)

    def merge_checkout(self, first_parent: str, second_parent: str) -> str:
        git(self.work, "checkout", "-B", "tested", first_parent)
        git(self.work, "merge", "--no-ff", "-m", "github merge", second_parent)
        return git_sha(self.work)

    def write_pr_event(self, base_sha: str, head_sha: str) -> Path:
        path = self.work / "event.json"
        path.write_text(
            json.dumps({"pull_request": {"base": {"sha": base_sha}, "head": {"sha": head_sha}}}),
            encoding="utf-8",
        )
        return path

    def plan_cli(
        self, *, tested_sha: str, event_path: Path, output: Path, github_output: Path | None = None
    ) -> subprocess.CompletedProcess[str]:
        args = [
            "plan",
            "--workflow",
            "web",
            "--repo-root",
            str(self.work),
            "--event-json",
            str(event_path),
            "--output-plan",
            str(output),
        ]
        if github_output is not None:
            args.extend(["--github-output", str(github_output)])
        return run_cli(
            args,
            env={"GITHUB_EVENT_NAME": "pull_request", "GITHUB_SHA": tested_sha},
            cwd=self.work,
        )


class PrCheckoutBindingTest(unittest.TestCase):
    def test_valid_merge_tree_can_narrow_docs(self) -> None:
        fx = PrCheckoutFixture()
        head = fx.commit_on_branch("pr", "README.md", "docs only\n")
        fx.clone_work()
        tested = fx.merge_checkout(fx.base_sha, head)
        event = fx.write_pr_event(fx.base_sha, head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "narrow")
        self.assertEqual(plan["reason_code"], "NARROW_DOCS")
        self.assertFalse(plan["jobs"]["web-checks"]["selected"])

    def test_valid_merge_frontend_selects_web(self) -> None:
        fx = PrCheckoutFixture()
        head = fx.commit_on_branch("pr", "apps/web/src/x.ts", "export {}\n")
        fx.clone_work()
        tested = fx.merge_checkout(fx.base_sha, head)
        event = fx.write_pr_event(fx.base_sha, head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "narrow")
        self.assertEqual(plan["reason_code"], "NARROW_FRONTEND_WEB_INSTALL")
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])

    def test_unrelated_code_merge_vs_docs_event_cannot_narrow(self) -> None:
        fx = PrCheckoutFixture()
        docs_head = fx.commit_on_branch("docs-pr", "README.md", "docs only\n")
        code_head = fx.commit_on_branch("code-pr", "src/lib.rs", "fn x() {}\n")
        fx.clone_work()
        tested = fx.merge_checkout(fx.base_sha, code_head)
        event = fx.write_pr_event(fx.base_sha, docs_head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_PR_MERGE_PARENTS_MISMATCH")
        self.assertTrue(plan["jobs"]["web-checks"]["selected"])

    def test_base_advance_with_exact_head_can_narrow(self) -> None:
        fx = PrCheckoutFixture()
        docs_head = fx.commit_on_branch("docs-pr", "README.md", "docs only\n")
        git(fx.origin, "checkout", "main")
        write_file(fx.origin, "src/lib.rs", "fn advanced() {}\n")
        git(fx.origin, "add", "src/lib.rs")
        git(fx.origin, "commit", "-m", "advance base")
        advanced_base = git_sha(fx.origin)
        fx.clone_work()
        tested = fx.merge_checkout(advanced_base, docs_head)
        event = fx.write_pr_event(fx.base_sha, docs_head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "narrow")
        self.assertEqual(plan["reason_code"], "NARROW_DOCS")
        self.assertEqual(plan["base_sha"], fx.base_sha)
        self.assertEqual(plan["tested_sha"], tested)

    def test_direct_head_checkout_cannot_narrow(self) -> None:
        fx = PrCheckoutFixture()
        docs_head = fx.commit_on_branch("docs-pr", "README.md", "docs only\n")
        fx.clone_work()
        git(fx.work, "checkout", "--detach", docs_head)
        event = fx.write_pr_event(fx.base_sha, docs_head)
        output = fx.work / "plan.json"
        proc = fx.plan_cli(tested_sha=docs_head, event_path=event, output=output)
        self.assertEqual(proc.returncode, 0, proc.stderr)
        plan = json.loads(output.read_text(encoding="utf-8"))
        self.assertEqual(plan["mode"], "full")
        self.assertEqual(plan["reason_code"], "FULL_PR_CHECKOUT_NOT_MERGE")


class GateSchemaTest(unittest.TestCase):
    def _plan(self, workflow: str, selected: dict[str, bool], plan_ok: bool = True) -> dict:
        jobs = {job: {"selected": selected.get(job, False)} for job in SEL.WORKFLOW_JOBS[workflow]}
        return {
            "version": SEL.PLAN_VERSION,
            "workflow": workflow,
            "mode": "narrow",
            "reason_code": "NARROW_DOCS",
            "plan_ok": plan_ok,
            "tested_sha": "a" * 40,
            "jobs": jobs,
        }

    def _needs(
        self,
        plan: object,
        workflow: str,
        results: dict[str, str] | None = None,
        *,
        plan_result: str = "success",
        plan_outputs: dict | None = None,
        extra: dict | None = None,
        omit_jobs: frozenset[str] | None = None,
        job_entries: dict[str, object] | None = None,
    ) -> str:
        needs: dict[str, object] = {}
        if plan_outputs is None:
            needs["ci-plan"] = {
                "result": plan_result,
                "outputs": {"plan_json": json.dumps(plan, separators=(",", ":"))},
            }
        else:
            needs["ci-plan"] = {"result": plan_result, "outputs": plan_outputs}
        omit = omit_jobs or frozenset()
        for job in SEL.WORKFLOW_JOBS[workflow]:
            if job in omit:
                continue
            if job_entries and job in job_entries:
                needs[job] = job_entries[job]
                continue
            needs[job] = {"result": (results or {}).get(job, "skipped"), "outputs": {}}
        if extra:
            needs.update(extra)
        return json.dumps(needs)

    def _gate(
        self,
        plan: dict,
        workflow: str,
        results: dict[str, str] | None = None,
        tested: str = "a" * 40,
        needs_json: str | None = None,
        event_name: str | None = None,
        event: object = None,
        **needs_kwargs: object,
    ) -> int:
        payload = needs_json if needs_json is not None else self._needs(
            plan, workflow, results, **needs_kwargs
        )
        if event_name is None:
            # Default: the event that legitimately produced this plan's opt-ins.
            chosen = {
                name: "true"
                for job, name in SEL.OPT_IN_JOBS.get(workflow, {}).items()
                if isinstance(plan, dict)
                and isinstance(plan.get("jobs"), dict)
                and (plan["jobs"].get(job) or {}).get("selected") is True
            }
            event_name, event = ("workflow_dispatch", {"inputs": chosen}) if chosen else ("pull_request", {})
        with tempfile.TemporaryDirectory() as tmp:
            event_path = Path(tmp) / "event.json"
            event_path.write_text(
                event if isinstance(event, str) else json.dumps(event), encoding="utf-8"
            )
            with mock.patch.dict(
                os.environ, {"GITHUB_EVENT_NAME": event_name, "GITHUB_EVENT_PATH": str(event_path)}
            ):
                return SEL.cmd_gate(
                    [
                        "--workflow",
                        workflow,
                        "--needs-json",
                        payload,
                        "--tested-sha",
                        tested,
                    ]
                )

    def test_rust_native_arm64_gate_rejects_incomplete_results(self) -> None:
        plan = self._plan("rust", {job: True for job in SEL.WORKFLOW_JOBS["rust"]})
        results = {job: "success" for job in SEL.WORKFLOW_JOBS["rust"]}
        self.assertEqual(self._gate(plan, "rust", results), 0)
        for result in ("failure", "cancelled", "skipped"):
            with self.subTest(result=result):
                self.assertEqual(self._gate(plan, "rust", {**results, "native-arm64": result}), 1)
        self.assertEqual(
            self._gate(plan, "rust", results, omit_jobs=frozenset({"native-arm64"})), 1
        )

    def test_web_budget_lanes_require_both_selected_success(self) -> None:
        selected = {"web-checks": True, "web-native-checks": True}
        plan = self._plan("web", selected)
        results = {job: "success" for job in selected}
        self.assertEqual(self._gate(plan, "web", results), 0)
        for job in selected:
            for result in ("failure", "cancelled", "skipped"):
                with self.subTest(job=job, result=result):
                    self.assertEqual(self._gate(plan, "web", {**results, job: result}), 1)
        self.assertEqual(self._gate(plan, "web", results,
                                   omit_jobs=frozenset({"web-native-checks"})), 1)
        self.assertEqual(self._gate(self._plan("web", {}), "web"), 0)

    def test_unselected_must_be_skipped(self) -> None:
        plan = self._plan("web", {"web-checks": False})
        rc = self._gate(
            plan,
            "web",
            {
                "web-checks": "success",
                "workspace-browser-shard": "skipped",
                "collaboration-flow": "skipped",
            },
        )
        self.assertEqual(rc, 1)

    def test_web_lint_job_failure_reaches_required_gate(self) -> None:
        plan = self._plan("web", {"web-static": True})
        self.assertEqual(self._gate(plan, "web", {"web-static": "failure"}), 1)
        self.assertEqual(self._gate(plan, "web", {"web-static": "success"}), 0)

    def test_selected_missing_needs_key_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", omit_jobs=frozenset({"native-extraction"}))
        self.assertEqual(rc, 1)

    def test_malformed_needs_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        rc = self._gate(plan, "documents", needs_json="{not-json")
        self.assertEqual(rc, 1)

    def test_needs_list_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        rc = self._gate(plan, "documents", needs_json="[]")
        self.assertEqual(rc, 1)

    def test_missing_needs_json_rejected(self) -> None:
        rc = SEL.cmd_gate(
            ["--workflow", "documents", "--tested-sha", "a" * 40, "--needs-json", ""]
        )
        self.assertEqual(rc, 1)

    def test_missing_result_field_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(
            plan,
            "documents",
            job_entries={"native-extraction": {"outputs": {}}},
        )
        self.assertEqual(rc, 1)

    def test_result_wrong_type_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(
            plan,
            "documents",
            job_entries={"native-extraction": {"result": 1, "outputs": {}}},
        )
        self.assertEqual(rc, 1)

    def test_plan_result_failure_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", {"native-extraction": "success"}, plan_result="failure")
        self.assertEqual(rc, 1)

    def test_extra_unknown_job_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        rc = self._gate(
            plan,
            "documents",
            {"native-extraction": "skipped"},
            extra={"mystery": {"result": "success", "outputs": {}}},
        )
        self.assertEqual(rc, 1)

    def test_plan_not_ok_rejected(self) -> None:
        plan = self._plan("web", {"web-checks": True}, plan_ok=False)
        rc = self._gate(
            plan,
            "web",
            {
                "web-checks": "success",
                "workspace-browser-shard": "skipped",
                "collaboration-flow": "skipped",
            },
        )
        self.assertEqual(rc, 1)

    def test_selected_failure_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", {"native-extraction": "failure"})
        self.assertEqual(rc, 1)

    def test_selected_cancelled_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", {"native-extraction": "cancelled"})
        self.assertEqual(rc, 1)

    def test_selected_skip_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", {"native-extraction": "skipped"})
        self.assertEqual(rc, 1)

    def test_selected_success_ok(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", {"native-extraction": "success"})
        self.assertEqual(rc, 0)

    def test_invalid_json_top_type_rejected(self) -> None:
        rc = self._gate(
            ["not", "an", "object"],  # type: ignore[arg-type]
            "documents",
            {"native-extraction": "success"},
        )
        self.assertEqual(rc, 1)

    def test_unknown_plan_keys_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["extra"] = "nope"
        rc = self._gate(plan, "documents", {"native-extraction": "skipped"})
        self.assertEqual(rc, 1)

    def test_unknown_job_keys_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["jobs"]["native-extraction"]["reason_code"] = "NARROW_DOCS"
        rc = self._gate(plan, "documents", {"native-extraction": "skipped"})
        self.assertEqual(rc, 1)

    def test_strict_bool_rejects_string_true(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["jobs"]["native-extraction"]["selected"] = "true"
        rc = self._gate(plan, "documents", {"native-extraction": "skipped"})
        self.assertEqual(rc, 1)

    def test_strict_bool_rejects_integer(self) -> None:
        plan = self._plan("documents", {"native-extraction": False})
        plan["jobs"]["native-extraction"]["selected"] = 1
        rc = self._gate(plan, "documents", {"native-extraction": "success"})
        self.assertEqual(rc, 1)

    def test_invalid_plan_json_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(
            plan,
            "documents",
            plan_outputs={"plan_json": "{bad"},
            results={"native-extraction": "success"},
        )
        self.assertEqual(rc, 1)

    def test_tested_sha_mismatch_rejected(self) -> None:
        plan = self._plan("documents", {"native-extraction": True})
        plan["mode"] = "full"
        rc = self._gate(plan, "documents", {"native-extraction": "success"}, tested="b" * 40)
        self.assertEqual(rc, 1)


class OptInSelectionTest(unittest.TestCase):
    """upgrade-smoke-arm64 runs only on an explicit manual dispatch opt-in."""

    def _install_plan(self, event_name: str, *, event: object = None, paths=None, **kwargs: object) -> dict:
        opt_ins, err = SEL.dispatch_opt_ins("install", event_name, event if event is not None else {})
        return SEL.build_plan(
            workflow="install",
            event_name=event_name,
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha="c" * 40,
            tested_sha="b" * 40,
            paths=paths,
            fatal_error=kwargs.pop("fatal_error", None) or err,
            opt_in_inputs=opt_ins,
            **kwargs,
        )

    def test_ordinary_events_never_select_upgrade(self) -> None:
        opt_in_event = {"inputs": {"run_upgrade_smoke_arm": "true"}}
        cases = [
            ("pull_request", ["apps/web/src/x.ts"]),
            ("pull_request", ["docs/rewrite.md"]),
            ("pull_request", ["src/main.rs"]),
            ("pull_request", ["scripts/upgrade-smoke.sh"]),
            ("push", ["src/main.rs"]),
            ("merge_group", ["src/main.rs"]),
        ]
        for event_name, paths in cases:
            for event in ({}, opt_in_event):
                plan = self._install_plan(event_name, event=event, paths=paths)
                self.assertFalse(plan["jobs"]["upgrade-smoke-arm64"]["selected"], (event_name, paths))
        frontend = self._install_plan("pull_request", paths=["apps/web/src/x.ts"])
        self.assertTrue(frontend["jobs"]["install-smoke"]["selected"])
        self.assertTrue(frontend["jobs"]["backup-restore-smoke"]["selected"])

    def test_manual_dispatch_off_by_default(self) -> None:
        for event in ({}, {"inputs": None}, {"inputs": {}}, {"inputs": {"run_upgrade_smoke_arm": "false"}},
                      {"inputs": {"run_upgrade_smoke_arm": False}}):
            plan = self._install_plan("workflow_dispatch", event=event)
            self.assertTrue(plan["plan_ok"], event)
            self.assertEqual(plan["reason_code"], "FULL_EVENT_WORKFLOW_DISPATCH")
            self.assertFalse(plan["jobs"]["upgrade-smoke-arm64"]["selected"], event)
            self.assertTrue(plan["jobs"]["install-smoke"]["selected"])
            self.assertTrue(plan["jobs"]["backup-restore-smoke"]["selected"])

    def test_manual_dispatch_opt_in_selects_upgrade(self) -> None:
        for value in ("true", True):
            plan = self._install_plan("workflow_dispatch", event={"inputs": {"run_upgrade_smoke_arm": value}})
            self.assertTrue(plan["plan_ok"])
            self.assertEqual(
                {job: meta["selected"] for job, meta in plan["jobs"].items()},
                {"install-smoke": True, "backup-restore-smoke": True, "upgrade-smoke-arm64": True},
            )

    def test_opt_in_input_ignored_by_other_workflows_and_fatal_plans(self) -> None:
        plan = SEL.build_plan(
            workflow="install",
            event_name="workflow_dispatch",
            base_sha=None,
            head_sha=None,
            merge_base_sha=None,
            tested_sha="b" * 40,
            paths=None,
            fatal_error="TESTED_SHA_MISMATCH",
            opt_in_inputs=frozenset({"run_upgrade_smoke_arm"}),
        )
        self.assertFalse(plan["plan_ok"])
        self.assertFalse(plan["jobs"]["upgrade-smoke-arm64"]["selected"])
        _, err = SEL.dispatch_opt_ins("web", "workflow_dispatch", {"inputs": {"run_upgrade_smoke_arm": "true"}})
        self.assertEqual(err, "DISPATCH_INPUTS_UNKNOWN")

    def test_malformed_dispatch_inputs_fail_plan(self) -> None:
        for event, code in (
            ([], "DISPATCH_EVENT_INVALID"),
            ({"inputs": "true"}, "DISPATCH_INPUTS_INVALID"),
            ({"inputs": {"run_upgrade_smoke_arm": "TRUE"}}, "DISPATCH_INPUT_VALUE_INVALID"),
            ({"inputs": {"run_upgrade_smoke_arm": 1}}, "DISPATCH_INPUT_VALUE_INVALID"),
            ({"inputs": {"run_upgrade_smoke_arm": None}}, "DISPATCH_INPUT_VALUE_INVALID"),
            ({"inputs": {"run_upgrade_smoke_arm": "true", "old": "d" * 40}}, "DISPATCH_INPUTS_UNKNOWN"),
        ):
            plan = self._install_plan("workflow_dispatch", event=event)
            self.assertFalse(plan["plan_ok"], event)
            self.assertEqual(plan["reason_code"], code)
            self.assertFalse(plan["jobs"]["upgrade-smoke-arm64"]["selected"], event)

    def test_plan_cli_dispatch_opt_in_outputs(self) -> None:
        with GitRepoFixture() as fx:
            copy_workflows(fx.repo)
            write_minimal_rust_registry_stub(fx.repo)
            sha = fx.commit_file("README.md")
            for inputs, expected, plan_ok in (
                ({"run_upgrade_smoke_arm": "true"}, "true", "true"),
                ({"run_upgrade_smoke_arm": "false"}, "false", "true"),
                ({"run_upgrade_smoke_arm": "maybe"}, "false", "false"),
            ):
                event = fx.repo / "event.json"
                event.write_text(json.dumps({"inputs": inputs}), encoding="utf-8")
                gh_out = fx.repo / "gh-out.txt"
                proc = run_cli(
                    [
                        "plan", "--workflow", "install", "--repo-root", str(fx.repo),
                        "--event-json", str(event), "--output-plan", str(fx.repo / "plan.json"),
                        "--github-output", str(gh_out),
                    ],
                    env={"GITHUB_EVENT_NAME": "workflow_dispatch", "GITHUB_SHA": sha},
                )
                self.assertEqual(proc.returncode, 0, proc.stderr)
                lines = gh_out.read_text(encoding="utf-8").splitlines()
                self.assertIn(f"select_upgrade_smoke_arm64={expected}", lines, inputs)
                self.assertIn(f"plan_ok={plan_ok}", lines, inputs)
                self.assertIn("select_install_smoke=true", lines)


class WorkflowRegistryTest(unittest.TestCase):
    def test_web_current_build_handoff_positive_and_fail_closed(self) -> None:
        import copy
        data, error = SEL._load_yaml_mapping(ROOT / ".github/workflows/web.yml")
        self.assertIsNone(error)
        jobs = data["jobs"]
        self.assertEqual(SEL._verify_web_build_handoff(jobs), [])
        mutations = []
        bad = copy.deepcopy(jobs); bad["collaboration-flow"]["needs"] = "ci-plan"; mutations.append(bad)
        bad = copy.deepcopy(jobs); bad["collaboration-build"]["timeout-minutes"] = 16; mutations.append(bad)
        for job, key in (("collaboration-build", "prepare"), ("collaboration-build", "publish"), ("collaboration-flow", "browser")):
            bad = copy.deepcopy(jobs); step = next(s for s in bad[job]["steps"] if s.get("id") == key)
            step["continue-on-error"] = True; mutations.append(bad)
        bad = copy.deepcopy(jobs)
        step = next(s for s in bad["collaboration-flow"]["steps"] if str(s.get("uses", "")).startswith("actions/download-artifact@"))
        step["with"]["run-id"] = "foreign"; mutations.append(bad)
        bad = copy.deepcopy(jobs); bad["collaboration-build"]["outputs"]["handoff_sha256"] = ""; mutations.append(bad)
        bad = copy.deepcopy(jobs)
        next(s for s in bad["collaboration-flow"]["steps"] if s.get("id") == "browser")["run"] = "bash scripts/run-web-e2e.sh --ci-use-committed-api --with-selected-backends"; mutations.append(bad)
        bad = copy.deepcopy(jobs); bad["collaboration-flow"]["steps"].append({"uses": "actions/cache@anything", "with": {"path": "target"}}); mutations.append(bad)
        for i, bad in enumerate(mutations):
            with self.subTest(mutation=i): self.assertTrue(SEL._verify_web_build_handoff(bad))


    def test_web_budget_split_preserves_commands_and_selection(self) -> None:
        data, error = SEL._load_yaml_mapping(ROOT / ".github/workflows/web.yml")
        self.assertIsNone(error)
        jobs = data["jobs"]
        commands = {
            "web-checks": [
                "bash scripts/generate-api.sh",
                "git diff --exit-code -- apps/web/openapi.json apps/web/src/generated/api.ts",
                "cargo test --locked --offline --lib --features api-schema api::openapi::tests",
                "(cd apps/web && bun run test)",
                "(cd packages/editor && bun run test)",
                "bash scripts/test-web-e2e-groups.sh",
                "python3 scripts/fixtures/web-e2e/test-build-handoff.py",
            ],
            "web-native-checks": [
                "cargo clippy --locked --offline --all-targets --features db-tests,api-schema -- -D warnings",
                "cargo test --locked --offline --test static_api",
            ],
        }
        target_keys = []
        for job, required in commands.items():
            with self.subTest(job=job):
                spec = jobs[job]
                self.assertEqual(spec["needs"], "ci-plan")
                self.assertEqual(spec["if"], "needs.ci-plan.outputs.select_web_checks == 'true'")
                self.assertEqual(spec["runs-on"], "ubuntu-26.04")
                self.assertEqual(spec["timeout-minutes"], 15)
                self.assertIn(job, jobs["web-ci-gate"]["needs"])
                lines = [line for step in SEL._run_steps(spec) for line in step["run"].splitlines()]
                actual = [line for line in lines if line in sum(commands.values(), [])]
                self.assertEqual(actual, required)
                for step in spec["steps"]:
                    self.assertNotIn("continue-on-error", step)
                    self.assertNotIn("if", step)
                cache = next(step["with"] for step in spec["steps"]
                             if step.get("with", {}).get("path") == "target")
                key = cache["key"]
                self.assertIn(job + "-ubuntu-26.04-${{ runner.arch }}-1.98.1-", key)
                self.assertIn("devdebug0-testdebug0", key)
                self.assertIn("${{ github.sha }}", key)
                self.assertIn("'crates/**'", key)
                self.assertIn("${{ steps.sqlite.outputs.cache_identity }}", key)
                self.assertNotIn("restore-keys", cache)
                target_keys.append(key)
        self.assertNotEqual(*target_keys)
        for mode, families in (("full", set()), ("narrow", {"web_tests"}),
                               ("narrow", {"frontend_web_install"}), ("narrow", set())):
            decision = SEL.SelectionDecision(mode, "TEST", frozenset(families))
            self.assertEqual(SEL.workflow_job_selected("web", "web-checks", decision),
                             SEL.workflow_job_selected("web", "web-native-checks", decision))

    def test_eslint_prettier_run_once_in_lightweight_locked_web_static(self) -> None:
        data, error = SEL._load_yaml_mapping(ROOT / ".github/workflows/web.yml")
        self.assertIsNone(error)
        jobs = data["jobs"]
        lint_steps = [
            (job_id, step)
            for job_id, job in jobs.items()
            for step in SEL._run_steps(job)
            if "bun run lint" in step["run"]
        ]
        self.assertEqual(len(lint_steps), 1)
        job_id, step = lint_steps[0]
        self.assertEqual(job_id, "web-static")
        self.assertEqual(step["run"].splitlines(), [
            "set -euo pipefail", "bun run lint:fixtures", "bun run lint", "bun run format:check",
        ])
        self.assertNotIn("continue-on-error", step)
        self.assertNotIn("if", step)
        steps = jobs[job_id]["steps"]
        install_index = next(i for i, entry in enumerate(steps) if "bun ci" in entry.get("run", ""))
        self.assertLess(install_index, steps.index(step))
        self.assertIn("web-static", jobs["web-ci-gate"]["needs"])
        self.assertEqual(jobs[job_id]["needs"], "ci-plan")
        self.assertFalse(any("cargo" in entry.get("run", "") or "rustup" in entry.get("run", "") for entry in steps))
        self.assertFalse(any("format:check" in entry.get("run", "") for jid, job in jobs.items() if jid != job_id for entry in SEL._run_steps(job)))

    def test_native_arm64_split_preserves_default_commands_and_build_inputs(self) -> None:
        data, error = SEL._load_yaml_mapping(ROOT / ".github/workflows/rust.yml")
        self.assertIsNone(error)
        jobs = data["jobs"]
        native = jobs["native-arm64"]
        self.assertEqual(native["runs-on"], "ubuntu-26.04-arm")
        self.assertEqual(native["timeout-minutes"], 15)
        self.assertEqual(native["needs"], "ci-plan")
        self.assertEqual(native["if"], "needs.ci-plan.outputs.select_native_arm64 == 'true'")
        self.assertNotIn("services", native)
        self.assertNotIn("env", native)
        self.assertNotIn("strategy", native)
        self.assertNotIn("continue-on-error", native)
        steps = native["steps"]
        check = steps[-1]
        self.assertEqual(check, {
            "name": "Native server build and policy tests (ARM64)",
            "run": "cargo build --locked --offline --bins\ncargo test --locked --offline --lib\n",
        })
        # Preserve every build input/cache step, with only the exact ARM mirror
        # override needed for the stalled Azure download. Signing stays intact.
        pg_steps = jobs["postgres"]["steps"]
        expected_setup = [dict(step) for step in pg_steps
                          if step.get("name") != "PostgreSQL service major matches matrix"][:6]
        mirror_setup = (
            "# This runner's Azure mirror stalled the pinned ARM libclang download.\n"
            "# Change only its mirror list; keep the Ubuntu archive Signed-By policy.\n"
            ". /etc/os-release\n"
            '[[ "$ID" == ubuntu && "$VERSION_ID" == 26.04 ]]\n'
            '[[ "$(dpkg --print-architecture)" == arm64 ]]\n'
            "test -f /etc/apt/apt-mirrors.txt\n"
            "grep -Fq 'mirror+file:/etc/apt/apt-mirrors.txt' /etc/apt/sources.list.d/ubuntu.sources\n"
            "grep -Fq 'Signed-By: /usr/share/keyrings/ubuntu-archive-keyring.gpg' /etc/apt/sources.list.d/ubuntu.sources\n"
            "test -s /usr/share/keyrings/ubuntu-archive-keyring.gpg\n"
            "printf '%s\\n' 'https://ports.ubuntu.com/ubuntu-ports/' | sudo tee /etc/apt/apt-mirrors.txt >/dev/null\n"
        )
        prepare = next(step for step in expected_setup if step.get("id") == "sqlite")
        self.assertIn(X64_APT_MIRROR_SETUP, prepare["run"])
        prepare["run"] = prepare["run"].replace(X64_APT_MIRROR_SETUP, "", 1)
        prepare["run"] = prepare["run"].replace("set -euo pipefail\n", "set -euo pipefail\n" + mirror_setup, 1)
        self.assertEqual(steps[:-1], expected_setup)
        cache = next(step for step in steps if step.get("name") == "Restore server build outputs")
        self.assertEqual(cache["with"]["path"], "target")
        self.assertIn("db-db-tests-nodebug", cache["with"]["key"])
        self.assertIn("steps.sqlite.outputs.cache_identity", cache["with"]["key"])
        self.assertNotIn("restore-keys", cache["with"])
        self.assertFalse(any(step.get("name") == check["name"] for step in pg_steps))
        self.assertEqual(jobs["postgres"]["timeout-minutes"], "${{ matrix.shard == 'b' && 20 || 15 }}")
        self.assertIn("native-arm64", SEL.WORKFLOW_JOBS["rust"])
        self.assertIn("native-arm64", jobs["rust-ci-gate"]["needs"])
        self.assertEqual(jobs["rust-ci-gate"]["if"], "always()")

    def test_native_arm64_selection_matches_former_postgres_arm_a_domain(self) -> None:
        for event, paths, selected in (
            ("pull_request", ["docs/rewrite.md"], False),
            ("pull_request", ["apps/web/src/x.ts"], False),
            ("pull_request", ["apps/web/e2e/navigation.spec.ts"], False),
            ("pull_request", ["src/main.rs"], True),
            ("pull_request", [".github/workflows/rust.yml"], True),
            ("push", None, True),
            ("merge_group", None, True),
            ("workflow_dispatch", None, True),
            ("unknown", None, True),
        ):
            with self.subTest(event=event, paths=paths):
                plan = SEL.build_plan(workflow="rust", event_name=event, base_sha=None,
                                      head_sha=None, merge_base_sha=None, tested_sha="a" * 40,
                                      paths=paths)
                self.assertEqual(plan["jobs"]["native-arm64"]["selected"], selected)
                self.assertEqual(plan["jobs"]["native-arm64"], plan["jobs"]["postgres"])
        plan = SEL.build_plan(workflow="rust", event_name="pull_request", base_sha=None,
                              head_sha=None, merge_base_sha=None, tested_sha="a" * 40,
                              paths=None, fatal_error="TESTED_SHA_MISMATCH")
        self.assertFalse(plan["plan_ok"])
        self.assertTrue(plan["jobs"]["native-arm64"]["selected"])

    def test_workflows_match_planner(self) -> None:
        errors = SEL.verify_workflow_registry()
        self.assertEqual(errors, [], msg="\n".join(errors))

    def test_rust_suite_inventory_matches_repo(self) -> None:
        errors = SEL.verify_rust_suite_registry(ROOT)
        self.assertEqual(errors, [], msg="\n".join(errors))

    def test_requirements_pin_pyyaml(self) -> None:
        text = (ROOT / "scripts" / "ci_selection_requirements.txt").read_text(encoding="utf-8")
        self.assertIn("PyYAML==6.0.3", text)


class RustSuiteRegistryFixture:
    """Minimal tree with real rust.yml wiring and a trimmed Cargo [[test]] registry."""

    def __init__(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name)
        (self.root / "scripts").mkdir(parents=True, exist_ok=True)
        shutil.copytree(ROOT / ".github" / "workflows", self.root / ".github" / "workflows")
        collab_script = self.root / SEL.RUST_COLLAB_CI_SCRIPT
        collab_script.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / SEL.RUST_COLLAB_CI_SCRIPT, collab_script)
        probe_script = self.root / SEL.RUST_CAPACITY_PROBE_SCRIPT
        probe_script.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / SEL.RUST_CAPACITY_PROBE_SCRIPT, probe_script)

    def close(self) -> None:
        self.tmp.cleanup()

    def __enter__(self) -> RustSuiteRegistryFixture:
        return self

    def __exit__(self, *args: object) -> None:
        self.close()

    def write_cargo(self, extra_targets: list[str] | None = None) -> None:
        write_minimal_rust_registry_stub(self.root, extra_targets)

    def write_autotest_rs(
        self,
        name: str,
        *,
        header: str = '#![cfg(feature = "db-tests")]\n',
        pad_lines: int = 0,
    ) -> None:
        path = self.root / "tests" / f"{name}.rs"
        path.parent.mkdir(parents=True, exist_ok=True)
        lines = ["//! pad"] * pad_lines
        lines.append(header.rstrip("\n"))
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")

    def mutate_rust_workflow(self, mutator) -> None:
        rust = self.root / ".github" / "workflows" / "rust.yml"
        data, parse_err = SEL._load_yaml_mapping(rust)
        if parse_err:
            raise AssertionError(parse_err)
        mutator(data)
        import yaml

        rust.write_text(yaml.safe_dump(data, sort_keys=False), encoding="utf-8")


class RustSuiteRegistryTest(unittest.TestCase):
    def test_native_arm64_wrong_scheduler_and_check_weakening_fail(self) -> None:
        def mutate_native(data: dict, field: str, value: object) -> None:
            job = data["jobs"]["native-arm64"]
            if field in ("runs-on", "timeout-minutes", "continue-on-error", "strategy"):
                job[field] = value
            else:
                job["steps"][-1][field] = value

        for field, value, expected in (
            ("runs-on", "ubuntu-26.04", "run once on ubuntu-26.04-arm"),
            ("strategy", {"matrix": {"shard": ["a", "b"]}}, "run once"),
            ("timeout-minutes", 20, "15 minute budget"),
            ("continue-on-error", True, "fail on build/policy errors"),
            ("if", "false", "exact unconditional"),
            ("env", {"CARGO_TARGET_DIR": "other"}, "exact unconditional"),
            ("run", "cargo test --locked --offline --lib --features db-tests", "exact unconditional"),
            ("run", "cargo build --locked --offline --bins\ncargo test --locked --offline --lib some_filter", "exact unconditional"),
        ):
            with self.subTest(field=field, value=value), RustSuiteRegistryFixture() as fx:
                fx.write_cargo()
                fx.mutate_rust_workflow(lambda data: mutate_native(data, field, value))
                errors = SEL.verify_workflow_registry(fx.root)
                self.assertIn(expected, "\n".join(errors))

    def test_old_combined_arm_scheduler_is_rejected(self) -> None:
        def restore_old_scheduler(data: dict) -> None:
            native = data["jobs"].pop("native-arm64")
            step = native["steps"][-1]
            step["if"] = "runner.arch == 'ARM64' && matrix.shard == 'a'"
            data["jobs"]["postgres"]["steps"].insert(7, step)
            data["jobs"]["rust-ci-gate"]["needs"].remove("native-arm64")
            data["jobs"]["ci-plan"]["outputs"].pop("select_native_arm64")

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(restore_old_scheduler)
            errors = SEL.verify_workflow_registry(fx.root)
        self.assertIn("native-arm64 job missing", "\n".join(errors))
        self.assertIn("every registered job", "\n".join(errors))

    def test_native_arm64_must_share_selection_and_gate_requirements(self) -> None:
        for mutate, expected in (
            (lambda data: data["jobs"]["native-arm64"].update({"if": "false"}), "if must be"),
            (lambda data: data["jobs"]["rust-ci-gate"]["needs"].remove("native-arm64"), "every registered job"),
            (lambda data: data["jobs"]["ci-plan"]["outputs"].pop("select_native_arm64"), "missing selector output"),
            (lambda data: data["jobs"]["native-arm64"]["steps"].pop(), "exactly once"),
        ):
            with self.subTest(expected=expected), RustSuiteRegistryFixture() as fx:
                fx.write_cargo()
                fx.mutate_rust_workflow(mutate)
                errors = SEL.verify_workflow_registry(fx.root)
                self.assertIn(expected, "\n".join(errors))

    def _execute_selected_install_artifact_prefix(self, migrate_parent: str) -> dict:
        jobs, err = SEL._rust_workflow_jobs(ROOT)
        self.assertIsNone(err)
        step = next(item for item in jobs["postgres"]["steps"] if item.get("name") == SEL.RUST_SELECTED_INSTALL_STEP)
        body = step["run"].split("<<'PYINSTALL'\n", 1)[1].rsplit("\nPYINSTALL", 1)[0]
        # Execute actual artifact parsing through its first top-level assertion.
        # Stop before filesystem ownership/setup or any product child launch.
        assertion = next(node for node in ast.parse(body).body if isinstance(node, ast.Assert))
        prefix = "\n".join(body.splitlines()[:assertion.end_lineno])
        records = [
            {
                "reason": "compiler-artifact", "target": {"name": name, "kind": [kind]},
                "profile": {"test": is_test}, "features": ["db-tests"],
                "executable": str(Path(parent) / name),
            }
            for name, kind, is_test, parent in (
                ("fvoci-server", "bin", False, "/fixture/bin"),
                ("fvoci-migrate", "bin", False, migrate_parent),
                ("selected_install_lifetime", "test", True, "/fixture/bin/deps"),
            )
        ]
        with tempfile.TemporaryDirectory() as directory:
            artifact_path = Path(directory) / "compiler-artifacts.jsonl"
            artifact_path.write_text("\n".join(json.dumps(record) for record in records))
            namespace: dict = {}
            with mock.patch.object(sys, "argv", ["selected-install-extracted.py", str(artifact_path)]):
                exec(compile(prefix, "selected-install-workflow-prefix.py", "exec"), namespace)
            return namespace

    def test_selected_install_actual_artifact_paths_reach_healthy_assertion(self) -> None:
        namespace = self._execute_selected_install_artifact_prefix("/fixture/bin")
        self.assertEqual(namespace["server"], Path("/fixture/bin/fvoci-server"))
        self.assertEqual(namespace["migrate"], Path("/fixture/bin/fvoci-migrate"))

    def test_selected_install_actual_artifact_paths_reject_unequal_parent(self) -> None:
        with self.assertRaises(AssertionError):
            self._execute_selected_install_artifact_prefix("/fixture/other-bin")

    def test_selected_install_exact_supported_execution_scope(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo(["selected_install_lifetime"])
            self.assertEqual(SEL.verify_rust_suite_registry(fx.root), [])
            jobs, err = SEL._rust_workflow_jobs(fx.root)
            self.assertIsNone(err)
            assigned, err = SEL.selected_install_inventory(jobs)
            self.assertIsNone(err)
            self.assertEqual(assigned, {"selected_install_lifetime"})
            step = next(item for item in jobs["postgres"]["steps"] if item.get("name") == SEL.RUST_SELECTED_INSTALL_STEP)
            self.assertIn('prefix="fvoci-selected-install-", dir="/run"', step["run"])
            self.assertEqual(jobs["native-arm64"]["steps"][-1]["run"].strip(), SEL.RUST_NATIVE_ARM64_RUN)
            rows = jobs["postgres"]["strategy"]["matrix"]["include"]
            self.assertEqual(
                sorted(row["runner"] for row in rows if row["shard"] == "b" and row["pg_major"] == "18"),
                ["ubuntu-26.04", "ubuntu-26.04-arm"],
            )

    def test_selected_install_missing_or_masked_execution_fails(self) -> None:
        for mutation in ("missing", "duplicate", "disabled", "masked", "no-run-only", "filtered", "zero-count", "wrong-helper-feature", "missing-arm"):
            with self.subTest(mutation=mutation), RustSuiteRegistryFixture() as fx:
                fx.write_cargo(["selected_install_lifetime"])
                def weaken(data: dict) -> None:
                    job = data["jobs"]["postgres"]
                    steps = job["steps"]
                    step = next(item for item in steps if item.get("name") == SEL.RUST_SELECTED_INSTALL_STEP)
                    if mutation == "missing":
                        steps.remove(step)
                    elif mutation == "duplicate":
                        steps.append(dict(step))
                    elif mutation == "disabled":
                        step["if"] = "false"
                    elif mutation == "masked":
                        step["continue-on-error"] = True
                    elif mutation == "no-run-only":
                        step["run"] = 'cargo test --features db-tests --test selected_install_lifetime --no-run'
                    elif mutation == "filtered":
                        step["run"] = step["run"].replace('"--test-threads=1", "--nocapture"', '"nonexistent_filter", "--nocapture"')
                    elif mutation == "zero-count":
                        step["run"] = step["run"].replace('4 passed;', '0 passed;')
                    elif mutation == "wrong-helper-feature":
                        helper = next(item for item in steps if item.get("name") == "Build production helper for PostgreSQL B native fixtures")
                        helper["run"] = helper["run"].replace('--features worker ', '')
                    else:
                        row = next(row for row in job["strategy"]["matrix"]["include"] if row["runner"] == "ubuntu-26.04-arm" and row["shard"] == "b")
                        row["pg_major"] = "17"
                fx.mutate_rust_workflow(weaken)
                errors = SEL.verify_rust_suite_registry(fx.root)
                self.assertTrue(errors, "selected install actual execution must be mandatory")
                self.assertIn("selected install" if mutation not in ("missing", "duplicate") else SEL.RUST_SELECTED_INSTALL_STEP, "\n".join(errors))

    def test_selected_install_cannot_be_double_assigned_as_pg_fixture(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo(["selected_install_lifetime"])
            def duplicate(data: dict) -> None:
                for row in data["jobs"]["postgres"]["strategy"]["matrix"]["include"]:
                    if row["shard"] == "b":
                        row["tests"] += " --test selected_install_lifetime"
            fx.mutate_rust_workflow(duplicate)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertIn("assigned to multiple CI buckets: selected_install_lifetime", "\n".join(errors))

    def test_new_cargo_target_without_ci_row_fails(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo(["missing_db_target_probe"])
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(errors, "expected missing inventory failure")
        joined = "\n".join(errors)
        self.assertIn("missing_db_target_probe", joined)
        self.assertIn("missing from rust.yml inventory", joined)

    def test_postgres_arm64_row_omission_fails(self) -> None:
        def drop_search_meili_on_arm(data: dict) -> None:
            rows = data["jobs"]["postgres"]["strategy"]["matrix"]["include"]
            for row in rows:
                if row.get("runner") == "ubuntu-26.04-arm":
                    row["tests"] = row["tests"].replace(" --test search_meili", "")

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(drop_search_meili_on_arm)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(errors)
        joined = "\n".join(errors)
        self.assertIn("search_meili", joined)
        self.assertIn("postgres matrix missing", joined)

    def _collab_script_inventory(self, fx: "RustSuiteRegistryFixture", body: str):
        (fx.root / SEL.RUST_COLLAB_CI_SCRIPT).write_text(
            "#!/usr/bin/env bash\n" + body, encoding="utf-8"
        )
        return SEL.collaboration_script_inventory(fx.root)

    def test_collaboration_script_two_invocations_union_all_targets(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            tests, err = SEL.collaboration_script_inventory(fx.root)
        self.assertIsNone(err)
        self.assertEqual(len(tests), 10)
        self.assertIn("task_collab_integration", tests)
        self.assertIn("collab_product", tests)

    def test_collaboration_script_target_in_two_invocations_fails(self) -> None:
        prefix = "cargo test --locked --offline --no-fail-fast --features db-tests \\\n"
        body = (
            prefix + "  --test collab_product \\\n  --test task_collab_integration \\\n  | tee log\n"
            + prefix + "  --test task_collab_integration \\\n  -- --test-threads=1 \\\n  | tee -a log\n"
        )
        with RustSuiteRegistryFixture() as fx:
            tests, err = self._collab_script_inventory(fx, body)
        self.assertEqual(tests, set())
        self.assertIn("more than once", err or "")
        self.assertIn("task_collab_integration", err or "")

    def test_collaboration_script_libtest_suffix_allows_only_scheduling(self) -> None:
        prefix = "cargo test --locked --offline --no-fail-fast --features db-tests \\\n"
        for suffix, allowed in (
            ("--test-threads=1", True),
            ("--nocapture", True),
            ("some_filter", False),
            ("--skip personal_transfer", False),
            ("--ignored", False),
            ("--test-threads=4", False),
        ):
            body = prefix + "  --test task_collab_integration \\\n  -- " + suffix + " \\\n  | tee log\n"
            with self.subTest(suffix=suffix), RustSuiteRegistryFixture() as fx:
                tests, err = self._collab_script_inventory(fx, body)
                if allowed:
                    self.assertIsNone(err)
                    self.assertEqual(tests, {"task_collab_integration"})
                else:
                    self.assertEqual(tests, set())
                    self.assertIn("libtest filter", err or "")

    def test_trimmed_inventory_with_real_workflow_passes(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertEqual(errors, [], msg="\n".join(errors))

    def test_verify_workflows_surfaces_rust_inventory_failure(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo(["missing_db_target_probe"])
            proc = run_cli(["verify-workflows", "--repo-root", str(fx.root)])
        self.assertEqual(proc.returncode, 1, proc.stderr)
        self.assertIn("missing_db_target_probe", proc.stderr)

    def test_missing_cargo_fails_instead_of_silent_pass(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            (fx.root / "Cargo.toml").unlink()
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertIn("missing root Cargo.toml", "\n".join(errors))

    def test_autodiscovered_root_test_without_ci_row_fails(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.write_autotest_rs("missing_db_target_probe")
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("missing_db_target_probe" in err for err in errors))

    def test_postgres_decoy_test_string_without_matrix_execution_fails(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            rust = fx.root / ".github" / "workflows" / "rust.yml"
            text = rust.read_text(encoding="utf-8")
            decoy = (
                "      - name: PostgreSQL integration tests decoy\n"
                "        run: echo --test db_integration --features db-tests\n"
            )
            text = text.replace(
                "      - name: PostgreSQL integration tests\n",
                decoy + "      - name: PostgreSQL integration tests\n",
                1,
            )
            text = text.replace(
                "        run: cargo test --locked --offline --no-fail-fast --features db-tests ${{ matrix.tests }}\n",
                "        run: cargo test --locked --offline --no-fail-fast --features db-tests\n",
                1,
            )
            rust.write_text(text, encoding="utf-8")
            errors = SEL.verify_rust_suite_registry(fx.root)
        joined = "\n".join(errors)
        self.assertIn("matrix.tests", joined)

    def test_s3_missing_db_features_on_execution_command_fails(self) -> None:
        def strip_db_features(data: dict) -> None:
            steps = data["jobs"]["postgres"]["steps"]
            for step in steps:
                if step.get("name") == SEL.RUST_S3_INTEGRATION_STEP:
                    step["run"] = (
                        "bash scripts/start-test-minio.sh cargo test --locked --offline "
                        "--no-fail-fast --test attachment_s3_integration"
                    )

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(strip_db_features)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("S3 integration step" in err for err in errors))

    def test_late_crate_cfg_autotest_is_registered(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.write_autotest_rs("missing_db_target_probe", pad_lines=12)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("missing_db_target_probe" in err for err in errors))

    def test_item_level_cfg_only_root_test_fails(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            path = fx.root / "tests" / "missing_db_target_probe.rs"
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text('#[cfg(feature = "db-tests")]\nmod suite {}\n', encoding="utf-8")
            errors = SEL.verify_rust_suite_registry(fx.root)
        joined = "\n".join(errors)
        self.assertIn("missing_db_target_probe", joined)
        self.assertIn("no crate", joined)

    def test_postgres_integration_step_if_false_fails(self) -> None:
        def disable_postgres_step(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["if"] = "false"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(disable_postgres_step)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("must not have an if condition" in err for err in errors))

    def test_s3_integration_step_if_false_fails(self) -> None:
        def disable_s3_step(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_S3_INTEGRATION_STEP:
                    step["if"] = "false"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(disable_s3_step)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("S3 integration step if must be" in err for err in errors))

    def test_collaboration_integration_step_if_false_fails(self) -> None:
        def disable_collab_step(data: dict) -> None:
            for step in data["jobs"]["collaboration"]["steps"]:
                if step.get("name") == SEL.RUST_COLLAB_INTEGRATION_STEP:
                    step["if"] = "false"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(disable_collab_step)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("collaboration integration step must not have an if condition" in err for err in errors))

    def test_postgres_integration_continue_on_error_fails(self) -> None:
        def mask_postgres_step(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["continue-on-error"] = True

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(mask_postgres_step)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("continue-on-error" in err for err in errors))

    def test_collaboration_echo_script_not_execution_fails(self) -> None:
        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            rust = fx.root / ".github" / "workflows" / "rust.yml"
            text = rust.read_text(encoding="utf-8")
            rust.write_text(
                text.replace(
                    "        run: bash scripts/run-rust-collaboration-ci-tests.sh\n",
                    "        run: echo bash scripts/run-rust-collaboration-ci-tests.sh\n",
                ),
                encoding="utf-8",
            )
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("collaboration integration step" in err for err in errors))

    def test_postgres_integration_no_run_suffix_fails(self) -> None:
        def add_no_run(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["run"] = step["run"] + " --no-run"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(add_no_run)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("--no-run" in err for err in errors))

    def test_postgres_integration_exclude_fails(self) -> None:
        def add_exclude(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["run"] = step["run"].replace(
                        "${{ matrix.tests }}",
                        "--exclude fvoci-server ${{ matrix.tests }}",
                    )

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(add_exclude)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("--exclude" in err for err in errors))

    def test_postgres_integration_libtest_skip_fails(self) -> None:
        def add_libtest_filter(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["run"] = step["run"] + " -- --skip '*'"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(add_libtest_filter)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("libtest filter" in err for err in errors))

    def test_postgres_integration_shell_or_true_fails(self) -> None:
        def add_or_true(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["run"] = step["run"] + " || true"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(add_or_true)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("shell operator" in err for err in errors))

    def test_postgres_matrix_tests_no_run_fragment_fails(self) -> None:
        def poison_matrix_tests(data: dict) -> None:
            rows = data["jobs"]["postgres"]["strategy"]["matrix"]["include"]
            rows[0]["tests"] = "--no-run --test db_integration"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(poison_matrix_tests)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("--no-run" in err or "--test NAME" in err for err in errors))

    def test_postgres_integration_continue_on_error_string_fails(self) -> None:
        def string_continue_on_error(data: dict) -> None:
            for step in data["jobs"]["postgres"]["steps"]:
                if step.get("name") == SEL.RUST_POSTGRES_INTEGRATION_STEP:
                    step["continue-on-error"] = "true"

        with RustSuiteRegistryFixture() as fx:
            fx.write_cargo()
            fx.mutate_rust_workflow(string_continue_on_error)
            errors = SEL.verify_rust_suite_registry(fx.root)
        self.assertTrue(any("continue-on-error" in err for err in errors))


class RegistryMutationCliTest(unittest.TestCase):
    def _mutated_root(self) -> Path:
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp, True)
        copy_workflows(tmp)
        write_minimal_rust_registry_stub(tmp)
        return tmp

    def _plan_against(self, repo_root: Path) -> tuple[subprocess.CompletedProcess[str], Path]:
        event = dummy_event_path(repo_root)
        output = repo_root / "green-plan.json"
        github_output = repo_root / "github-output.txt"
        proc = run_cli(
            [
                "plan",
                "--workflow",
                "web",
                "--repo-root",
                str(repo_root),
                "--event-json",
                str(event),
                "--output-plan",
                str(output),
                "--github-output",
                str(github_output),
            ],
            env={"GITHUB_EVENT_NAME": "pull_request", "GITHUB_SHA": "a" * 40},
        )
        return proc, output

    def _assert_no_green_outputs(self, proc: subprocess.CompletedProcess[str], output: Path, needle: str) -> None:
        self.assertEqual(proc.returncode, 1, proc.stderr)
        self.assertIn("workflow registry validation failed", proc.stderr)
        self.assertIn(needle, proc.stderr)
        self.assertFalse(output.exists(), "plan must not emit outputs after registry failure")
        github_output = output.parent / "github-output.txt"
        self.assertFalse(github_output.exists(), "GITHUB_OUTPUT must stay empty after registry failure")

    def test_pr_path_filters_cannot_leave_gate_pending(self) -> None:
        root = self._mutated_root()
        path = root / ".github/workflows/web.yml"
        path.write_text(path.read_text().replace("  pull_request:\n", "  pull_request:\n    paths: ['apps/web/**']\n", 1))
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "pull_request must be unfiltered")

    def test_checkout_ref_and_repository_overrides_rejected(self) -> None:
        for override in ("ref: attacker-head", "repository: attacker/repo", "fetch-depth: 1"):
            with self.subTest(override=override):
                root = self._mutated_root()
                path = root / ".github/workflows/web.yml"
                path.write_text(path.read_text().replace("          fetch-depth: 0", "          " + override, 1))
                proc, output = self._plan_against(root)
                self._assert_no_green_outputs(proc, output, "ci-plan must checkout the event merge")

    def test_runner_sha_yaml_override_rejected(self) -> None:
        root = self._mutated_root()
        path = root / ".github/workflows/web.yml"
        path.write_text(path.read_text().replace("          GITHUB_EVENT_NAME:", "          GITHUB_SHA: attacker-head\n          GITHUB_EVENT_NAME:", 1))
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "must not override trusted GITHUB_SHA")

    def test_new_job_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        injected = """
  new-suite:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_new_suite == 'true'
    runs-on: ubuntu-26.04
    steps:
      - run: echo new
"""
        web.write_text(text.replace("  web-ci-gate:", injected + "  web-ci-gate:"), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "unregistered job id new-suite")

    def _mutate_install(self, root: Path, old: str, new: str) -> None:
        path = root / ".github" / "workflows" / "install.yml"
        text = path.read_text(encoding="utf-8")
        self.assertEqual(text.count(old), 1, old)
        path.write_text(text.replace(old, new), encoding="utf-8")

    def test_install_opt_in_wiring_mutations_rejected_before_outputs(self) -> None:
        cases = (
            ("        default: false\n", "        default: true\n", "input run_upgrade_smoke_arm must be"),
            ("        type: boolean\n", "        type: string\n", "input run_upgrade_smoke_arm must be"),
            (
                "      run_upgrade_smoke_arm:\n",
                "      upgrade_old:\n        type: string\n      run_upgrade_smoke_arm:\n",
                "workflow_dispatch inputs must be exactly",
            ),
            (
                "  upgrade-smoke-arm64:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n    runs-on: ubuntu-26.04-arm\n",
                "  upgrade-smoke-arm64:\n    needs: ci-plan\n    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n    runs-on: ubuntu-26.04\n",
                "upgrade-smoke-arm64 runs-on must be ubuntu-26.04-arm",
            ),
            (
                "    if: needs.ci-plan.outputs.select_upgrade_smoke_arm64 == 'true'\n",
                "    if: github.event_name == 'workflow_dispatch'\n",
                "upgrade-smoke-arm64 if must be",
            ),
            (
                "    needs: [ci-plan, install-smoke, backup-restore-smoke, upgrade-smoke-arm64]\n",
                "    needs: [ci-plan, install-smoke, backup-restore-smoke]\n",
                "install-ci-gate needs must be",
            ),
            (
                "      select_upgrade_smoke_arm64: ${{ steps.plan.outputs.select_upgrade_smoke_arm64 }}\n",
                "",
                "missing selector output select_upgrade_smoke_arm64",
            ),
        )
        for old, new, needle in cases:
            root = self._mutated_root()
            self._mutate_install(root, old, new)
            proc, output = self._plan_against(root)
            self._assert_no_green_outputs(proc, output, needle)

    def test_opt_in_input_on_other_workflow_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        web.write_text(
            text.replace(
                "  workflow_dispatch:\n",
                "  workflow_dispatch:\n    inputs:\n      run_upgrade_smoke_arm:\n        type: boolean\n        default: false\n",
                1,
            ),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "web: workflow_dispatch inputs must be exactly []")

    def test_gate_suffixed_product_job_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        injected = """
  sneaky-ci-gate:
    needs: ci-plan
    if: needs.ci-plan.outputs.select_sneaky_ci_gate == 'true'
    runs-on: ubuntu-26.04
    steps:
      - run: echo sneaky
"""
        web.write_text(text.replace("  web-ci-gate:", injected + "  web-ci-gate:"), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "unregistered job id sneaky-ci-gate")

    def test_release_workflow_must_stay_tag_only(self) -> None:
        root = self._mutated_root()
        release = root / ".github" / "workflows" / "release.yml"
        text = release.read_text(encoding="utf-8")
        release.write_text(text.replace("  workflow_dispatch:", "  pull_request:\n  workflow_dispatch:", 1), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "release.yml: triggers must be exactly push (tags) and workflow_dispatch")

    def test_release_workflow_write_scope_outside_listed_job_rejected(self) -> None:
        root = self._mutated_root()
        release = root / ".github" / "workflows" / "release.yml"
        text = release.read_text(encoding="utf-8")
        marker = "      contents: read\n      checks: read\n"
        self.assertIn(marker, text)
        release.write_text(text.replace(marker, "      contents: write\n      checks: read\n", 1), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "release.yml: verify may not write ['contents']")

    def test_release_workflow_per_tag_concurrency_rejected(self) -> None:
        root = self._mutated_root()
        release = root / ".github" / "workflows" / "release.yml"
        text = release.read_text(encoding="utf-8")
        marker = "  group: release-ghcr-fvoci\n"
        self.assertIn(marker, text)
        release.write_text(text.replace(marker, "  group: release-${{ github.ref_name }}\n", 1), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(
            proc, output, "release.yml: concurrency must be one fixed group with cancel-in-progress: false"
        )

    def test_release_publish_job_may_not_write_contents(self) -> None:
        root = self._mutated_root()
        release = root / ".github" / "workflows" / "release.yml"
        text = release.read_text(encoding="utf-8")
        marker = "  publish:\n    needs: [verify, index, smoke]\n"
        self.assertIn(marker, text)
        publish = text.index(marker)
        scope = text.index("      packages: write\n", publish)
        release.write_text(text[:scope] + "      contents: write\n" + text[scope:], encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "release.yml: publish may not write ['contents']")

    def test_new_workflow_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        extra = root / ".github" / "workflows" / "extra.yml"
        extra.write_text("name: Extra\non: push\njobs:\n  extra-job:\n    runs-on: ubuntu-26.04\n    steps:\n      - run: echo x\n", encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "unknown workflow file extra.yml")

    def test_turso_manual_workflow_security_boundaries_before_outputs(self) -> None:
        target_init = "          printf 'CARGO_TARGET_DIR=%s/turso-target\\n' \"$RUNNER_TEMP\" >> \"$GITHUB_ENV\"\n"
        cases = [
            ("  workflow_dispatch:\n", "  pull_request_target:\n", "manual dispatch only"),
            ("      destructive:\n", "      checkout_sha:\n", "fixed phase inputs"),
            ("  contents: read\n", "  contents: write\n", "contents read only"),
            ("  cancel-in-progress: false\n", "  cancel-in-progress: true\n", "fixed database concurrency"),
            ("github.repository == 'AISFlow/fvoci'", "github.repository == 'attacker/fvoci'", "trusted admission"),
            ("ref: ${{ github.sha }}", "ref: ${{ inputs.checkout_sha }}", "pre-Environment admission steps"),
            ("persist-credentials: false", "persist-credentials: true", "pre-Environment admission steps"),
            ("    needs: admission\n", "    needs: []\n", "runtime needs successful trusted admission"),
            ("    environment: fvoci-turso-test\n", "    environment: production\n", "fixed Environment"),
            (" --admit\n", " --consume\n", "pre-Environment admission steps"),
            ("--no-run --message-format=json", "--message-format=json", "fixed fresh compilation"),
            ("      CARGO_INCREMENTAL: 0\n", "      TOKEN: ${{ secrets.FVOCI_TEST_TURSO_AUTH_TOKEN }}\n", "credential-free compiler environment"),
            (" --consume\n", " --consume || true\n", "only one sanitized runtime step"),
            ("branches: [fvoci/v060-turso-verified-connection]", "branches: ['*']", "fixed credential-free bootstrap"),
            ("    if: github.event_name == 'workflow_dispatch'", "    if: github.event_name == 'push'", "runtime needs successful trusted admission"),
            ("      CARGO_INCREMENTAL: 0\n", "      CARGO_INCREMENTAL: 0\n      CARGO_TARGET_DIR: ${{ runner.temp }}/turso-target\n", "credential-free compiler environment"),
            (target_init, "", "maintained pinned compiler/native preparation"),
            (target_init, target_init.replace("$RUNNER_TEMP", "/foreign"), "maintained pinned compiler/native preparation"),
            (target_init + "          rustup toolchain install 1.98.1 --profile minimal\n", "          rustup toolchain install 1.98.1 --profile minimal\n" + target_init, "maintained pinned compiler/native preparation"),
        ]
        for old, new, needle in cases:
            with self.subTest(boundary=needle):
                root = self._mutated_root()
                path = root / ".github" / "workflows" / "turso-test.yml"
                text = path.read_text(encoding="utf-8")
                self.assertIn(old, text)
                path.write_text(text.replace(old, new, 1), encoding="utf-8")
                proc, output = self._plan_against(root)
                self._assert_no_green_outputs(proc, output, needle)

    def test_turso_target_initialization_moved_to_compile_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        path = root / ".github" / "workflows" / "turso-test.yml"
        text = path.read_text(encoding="utf-8")
        target_init = "          printf 'CARGO_TARGET_DIR=%s/turso-target\\n' \"$RUNNER_TEMP\" >> \"$GITHUB_ENV\"\n"
        self.assertIn(target_init, text)
        self.assertIn("          cargo test --locked", text)
        text = text.replace(target_init, "", 1).replace("          cargo test --locked", target_init + "          cargo test --locked", 1)
        path.write_text(text, encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "maintained pinned compiler/native preparation")
        self.assertIn("fixed fresh compilation", proc.stderr)

    def test_old_runner_labels_rejected_before_outputs(self) -> None:
        for filename in (*SEL.WORKFLOW_YAML.values(), SEL.RELEASE_WORKFLOW_FILE, SEL.TURSO_MANUAL_WORKFLOW_FILE):
            for old_label in ("ubuntu-24.04", "ubuntu-22.04", "ubuntu-latest"):
                with self.subTest(workflow=filename, runner=old_label):
                    root = self._mutated_root()
                    path = root / ".github" / "workflows" / filename
                    text = path.read_text(encoding="utf-8")
                    self.assertIn("runs-on: ubuntu-26.04", text)
                    path.write_text(text.replace("runs-on: ubuntu-26.04", "runs-on: " + old_label, 1), encoding="utf-8")
                    proc, output = self._plan_against(root)
                    self._assert_no_green_outputs(proc, output, "requires explicit Ubuntu 26.04 runners")

    def test_old_os_cache_prefix_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        path = root / ".github" / "workflows" / "collab-engine.yml"
        text = path.read_text(encoding="utf-8")
        self.assertIn("restore-keys: v1-collab-engine-ubuntu-26.04-", text)
        path.write_text(text.replace("restore-keys: v1-collab-engine-ubuntu-26.04-",
                                     "restore-keys: v1-collab-engine-ubuntu-24.04-"), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "cache restore-keys must bind Ubuntu 26.04")

    def test_missing_selector_output_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        web.write_text(text.replace("      select_web_checks: ${{ steps.plan.outputs.select_web_checks }}\n", ""), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "missing selector output select_web_checks")

    def test_native_budget_registry_mutations_rejected_before_outputs(self) -> None:
        for mutation in ("missing-job", "missing-gate-need", "wrong-condition"):
            with self.subTest(mutation=mutation):
                root = self._mutated_root()
                web = root / ".github/workflows/web.yml"
                text = web.read_text(encoding="utf-8")
                if mutation == "missing-job":
                    start = text.index("  web-native-checks:")
                    end = text.index("  workspace-browser-shard:", start)
                    changed = text[:start] + text[end:]
                    needle = "web-native-checks"
                elif mutation == "missing-gate-need":
                    changed = text.replace("        web-native-checks,\n", "", 1)
                    needle = "needs must be ci-plan and every registered job"
                else:
                    start = text.index("  web-native-checks:")
                    changed = text[:start] + text[start:].replace(
                        "needs.ci-plan.outputs.select_web_checks == 'true'",
                        "needs.ci-plan.outputs.select_web_static == 'true'", 1)
                    needle = "web-native-checks if must be"
                self.assertNotEqual(text, changed, "negative control must mutate the workflow")
                web.write_text(changed, encoding="utf-8")
                proc, output = self._plan_against(root)
                self._assert_no_green_outputs(proc, output, needle)

    def test_gate_needs_mismatch_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        gate_start = text.index("  web-ci-gate:")
        before_gate, gate = text[:gate_start], text[gate_start:]
        original_needs = """    needs:
      [
        ci-plan,
        web-static,
        web-checks,
        web-native-checks,
        workspace-browser-shard,
        collaboration-build,
        collaboration-flow,
      ]"""
        self.assertIn(original_needs, gate)
        mutated = before_gate + gate.replace(original_needs, "    needs: [ci-plan, web-checks]", 1)
        self.assertNotEqual(text, mutated, "negative control must mutate the actual gate")
        web.write_text(mutated, encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "needs must be ci-plan and every registered job")

    def test_swapped_needs_json_expr_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        rust = root / ".github" / "workflows" / "rust.yml"
        text = rust.read_text(encoding="utf-8")
        rust.write_text(
            text.replace(
                "NEEDS_JSON: ${{ toJSON(needs) }}",
                "NEEDS_JSON: ${{ toJSON(needs.postgres) }}",
            ),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "env must be exactly")

    def test_forged_needs_json_literal_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        rust = root / ".github" / "workflows" / "rust.yml"
        text = rust.read_text(encoding="utf-8")
        rust.write_text(
            text.replace(
                "NEEDS_JSON: ${{ toJSON(needs) }}",
                'NEEDS_JSON: \'{"fast":{"result":"success"}}\'',
            ),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "env must be exactly")

    def test_swapped_tested_sha_expr_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        rust = root / ".github" / "workflows" / "rust.yml"
        text = rust.read_text(encoding="utf-8")
        rust.write_text(
            text.replace(
                "TESTED_SHA: ${{ github.sha }}",
                "TESTED_SHA: ${{ needs.fast.result }}",
            ),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "env must be exactly")

    def test_extra_job_env_mapping_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        rust = root / ".github" / "workflows" / "rust.yml"
        text = rust.read_text(encoding="utf-8")
        rust.write_text(
            text.replace(
                "          TESTED_SHA: ${{ github.sha }}\n",
                "          TESTED_SHA: ${{ github.sha }}\n          JOB_FAST: ${{ needs.postgres.result }}\n",
            ),
            encoding="utf-8",
        )
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "env must be exactly")

    def test_decoy_echo_gate_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        canonical = (
            '          python3 scripts/ci_selection.py gate --workflow web '
            '--needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n'
        )
        decoy = (
            '          echo python3 scripts/ci_selection.py gate --workflow web '
            '--needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n'
        )
        self.assertIn(canonical, text)
        web.write_text(text.replace(canonical, decoy), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "canonical gate invocation")

    def test_commented_gate_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        canonical = (
            '          python3 scripts/ci_selection.py gate --workflow web '
            '--needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n'
        )
        commented = (
            '          # python3 scripts/ci_selection.py gate --workflow web '
            '--needs-json "$NEEDS_JSON" --tested-sha "$TESTED_SHA"\n'
            "          true\n"
        )
        self.assertIn(canonical, text)
        web.write_text(text.replace(canonical, commented), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "canonical gate invocation")

    def test_rust_plan_missing_selector_wrapper_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        rust = root / ".github" / "workflows" / "rust.yml"
        text = rust.read_text(encoding="utf-8")
        wrapper = "          bash scripts/test-ci-selection.sh\n"
        self.assertIn(wrapper, text)
        rust.write_text(text.replace(wrapper, "", 1), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "must run scripts/test-ci-selection.sh")

    def test_web_plan_duplicate_selector_wrapper_rejected_before_outputs(self) -> None:
        root = self._mutated_root()
        web = root / ".github" / "workflows" / "web.yml"
        text = web.read_text(encoding="utf-8")
        injected = "          bash scripts/test-ci-selection.sh\n"
        marker = "          python3 scripts/ci_selection.py plan \\\n"
        self.assertIn(marker, text)
        web.write_text(text.replace(marker, injected + marker, 1), encoding="utf-8")
        proc, output = self._plan_against(root)
        self._assert_no_green_outputs(proc, output, "must not duplicate scripts/test-ci-selection.sh")


AGENT_DOCS = ("AGENTS.md", ".agents/environment.md")


def plan_all_workflows(paths: list[str] | None, event_name: str = "pull_request", **kwargs: object) -> dict:
    return {
        workflow: SEL.build_plan(
            workflow=workflow,
            event_name=event_name,
            base_sha="a" * 40,
            head_sha="b" * 40,
            merge_base_sha="c" * 40,
            tested_sha="b" * 40,
            paths=paths,
            **kwargs,
        )
        for workflow in SEL.WORKFLOW_JOBS
    }


class ImpactUnionTest(unittest.TestCase):
    def assert_selected_workflows(self, paths: list[str], selected: set[str]) -> None:
        for workflow, plan in plan_all_workflows(paths).items():
            self.assertEqual(plan["mode"], "narrow", paths)
            for job, meta in plan["jobs"].items():
                self.assertIs(meta["selected"], workflow in selected and not is_opt_in(workflow, job), (paths, workflow, job))

    def test_browser_and_unit_tests_run_web_without_install(self) -> None:
        for path in (
            "apps/web/e2e/new-flow.spec.ts",
            "apps/web/e2e-pending/workspace-wiki-vue-collab.spec.ts",
            "apps/web/e2e/helpers.ts",
            "apps/web/e2e/mfa-helpers.ts",
            "apps/web/e2e/workspace-wiki-vue-editor.ts",
            "apps/web/e2e-pending/collab-helpers.ts",
            "apps/web/e2e-pending/collab-helpers.test.ts",
            "apps/web/src/vue/router.test.ts",
            "packages/editor/test/vue-menu-selection.test.ts",
        ):
            self.assert_selected_workflows([path], {"web"})
            self.assert_selected_workflows(["docs/rewrite.md", path], {"web"})

    def test_editor_ui_keeps_browser_and_install(self) -> None:
        for path in (
            "packages/editor/src/vue/FvociEditor.vue",
            "packages/editor/src/react/block-menu.tsx",
            "packages/editor/src/react/editor.css",
            "packages/editor/src/clipboard.ts",
            "packages/editor/src/gutter-actions.ts",
            "packages/editor/src/menu-roving.ts",
        ):
            self.assert_selected_workflows([path], {"web", "install"})
            self.assert_selected_workflows(["README.md", "apps/web/e2e/foo.spec.ts", path], {"web", "install"})

    def test_new_explanatory_docs_are_exact(self) -> None:
        self.assert_selected_workflows(["docs/RELEASING.md", "docs/collab-engine-comparison.md"], set())
        for path in ("docs/fixtures/example.md", "docs/generated/api.md", "docs/other.md", "docs/collab-engine-comparison.md.bak"):
            self.assertEqual(SEL.decide_from_paths([path]).mode, "full", path)

    def test_backend_contracts_harness_and_unknown_stay_full(self) -> None:
        for path in (
            "packages/editor/src/tiptap-schema.ts",
            "packages/editor/src/collab-tiptap.ts",
            "packages/editor/src/json.ts",
            "packages/editor/src/export/pdf.tsx",
            "packages/editor/src/fonts/NotoSansKR.ttf",
            "packages/editor/src/react/schema.tsx",
            "packages/editor/src/vue/new.wasm",
            "packages/editor/test/schema-dump.ts",
            "packages/editor/test/setup/vue-sfc.ts",
            "packages/editor/tsconfig.json",
            "packages/i18n/src/locales/ko.json",
            "apps/web/e2e/fixtures/markdown-import.zip",
            "apps/web/e2e/nested/foo.spec.ts",
            "apps/web/e2e/new-harness.ts",
            "apps/web/e2e-pending/collab-restart.ts",
            "apps/web/e2e-pending/collab-wire.ts",
            "apps/web/e2e-pending/collab-attachment-oracle.ts",
            "apps/web/e2e-pending/collab-playwright.config.ts",
            "apps/web/src/generated/api.test.ts",
            "apps/web/src/fixtures/backend.sql",
            "apps/web/src/new-contract.json",
            "scripts/run-web-e2e.sh",
            "scripts/ci_selection.py",
            "src/auth.rs",
            "migrations/045.sql",
            "new-unknown-file.ts",
            "apps/web/src/../../src/main.rs",
            "apps/web//src/test.ts",
        ):
            for workflow, plan in plan_all_workflows(["README.md", "apps/web/e2e/foo.spec.ts", path]).items():
                self.assertEqual(plan["mode"], "full", path)
                assert_full_selection(self, workflow, plan, path)

    def test_candidate_golden_plans(self) -> None:
        # Frozen GitHub API inventories are regression evidence, never planner
        # input. Production derives its paths exclusively from verified Git.
        snapshot = json.loads((ROOT / "scripts/fixtures/ci-selection/candidates.json").read_text())
        self.assertEqual({item["number"] for item in snapshot["candidates"]}, {265, 267, 269, 270, 271, 263, 280})
        for item in snapshot["candidates"]:
            self.assertTrue(SEL.validate_sha(item["head_sha"]))
            paths = item["paths"]
            expected = set() if item["number"] in {263, 280} else {"web", "install"}
            with self.subTest(pr=item["number"], head=item["head_sha"]):
                self.assert_selected_workflows(paths, expected)


class PrMergeImpactTest(unittest.TestCase):
    def resolve(self, fx: PrCheckoutFixture, base: str, head: str, tested: str):
        with mock.patch.dict(os.environ, {"GITHUB_SHA": tested}):
            return SEL.resolve_selection_inputs(fx.work, {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}}, "pull_request")

    def test_exact_parents_merge_only_add_delete_rename_force_full(self) -> None:
        for operation in ("add", "delete", "rename"):
            with self.subTest(operation=operation), PrCheckoutFixture() as fx:
                write_file(fx.origin, "src/keep.rs", "base backend\n")
                git(fx.origin, "add", ".")
                git(fx.origin, "commit", "-m", "backend base")
                base = git_sha(fx.origin)
                head = fx.commit_on_branch("docs", "README.md", "changed docs\n")
                fx.clone_work()
                fx.merge_checkout(base, head)
                if operation == "add":
                    write_file(fx.work, "src/injected.rs", "merge only\n")
                elif operation == "delete":
                    git(fx.work, "rm", "src/keep.rs")
                else:
                    (fx.work / "apps/web/src").mkdir(parents=True, exist_ok=True)
                    git(fx.work, "mv", "src/keep.rs", "apps/web/src/disguised.ts")
                git(fx.work, "add", ".")
                git(fx.work, "commit", "--amend", "--no-edit")
                tested = git_sha(fx.work)
                inputs = self.resolve(fx, base, head, tested)
                self.assertIsNone(inputs.fatal_error)
                self.assertIsNone(inputs.force_full_reason)
                self.assertIn("README.md", inputs.paths)
                self.assertEqual(SEL.decide_from_paths(inputs.paths).mode, "full")

    def test_advanced_base_merge_resolution_only_change_is_classified(self) -> None:
        with PrCheckoutFixture() as fx:
            head = fx.commit_on_branch("docs", "README.md", "changed docs\n")
            write_file(fx.origin, "src/advanced.rs", "base backend\n")
            git(fx.origin, "add", ".")
            git(fx.origin, "commit", "-m", "advanced base")
            advanced = git_sha(fx.origin)
            fx.clone_work()
            fx.merge_checkout(advanced, head)
            write_file(fx.work, "src/advanced.rs", "merge resolution\n")
            git(fx.work, "add", ".")
            git(fx.work, "commit", "--amend", "--no-edit")
            inputs = self.resolve(fx, fx.base_sha, head, git_sha(fx.work))
            self.assertIsNone(inputs.force_full_reason)
            self.assertIn("src/advanced.rs", inputs.paths)
            self.assertEqual(SEL.decide_from_paths(inputs.paths).mode, "full")

    def test_cumulative_head_changes_survive_merge_tree_omission(self) -> None:
        with PrCheckoutFixture() as fx:
            head = fx.commit_on_branch("backend", "src/new.rs", "backend change\n")
            fx.clone_work()
            fx.merge_checkout(fx.base_sha, head)
            git(fx.work, "rm", "src/new.rs")
            write_file(fx.work, "README.md", "merge omitted backend\n")
            git(fx.work, "add", ".")
            git(fx.work, "commit", "--amend", "--no-edit")
            inputs = self.resolve(fx, fx.base_sha, head, git_sha(fx.work))
            self.assertIn("src/new.rs", inputs.paths)
            self.assertEqual(SEL.decide_from_paths(inputs.paths).mode, "full")

    def test_unrelated_or_older_first_parent_cannot_narrow(self) -> None:
        for kind in ("unrelated", "older"):
            with self.subTest(kind=kind), PrCheckoutFixture() as fx:
                head = fx.commit_on_branch("docs", "README.md", "changed docs\n")
                write_file(fx.origin, "src/advanced.rs", "advance\n")
                git(fx.origin, "add", ".")
                git(fx.origin, "commit", "-m", "advance")
                event_base = git_sha(fx.origin)
                fx.clone_work()
                if kind == "older":
                    tested = fx.merge_checkout(fx.base_sha, head)
                else:
                    git(fx.work, "checkout", "--orphan", "unrelated")
                    git(fx.work, "rm", "-rf", ".")
                    write_file(fx.work, "unrelated", "x\n")
                    git(fx.work, "add", ".")
                    git(fx.work, "commit", "-m", "unrelated root")
                    tree = git_sha(fx.work, f"{head}^{{tree}}")
                    unrelated = git_sha(fx.work)
                    tested = git(fx.work, "commit-tree", tree, "-p", unrelated, "-p", head, "-m", "spoofed merge").stdout.strip()
                    git(fx.work, "checkout", "--detach", tested)
                inputs = self.resolve(fx, event_base, head, tested)
                self.assertEqual(inputs.force_full_reason, "FULL_PR_MERGE_PARENTS_MISMATCH")
                self.assertIsNone(inputs.paths)

    def test_missing_history_and_tested_sha_mismatch_fail_closed(self) -> None:
        with PrCheckoutFixture() as fx:
            head = fx.commit_on_branch("docs", "README.md", "changed docs\n")
            fx.clone_work()
            tested = fx.merge_checkout(fx.base_sha, head)
            inputs = self.resolve(fx, fx.base_sha, head, head)
            self.assertEqual(inputs.fatal_error, "TESTED_SHA_MISMATCH")
            with mock.patch.object(SEL, "git_fetch_origin", return_value="FETCH_FAILED"):
                inputs = self.resolve(fx, "f" * 40, head, tested)
            self.assertEqual(inputs.fatal_error, "FETCH_FAILED")
            self.assertIsNone(inputs.paths)

    def test_actual_merge_diff_failure_is_fatal(self) -> None:
        with PrCheckoutFixture() as fx:
            head = fx.commit_on_branch("docs", "README.md", "changed docs\n")
            fx.clone_work()
            tested = fx.merge_checkout(fx.base_sha, head)
            with mock.patch.object(SEL, "git_diff_paths", side_effect=[(["README.md"], None), ([], "GIT_DIFF_FAILED")]):
                inputs = self.resolve(fx, fx.base_sha, head, tested)
            self.assertEqual(inputs.fatal_error, "GIT_DIFF_FAILED")
            self.assertIsNone(inputs.paths)


class AgentDocsSelectionTest(unittest.TestCase):
    """AGENTS.md and .agents/environment.md are role/environment records only."""

    def assert_docs_only(self, paths: list[str]) -> None:
        for workflow, plan in plan_all_workflows(paths).items():
            self.assertEqual(plan["mode"], "narrow", (workflow, paths))
            self.assertEqual(plan["reason_code"], "NARROW_DOCS", (workflow, paths))
            self.assertTrue(plan["plan_ok"])
            for job, meta in plan["jobs"].items():
                self.assertFalse(meta["selected"], (workflow, job, paths))

    def assert_full(self, paths: list[str], reason: str | None = None) -> None:
        for workflow, plan in plan_all_workflows(paths).items():
            self.assertEqual(plan["mode"], "full", (workflow, paths))
            if reason is not None:
                self.assertEqual(plan["reason_code"], reason, (workflow, paths))
            assert_full_selection(self, workflow, plan, paths)

    def test_exact_agent_docs_classify_as_docs(self) -> None:
        for path in AGENT_DOCS:
            self.assertEqual(SEL.classify_path(path), "docs", path)

    def test_other_agents_paths_stay_broaden_or_unknown(self) -> None:
        for path in (
            ".agents/skills/fvoci-fast-verify/SKILL.md",
            ".agents/skills/fvoci-standard-implementations/references/candidates.md",
            ".agents/environment.md.bak",
            ".agents/environment.mdx",
            ".agents/other.md",
            ".agents/sub/environment.md",
        ):
            self.assertEqual(SEL.classify_path(path), "broaden", path)
        for path in ("agents/environment.md", "AGENTS.MD", "apps/AGENTS.md", "AGENTS.md.orig"):
            self.assertEqual(SEL.classify_path(path), "unknown", path)
        self.assertEqual(SEL.classify_path("docs/AGENTS.md"), "broaden")
        self.assertEqual(SEL.classify_path(".agents/"), "unknown")
        self.assertEqual(SEL.classify_path("scripts/AGENTS.md"), "broaden")

    def test_explicit_docs_never_overlap_build_inputs(self) -> None:
        for path in SEL._EXPLICIT_DOCS:
            self.assertNotIn(path, SEL._BROADEN_EXACT)
            for marker in SEL._MANIFEST_MARKERS:
                self.assertNotIn(marker, path)
            self.assertFalse(path.endswith("/"), path)
            self.assertNotIn("*", path)

    def test_pr135_cumulative_paths_docs_only(self) -> None:
        self.assert_docs_only(["AGENTS.md"])
        self.assert_docs_only([".agents/environment.md"])
        self.assert_docs_only(["AGENTS.md", ".agents/environment.md"])
        self.assert_docs_only([".agents/environment.md", "AGENTS.md", "docs/rewrite.md", "README.md"])

    def test_agent_docs_with_code_or_selector_is_full(self) -> None:
        for extra in (
            "src/lib.rs",
            "tests/db_integration.rs",
            "migrations/0001_init.sql",
            ".github/workflows/rust.yml",
            ".github/workflows/web.yml",
            "scripts/ci_selection.py",
            "scripts/test_ci_selection.py",
            "scripts/test-ci-selection.sh",
            "scripts/ci_selection_requirements.txt",
            ".agents/skills/fvoci-fast-verify/SKILL.md",
        ):
            self.assert_full([*AGENT_DOCS, extra], "FULL_PATH_BROADEN")

    def test_agent_docs_with_executable_config_is_full(self) -> None:
        for extra in (
            "Cargo.toml",
            "Cargo.lock",
            "rust-toolchain.toml",
            "Dockerfile",
            ".dockerignore",
            "infra/rust/Dockerfile",
            "apps/web/package.json",
            "apps/web/playwright.config.ts",
            "crates/collab-engine/Cargo.toml",
            "packages/editor/package.json",
            "package.json",
            "bun.lock",
            "eslint.config.mjs",
            ".prettierrc.json",
            ".prettierignore",
            "bunfig.toml",
            ".bun-version",
            "patches/@volar%2Ftypescript@2.4.28.patch",
            "scripts/document-convert/package.json",
        ):
            self.assert_full([*AGENT_DOCS, extra], "FULL_PATH_BROADEN")

    def test_agent_docs_with_fixture_or_unknown_is_full(self) -> None:
        self.assert_full([*AGENT_DOCS, "compat/fixtures/x.json"], "FULL_PATH_BROADEN")
        self.assert_full([*AGENT_DOCS, "scripts/fixtures/web-e2e/x.sh"], "FULL_PATH_BROADEN")
        self.assert_full([*AGENT_DOCS, "docs/other.md"], "FULL_PATH_BROADEN")
        for extra in (".gitignore", "LICENSE", "third-party/x.md", "apps/AGENTS.md", "notes.md"):
            self.assert_full([*AGENT_DOCS, extra], "FULL_UNKNOWN_PATH")

    def test_agent_docs_with_frontend_unions_impacts(self) -> None:
        for workflow, plan in plan_all_workflows([*AGENT_DOCS, "apps/web/src/x.ts"]).items():
            self.assertEqual(plan["mode"], "narrow")
            self.assertEqual(plan["reason_code"], "NARROW_FRONTEND_WEB_INSTALL")
            for job, meta in plan["jobs"].items():
                self.assertIs(meta["selected"], workflow in {"web", "install"} and not is_opt_in(workflow, job))

    def test_always_full_events_ignore_agent_docs(self) -> None:
        for event_name, reason in (
            ("push", "FULL_EVENT_PUSH"),
            ("merge_group", "FULL_EVENT_MERGE_GROUP"),
            ("workflow_dispatch", "FULL_EVENT_WORKFLOW_DISPATCH"),
        ):
            for workflow, plan in plan_all_workflows(list(AGENT_DOCS), event_name).items():
                self.assertEqual(plan["mode"], "full", (event_name, workflow))
                self.assertEqual(plan["reason_code"], reason)
                self.assertTrue(plan["plan_ok"])
                assert_full_selection(self, workflow, plan, event_name)

    def test_diff_failure_fails_closed(self) -> None:
        for fatal in ("GIT_DIFF_FAILED", "DIFF_TRUNCATED", "DIFF_TRUNCATED_RENAME", "FETCH_FAILED"):
            for workflow, plan in plan_all_workflows(list(AGENT_DOCS), fatal_error=fatal).items():
                self.assertEqual(plan["mode"], "full", (fatal, workflow))
                self.assertEqual(plan["reason_code"], fatal)
                self.assertFalse(plan["plan_ok"])
                assert_full_selection(self, workflow, plan, fatal)
        for workflow, plan in plan_all_workflows(None).items():
            self.assertEqual(plan["reason_code"], "FULL_MISSING_PATHS", workflow)
            self.assertFalse(plan["plan_ok"])
        paths, err = SEL.parse_name_status_z(b"M\0AGENTS.md\0M\0.agents/environment.md")
        self.assertEqual((paths, err), ([], "DIFF_TRUNCATED"))
        paths, err = SEL.parse_name_status_z(b"R100\0AGENTS.md\0")
        self.assertEqual((paths, err), ([], "DIFF_TRUNCATED_RENAME"))

    def test_git_diff_command_failure_fails_closed(self) -> None:
        with GitRepoFixture() as fx:
            base = fx.commit_file("AGENTS.md", "a\n")
            paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, "f" * 40)
            self.assertIsNone(paths)
            self.assertIn(err, {"REV_PARSE_FAILED", "MERGE_BASE_FAILED"})
            head = fx.commit_file(".agents/environment.md", "b\n")
            real_git = SEL._git

            def failing_diff(repo: Path, *args: str, text: bool = True):
                if args and args[0] == "diff":
                    return subprocess.CompletedProcess(["git", *args], 128, b"", b"boom")
                return real_git(repo, *args, text=text)

            with mock.patch.object(SEL, "_git", failing_diff):
                paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, head)
            self.assertIsNone(paths)
            self.assertEqual(err, "GIT_DIFF_FAILED")

    def _diff(self, fx: GitRepoFixture, base: str) -> list[str]:
        paths, err, _ = SEL.diff_paths_for_pr(fx.repo, base, git_sha(fx.repo))
        self.assertIsNone(err)
        return paths or []

    def test_real_diff_modify_agent_docs_is_docs_only(self) -> None:
        with GitRepoFixture() as fx:
            fx.commit_file("AGENTS.md", "a\n")
            base = fx.commit_file(".agents/environment.md", "a\n")
            fx.commit_file("AGENTS.md", "b\n")
            fx.commit_file(".agents/environment.md", "b\n")
            paths = self._diff(fx, base)
            self.assertEqual(sorted(paths), [".agents/environment.md", "AGENTS.md"])
            self.assertEqual(SEL.decide_from_paths(paths).reason_code, "NARROW_DOCS")

    def test_real_diff_delete_agent_docs(self) -> None:
        with GitRepoFixture() as fx:
            fx.commit_file("AGENTS.md", "a\n")
            base = fx.commit_file(".agents/skills/x/SKILL.md", "a\n")
            fx.delete_file("AGENTS.md")
            paths = self._diff(fx, base)
            self.assertEqual(paths, ["AGENTS.md"])
            self.assertEqual(SEL.decide_from_paths(paths).reason_code, "NARROW_DOCS")
            fx.delete_file(".agents/skills/x/SKILL.md")
            paths = self._diff(fx, base)
            self.assertIn(".agents/skills/x/SKILL.md", paths)
            self.assertEqual(SEL.decide_from_paths(paths).mode, "full")

    def test_real_diff_rename_checks_old_and_new_names(self) -> None:
        body = "role record line\n" * 20
        cases = (
            ("AGENTS.md", "notes/AGENTS.md", "FULL_UNKNOWN_PATH"),
            (".agents/environment.md", ".agents/skills/environment.md", "FULL_PATH_BROADEN"),
            (".agents/skills/x/SKILL.md", ".agents/environment.md", "FULL_PATH_BROADEN"),
            ("src/env.md", "AGENTS.md", "FULL_PATH_BROADEN"),
            ("AGENTS.md", "Cargo.toml", "FULL_PATH_BROADEN"),
        )
        for old, new, reason in cases:
            with GitRepoFixture() as fx:
                base = fx.commit_file(old, body)
                (fx.repo / new).parent.mkdir(parents=True, exist_ok=True)
                fx.rename_file(old, new)
                paths = self._diff(fx, base)
                self.assertEqual(paths, [old, new], (old, new))
                decision = SEL.decide_from_paths(paths)
                self.assertEqual(decision.mode, "full", (old, new))
                self.assertEqual(decision.reason_code, reason, (old, new))

    def test_real_diff_rename_between_agent_docs_stays_docs(self) -> None:
        body = "role record line\n" * 20
        with GitRepoFixture() as fx:
            base = fx.commit_file(".agents/environment.md", body)
            fx.rename_file(".agents/environment.md", "AGENTS.md")
            paths = self._diff(fx, base)
            self.assertEqual(paths, [".agents/environment.md", "AGENTS.md"])
            self.assertEqual(SEL.decide_from_paths(paths).reason_code, "NARROW_DOCS")

    def test_pr_merge_checkout_agent_docs_narrow_docs(self) -> None:
        with PrCheckoutFixture() as fx:
            git(fx.origin, "checkout", "-B", "pr", "main")
            for rel in AGENT_DOCS:
                write_file(fx.origin, rel, "role record\n")
            git(fx.origin, "add", *AGENT_DOCS)
            git(fx.origin, "commit", "-m", "agent docs")
            head = git_sha(fx.origin)
            git(fx.origin, "checkout", "main")
            fx.clone_work()
            tested = fx.merge_checkout(fx.base_sha, head)
            event = fx.write_pr_event(fx.base_sha, head)
            output = fx.work / "plan.json"
            proc = fx.plan_cli(tested_sha=tested, event_path=event, output=output)
            self.assertEqual(proc.returncode, 0, proc.stderr)
            plan = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(plan["mode"], "narrow")
            self.assertEqual(plan["reason_code"], "NARROW_DOCS")
            self.assertEqual(plan["path_count"], 2)
            self.assertTrue(plan["plan_ok"])
            self.assertFalse(any(meta["selected"] for meta in plan["jobs"].values()))


class AgentDocsGateTest(unittest.TestCase):
    """Docs-only plan: unselected jobs must be skipped; selected jobs must succeed."""

    _plan = GateSchemaTest._plan
    _needs = GateSchemaTest._needs
    _gate = GateSchemaTest._gate

    def test_opt_in_gate_selected_bad_result_or_missing_rejected(self) -> None:
        plan = self._plan("install", {"upgrade-smoke-arm64": True})
        ok = {"upgrade-smoke-arm64": "success"}
        self.assertEqual(self._gate(plan, "install", ok), 0)
        for result in ("failure", "cancelled", "skipped"):
            self.assertEqual(self._gate(plan, "install", {"upgrade-smoke-arm64": result}), 1, result)
        self.assertEqual(
            self._gate(plan, "install", ok, omit_jobs=frozenset({"upgrade-smoke-arm64"})), 1
        )

    def test_opt_in_gate_unchosen_ran_rejected(self) -> None:
        plan = self._plan("install", {})
        for event_name, event in (
            ("pull_request", {}),
            ("push", {}),
            ("merge_group", {}),
            ("workflow_dispatch", {"inputs": {"run_upgrade_smoke_arm": "false"}}),
        ):
            self.assertEqual(self._gate(plan, "install", event_name=event_name, event=event), 0)
            for result in ("success", "failure", "cancelled"):
                rc = self._gate(
                    plan,
                    "install",
                    {"upgrade-smoke-arm64": result},
                    event_name=event_name,
                    event=event,
                )
                self.assertEqual(rc, 1, (event_name, result))

    def test_opt_in_gate_rejects_plan_override(self) -> None:
        # A plan that selects the job without the event opt-in, or drops a real
        # opt-in, fails even when the job result matches the plan.
        forced = self._plan("install", {"upgrade-smoke-arm64": True})
        for event_name, event in (
            ("pull_request", {"inputs": {"run_upgrade_smoke_arm": "true"}}),
            ("push", {}),
            ("merge_group", {}),
            ("workflow_dispatch", {"inputs": {"run_upgrade_smoke_arm": "false"}}),
            ("workflow_dispatch", {}),
        ):
            rc = self._gate(
                forced,
                "install",
                {"upgrade-smoke-arm64": "success"},
                event_name=event_name,
                event=event,
            )
            self.assertEqual(rc, 1, (event_name, event))
        dropped = self._plan("install", {})
        rc = self._gate(
            dropped,
            "install",
            event_name="workflow_dispatch",
            event={"inputs": {"run_upgrade_smoke_arm": "true"}},
        )
        self.assertEqual(rc, 1)

    def test_opt_in_gate_malformed_event_rejected(self) -> None:
        plan = self._plan("install", {})
        for event_name, event in (
            ("workflow_dispatch", "{bad"),
            ("workflow_dispatch", []),
            ("workflow_dispatch", {"inputs": []}),
            ("workflow_dispatch", {"inputs": {"run_upgrade_smoke_arm": "yes"}}),
            ("workflow_dispatch", {"inputs": {"run_upgrade_smoke_arm": 1}}),
            ("workflow_dispatch", {"inputs": {"other": "true"}}),
            ("schedule", {}),
        ):
            self.assertEqual(
                self._gate(plan, "install", event_name=event_name, event=event), 1, (event_name, event)
            )
        payload = self._needs(plan, "install")
        for missing in ("GITHUB_EVENT_NAME", "GITHUB_EVENT_PATH"):
            with mock.patch.dict(os.environ, {"GITHUB_EVENT_NAME": "push", "GITHUB_EVENT_PATH": "/nonexistent"}):
                os.environ.pop(missing)
                rc = SEL.cmd_gate(
                    ["--workflow", "install", "--needs-json", payload, "--tested-sha", "a" * 40]
                )
            self.assertEqual(rc, 1, missing)

    def test_docs_plan_all_skipped_passes(self) -> None:
        for workflow in SEL.WORKFLOW_JOBS:
            plan = self._plan(workflow, {})
            self.assertEqual(self._gate(plan, workflow), 0, workflow)

    def test_docs_plan_unselected_ran_rejected(self) -> None:
        for workflow, jobs in SEL.WORKFLOW_JOBS.items():
            for job in jobs:
                for result in ("success", "failure", "cancelled"):
                    plan = self._plan(workflow, {})
                    self.assertEqual(self._gate(plan, workflow, {job: result}), 1, (workflow, job, result))

    def test_full_plan_selected_bad_result_rejected(self) -> None:
        for workflow, jobs in SEL.WORKFLOW_JOBS.items():
            plan = self._plan(workflow, {job: True for job in jobs})
            plan["mode"] = "full"
            plan["reason_code"] = "FULL_PATH_BROADEN"
            ok = {job: "success" for job in jobs}
            self.assertEqual(self._gate(plan, workflow, ok), 0, workflow)
            for job in jobs:
                for result in ("failure", "cancelled", "skipped"):
                    self.assertEqual(
                        self._gate(plan, workflow, {**ok, job: result}), 1, (workflow, job, result)
                    )
                self.assertEqual(
                    self._gate(plan, workflow, ok, omit_jobs=frozenset({job})), 1, (workflow, job)
                )


if __name__ == "__main__":
    unittest.main()
