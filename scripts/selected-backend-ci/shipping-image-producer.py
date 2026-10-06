#!/usr/bin/env python3
"""One pinned, producer-only shipping build. No application container is started.

Raw command output stays outside the explicit public artifact directory. The
image and closed receipts are a producer handback, not runtime qualification.
"""

from __future__ import annotations

import argparse
import errno
import gzip
import hashlib
import json
import os
import platform
import re
import select
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tarfile
import time
from datetime import datetime, timezone
from contextlib import ExitStack
from pathlib import Path, PurePosixPath

PRODUCT_SHA = "551583237a4cc31828d69fb1d3160c2e0f9179d6"
PRODUCT_TREE = "5c053668264efb053b7d7c5d6fd2ddd0461ba92c"
# The admitted local amd64 builder's actual immutable RepoDigest, not its tag.
BUILDKIT_IMAGE = "moby/buildkit@sha256:cec9f139f45e93c5c69c60f8b07cfad9f43f4ef6b6a6cd917527fea5ff2e3dea"
DISK_FLOOR = 32 * 1024**3
MEMORY_FLOOR = 16 * 1024**3
BUILDER_MEMORY = 12 * 1024**3
LEGACY_PROFILE = "legacy-16g"
HOSTED_PROFILE = "hosted-exclusive-12g-plus-2g-experimental"
HOST_RESERVE = 2 * 1024**3
SHA = re.compile(r"[0-9a-f]{40}\Z")
DIGEST = re.compile(r"[0-9a-f]{64}\Z")
PUBLIC_FILES = frozenset({"shipping-image.tar", "producer-receipt.json", "tracked-inputs.json"})
CANARY = b"FVOCI_ARTIFACT_CANARY_DO_NOT_PUBLISH"
BINARIES = frozenset({"fvoci-server", "fvoci-migrate", "collab-engine", "document-extract"})
EXPECTED_ENV = frozenset({
    "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
    "FVOCI_EXPORT_FONT_DIR=/opt/fvoci/share/fonts", "FVOCI_STATIC_DIR=/opt/fvoci/static",
    "FVOCI_COLLAB_ENGINE=/opt/fvoci/bin/collab-engine",
    "FVOCI_EXTRACTOR_BIN=/opt/fvoci/bin/document-extract",
    "FVOCI_STORAGE_DIR=/data/storage", "FVOCI_BIND=0.0.0.0:8080",
})
# Labels inherited from the unchanged pinned Ubuntu base, not user metadata.
EXPECTED_LABELS = {
    "org.opencontainers.image.created": "2026-09-27T10:25:52.887617+00:00",
    "org.opencontainers.image.description": "The Ubuntu container image maintained by Canonical\n\nUbuntu is a Debian-based Linux operating system that runs from the desktop to the cloud, to all your internet connected things.\nIt is the world's most popular operating system across public clouds and OpenStack clouds.\nIt is the number one platform for containers; from Docker to Kubernetes to LXD, Ubuntu can run your containers at scale.\nFast, secure and simple, Ubuntu powers millions of PCs worldwide.\n",
    "org.opencontainers.image.title": "ubuntu", "org.opencontainers.image.version": "26.04",
}


class Refusal(Exception):
    """Only closed codes, never command output or host exception strings."""


def require(ok: bool, code: str) -> None:
    if not ok:
        raise Refusal(code)


def utc() -> str:
    return datetime.now(timezone.utc).isoformat()


def encoded(value: object) -> bytes:
    return (json.dumps(value, ensure_ascii=True, sort_keys=True, separators=(",", ":")) + "\n").encode()


def digest(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def safe_path(name: str) -> bool:
    return bool(name) and "\\" not in name and not name.startswith("/") and all(
        part not in {"", ".", ".."} for part in name.split("/")
    ) and not any(ord(c) < 32 or ord(c) == 127 for c in name)


def git(root: Path, *args: str) -> bytes:
    p = subprocess.run(["git", "-C", str(root), *args], capture_output=True, check=False)
    require(p.returncode == 0, "SOURCE_GIT_FAILED")
    return p.stdout


def checkout(root: Path, sha: str, tree: str) -> dict:
    require(bool(SHA.fullmatch(sha)) and bool(SHA.fullmatch(tree)), "SOURCE_PIN_INVALID")
    require(root.is_dir() and not root.is_symlink(), "SOURCE_ROOT_INVALID")
    require(git(root, "rev-parse", "HEAD").strip().decode() == sha, "SOURCE_HEAD_DRIFT")
    require(git(root, "rev-parse", "HEAD^{tree}").strip().decode() == tree, "SOURCE_TREE_DRIFT")
    # Include ignored files: neither ignored native/dist files nor credentials
    # can enter the tracked-only physical context by accident.
    require(not git(root, "status", "--porcelain=v1", "--untracked-files=all", "--ignored"), "SOURCE_WORKTREE_DRIFT")
    rows = {}
    for entry in git(root, "ls-tree", "-rz", "--full-tree", sha).split(b"\0"):
        if not entry:
            continue
        meta, raw_name = entry.split(b"\t", 1)
        mode, kind, blob = meta.decode().split()
        name = raw_name.decode("utf-8")
        require(safe_path(name) and kind == "blob" and mode in {"100644", "100755"}, "SOURCE_TRACKED_TYPE_INVALID")
        file = root / name
        require(all(not x.is_symlink() for x in [file, *file.parents[:len(PurePosixPath(name).parts)-1]]), "SOURCE_SYMLINK")
        require(file.is_file() and stat.S_IMODE(file.stat().st_mode) == int(mode[-3:], 8), "SOURCE_MODE_DRIFT")
        data = file.read_bytes()
        actual_blob = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
        require(actual_blob == blob, "SOURCE_BYTES_DRIFT")
        rows[name] = {"blob": blob, "mode": mode, "bytes": len(data), "sha256": digest(data)}
    require(bool(rows), "SOURCE_EMPTY")
    return rows


def snapshot(root: Path, target: Path, rows: dict) -> None:
    target.mkdir(mode=0o700)
    for name, row in rows.items():
        file = target / name
        file.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        data = (root / name).read_bytes()
        require(digest(data) == row["sha256"], "SOURCE_SNAPSHOT_DRIFT")
        with file.open("xb") as f:
            f.write(data)
        file.chmod(int(row["mode"][-3:], 8))


def verify_snapshot(target: Path, rows: dict) -> None:
    files = {}
    for file in target.rglob("*"):
        require(not file.is_symlink(), "CONTEXT_SYMLINK")
        if file.is_dir():
            continue
        require(file.is_file(), "CONTEXT_TYPE_INVALID")
        files[file.relative_to(target).as_posix()] = file
    require(set(files) == set(rows), "CONTEXT_PATH_DRIFT")
    for name, file in files.items():
        require(stat.S_IMODE(file.stat().st_mode) == int(rows[name]["mode"][-3:], 8) and
                file.stat().st_size == rows[name]["bytes"] and digest(file.read_bytes()) == rows[name]["sha256"], "CONTEXT_BYTES_MODE_DRIFT")


def cgroup_memory(root: Path = Path("/sys/fs/cgroup")) -> int | None:
    limit = root / "memory.max"
    if not limit.is_file():
        return None
    raw = limit.read_text().strip()
    return None if raw == "max" else max(0, int(raw) - int((root / "memory.current").read_text()))


def resources(paths: list[Path]) -> dict:
    mem = next(int(line.split()[1]) * 1024 for line in Path("/proc/meminfo").read_text().splitlines() if line.startswith("MemAvailable:"))
    remaining = cgroup_memory()
    return {"free_bytes": min(shutil.disk_usage(p).free for p in paths), "host_mem_available": mem,
            "cgroup_memory_available": remaining, "effective_mem_available": min(mem, remaining) if remaining is not None else mem}


def admit(measured: dict, profile: str = LEGACY_PROFILE, *, running: bool = False) -> None:
    require(profile in {LEGACY_PROFILE, HOSTED_PROFILE}, "RESOURCE_PROFILE_INVALID")
    floor = MEMORY_FLOOR if profile == LEGACY_PROFILE else HOST_RESERVE + (0 if running else BUILDER_MEMORY)
    require(measured["free_bytes"] >= DISK_FLOOR and measured["effective_mem_available"] >= floor, "RESOURCE_NOTADMITTED")


def cgroup_open_directory(stack: ExitStack, path, *, parent: int | None = None) -> int:
    fd = os.open(path, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=parent)
    stack.callback(os.close, fd)
    return fd


def cgroup_open_absolute(stack: ExitStack, path: Path) -> int:
    require(path.is_absolute() and ".." not in path.parts, "CGROUP_PATH_INVALID")
    fd = cgroup_open_directory(stack, "/")
    for part in path.parts[1:]:
        fd = cgroup_open_directory(stack, part, parent=fd)
    return fd


def cgroup_text(fd: int, name: str) -> str:
    """Bounded, non-symlink lookup relative to a held kernel directory."""
    child = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=fd)
    try:
        info = os.fstat(child)
        require(stat.S_ISREG(info.st_mode) and info.st_dev == os.fstat(fd).st_dev, "CGROUP_PATH_INVALID")
        data = bytearray()
        while part := os.read(child, min(4096, 65537 - len(data))):
            data.extend(part)
            require(len(data) <= 65536, "CGROUP_METADATA_UNKNOWN")
        return data.decode("utf-8")
    finally:
        os.close(child)


def cgroup_filesystem(fd: int, expected: bytes) -> str:
    """Fixed maintained stat primitive; no shell, privilege or ABI structure."""
    tool = Path("/usr/bin/stat")
    for parent in tool.parents:
        info = parent.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid == 0 and not info.st_mode & 0o022, "CGROUP_METADATA_UNKNOWN")
    before = tool.lstat()
    require(stat.S_ISREG(before.st_mode) and before.st_uid == 0 and not before.st_mode & 0o022 and 0 < before.st_size <= 1024**2,
            "CGROUP_METADATA_UNKNOWN")
    tool_hash = digest(tool.read_bytes())
    child = subprocess.Popen([str(tool), "--file-system", "--format=%t", "--", f"/proc/self/fd/{fd}"],
        pass_fds=(fd,), stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        env={"LC_ALL": "C", "PATH": "/usr/bin:/bin"})
    try:
        deadline = time.monotonic() + 5; output = bytearray()
        while True:
            remaining = deadline - time.monotonic()
            require(remaining > 0 and bool(select.select([child.stdout], [], [], remaining)[0]), "CGROUP_METADATA_UNKNOWN")
            part = os.read(child.stdout.fileno(), 33 - len(output))
            if not part: break
            output.extend(part)
            require(len(output) <= 32, "CGROUP_METADATA_UNKNOWN")
        child.wait(timeout=max(0.001, deadline - time.monotonic()))
        require(child.returncode == 0 and bytes(output) == expected + b"\n", "CGROUP_HOST_ROOT_INVALID")
        after = tool.lstat()
        require((before.st_dev, before.st_ino, before.st_mode, before.st_uid) ==
                (after.st_dev, after.st_ino, after.st_mode, after.st_uid) and digest(tool.read_bytes()) == tool_hash,
                "CGROUP_METADATA_UNKNOWN")
        return tool_hash
    finally:
        if child.poll() is None: child.kill()
        child.wait(timeout=5)
        child.stdout.close()


def cgroup_mounts(text: str, root: Path, proc: Path, pid: str) -> list:
    """Only mountinfo's fixed kernel fields/escapes; refuse ambiguous masks."""
    rows = []; ids = set()
    escapes = {"040": " ", "011": "\t", "012": "\n", "134": "\\"}
    for line in text.splitlines():
        left, separator, right = line.partition(" - "); fields = left.split(); tail = right.split()
        require(separator and len(fields) >= 6 and len(tail) == 3 and fields[0].isdigit() and fields[1].isdigit() and
                bool(re.fullmatch(r"[0-9]+:[0-9]+", fields[2])) and fields[0] not in ids, "CGROUP_METADATA_UNKNOWN")
        ids.add(fields[0])
        decoded = []
        for value in fields[3:5]:
            require(not re.search(r"\\(?!040|011|012|134)", value), "CGROUP_METADATA_UNKNOWN")
            decoded.append(re.sub(r"\\(040|011|012|134)", lambda m: escapes[m[1]], value))
        rows.append([fields[0], fields[1], fields[2], *decoded, tail[0]])
    mounts = [r for r in rows if r[4] == str(root)]
    procs = [r for r in rows if r[4] == str(proc)]
    require(len(mounts) == len(procs) == 1 and mounts[0][3] == procs[0][3] == "/" and
            mounts[0][5] == "cgroup2" and procs[0][5] == "proc", "CGROUP_ANCESTORS_HIDDEN")
    targets = [proc / str(os.getpid()), proc / pid, proc / "self"]
    for row in rows:
        point = Path(row[4])
        require(point.is_absolute(), "CGROUP_METADATA_UNKNOWN")
        require(not (point != root and root in point.parents), "CGROUP_ANCESTORS_HIDDEN")
        require(not any(point == p or p in point.parents or (point != proc and proc in point.parents and point in p.parents)
                        for p in targets), "CGROUP_ANCESTORS_HIDDEN")
    return rows


def cgroup_pid(stack: ExitStack, proc_fd: int, pid: str) -> tuple[int, int, str]:
    fd = cgroup_open_directory(stack, pid, parent=proc_fd)
    raw = cgroup_text(fd, "stat"); first, separator, rest = raw.rpartition(") "); fields = rest.split()
    require(separator and first.split(" (", 1)[0] == pid and len(fields) >= 20 and fields[0] not in {"Z", "X", "x"},
            "CGROUP_METADATA_UNKNOWN")
    tick = int(fields[19]); require(tick > 0, "CGROUP_METADATA_UNKNOWN")
    lines = cgroup_text(fd, "cgroup").splitlines()
    require(len(lines) == 1 and lines[0].startswith("0::/"), "CGROUP_METADATA_UNKNOWN")
    relative = lines[0][4:]
    require((not relative or safe_path(relative)) and not relative.endswith(" (deleted)"), "CGROUP_PATH_INVALID")
    return fd, tick, relative


def cgroup_chain(pid: str = "self", root: Path = Path("/sys/fs/cgroup"), proc: Path = Path("/proc"), *,
                 relative: str | None = None, proof: dict | None = None) -> list[dict]:
    """Authenticate global root and live direct membership on every sample."""
    try:
        pid = str(os.getpid()) if pid == "self" else pid
        require(bool(re.fullmatch(r"[1-9][0-9]*", pid)), "CGROUP_METADATA_UNKNOWN")
        with ExitStack() as stack:
            root_fd = cgroup_open_absolute(stack, root); proc_fd = cgroup_open_absolute(stack, proc)
            root_tool = cgroup_filesystem(root_fd, b"63677270"); proc_tool = cgroup_filesystem(proc_fd, b"9fa0")
            self_fd = cgroup_open_directory(stack, str(os.getpid()), parent=proc_fd)
            mounts = cgroup_mounts(cgroup_text(self_fd, "mountinfo"), root, proc, pid)
            for path, fd in [(root, root_fd), (proc, proc_fd)]:
                record = next(r for r in mounts if r[4] == str(path)); major, minor = map(int, record[2].split(":"))
                require(os.fstat(fd).st_dev == os.makedev(major, minor), "CGROUP_HOST_ROOT_INVALID")
            for marker in ["cgroup.type", "memory.max"]:
                try:
                    cgroup_text(root_fd, marker)
                except OSError as e:
                    if e.errno != errno.ENOENT: raise
                else: raise Refusal("CGROUP_HOST_ROOT_INVALID")
            require({"cpu", "memory"} <= set(cgroup_text(root_fd, "cgroup.controllers").split()), "CGROUP_HOST_ROOT_INVALID")
            identity = lambda fd: (os.fstat(fd).st_dev, os.fstat(fd).st_ino)
            root_identity = digest(encoded([identity(root_fd), identity(proc_fd), mounts, root_tool, proc_tool]))
            require(proof is None or not proof or proof.get("root") == root_identity, "CGROUP_METADATA_UNKNOWN")
            if relative is None:
                pid_fd, tick, relative = cgroup_pid(stack, proc_fd, pid)
                live_identity = [pid, tick, relative]
                require(proof is None or "pid" not in proof or proof["pid"] == live_identity, "CGROUP_METADATA_UNKNOWN")
            else:
                require(proof is not None and proof.get("root") == root_identity, "CGROUP_METADATA_UNKNOWN")
                live_identity = None
            require(not relative or safe_path(relative), "CGROUP_PATH_INVALID")
            rows = []; directories = [("", root_fd)]; fd = root_fd
            for index, part in enumerate(PurePosixPath(relative).parts):
                fd = cgroup_open_directory(stack, part, parent=fd)
                name = "/".join(PurePosixPath(relative).parts[:index+1])
                require(cgroup_text(fd, "cgroup.type").strip() == "domain", "CGROUP_METADATA_UNKNOWN")
                directories.append((name, fd))
            def membership():
                values = cgroup_text(directories[-1][1], "cgroup.procs").splitlines()
                require(all(re.fullmatch(r"[1-9][0-9]*", value) for value in values) and pid in values,
                        "CGROUP_METADATA_UNKNOWN")
            if live_identity is not None: membership()
            for name, fd in directories[1:]:
                maximum = cgroup_text(fd, "memory.max").strip()
                maximum = None if maximum == "max" else int(maximum)
                current = int(cgroup_text(fd, "memory.current").strip())
                peak = int(cgroup_text(fd, "memory.peak").strip())
                pairs = [line.split() for line in cgroup_text(fd, "memory.events").splitlines()]
                require(all(len(p) == 2 for p in pairs) and len({p[0] for p in pairs}) == len(pairs), "CGROUP_METADATA_UNKNOWN")
                events = {k: int(v) for k, v in pairs}
                require({"oom", "oom_kill", "oom_group_kill"} <= set(events) and all(v >= 0 for v in events.values()) and
                        current >= 0 and peak >= current and (maximum is None or maximum > 0), "CGROUP_METADATA_UNKNOWN")
                cpu = cgroup_text(fd, "cpu.max").split()
                require(len(cpu) == 2 and (cpu[0] == "max" or int(cpu[0]) > 0) and int(cpu[1]) > 0, "CGROUP_METADATA_UNKNOWN")
                s = os.fstat(fd)
                container = re.fullmatch(r"(?:docker-)?([0-9a-f]{64})(?:\.scope)?", PurePosixPath(name).name)
                rows.append({"path": name, "identity": digest(encoded([name, s.st_dev, s.st_ino])), "memory_max": maximum,
                             "current": current, "peak": peak, "events": events, "cpu_max": cpu,
                             "container_id": container[1] if container else None})
            if live_identity is not None:
                after_fd, after_tick, after_relative = cgroup_pid(stack, proc_fd, pid)
                require(identity(after_fd) == identity(pid_fd) and [pid, after_tick, after_relative] == live_identity,
                        "CGROUP_METADATA_UNKNOWN")
                membership()
            require(cgroup_mounts(cgroup_text(self_fd, "mountinfo"), root, proc, pid) == mounts, "CGROUP_METADATA_UNKNOWN")
            reopened = cgroup_open_absolute(stack, root)
            require(identity(reopened) == identity(root_fd) and
                    identity(cgroup_open_absolute(stack, proc)) == identity(proc_fd), "CGROUP_METADATA_UNKNOWN")
            for part, (_, original) in zip(PurePosixPath(relative).parts, directories[1:]):
                reopened = cgroup_open_directory(stack, part, parent=reopened)
                require(identity(reopened) == identity(original), "CGROUP_METADATA_UNKNOWN")
            if proof is not None:
                proof["root"] = root_identity
                if live_identity is not None: proof["pid"] = live_identity
            return rows
    except (OSError, ValueError, IndexError, subprocess.TimeoutExpired) as e:
        raise Refusal("CGROUP_METADATA_UNKNOWN") from e


def image_config(config: dict) -> None:
    require(config.get("os") == "linux" and config.get("architecture") == "amd64", "IMAGE_OS_ARCH_INVALID")
    cfg = config.get("config", {})
    require(set(cfg) <= {"Env", "User", "WorkingDir", "Entrypoint", "Cmd", "Labels", "Volumes", "OnBuild", "ExposedPorts"} and
            cfg.get("ExposedPorts") == {"8080/tcp": {}}, "IMAGE_CONFIG_FIELDS_INVALID")
    require(set(cfg.get("Env", [])) == EXPECTED_ENV and len(cfg.get("Env", [])) == len(EXPECTED_ENV), "IMAGE_ENV_NOT_ALLOWLISTED")
    require(cfg.get("Labels") == EXPECTED_LABELS and not cfg.get("Volumes") and not cfg.get("OnBuild"), "IMAGE_CONFIG_NOT_ALLOWLISTED")
    require(cfg.get("User") == "fvoci" and cfg.get("WorkingDir") == "/opt/fvoci" and
            cfg.get("Entrypoint") == ["/opt/fvoci/bin/fvoci-migrate", "--start"] and not cfg.get("Cmd"), "IMAGE_ENTRYPOINT_INVALID")


def hash_stream(stream, require_stamp: bool = False, scan_canary: bool = False, *, guard=None) -> tuple[str, int, bytes]:
    h = hashlib.sha256(); size = 0; header = b""; tail = b""; stamp_seen = False
    canary_tail = b""
    while part := stream.read(1024 * 1024):
        if guard is not None: guard()
        if scan_canary:
            combined = canary_tail + part
            require(CANARY not in combined, "ARTIFACT_CANARY")
            canary_tail = combined[-(len(CANARY)-1):]
        if len(header) < 64:
            header += part[:64-len(header)]
        if require_stamp:
            stamp_seen |= PRODUCT_SHA.encode() in tail + part
            tail = part[-len(PRODUCT_SHA):]
        h.update(part); size += len(part)
    require(not require_stamp or stamp_seen, "IMAGE_SOURCE_STAMP_MISSING")
    if guard is not None: guard()
    return h.hexdigest(), size, header


def protected_whiteout(name: str) -> bool:
    path = PurePosixPath(name)
    if not path.name.startswith(".wh."):
        return False
    require(safe_path(name), "IMAGE_OUTPUT_PATH_INVALID")
    parent = "" if str(path.parent) == "." else str(path.parent)
    target = parent if path.name == ".wh..wh..opq" else "/".join(
        part for part in [parent, path.name.removeprefix(".wh.")] if part
    )
    protected = ("opt/fvoci/bin", "opt/fvoci/static", "etc/os-release", "usr/lib/os-release")
    return not target or any(target == p or p.startswith(target + "/") or target.startswith(p + "/") for p in protected)


def inspect_saved_image(path: Path, image_id: str, *, scan_canary: bool = False, guard=None) -> dict:
    """Read the Docker save archive without extracting any untrusted tar paths."""
    require(bool(re.fullmatch(r"sha256:[0-9a-f]{64}", image_id)), "IMAGE_ID_INVALID")
    if guard is not None: guard()
    binaries = {}; static_files = {}; os_release = None; oci_layers = None
    with tarfile.open(path, "r:") as outer:
        members = outer.getmembers()
        if guard is not None: guard()
        require(len({m.name for m in members}) == len(members) and all(safe_path(m.name.rstrip("/")) and
                (m.isfile() or m.isdir()) for m in members), "IMAGE_ARCHIVE_PATH_INVALID")
        manifest = json.load(outer.extractfile("manifest.json"))
        require(isinstance(manifest, list) and len(manifest) == 1, "IMAGE_MANIFEST_INVALID")
        item = manifest[0]
        require(set(item) == {"Config", "RepoTags", "Layers"} and isinstance(item["Layers"], list), "IMAGE_MANIFEST_INVALID")
        require(all(safe_path(x) for x in [item["Config"], *item["Layers"]]), "IMAGE_MANIFEST_PATH_INVALID")
        data = outer.extractfile(item["Config"]).read()
        config_id = "sha256:" + digest(data)
        # Docker29's containerd image ID is the OCI manifest digest. Legacy
        # Docker uses the config digest; bind each to its actual saved bytes.
        if config_id != image_id:
            leaf_path = "blobs/sha256/" + image_id.removeprefix("sha256:")
            require(leaf_path in outer.getnames(), "IMAGE_MANIFEST_ID_MISSING")
            leaf_bytes = outer.extractfile(leaf_path).read()
            require("sha256:" + digest(leaf_bytes) == image_id, "IMAGE_MANIFEST_ID_MISMATCH")
            leaf = json.loads(leaf_bytes)
            require(leaf.get("schemaVersion") == 2 and leaf.get("mediaType") in {
                "application/vnd.oci.image.manifest.v1+json", "application/vnd.docker.distribution.manifest.v2+json"} and
                leaf.get("config", {}).get("digest") == config_id and leaf["config"].get("size") == len(data), "IMAGE_CONFIG_DESCRIPTOR_MISMATCH")
            oci_layers = leaf.get("layers")
            require(isinstance(oci_layers, list) and len(oci_layers) == len(item["Layers"]), "IMAGE_LAYER_DESCRIPTOR_MISMATCH")
        config = json.loads(data); image_config(config)
        diff_ids = config.get("rootfs", {}).get("diff_ids", [])
        require(config.get("rootfs", {}).get("type") == "layers" and len(diff_ids) == len(item["Layers"]) and
                all(re.fullmatch(r"sha256:[0-9a-f]{64}", x) for x in diff_ids), "IMAGE_LAYER_IDENTITIES_INVALID")
        for index, (name, diff_id) in enumerate(zip(item["Layers"], diff_ids, strict=True)):
            if guard is not None: guard()
            if oci_layers is not None:
                with outer.extractfile(name) as source:
                    compressed_sha, compressed_bytes, _ = hash_stream(source, guard=guard)
                descriptor = oci_layers[index]
                require(descriptor.get("digest") == "sha256:" + compressed_sha and descriptor.get("size") == compressed_bytes,
                        "IMAGE_LAYER_DESCRIPTOR_MISMATCH")
            source = outer.extractfile(name)
            compressed = source.read(2) == b"\x1f\x8b"; source.close()
            with outer.extractfile(name) as source:
                stream = gzip.GzipFile(fileobj=source) if compressed else source
                # Public validation scans the entire decoded stream, including
                # unrelated members and bytes beyond the tar end marker.
                layer_sha, _, _ = hash_stream(stream, scan_canary=scan_canary, guard=guard)
                require("sha256:" + layer_sha == diff_id, "IMAGE_LAYER_ID_MISMATCH")
            with tarfile.open(fileobj=outer.extractfile(name), mode="r|*") as layer:
                for member in layer:
                    if guard is not None: guard()
                    n = member.name.removeprefix("./").rstrip("/")
                    require(not protected_whiteout(n), "IMAGE_OUTPUT_WHITEOUT")
                    relevant = n.startswith("opt/fvoci/bin/") or n.startswith("opt/fvoci/static/") or n in {"etc/os-release", "usr/lib/os-release"}
                    if not relevant or member.isdir():
                        continue
                    # Ubuntu's etc/os-release is a link to usr/lib/os-release;
                    # inspect the latter, never follow arbitrary archive links.
                    if n == "etc/os-release" and member.issym() and member.linkname == "../usr/lib/os-release":
                        continue
                    require(safe_path(n) and member.isfile() and ".wh." not in n, "IMAGE_OUTPUT_PATH_INVALID")
                    source = layer.extractfile(member)
                    if n in {"etc/os-release", "usr/lib/os-release"}:
                        text = source.read(16385)
                        require(len(text) <= 16384, "IMAGE_OS_RELEASE_INVALID")
                        fields = dict(line.split("=", 1) for line in text.decode().splitlines() if "=" in line)
                        require(fields.get("ID", "").strip('"') == "ubuntu" and fields.get("VERSION_ID", "").strip('"') == "26.04", "IMAGE_UBUNTU_INVALID")
                        os_release = digest(text)
                        continue
                    sha, size, header = hash_stream(source, require_stamp=n == "opt/fvoci/bin/fvoci-server", guard=guard)
                    row = {"sha256": sha, "bytes": size, "mode": f"{member.mode:04o}", "uid": member.uid, "gid": member.gid}
                    require(member.uid == member.gid == 0 and member.mode & 0o022 == 0, "IMAGE_OUTPUT_OWNERSHIP_INVALID")
                    if n.startswith("opt/fvoci/bin/"):
                        b = n.split("/")[-1]
                        require(b in BINARIES and len(header) == 64 and header[:6] == b"\x7fELF\x02\x01" and
                                struct.unpack_from("<H", header, 18)[0] == 62 and member.mode == 0o755, "IMAGE_ELF_INVALID")
                        binaries[b] = row
                    else:
                        static_files[n.removeprefix("opt/fvoci/static/")] = row
    require(set(binaries) == BINARIES and bool(static_files) and "index.html" in static_files and os_release is not None, "IMAGE_OUTPUT_MISSING")
    return {"image_id": image_id, "config_id": config_id, "manifest_id": image_id if oci_layers is not None else None,
            "binaries": binaries, "static_files": static_files, "os_release_sha256": os_release}


def validate_public(directory: Path, receipt: dict, inputs: dict, *, guard=None) -> None:
    if guard is not None: guard()
    require(directory.is_dir() and not directory.is_symlink(), "ARTIFACT_ROOT_INVALID")
    require({p.name for p in directory.iterdir()} == PUBLIC_FILES, "ARTIFACT_ALLOWLIST_INVALID")
    for file in directory.iterdir():
        require(file.is_file() and not file.is_symlink() and file.stat().st_nlink == 1 and stat.S_IMODE(file.stat().st_mode) == 0o600, "ARTIFACT_FILE_INVALID")
    # Expected values come from the closed producer, not a user-provided receipt.
    for name, value in [("producer-receipt.json", receipt), ("tracked-inputs.json", inputs)]:
        data = (directory / name).read_bytes()
        require(data == encoded(value) and CANARY not in data, "ARTIFACT_RECEIPT_INVALID")
    with (directory / "shipping-image.tar").open("rb") as stream:
        tail = b""
        while part := stream.read(1024 * 1024):
            if guard is not None: guard()
            require(CANARY not in tail + part, "ARTIFACT_CANARY")
            tail = part[-len(CANARY):]
    with (directory / "shipping-image.tar").open("rb") as stream:
        sha, size, _ = hash_stream(stream, guard=guard)
    require(receipt["saved_image"] == {"sha256": sha, "bytes": size}, "ARTIFACT_IMAGE_DRIFT")
    require(inspect_saved_image(directory / "shipping-image.tar", receipt["image"]["image_id"], scan_canary=True, guard=guard) == receipt["image"], "ARTIFACT_IMAGE_RECEIPT_DRIFT")
    if guard is not None: guard()


class Producer:
    def __init__(self, work: Path, builder: str, profile: str = LEGACY_PROFILE):
        require(profile in {LEGACY_PROFILE, HOSTED_PROFILE}, "RESOURCE_PROFILE_INVALID")
        self.work = work; self.builder = builder; self.cid = None; self.stages = []; self.paths = []
        self.builder_created = False; self.minimum = None; self.host = None
        self.builder_closing = False; self.builder_parent = None; self.builder_terminal = None
        self.profile = profile; self.builder_closed = False; self.cgroups = {}; self.chains = {}; self.phases = {}
        self.cgroup_proofs = {}
        self.memory_floor = MEMORY_FLOOR if profile == LEGACY_PROFILE else BUILDER_MEMORY + HOST_RESERVE

    def check_resources(self, stage: str, *, running: bool = False) -> dict:
        measured = resources(self.paths)
        if self.profile == HOSTED_PROFILE:
            chains = {"launcher": cgroup_chain(proof=self.cgroup_proofs.setdefault("launcher", {}))}
            live = None
            if self.builder_created and not self.builder_closed:
                if self.cid is None:
                    ids = self.command("builder-discovery-" + str(len(self.stages)), ["docker", "ps", "-aq", "--no-trunc",
                        "--filter", "name=^/buildx_buildkit_" + self.builder + "0$"]).decode().splitlines()
                    require(len(ids) <= 1 and all(re.fullmatch(r"[0-9a-f]{64}", cid) for cid in ids), "BUILDER_IDENTITY_DRIFT")
                    if ids: self.cid = ids[0]
                if self.cid is not None:
                    live = self.owned_builder()
                    require(type(live["State"].get("Running")) is bool and type(live["State"].get("OOMKilled")) is bool,
                            "BUILDER_METADATA_UNKNOWN")
                    require(not live["State"].get("OOMKilled"), "BUILDER_OOM")
                    if live["State"]["Running"]:
                        require(type(live["State"]["Pid"]) is int and live["State"]["Pid"] > 0, "BUILDER_PID_INVALID")
                        chains["builder"] = cgroup_chain(str(live["State"]["Pid"]), proof=self.cgroup_proofs.setdefault("builder", {}))
                        require(self.cgroup_proofs["builder"].get("root") == self.cgroup_proofs["launcher"].get("root"),
                                "CGROUP_IDENTITY_DRIFT")
                        after = self.owned_builder()
                        require(type(after["State"].get("OOMKilled")) is bool and type(after["State"].get("Pid")) is int,
                                "BUILDER_METADATA_UNKNOWN")
                        require(not after["State"].get("OOMKilled"), "BUILDER_OOM")
                        require(after["State"].get("Running") is True and after["State"].get("Pid") == live["State"]["Pid"],
                                "BUILDER_IDENTITY_DRIFT")
                        require(bool(chains["builder"]), "BUILDER_CGROUPS_INVALID")
                        leaf = chains["builder"][-1]
                        require(leaf["container_id"] == self.cid and leaf["memory_max"] == BUILDER_MEMORY and leaf["cpu_max"] == ["200000", "100000"] and
                                leaf["current"] <= BUILDER_MEMORY and leaf["peak"] <= BUILDER_MEMORY, "BUILDER_CGROUPS_INVALID")
                        self.builder_parent = chains["builder"][-2]["path"] if len(chains["builder"]) > 1 else ""
                    else:
                        require((self.builder_closing or stage in {"before-bootstrap", "builder-bootstrap"}) and
                                type(live["State"].get("Pid")) is int and live["State"]["Pid"] == 0, "BUILDER_NOT_RUNNING")
                        if self.builder_closing and self.builder_parent is not None:
                            # PID0 cannot supply final leaf counters. Re-read the
                            # recorded parent chain; never substitute stale usage.
                            chains["builder-ancestors"] = cgroup_chain(relative=self.builder_parent,
                                proof=self.cgroup_proofs.setdefault("builder", {}))
                            require(self.cgroup_proofs["builder"].get("root") == self.cgroup_proofs["launcher"].get("root"),
                                    "CGROUP_IDENTITY_DRIFT")
                            require([r["identity"] for r in chains["builder-ancestors"]] == self.chains["builder"][:-1],
                                    "CGROUP_IDENTITY_DRIFT")
                require(self.builder_closing or stage in {"before-bootstrap", "builder-bootstrap"} or "builder" in chains,
                        "BUILDER_METADATA_UNKNOWN")
                # Until the verified container is live, retain the full starting
                # commitment. Absence during bootstrap never proves a live cap.
                running = "builder" in chains or (self.builder_closing and "builder" in self.chains)
            available = measured["effective_mem_available"]
            for role, rows in chains.items():
                ids = [r["identity"] for r in rows]
                require(role not in self.chains or self.chains[role] == ids, "CGROUP_IDENTITY_DRIFT")
                self.chains[role] = ids
                for index, row in enumerate(rows):
                    key = row["identity"]; old = self.cgroups.get(key)
                    fixed = {k: row[k] for k in ["memory_max", "cpu_max"]}
                    if old is None:
                        if role == "builder" and index == len(rows)-1:
                            require(all(row["events"][k] == 0 for k in ["oom", "oom_kill", "oom_group_kill"]), "BUILDER_OOM")
                        old = {**fixed, "initial_events": row["events"].copy(), "last_events": row["events"].copy(), "peak": row["peak"]}
                        self.cgroups[key] = old
                    require(all(old[k] == v for k, v in fixed.items()) and row["peak"] >= old["peak"] and
                            set(row["events"]) == set(old["last_events"]) and
                            all(row["events"][k] >= v for k, v in old["last_events"].items()), "CGROUP_COUNTERS_DRIFT")
                    require(all(row["events"][k] == old["initial_events"][k] for k in ["oom", "oom_kill", "oom_group_kill"]), "CGROUP_OOM_INCREMENT")
                    old.update(last_events=row["events"].copy(), peak=row["peak"], current=row["current"])
                    if row["memory_max"] is not None and not (role == "builder" and index == len(rows)-1):
                        require(role not in {"builder", "builder-ancestors"} or row["memory_max"] >= BUILDER_MEMORY + HOST_RESERVE, "BUILDER_ANCESTOR_BUDGET_INVALID")
                        available = min(available, max(0, row["memory_max"] - row["current"]))
            measured["effective_mem_available"] = available
        if self.minimum is None:
            self.minimum = measured.copy()
        else:
            for key, value in measured.items():
                if value is not None:
                    old = self.minimum[key]; self.minimum[key] = min(old, value) if old is not None else value
        phase = self.phases.setdefault(stage, {"minimum_available": measured["effective_mem_available"], "minimum_disk": measured["free_bytes"], "samples": 0})
        phase["minimum_available"] = min(phase["minimum_available"], measured["effective_mem_available"])
        phase["minimum_disk"] = min(phase["minimum_disk"], measured["free_bytes"]); phase["samples"] += 1
        self.memory_floor = MEMORY_FLOOR if self.profile == LEGACY_PROFILE else HOST_RESERVE + (0 if running else BUILDER_MEMORY)
        phase["memory_floor"] = self.memory_floor
        admit(measured, self.profile, running=running)
        return measured

    def profile_receipt(self) -> dict:
        return json.loads(encoded({"name": self.profile, "experimental_sufficiency": "NOT_PROVEN" if self.profile == HOSTED_PROFILE else None,
            "start_memory_floor": MEMORY_FLOOR if self.profile == LEGACY_PROFILE else BUILDER_MEMORY + HOST_RESERVE,
            "running_memory_floor": MEMORY_FLOOR if self.profile == LEGACY_PROFILE else HOST_RESERVE,
            "builder_budget": BUILDER_MEMORY, "host_reserve": HOST_RESERVE if self.profile == HOSTED_PROFILE else None,
            "phases": self.phases, "chains": self.chains, "cgroups": self.cgroups,
            **({"builder_terminal": self.builder_terminal} if self.profile == HOSTED_PROFILE else {})}))

    def command(self, stage: str, argv: list[str], monitored: bool = False) -> bytes:
        start = utc()
        with (self.work / (stage + ".private.log")).open("xb") as output:
            p = subprocess.Popen(argv, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
            try:
                while p.poll() is None:
                    if monitored:
                        self.check_resources(stage, running=True)
                    try:
                        p.wait(timeout=1)
                    except subprocess.TimeoutExpired:
                        pass
                if monitored and p.returncode == 0:
                    self.check_resources(stage, running=True)
            except BaseException:
                if p.poll() is None:
                    try:
                        os.killpg(p.pid, signal.SIGTERM)
                    except ProcessLookupError:
                        pass
                try:
                    p.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    os.killpg(p.pid, signal.SIGKILL)
                    p.wait()
                raise
            finally:
                self.stages.append({"stage": stage, "start_utc": start, "end_utc": utc(), "pid": p.pid, "exit": p.returncode})
        require(p.returncode == 0, "PRODUCER_COMMAND_FAILED")
        return (self.work / (stage + ".private.log")).read_bytes() if not monitored else b""

    def owned_builder(self) -> dict:
        require(self.cid is not None, "BUILDER_ID_MISSING")
        rows = json.loads(self.command("builder-identity-" + str(len(self.stages)), ["docker", "inspect", self.cid]))
        require(len(rows) == 1 and rows[0]["Id"] == self.cid and rows[0]["Name"] == "/buildx_buildkit_" + self.builder + "0", "BUILDER_IDENTITY_DRIFT")
        r = rows[0]; h = r["HostConfig"]
        require(r["Image"] == BUILDKIT_IMAGE.split("@", 1)[1] and h["Memory"] == BUILDER_MEMORY and
                h["CpuPeriod"] == 100000 and h["CpuQuota"] == 200000 and not h.get("PortBindings"), "BUILDER_CAPS_INVALID")
        return r

    def stop_builder(self) -> None:
        if self.cid is None and self.builder_created:
            # Bootstrap can fail after creating the container. Pre-creation
            # absence plus the new builder lease permits only this exact name;
            # owned_builder still refuses every mismatched image/cap/identity.
            raw = self.command("failed-bootstrap-id", ["docker", "ps", "-aq", "--no-trunc", "--filter", "name=^/buildx_buildkit_" + self.builder + "0$"]).decode().splitlines()
            require(len(raw) <= 1, "BUILDER_IDENTITY_DRIFT")
            if raw:
                self.cid = raw[0]
        if self.cid is None:
            return
        r = self.owned_builder()
        experimental = self.profile == HOSTED_PROFILE
        self.builder_closing = experimental
        try:
            if experimental: self.check_resources("builder-closing", running=True)
            if r["State"]["Running"]:
                if experimental:
                    self.command("builder-stop-" + str(len(self.stages)), ["docker", "stop", self.cid], monitored=True)
                else:
                    self.command("builder-stop-" + str(len(self.stages)), ["docker", "stop", self.cid])
            r = self.owned_builder()
            if experimental:
                require(type(r["State"].get("Running")) is bool and type(r["State"].get("OOMKilled")) is bool,
                        "BUILDER_METADATA_UNKNOWN")
                require(not r["State"]["OOMKilled"], "BUILDER_OOM")
                require(type(r["State"].get("Pid")) is int, "BUILDER_PID_INVALID")
            require(not r["State"]["Running"] and r["State"]["Pid"] == 0 and not r["NetworkSettings"].get("Ports"), "BUILDER_CLOSURE_FAILED")
            if experimental:
                self.check_resources("builder-closing", running=True)
                self.builder_terminal = {"oom_killed": False, "cgroup_counters": "UNKNOWN"}
            self.builder_closed = True
        finally:
            self.builder_closing = False

    def build(self, product: Path, tooling: Path, tooling_sha: str) -> dict:
        require(platform.system() == "Linux" and platform.machine() == "x86_64", "HOST_OS_ARCH_INVALID")
        host = dict(line.split("=", 1) for line in Path("/etc/os-release").read_text().splitlines() if "=" in line)
        require(host.get("ID", "").strip('"') == "ubuntu" and host.get("VERSION_ID", "").strip('"') == "26.04", "HOST_UBUNTU_INVALID")
        tooling_tree = git(tooling, "rev-parse", "HEAD^{tree}").strip().decode()
        tools = checkout(tooling, tooling_sha, tooling_tree)
        require((tooling / "scripts/selected-backend-ci/shipping-image-producer.py").resolve() == Path(__file__).resolve(), "TOOLING_SCRIPT_MISMATCH")
        inputs = checkout(product, PRODUCT_SHA, PRODUCT_TREE)
        require(len(inputs) == 2134, "PRODUCT_TRACKED_COUNT_INVALID")
        docker_root = self.command("docker-root", ["docker", "info", "--format", "{{.DockerRootDir}}"] ).decode().strip()
        require(bool(re.fullmatch(r"/[A-Za-z0-9_./-]+", docker_root)) and ".." not in docker_root.split("/"), "DOCKER_ROOT_INVALID")
        self.paths = [self.work, product, Path(docker_root)]
        self.host = {"os": "ubuntu", "version": "26.04", "arch": "amd64", "docker_root": docker_root}
        before = self.check_resources("preflight")
        context = self.work / "context"; snapshot(product, context, inputs); verify_snapshot(context, inputs)
        (self.work / "buildkitd.toml").write_text("[worker.oci]\n  max-parallelism = 2\n")
        require(not self.command("builder-absence", ["docker", "ps", "-aq", "--no-trunc", "--filter", "name=^/buildx_buildkit_" + self.builder + "0$"]).strip(), "BUILDER_ALREADY_EXISTS")
        self.command("builder-create", ["docker", "buildx", "create", "--name", self.builder, "--driver", "docker-container",
            "--driver-opt", "image=" + BUILDKIT_IMAGE, "--driver-opt", "memory=12g", "--driver-opt", "cpu-period=100000",
            "--driver-opt", "cpu-quota=200000", "--buildkitd-config", str(self.work / "buildkitd.toml"), "--platform", "linux/amd64"])
        self.builder_created = True
        self.check_resources("before-bootstrap")
        self.command("builder-bootstrap", ["docker", "buildx", "inspect", self.builder, "--bootstrap"], monitored=True)
        cid = self.command("builder-id", ["docker", "inspect", "--format", "{{.Id}}", "buildx_buildkit_" + self.builder + "0"]).decode().strip()
        require(self.cid is None or self.cid == cid, "BUILDER_IDENTITY_DRIFT"); self.cid = cid
        require(bool(re.fullmatch(r"[0-9a-f]{64}", self.cid)), "BUILDER_ID_INVALID")
        self.owned_builder()
        actual_caps = self.command("builder-cgroups", ["docker", "exec", self.cid, "cat", "/sys/fs/cgroup/memory.max", "/sys/fs/cgroup/cpu.max"]).decode().splitlines()
        require(actual_caps == [str(BUILDER_MEMORY), "200000 100000"], "BUILDER_CGROUPS_INVALID")
        version = self.command("builder-version", ["docker", "exec", self.cid, "buildkitd", "--version"]).decode()
        require(bool(re.search(r"\bv0\.26\.[0-9]+\b", version)), "BUILDER_VERSION_INVALID")
        tag = "fvoci-shipping-producer:" + PRODUCT_SHA + "-" + self.builder
        self.command("shipping-build", ["docker", "buildx", "build", "--builder", self.builder, "--platform", "linux/amd64", "--load",
            "--provenance=false", "--build-arg", "FVOCI_BUILD_SHA=" + PRODUCT_SHA, "-f", str(context / "infra/rust/Dockerfile"), "-t", tag, str(context)], monitored=True)
        verify_snapshot(context, inputs)
        require(checkout(product, PRODUCT_SHA, PRODUCT_TREE) == inputs and checkout(tooling, tooling_sha, tooling_tree) == tools, "SOURCE_AFTER_DRIFT")
        if self.profile == HOSTED_PROFILE: self.check_resources("before-builder-stop", running=True)
        self.stop_builder()
        image_id = self.command("image-id", ["docker", "image", "inspect", "--format", "{{.Id}}", tag]).decode().strip()
        public = self.work / "public"; public.mkdir(mode=0o700)
        image = public / "shipping-image.tar"
        self.command("image-save", ["docker", "save", "--output", str(image), image_id], monitored=True)
        image.chmod(0o600)
        guard = (lambda: self.check_resources("python-validation", running=True)) if self.profile == HOSTED_PROFILE else None
        outputs = inspect_saved_image(image, image_id, guard=guard)
        with image.open("rb") as stream:
            saved_sha, saved_bytes, _ = hash_stream(stream, guard=guard)
        receipt = {"version": 1, "scope": "PRODUCER_ONLY_NOT_RUNTIME_QUALIFIED", "product_sha": PRODUCT_SHA, "product_tree": PRODUCT_TREE,
            "tooling_sha": tooling_sha, "tooling_tree": tooling_tree, "tracked_inputs_sha256": digest(encoded(inputs)),
            "host": self.host,
            "resource_before": before, "resource_after": resources(self.paths), "minimum_sampled_resources": self.minimum.copy(),
            "disk_floor": DISK_FLOOR, "memory_floor": self.memory_floor, "resource_profile": self.profile_receipt(),
            "builder": {"id": self.cid, "memory": BUILDER_MEMORY, "cpu_quota": 200000, "cpu_period": 100000, "cgroups": actual_caps,
                "image": BUILDKIT_IMAGE, "stopped": True}, "stages": self.stages, "image": outputs,
            "saved_image": {"sha256": saved_sha, "bytes": saved_bytes}}
        for name, value in [("producer-receipt.json", receipt), ("tracked-inputs.json", inputs)]:
            (public / name).write_bytes(encoded(value)); (public / name).chmod(0o600)
        validate_public(public, receipt, inputs, guard=guard)
        if guard is not None:
            after = self.check_resources("complete", running=True)
            require(checkout(product, PRODUCT_SHA, PRODUCT_TREE) == inputs and checkout(tooling, tooling_sha, tooling_tree) == tools, "SOURCE_AFTER_DRIFT")
            receipt.update(resource_after=after, minimum_sampled_resources=self.minimum.copy(), resource_profile=self.profile_receipt())
            file = public / "producer-receipt.json"; file.write_bytes(encoded(receipt))
            require(file.read_bytes() == encoded(receipt) and not file.is_symlink() and file.stat().st_nlink == 1 and
                    stat.S_IMODE(file.stat().st_mode) == 0o600 and {p.name for p in public.iterdir()} == PUBLIC_FILES, "ARTIFACT_RECEIPT_INVALID")
        return receipt


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--product-root", required=True, type=Path)
    parser.add_argument("--tooling-root", required=True, type=Path)
    parser.add_argument("--tooling-sha", required=True)
    parser.add_argument("--work-dir", required=True, type=Path)
    parser.add_argument("--run-id", required=True)
    parser.add_argument("--resource-profile", choices=[LEGACY_PROFILE, HOSTED_PROFILE], default=LEGACY_PROFILE)
    args = parser.parse_args(argv)
    producer = None
    os.umask(0o077)
    def cancelled(_signum, _frame):
        raise Refusal("PRODUCER_CANCELLED")
    signal.signal(signal.SIGTERM, cancelled)
    def failure(code: str) -> int:
        closure = "NO_OWN_BUILDER_STARTED"
        if producer is not None and producer.builder_created:
            try:
                producer.stop_builder()
                closure = "OWN_BUILDER_CLOSED"
            except Refusal as e:
                closure = str(e)
            except Exception:
                closure = "OWN_BUILDER_CLOSURE_UNVERIFIED"
        print(encoded({"result": code, "closure": closure,
            "host": producer.host if producer else None,
            "disk_floor": DISK_FLOOR, "memory_floor": producer.memory_floor if producer else MEMORY_FLOOR,
            "minimum_sampled_resources": producer.minimum if producer else None,
            "resource_profile": producer.profile_receipt() if producer else {"name": args.resource_profile}}).decode().strip(), file=sys.stderr)
        return 1
    try:
        require(bool(re.fullmatch(r"[0-9]+-[0-9]+", args.run_id)) and bool(SHA.fullmatch(args.tooling_sha)), "ARGUMENT_INVALID")
        roots = [p.resolve() for p in [args.product_root, args.tooling_root, args.work_dir]]
        require(all(a != b and a not in b.parents and b not in a.parents for i, a in enumerate(roots) for b in roots[i+1:]), "ROOTS_NOT_ISOLATED")
        args.work_dir.mkdir(mode=0o700)
        producer = Producer(args.work_dir, "fvoci-shipping-" + args.run_id, args.resource_profile)
        producer.build(args.product_root, args.tooling_root, args.tooling_sha)
        print("PRODUCER_ONLY_PASS_NOT_RUNTIME_QUALIFIED")
        return 0
    except Refusal as e:
        return failure(str(e))
    except (Exception, KeyboardInterrupt):
        # Never let a raw exception/command dump become public Actions output.
        return failure("PRODUCER_UNEXPECTED_FAILURE")


if __name__ == "__main__":
    raise SystemExit(main())
