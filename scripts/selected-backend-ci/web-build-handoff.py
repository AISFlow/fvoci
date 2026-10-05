#!/usr/bin/env python3
"""Current Web CI build handoff only; no cache fallback or generic relocation.
Receipts retain their exact producer paths. A different runner path or physical
input requires a new build, not rewriting an admitted input map.
"""
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import stat
import tarfile

ROOT = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location("selected_ci", ROOT / "scripts/run-selected-backend-e2e.py")
CI = importlib.util.module_from_spec(spec)
spec.loader.exec_module(CI)
RECEIPTS = ("before.json", "after.json", "build-env-inputs.json", "build-environment.json",
            "compile-receipt.json", "bundle.json", "web-receipt.json", "abi-receipt.json")
STAGES = ("main", "lib", "install", "engine")


def digest(data):
    return hashlib.sha256(data).hexdigest()


def regular(path):
    path = Path(path)
    assert path.is_absolute() and path.resolve() == path, "nonphysical handoff path"
    assert stat.S_ISREG(path.lstat().st_mode), "nonregular handoff file"
    return path


def context(job):
    assert os.environ.get("CI") == os.environ.get("GITHUB_ACTIONS") == "true"
    assert os.environ.get("GITHUB_JOB") == job, "wrong handoff job"
    CI.identity()
    assert os.environ.get("FVOCI_WEB_BUILD_PHASE") == ("prepare" if job == "collaboration-build" else "consume")
    return {"repository": os.environ["GITHUB_REPOSITORY"],
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


def qualify(output):
    before = read(output, "before.json")
    assert before == read(output, "after.json") == CI.inputs(), "current physical inputs differ"
    assert read(output, "build-env-inputs.json") == CI.build_env(), "compiler environment differs"
    receipt = read(output, "compile-receipt.json")
    assert receipt["source"] == before["head"] and receipt["tree"] == before["tree"]
    assert receipt["exit_code"] == 0 and receipt["full_inputs_unchanged"] is True
    assert len(receipt["stages"]) == 4
    for name, stage in zip(STAGES, receipt["stages"]):
        assert stage == {**read(output, name + "-stage.json"),
                         "compilerMessages": CI.reference(output / (name + "-compiler.jsonl"))}
        assert stage["exit_code"] == 0
    bundle = read(output, "bundle.json")
    assert bundle["source"] == before["head"] and bundle["tree"] == before["tree"]
    assert bundle["full_inputs_unchanged"] is True
    assert len(bundle["binaries"]) == 6
    names = set()
    for path, record in bundle["binaries"].items():
        executable = regular(path)
        assert CI.sha(executable) == record["sha256"] and executable.stat().st_size == record["bytes"]
        name = record["target"]["name"]; names.add(name)
        assert record["compiledSource"] == before["head"]
        assert record["targetTriple"] == "x86_64-unknown-linux-gnu"
        assert sorted(record["features"]) == (["default", "worker"] if name == "collab-engine" else ["api-schema", "db-tests"])
        assert record["profile"]["test"] == (name in ("fvoci_server", "selected_install_lifetime"))
    assert names == {"fvoci-server", "fvoci-migrate", "fvoci-e2e-fixture", "fvoci_server", "selected_install_lifetime", "collab-engine"}
    env = read(output, "build-environment.json")
    assert env["rustc"] == CI.call(["rustc", "-Vv"]) and env["cargo"] == CI.call(["cargo", "-V"])
    assert env["bun"] == CI.call(["bun", "-v"]) == "1.4.2"
    assert env["os_release"] == Path("/etc/os-release").read_text()
    assert 'release: 1.98.1' in env["rustc"] and 'host: x86_64-unknown-linux-gnu' in env["rustc"]
    assert env["features"] == ["api-schema", "db-tests"] and env["nativeFeatures"] == ["worker"]
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
    assert web["dist_files"] and web["dist_files"] == {str(p.relative_to(dist)): CI.sha(p) for p in dist.rglob("*") if p.is_file()}
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
            {*RECEIPTS, *(n + suffix for n in STAGES for suffix in ("-stage.json", "-compiler.jsonl"))}) or any(
        path.is_relative_to(base) for base in (ROOT / "target/debug", ROOT / "crates/collab-engine/target/debug"))


def expected_files(bundle, output):
    files = [output / name for name in RECEIPTS]
    files += [output / (n + suffix) for n in STAGES for suffix in ("-stage.json", "-compiler.jsonl")]
    executables = {Path(p) for p in bundle["binaries"]}
    files += sorted(executables)
    core = {Path(p) for a in bundle["compiler_artifacts"]
            if a["target"]["name"] == "fvoci_server" and not a["profile"]["test"]
            and a["features"] == ["api-schema", "db-tests"]
            for p in a["filenames"] if p.endswith((".rlib", ".rmeta"))}
    assert core and any(p.suffix == ".rlib" for p in core), "missing emitted core library"
    files += sorted(core)
    assert len(files) == len(set(files))
    return files, executables, core


def export():
    current = context("collaboration-build"); output, packet = paths()
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
    manifest = {"schema": 1, **current, "producer_job": "collaboration-build", "consumer_job": "collaboration-flow",
                "root": str(ROOT), "output": str(output), "payload_sha256": CI.sha(archive), "entries": entries}
    CI.write(packet / "handoff.json", manifest)
    with open(os.environ["GITHUB_OUTPUT"], "a") as f:
        f.write("handoff_sha256=" + CI.sha(packet / "handoff.json") + "\n")


def admit():
    current = context("collaboration-flow"); output, packet = paths()
    expected = os.environ["FVOCI_WEB_BUILD_HANDOFF_SHA256"]
    assert re.fullmatch(r"[0-9a-f]{64}", expected)
    manifest_path = regular(packet / "handoff.json")
    assert CI.sha(manifest_path) == expected, "producer manifest digest differs"
    m = json.loads(manifest_path.read_bytes())
    assert all(m[k] == v for k, v in current.items()), "foreign/stale producer"
    assert m["schema"] == 1 and m["producer_job"] == "collaboration-build" and m["consumer_job"] == "collaboration-flow"
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
        expected_files_list, _, _ = expected_files(packet_bundle, output)
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


if __name__ == "__main__":
    import sys
    assert len(sys.argv) == 2 and sys.argv[1] in ("export", "admit", "consume")
    {"export": export, "admit": admit, "consume": consume}[sys.argv[1]]()
