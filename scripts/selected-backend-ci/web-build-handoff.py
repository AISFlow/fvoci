#!/usr/bin/env python3
"""Current Web CI build handoff only; no cache fallback or generic relocation.
Receipts retain their exact producer paths. A different runner path or physical
input requires a new build, not rewriting an admitted input map. Ordinary browser
shards receive the freshly emitted dist as part of this same packet: full input
equality and file hashes replace the collaboration consumer fresh-dist rebuild.
"""
import hashlib
import importlib.util
import io
import json
import os
import platform
from pathlib import Path
import re
import stat
import sys
import tarfile
import time

# Importing the CI helper must not change its qualified untracked input map.
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("selected_ci", ROOT / "scripts/run-selected-backend-e2e.py")
CI = importlib.util.module_from_spec(spec)
spec.loader.exec_module(CI)
RECEIPTS = ("before.json", "after.json", "build-env-inputs.json", "build-environment.json",
            "compile-receipt.json", "bundle.json", "web-receipt.json", "abi-receipt.json")
STAGES = ("main", "lib", "install", "engine")


BROWSER_STAGES = ("fixture", "default", "engine")


def browser():
    return os.environ.get("GITHUB_JOB") in ("workspace-browser-build", "workspace-browser-shard")


def stages():
    return BROWSER_STAGES if browser() else STAGES


def jobs():
    return ("workspace-browser-build", "workspace-browser-shard") if browser() else ("collaboration-build", "collaboration-flow")


def build_inputs():
    # Same complete physical source/native/dependency/toolchain closure as the
    # collaboration packet, plus build-time frontend environment (hashes only).
    value = CI.inputs()
    if browser():
        value = {**value, "frontend_env": {k: digest(v.encode()) for k, v in sorted(os.environ.items())
                 if k.startswith(("VITE_", "BUN_", "NODE_")) or k in ("CI", "API_PROXY_TARGET", "SOURCE_DATE_EPOCH", "TZ", "LANG", "LC_ALL")},
                 "frontend_env_files": {str(p.relative_to(ROOT)): CI.sha(regular(p))
                     for directory in (ROOT, ROOT / "apps/web") for p in sorted(directory.glob(".env*"))}}
    return value


def dist_files():
    dist = ROOT / "apps/web/dist"
    assert dist.is_dir() and dist.resolve() == dist, "missing/nonphysical fresh dist"
    entries = list(dist.rglob("*"))
    assert all(not p.is_symlink() for p in entries), "symlink dist asset"
    files = {str(p.relative_to(dist)): CI.sha(regular(p)) for p in entries if not p.is_dir()}
    assert files, "empty fresh dist"
    return files


def digest(data):
    return hashlib.sha256(data).hexdigest()


def regular(path):
    path = Path(path)
    assert path.is_absolute() and path.resolve() == path, "nonphysical handoff path"
    assert stat.S_ISREG(path.lstat().st_mode), "nonregular handoff file"
    return path


_COLLAB_CONSUMERS = frozenset({
    "collaboration-flow",
    "collaboration-install-on",
    "collaboration-postgres-on",
    "collaboration-sqlite-on",
    "collaboration-postgres-off",
    "collaboration-sqlite-off",
})


def context(job):
    assert os.environ.get("CI") == os.environ.get("GITHUB_ACTIONS") == "true"
    actual = os.environ.get("GITHUB_JOB")
    if job == "collaboration-flow" and actual in _COLLAB_CONSUMERS:
        job = actual
    assert actual == job, "wrong handoff job"
    if browser():
        assert CI.call(["git", "rev-parse", "HEAD"]) == os.environ["GITHUB_SHA"], "checkout differs from tested SHA"
        assert CI.subprocess.run(["git", "diff", "--quiet", "HEAD"], cwd=ROOT).returncode == 0
        assert re.fullmatch(r"[0-9]+", os.environ["GITHUB_RUN_ID"])
        assert re.fullmatch(r"[0-9]+", os.environ["GITHUB_RUN_ATTEMPT"])
    else:
        CI.identity()
    assert os.environ.get("FVOCI_WEB_BUILD_PHASE") == ("prepare" if job == jobs()[0] else "consume")
    runtime = {"system": platform.system(), "machine": platform.machine(),
               "os_release_sha256": digest(Path("/etc/os-release").read_bytes())} if browser() else None
    assert not browser() or runtime["system"] == "Linux" and runtime["machine"] == "x86_64", "unsupported browser platform"
    return {**({"platform": runtime} if browser() else {}), "repository": os.environ["GITHUB_REPOSITORY"],
            "run": os.environ["GITHUB_RUN_ID"], "attempt": os.environ["GITHUB_RUN_ATTEMPT"],
            "source": CI.call(["git", "rev-parse", "HEAD"]),
            "tree": CI.call(["git", "rev-parse", "HEAD^{tree}"])}


def paths():
    output = Path(os.environ["FVOCI_SELECTED_CI_OUTPUT"])
    assert output.is_absolute() and output.resolve() == output
    assert output.is_dir() and output.stat().st_uid == os.getuid()
    assert output.stat().st_mode & 0o777 == 0o700
    packet = Path(os.environ["FVOCI_WEB_BUILD_HANDOFF"])
    assert packet.is_absolute() and packet.resolve() == packet
    return output, packet


def read(output, name):
    return json.loads(regular(output / name).read_bytes())


def input_diagnostics(output, before, after, current):
    """Bounded hash-only evidence; never serialize paths, status or env values."""
    fields = ("head", "tree", "status", "tracked", "external", "untracked", *(["frontend_env", "frontend_env_files"] if browser() else []))
    limit = 512

    def fingerprint(value):
        return digest(json.dumps(value, sort_keys=True, separators=(",", ":")).encode())

    def snapshot(value):
        result = {"sha256": fingerprint(value), "fields": {}}
        for field in fields:
            item = value.get(field)
            record = {"sha256": fingerprint(item)}
            if isinstance(item, dict):
                entries = sorted((fingerprint(k), fingerprint(v)) for k, v in item.items())
                record.update(count=len(entries), entries=dict(entries[:limit]),
                              truncated=len(entries) > limit)
            result["fields"][field] = record
        return result

    def delta(left, right):
        result = {}
        for field in fields:
            a, b = left.get(field), right.get(field)
            if a == b:
                continue
            record = {"before_sha256": fingerprint(a), "after_sha256": fingerprint(b)}
            if isinstance(a, dict) and isinstance(b, dict):
                changes = sorted((fingerprint(k), fingerprint(a.get(k)), fingerprint(b.get(k)))
                                 for k in a.keys() | b.keys() if k not in a or k not in b or a[k] != b[k])
                record.update(count=len(changes), entries=changes[:limit],
                              truncated=len(changes) > limit)
            result[field] = record
        return result

    for name, value in (("before", before), ("after", after), ("current", current)):
        CI.write(output / f"handoff-input-{name}-safe.json", snapshot(value))
    CI.write(output / "handoff-input-delta-safe.json",
             {"schema": 1, "entry_limit": limit, "before_after": delta(before, after),
              "after_current": delta(after, current)})
    print("handoff input mismatch: see bounded handoff-input-*-safe.json hashes", file=sys.stderr)


def qualify_browser_binary(record):
    """Require the actual emitted ordinary debug executable metadata."""
    assert record["target"].get("kind") == ["bin"], "browser target must be bin"
    profile = record["profile"]
    assert profile.get("opt_level") == "0", "browser opt_level must be present and 0"
    assert "debuginfo" in profile and (profile["debuginfo"] is None or
           type(profile["debuginfo"]) is int and profile["debuginfo"] == 0), "browser debuginfo must be present and 0/null"
    assert profile.get("test") is False, "browser test must be present and false"


def qualify(output):
    before = read(output, "before.json")
    after, current = read(output, "after.json"), build_inputs()
    if not before == after == current:
        input_diagnostics(output, before, after, current)
    assert before == after == current, "current physical inputs differ"
    assert read(output, "build-env-inputs.json") == CI.build_env(), "compiler environment differs"
    receipt = read(output, "compile-receipt.json")
    assert receipt["source"] == before["head"] and receipt["tree"] == before["tree"]
    assert receipt["exit_code"] == 0 and receipt["full_inputs_unchanged"] is True
    assert len(receipt["stages"]) == len(stages())
    for name, stage in zip(stages(), receipt["stages"]):
        assert stage == {**read(output, name + "-stage.json"),
                         "compilerMessages": CI.reference(output / (name + "-compiler.jsonl"))}
        assert stage["exit_code"] == 0
    bundle = read(output, "bundle.json")
    assert bundle["source"] == before["head"] and bundle["tree"] == before["tree"]
    assert bundle["full_inputs_unchanged"] is True
    assert len(bundle["binaries"]) == (4 if browser() else 6)
    names = set()
    for path, record in bundle["binaries"].items():
        executable = regular(path)
        assert CI.sha(executable) == record["sha256"] and executable.stat().st_size == record["bytes"]
        name = record["target"]["name"]; names.add(name)
        assert record["compiledSource"] == before["head"]
        assert record["targetTriple"] == "x86_64-unknown-linux-gnu"
        assert sorted(record["features"]) == (["default", "worker"] if name == "collab-engine" else (["db-tests"] if browser() and name == "fvoci-e2e-fixture" else ([] if browser() else ["api-schema", "db-tests"])))
        if browser():
            qualify_browser_binary(record)
        else:
            assert record["profile"]["test"] == (name in ("fvoci_server", "selected_install_lifetime"))
    assert names == ({"fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"} if browser() else {"fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "fvoci_server", "selected_install_lifetime", "collab-engine"})
    env = read(output, "build-environment.json")
    assert env["rustc"] == CI.call(["rustc", "-Vv"]) and env["cargo"] == CI.call(["cargo", "-V"])
    assert env["bun"] == CI.call(["bun", "-v"]) == "1.4.2"
    assert env["os_release"] == Path("/etc/os-release").read_text()
    assert 'release: 1.98.1' in env["rustc"] and 'host: x86_64-unknown-linux-gnu' in env["rustc"]
    assert env["features"] == ([] if browser() else ["api-schema", "db-tests"]) and env["nativeFeatures"] == ["worker"]
    assert env["profile"] == "debug" and env["devDebug"] == env["testDebug"] == "0"
    assert env["target"] == str(Path(os.environ["CARGO_TARGET_DIR"]).resolve())
    sqlite_keys = {"SQLITE3_LIB_DIR", "SQLITE3_INCLUDE_DIR", "SQLITE3_STATIC", "SQLITE3_NO_PKG_CONFIG"}
    assert isinstance(env["sqlite"], dict) and set(env["sqlite"]) == sqlite_keys
    assert env["sqlite"] == {k: os.environ[k] for k in sqlite_keys}
    assert env["sqlite"]["SQLITE3_STATIC"] == env["sqlite"]["SQLITE3_NO_PKG_CONFIG"] == "1"
    web = read(output, "web-receipt.json")
    assert web["source"] == before["head"] and web["tree"] == before["tree"]
    assert web["exit_code"] == 0 and web["full_inputs_unchanged"] is True
    dist = ROOT / "apps/web/dist"
    assert web["servedDist"] == str(dist)
    assert web["dist_files"] and web["dist_files"] == dist_files()
    abi = read(output, "abi-receipt.json")
    assert abi["currentSource"] == before["head"] and abi["currentELFDependenciesVerified"] is True
    assert abi["host_runtime_files"] == {p: CI.sha(p) for p in CI.abi_files()}
    assert set(abi["actualCurrentELFldd"]) == set(bundle["binaries"])
    for executable in bundle["binaries"]:
        # Retain the producer's raw ldd receipt, but compare resolved dependency
        # identities: ASLR mapping addresses are not library inputs.
        recorded = abi["actualCurrentELFldd"][executable]
        actual = CI.subprocess.run(["ldd", executable], capture_output=True, text=True)
        assert actual.returncode == 0, "current ELF ldd failed"
        assert "not found" not in recorded and "not found" not in actual.stdout + actual.stderr
        recorded_paths = set(CI.elf_dependencies(0, recorded, ""))
        actual_paths = set(CI.elf_dependencies(actual.returncode, actual.stdout, actual.stderr))
        assert recorded_paths and recorded_paths == actual_paths, "current ELF dependency set differs"
        assert actual_paths <= set(abi["host_runtime_files"]), "unqualified current ELF dependency"
        assert all(CI.sha(path) == abi["host_runtime_files"][path] for path in actual_paths), "current ELF library bytes differ"
    return bundle


def allowed_destination(path, output):
    path = Path(path)
    assert path.is_absolute() and path.resolve() == path, "nonphysical destination"
    return (path.parent == output and path.name in
            {*RECEIPTS, *(n + suffix for n in stages() for suffix in ("-stage.json", "-compiler.jsonl"))}) or any(
        path.is_relative_to(base) for base in (ROOT / "target/debug", ROOT / "crates/collab-engine/target/debug", *([ROOT / "apps/web/dist"] if browser() else [])))


def expected_files(bundle, output, web=None):
    files = [output / name for name in RECEIPTS]
    files += [output / (n + suffix) for n in stages() for suffix in ("-stage.json", "-compiler.jsonl")]
    executables = {Path(p) for p in bundle["binaries"]}
    files += sorted(executables)
    core = {Path(p) for a in bundle["compiler_artifacts"]
            if a["target"]["name"] == "fvoci_server" and not a["profile"]["test"]
            and a["features"] == ["api-schema", "db-tests"]
            for p in a["filenames"] if p.endswith((".rlib", ".rmeta"))}
    assert browser() or (core and any(p.suffix == ".rlib" for p in core)), "missing emitted core library"
    if browser():
        files += [ROOT / "apps/web/dist" / name for name in (web or read(output, "web-receipt.json"))["dist_files"]]
    assert not browser() or not core, "browser packet must not contain schema-feature core"
    files += sorted(core)
    assert len(files) == len(set(files))
    return files, executables, core


def export():
    current = context(jobs()[0]); output, packet = paths()
    bundle = qualify(output)
    assert not packet.exists(); packet.mkdir(mode=0o700)
    files, executables, core = expected_files(bundle, output)
    entries = {}
    archive = packet / "payload.tar"
    with tarfile.open(archive, "x") as tar:
        for i, path in enumerate(files):
            path = regular(path); assert allowed_destination(path, output)
            data = path.read_bytes(); member = f"f{i:03d}"
            mode = 0o555 if path in executables else (0o444 if path in core else 0o600)
            entries[member] = {"path": str(path), "sha256": digest(data), "bytes": len(data), "mode": mode,
                               "producer_inode": path.stat().st_ino, "producer_mode": stat.S_IMODE(path.stat().st_mode)}
            info = tarfile.TarInfo(member); info.size = len(data); info.mode = mode
            tar.addfile(info, io.BytesIO(data))
    manifest = {"schema": 1, **current, "producer_job": jobs()[0], "consumer_job": jobs()[1],
                "root": str(ROOT), "output": str(output), "payload_sha256": CI.sha(archive), "entries": entries}
    CI.write(packet / "handoff.json", manifest)
    with open(os.environ["GITHUB_OUTPUT"], "a") as f:
        f.write("handoff_sha256=" + CI.sha(packet / "handoff.json") + "\n")


def admit():
    current = context(jobs()[1]); output, packet = paths()
    expected = os.environ["FVOCI_WEB_BUILD_HANDOFF_SHA256"]
    assert re.fullmatch(r"[0-9a-f]{64}", expected)
    manifest_path = regular(packet / "handoff.json")
    assert CI.sha(manifest_path) == expected, "producer manifest digest differs"
    m = json.loads(manifest_path.read_bytes())
    assert all(m[k] == v for k, v in current.items()), "foreign/stale producer"
    assert m["schema"] == 1 and m["producer_job"] == jobs()[0] and m["consumer_job"] == jobs()[1]
    assert m["root"] == str(ROOT) and m["output"] == str(output), "no path relocation"
    assert CI.shutil.disk_usage(output).free >= 20_000_000_000, "consumer START disk floor"
    archive = regular(packet / "payload.tar")
    assert CI.sha(archive) == m["payload_sha256"]
    return current, output, packet, expected, m, archive


def consume():
    current, output, packet, expected, m, archive = admit()
    entries = m["entries"]; assert entries
    with tarfile.open(archive, "r:") as tar:
        members = tar.getmembers()
        assert len(members) == len(entries) and {p.name for p in members} == set(entries)
        destinations = [e["path"] for e in entries.values()]
        assert len(destinations) == len(set(destinations))
        assert set(str(output / p) for p in RECEIPTS) <= set(destinations)
        bundle_member = next(p for p in members if entries[p.name]["path"] == str(output / "bundle.json"))
        packet_bundle = json.loads(tar.extractfile(bundle_member).read())
        packet_web = None
        if browser():
            web_member = next(p for p in members if entries[p.name]["path"] == str(output / "web-receipt.json"))
            packet_web = json.loads(tar.extractfile(web_member).read())
        expected_files_list, _, _ = expected_files(packet_bundle, output, packet_web)
        assert set(destinations) == {str(p) for p in expected_files_list}, "extra/missing packet destination"
        for member in members:
            e = entries[member.name]; path = Path(e["path"])
            assert member.isfile() and not member.linkname and member.size == e["bytes"]
            assert e["mode"] in (0o555, 0o444, 0o600) and member.mode == e["mode"]
            assert allowed_destination(path, output) and not path.exists(), "foreign/existing destination"
            data = tar.extractfile(member).read(); assert digest(data) == e["sha256"]
        for member in members:
            e = entries[member.name]; path = Path(e["path"])
            path.parent.mkdir(parents=True, exist_ok=True)
            with path.open("xb") as f:
                f.write(tar.extractfile(member).read()); os.fchmod(f.fileno(), e["mode"])
    bundle = qualify(output)
    assert all(stat.S_IMODE(Path(p).stat().st_mode) == 0o555 for p in bundle["binaries"])
    CI.write(output / "handoff-consumed.json", {**current, "producer_manifest_sha256": expected,
             "full_current_physical_inputs_equal": True, "fresh_dist_equal": True,
             "received": {p: {"sha256": CI.sha(p), "inode": Path(p).stat().st_ino,
                                  "mode": stat.S_IMODE(Path(p).stat().st_mode)} for p in destinations}})



def browser_before():
    context("workspace-browser-build"); output, _ = paths()
    assert browser() and not (ROOT / "apps/web/dist").exists(), "producer must start without dist"
    release = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines() if "=" in line)
    assert release["ID"].strip('"') == "ubuntu" and release["VERSION_ID"].strip('"') == "26.04"
    assert os.environ.get("CARGO_BUILD_TARGET", "x86_64-unknown-linux-gnu") == "x86_64-unknown-linux-gnu"
    assert os.environ.get("FVOCI_E2E_PROFILE", "debug") == "debug", "browser packet is debug only"
    CI.write(output / "build-env-inputs.json", CI.build_env())
    CI.write(output / "before.json", build_inputs())
    CI.write(output / "build-environment.json", {
        "rustc": CI.call(["rustc", "-Vv"]), "cargo": CI.call(["cargo", "-V"]), "bun": CI.call(["bun", "-v"]),
        "os_release": Path("/etc/os-release").read_text(), "target": str(Path(os.environ["CARGO_TARGET_DIR"]).resolve()),
        "features": [], "nativeFeatures": ["worker"], "profile": "debug",
        "devDebug": os.environ.get("CARGO_PROFILE_DEV_DEBUG"), "testDebug": os.environ.get("CARGO_PROFILE_TEST_DEBUG"),
        "sqlite": {k: os.environ[k] for k in ("SQLITE3_LIB_DIR", "SQLITE3_INCLUDE_DIR", "SQLITE3_STATIC", "SQLITE3_NO_PKG_CONFIG")}})


def browser_stage(name):
    context("workspace-browser-build"); output, _ = paths()
    assert name in BROWSER_STAGES
    commands = {
        "fixture": ["cargo", "build", "--locked", "--offline", "--bin", "fvoci-e2e-fixture", "--features", "db-tests"],
        "default": ["cargo", "build", "--locked", "--offline", "--bin", "fvoci-server", "--bin", "fvoci-migrate"],
        "engine": ["cargo", "build", "--locked", "--offline", "--manifest-path", str(ROOT / "crates/collab-engine/Cargo.toml"), "--features", "worker", "--bin", "collab-engine"]}
    command = commands[name] + ["--message-format=json-render-diagnostics"]
    before = read(output, "before.json"); started = time.monotonic()
    with (output / (name + "-compiler.jsonl")).open("x") as out, (output / (name + "-stderr.log")).open("x") as err:
        result = CI.subprocess.run(command, cwd=ROOT, stdout=out, stderr=err)
    CI.write(output / (name + "-stage.json"), {"source": before["head"], "tree": before["tree"],
             "command": command, "exit_code": result.returncode, "seconds": time.monotonic() - started})
    return result.returncode


def browser_after():
    context("workspace-browser-build"); output, _ = paths()
    before = read(output, "before.json"); after = build_inputs()
    CI.write(output / "after.json", after)
    assert before == after, "physical inputs changed during build"
    assert read(output, "build-env-inputs.json") == CI.build_env(), "compiler environment changed"
    records = []; artifacts = []
    for name in BROWSER_STAGES:
        record = read(output, name + "-stage.json"); assert record["exit_code"] == 0
        log = output / (name + "-compiler.jsonl")
        records.append({**record, "compilerMessages": CI.reference(log)})
        artifacts += [value for line in log.read_text().splitlines() if (value := json.loads(line)).get("reason") == "compiler-artifact"]
    bins = {}
    for name in ("fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "collab-engine"):
        matches = [a for a in artifacts if a["target"]["name"] == name and a["executable"]]
        assert len(matches) == 1, "missing/ambiguous emitted browser binary"
        a = matches[0]; qualify_browser_binary(a); path = regular(a["executable"])
        bins[str(path)] = {"sha256": CI.sha(path), "bytes": path.stat().st_size, "compiledSource": before["head"],
                          "targetTriple": "x86_64-unknown-linux-gnu", "target": a["target"], "features": a["features"], "profile": a["profile"]}
    CI.write(output / "bundle.json", {"source": before["head"], "tree": before["tree"], "full_inputs_unchanged": True, "binaries": bins, "compiler_artifacts": artifacts})
    CI.write(output / "compile-receipt.json", {"source": before["head"], "tree": before["tree"], "exit_code": 0, "full_inputs_unchanged": True, "stages": records})
    CI.write(output / "web-receipt.json", {"source": before["head"], "tree": before["tree"], "exit_code": 0, "full_inputs_unchanged": True,
             "dist_files": dist_files(), "servedDist": str(ROOT / "apps/web/dist"),
             "scope": "fresh producer dist; consumers require full identical physical inputs and exact asset hashes, without rebuilding"})
    libraries = {p: CI.sha(p) for p in CI.abi_files()}; ldd = {}
    for path in bins:
        result = CI.subprocess.run(["ldd", path], capture_output=True, text=True)
        dependencies = CI.elf_dependencies(result.returncode, result.stdout, result.stderr)
        assert dependencies and set(dependencies) <= set(libraries)
        ldd[path] = result.stdout
    CI.write(output / "abi-receipt.json", {"currentSource": before["head"], "currentELFDependenciesVerified": True,
             "host_runtime_files": libraries, "actualCurrentELFldd": ldd})


if __name__ == "__main__":
    assert len(sys.argv) in (2, 3)
    if sys.argv[1] == "browser-stage":
        assert len(sys.argv) == 3
        sys.exit(browser_stage(sys.argv[2]))
    assert len(sys.argv) == 2
    {"export": export, "admit": admit, "consume": consume,
     "browser-before": browser_before, "browser-after": browser_after}[sys.argv[1]]()
