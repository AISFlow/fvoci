#!/usr/bin/env python3
"""One pinned launcher reader observation; never invoke the image producer."""

import ast
import hashlib
import types
import json
import sys
import os
import re
import stat
import select
import subprocess
import time
from contextlib import contextmanager
from datetime import datetime, timezone
from pathlib import Path

READER_SHA = "d15ad53b5877cf97748812d2d3733e80b8a3dbe6"
READER_SOURCE_SHA256 = "7d95318a8ae6ce4eee856a0d695e09b6561ade1f953e5f47c7adc51a4f557909"
READER_PATH = Path(__file__).resolve().parents[2].parent / "reader/scripts/selected-backend-ci/shipping-image-producer.py"
CODES = {"CGROUP_METADATA_UNKNOWN", "CGROUP_ANCESTORS_HIDDEN", "CGROUP_HOST_ROOT_INVALID", "CGROUP_PATH_INVALID"}
FUNCTION_RANGES = {'cgroup_open_directory': (168, 171), 'cgroup_open_absolute': (174, 179), 'cgroup_text': (182, 194), 'cgroup_filesystem': (197, 229), 'cgroup_mounts': (232, 257), 'cgroup_pid': (260, 270), 'cgroup_chain': (273, 351), 'membership': (312, 315)}
STAGES = {
    169: "DIRECTORY_OPEN", 175: "DIRECTORY_PATH_PREDICATE", 184: "FILE_OPEN",
    187: "FILE_IDENTITY_PREDICATE", 189: "FILE_READ", 191: "FILE_READ_BOUND", 192: "TEXT_DECODE",
    201: "STAT_TOOL_PARENT_LSTAT", 202: "STAT_TOOL_PARENT_PREDICATE", 203: "STAT_TOOL_LSTAT",
    204: "STAT_TOOL_PREDICATE", 206: "STAT_TOOL_HASH", 207: "STAT_SPAWN", 214: "STAT_OUTPUT_DEADLINE",
    215: "STAT_OUTPUT_READ", 218: "STAT_OUTPUT_BOUND", 219: "STAT_WAIT", 220: "FILESYSTEM_TYPE_PREDICATE",
    221: "STAT_TOOL_LSTAT", 222: "STAT_TOOL_IDENTITY_PREDICATE", 228: "STAT_CLOSING_WAIT",
    238: "MOUNTINFO_PARSE", 243: "MOUNTINFO_ESCAPE_PREDICATE", 248: "GLOBAL_MOUNT_PREDICATE",
    253: "MOUNT_PATH_PREDICATE", 254: "MOUNT_MASK_PREDICATE", 255: "PROC_MOUNT_MASK_PREDICATE",
    262: "PID_STAT_READ_PARSE", 263: "PID_STAT_PREDICATE", 265: "PID_START_TICK_PARSE",
    266: "MEMBERSHIP_PATH_READ", 267: "MEMBERSHIP_PATH_PREDICATE", 269: "MEMBERSHIP_PATH_PREDICATE",
    278: "PID_ARGUMENT_PREDICATE", 280: "ROOT_DIRECTORY_OPEN", 281: "FILESYSTEM_AUTHENTICATION",
    282: "PROC_SELF_PID_BIND", 284: "MOUNTINFO_READ", 287: "MOUNT_DEVICE_PREDICATE", 290: "ROOT_MARKER_LOOKUP",
    293: "ROOT_MARKER_PREDICATE", 294: "CONTROLLERS_READ_PREDICATE", 297: "ROOT_IDENTITY_PREDICATE",
    301: "PID_IDENTITY_PREDICATE", 303: "CLOSING_ROOT_IDENTITY_PREDICATE", 305: "MEMBERSHIP_PATH_PREDICATE",
    308: "ANCESTOR_DIRECTORY_OPEN", 310: "DOMAIN_TYPE_PREDICATE", 313: "DIRECT_MEMBERSHIP_READ",
    314: "DIRECT_MEMBERSHIP_PREDICATE", 318: "MEMORY_MAX_READ", 319: "MEMORY_MAX_PARSE",
    320: "MEMORY_CURRENT_READ_PARSE", 321: "MEMORY_PEAK_READ_PARSE", 322: "MEMORY_EVENTS_READ",
    323: "MEMORY_EVENTS_SHAPE", 324: "MEMORY_EVENTS_PARSE", 325: "MEMORY_COUNTERS_PREDICATE",
    327: "CPU_MAX_READ", 328: "CPU_MAX_PREDICATE", 329: "ANCESTOR_STAT",
    336: "PID_IDENTITY_RECHECK", 339: "MOUNT_IDENTITY_RECHECK", 341: "ROOT_IDENTITY_RECHECK",
    345: "ANCESTOR_IDENTITY_RECHECK",
}
FILES = {"cgroup.type": "CGROUP_TYPE_LOOKUP", "memory.max": "MEMORY_MAX_READ",
         "memory.current": "MEMORY_CURRENT_READ_PARSE", "memory.peak": "MEMORY_PEAK_READ_PARSE",
         "memory.events": "MEMORY_EVENTS_READ", "cpu.max": "CPU_MAX_READ", "cgroup.procs": "DIRECT_MEMBERSHIP_READ",
         "cgroup.controllers": "CONTROLLERS_READ_PREDICATE", "mountinfo": "MOUNTINFO_READ",
         "stat": "PID_STAT_READ_PARSE", "cgroup": "MEMBERSHIP_PATH_READ"}


def load_reader():
    path = READER_PATH
    if path.is_symlink() or not path.is_file() or path.resolve() != path:
        raise ValueError("OBSERVER_SOURCE_UNVERIFIED")
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != READER_SOURCE_SHA256:
        raise ValueError("OBSERVER_SOURCE_UNVERIFIED")
    functions = {n.name: (n.lineno, n.end_lineno) for n in ast.walk(ast.parse(data))
                 if isinstance(n, ast.FunctionDef) and n.name in FUNCTION_RANGES}
    if functions != FUNCTION_RANGES:
        raise ValueError("OBSERVER_SOURCE_UNVERIFIED")
    module = types.ModuleType("fixed_shipping_reader")
    module.__file__ = str(path)
    # Execute exactly the verified bytes, without a second read or cached pyc.
    exec(compile(data, str(path), "exec"), module.__dict__)
    return module, str(path)


def diagnostic(error, filename, refusal_type):
    cause = error.__cause__ if isinstance(error.__cause__, (OSError, ValueError, IndexError)) else error
    kind = next((label for cls, label in [(OSError, "OSError"), (ValueError, "ValueError"),
                                         (IndexError, "IndexError")] if isinstance(cause, cls)), "Refusal" if type(error) is refusal_type else "UNKNOWN")
    line = None; stage = "UNKNOWN"
    trace = cause.__traceback__
    while trace is not None:
        frame = trace.tb_frame
        bounds = FUNCTION_RANGES.get(frame.f_code.co_name)
        if frame.f_code.co_filename == filename and bounds and bounds[0] <= trace.tb_lineno <= bounds[1] and trace.tb_lineno in STAGES:
            line = trace.tb_lineno
            proposed = STAGES[line]
            if proposed not in {"FILE_OPEN", "FILE_READ", "TEXT_DECODE"} or stage == "UNKNOWN": stage = proposed
        trace = trace.tb_next
    number = cause.errno if isinstance(cause, OSError) and type(cause.errno) is int and 0 <= cause.errno <= 4095 else None
    if isinstance(cause, OSError) and line in STAGES and type(cause.filename) is str:
        # Classify only in memory. Never emit the filename or exception text.
        name = cause.filename
        if name in FILES:
            stage = "ROOT_MEMORY_MAX_LOOKUP" if stage == "ROOT_MARKER_LOOKUP" and name == "memory.max" else FILES[name]
    return {"primitive": stage, "exception_class": kind, "errno": number, "source_line": line if line in STAGES else None}


def observe(reader, filename):
    receipt = {"scope": "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED", "reader_sha": READER_SHA,
               "reader_source_sha256": READER_SOURCE_SHA256}
    try:
        reader.cgroup_chain()
    except Exception as error:
        code = error.args[0] if type(error) is reader.Refusal and len(error.args) == 1 and type(error.args[0]) is str else "CGROUP_OBSERVER_UNEXPECTED_FAILURE"
        receipt.update(result=code if code in CODES else "CGROUP_OBSERVER_UNEXPECTED_FAILURE", diagnostic=diagnostic(error, filename, reader.Refusal))
        return 1, receipt
    receipt.update(result="CGROUP_READER_OBSERVED_OK_NOT_IMAGE_QUALIFIED", diagnostic=None)
    return 0, receipt



# Diagnostic bounds are independent of the unchanged reader's 1MiB stat gate.
METADATA_BYTES = 32 * 1024**2
SYSTEM_ROOTS = (Path("/usr"), Path("/lib"), Path("/lib64"))
IDENTITY_FIELDS = ("st_dev", "st_ino", "st_mode", "st_uid", "st_gid", "st_size", "st_mtime_ns", "st_ctime_ns")


class MetadataMissing(Exception):
    """Only internal literal codes; exception text is never printed."""


def metadata_require(ok, code):
    if not ok: raise MetadataMissing(code)


def metadata_identity(info):
    return tuple(getattr(info, field) for field in IDENTITY_FIELDS)


def metadata_record(info):
    return dict(zip((field[3:] for field in IDENTITY_FIELDS), metadata_identity(info)))


def stat_predicate(info):
    # Five independent observations from ONE lstat, including both size bounds.
    fields = {"regular": stat.S_ISREG(info.st_mode), "uid_zero": info.st_uid == 0,
              "no_group_other_write": not bool(info.st_mode & 0o022),
              "positive_size": info.st_size > 0, "size_le1048576": info.st_size <= 1024**2}
    return {"booleans": fields, "original_predicate": all(fields.values())}


def metadata_tick(deadline):
    metadata_require(time.monotonic() < deadline, "DEADLINE")


def system_path(path):
    return (path.is_absolute() and ".." not in path.parts and
            any(path == root or root in path.parents for root in SYSTEM_ROOTS) and
            all(re.fullmatch(r"[A-Za-z0-9_.+-]{1,255}", part) for part in path.parts[1:]))


def system_leaf(path, deadline):
    """Bounded metadata-only system link provenance; never qualify reader204."""
    metadata_require(system_path(path), "SYSTEM_PATH")
    watches = {}; seen = set(); hops = 0
    while True:
        metadata_tick(deadline)
        metadata_require(path not in seen and len(path.parts) <= 32, "LINK_LOOP_OR_DEPTH")
        seen.add(path); restart = False
        for index in range(len(path.parts)):
            node = Path(*path.parts[:index+1]); info = node.lstat(); metadata_require(node not in watches or watches[node] == metadata_identity(info), "IDENTITY_DRIFT"); watches[node] = metadata_identity(info)
            metadata_require(info.st_uid == 0, "SYSTEM_UID")
            if stat.S_ISLNK(info.st_mode):
                hops += 1; metadata_require(hops <= 8, "LINK_HOPS")
                target = os.readlink(node); metadata_require(len(target) <= 4096, "LINK_BYTES")
                replacement = Path(target) if os.path.isabs(target) else node.parent / target
                path = Path(os.path.normpath(str(replacement.joinpath(*path.parts[index+1:]))))
                metadata_require(system_path(path), "SYSTEM_PATH")
                restart = True; break
            metadata_require(not info.st_mode & 0o022, "SYSTEM_MODE")
            if index < len(path.parts)-1: metadata_require(stat.S_ISDIR(info.st_mode), "SYSTEM_PARENT")
            else: metadata_require(stat.S_ISREG(info.st_mode), "SYSTEM_LEAF")
        if not restart: return path, info, watches, hops


def check_system_watches(watches):
    for path, identity in watches.items():
        metadata_require(metadata_identity(path.lstat()) == identity, "IDENTITY_DRIFT")


@contextmanager
def held_system_leaf(path, deadline):
    leaf, before, watches, hops = system_leaf(path, deadline)
    metadata_require(0 < before.st_size <= METADATA_BYTES, "DIAGNOSTIC_INPUT_BYTES")
    fd = os.open(leaf, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
    try:
        metadata_require(metadata_identity(os.fstat(fd)) == metadata_identity(before), "IDENTITY_DRIFT")
        yield fd, leaf, before, watches, hops
        metadata_tick(deadline); check_system_watches(watches)
        metadata_require(metadata_identity(os.fstat(fd)) == metadata_identity(before), "IDENTITY_DRIFT")
    finally: os.close(fd)


def metadata_hash(fd, deadline):
    digest = hashlib.sha256(); count = 0; os.lseek(fd, 0, os.SEEK_SET)
    while True:
        metadata_tick(deadline); part = os.read(fd, min(1024**2, METADATA_BYTES + 1 - count))
        if not part: break
        count += len(part); metadata_require(count <= METADATA_BYTES, "DIAGNOSTIC_INPUT_BYTES"); digest.update(part)
    os.lseek(fd, 0, os.SEEK_SET)
    return digest.hexdigest()


def metadata_error(error):
    if isinstance(error, MetadataMissing):
        reasons = {"DEADLINE", "SYSTEM_PATH", "LINK_LOOP_OR_DEPTH", "SYSTEM_UID", "LINK_HOPS", "LINK_BYTES",
                   "SYSTEM_MODE", "SYSTEM_PARENT", "SYSTEM_LEAF", "IDENTITY_DRIFT", "DIAGNOSTIC_INPUT_BYTES",
                   "QUERY_COUNT", "QUERY_DEADLINE", "QUERY_OUTPUT_BYTES", "QUERY_EXIT", "PACKAGE_OWNER_AMBIGUOUS",
                   "PACKAGE_OWNER_MISSING", "PACKAGE_FIELDS", "ELF_FIELDS", "ELF_INTERPRETER", "ELF_NEEDED",
                   "KERNEL_FIELDS", "OS_RELEASE_BYTES", "OS_RELEASE_FIELDS", "STAT_METADATA_MISSING", "CHILD_NOT_REAPED"}
        reason = error.args[0] if len(error.args) == 1 and type(error.args[0]) is str and error.args[0] in reasons else "UNKNOWN"
        return {"result": "MISSING", "kind": "METADATA_BOUNDARY", "reason": reason, "errno": None}
    if isinstance(error, subprocess.TimeoutExpired):
        return {"result": "MISSING", "kind": "QUERY_TIMEOUT", "errno": None}
    number = error.errno if isinstance(error, OSError) and type(error.errno) is int and 0 <= error.errno <= 4095 else None
    return {"result": "MISSING", "kind": "OSError" if isinstance(error, OSError) else "UNEXPECTED", "errno": number}


def metadata_query(argv, deadline, queries, *, input_fd=None):
    """Only the two fixed query utilities; bounded pipes, held executable, reap."""
    metadata_require(argv[0] in {"/usr/bin/dpkg-query", "/usr/bin/readelf"} and len(queries) < 3, "QUERY_COUNT")
    metadata_tick(deadline)
    receipt = {"tool": "DPKG_QUERY" if argv[0].endswith("dpkg-query") else "READELF",
               "start_utc": datetime.now(timezone.utc).isoformat(), "pid": None, "exit": None, "reaped": False}
    queries.append(receipt); child = None; stop = min(deadline, time.monotonic() + 5)
    try:
        with held_system_leaf(Path(argv[0]), deadline) as (fd, leaf, info, watches, hops):
            receipt.update(identity=metadata_record(info), link_hops=hops, sha256=metadata_hash(fd, deadline))
            metadata_tick(stop - 0.25)  # Reserve closing time inside the five-second query cap.
            child = subprocess.Popen(argv, executable=f"/proc/self/fd/{fd}",
                pass_fds=(fd,) if input_fd is None else (fd, input_fd),
                stdout=subprocess.PIPE, stderr=subprocess.PIPE, env={"LC_ALL": "C.UTF-8", "PATH": "/usr/bin:/bin"})
            receipt["pid"] = child.pid
            streams = {child.stdout: bytearray(), child.stderr: bytearray()}
            pending = set(streams)
            try:
                while pending:
                    remaining = stop - 0.25 - time.monotonic()
                    metadata_require(remaining > 0, "QUERY_DEADLINE")
                    ready = select.select(list(pending), [], [], remaining)[0]
                    metadata_require(bool(ready), "QUERY_DEADLINE")
                    for stream in ready:
                        cap = 32768 if stream is child.stdout else 4096
                        part = os.read(stream.fileno(), min(8192, cap + 1 - len(streams[stream])))
                        if not part: pending.remove(stream)
                        else:
                            streams[stream].extend(part)
                            metadata_require(len(streams[stream]) <= cap, "QUERY_OUTPUT_BYTES")
                child.wait(timeout=max(0.001, stop - 0.25 - time.monotonic()))
                receipt.update(exit=child.returncode, reaped=True)
                metadata_require(child.returncode == 0, "QUERY_EXIT")
                metadata_require(metadata_hash(fd, stop) == receipt["sha256"], "IDENTITY_DRIFT")
                output = bytes(streams[child.stdout])
            finally:
                try:
                    if child.poll() is None: child.kill()
                    child.wait(timeout=max(0.001, stop - time.monotonic()))
                    receipt.update(exit=child.returncode, reaped=True)
                finally:
                    child.stdout.close(); child.stderr.close()
            receipt["result"] = "QUERY_OBSERVED"
            return output
    except Exception as error:
        receipt.update(metadata_error(error)); raise
    finally:
        receipt["end_utc"] = datetime.now(timezone.utc).isoformat()


def installed_packages(paths, deadline, queries):
    output = metadata_query(["/usr/bin/dpkg-query", "--no-pager", "--search", "--", *map(str, paths)], deadline, queries).decode("ascii")
    owners = set(); covered = set()
    package = r"[a-z0-9][a-z0-9+.-]{1,100}(?::[a-z0-9][a-z0-9-]{0,20})?"
    for line in output.splitlines():
        match = re.fullmatch(f"({package}): (.+)", line)
        metadata_require(match is not None and match[2] in set(map(str, paths)), "PACKAGE_OWNER_AMBIGUOUS")
        owners.add(match[1]); covered.add(match[2])
    metadata_require(covered == set(map(str, paths)) and 0 < len(owners) <= 2, "PACKAGE_OWNER_MISSING")
    fmt = "${binary:Package}\\t${Version}\\t${Architecture}\\t${db:Status-Status}\\t${source:Package}\\t${source:Version}\\n"
    output = metadata_query(["/usr/bin/dpkg-query", "--no-pager", "--show", "--showformat="+fmt, "--", *sorted(owners)], deadline, queries).decode("ascii")
    records = []
    for line in output.splitlines():
        fields = line.split("\t")
        metadata_require(len(fields) == 6 and fields[0] in owners and fields[3] == "installed" and
            re.fullmatch(package, fields[0]) and re.fullmatch(r"[0-9][A-Za-z0-9+.:~_-]{0,127}", fields[1]) and
            re.fullmatch(r"[a-z0-9][a-z0-9-]{0,31}", fields[2]) and re.fullmatch(r"[a-z0-9][a-z0-9+.-]{1,100}", fields[4]) and
            re.fullmatch(r"[0-9][A-Za-z0-9+.:~_-]{0,127}", fields[5]), "PACKAGE_FIELDS")
        records.append(dict(zip(("package", "version", "architecture", "status", "source_package", "source_version"), fields)))
    metadata_require(len(records) == len(owners) and {r["package"] for r in records} == owners, "PACKAGE_FIELDS")
    return {"result": "OBSERVED", "records": records}


def installed_elf(fd, deadline, queries):
    output = metadata_query(["/usr/bin/readelf", "--wide", "--file-header", "--program-headers", "--dynamic", "--", f"/proc/self/fd/{fd}"],
                            deadline, queries, input_fd=fd).decode("ascii")
    fields = {}
    labels = {"Class": {"ELF32": "ELF32", "ELF64": "ELF64"},
              "Data": {"2's complement, little endian": "LITTLE_ENDIAN", "2's complement, big endian": "BIG_ENDIAN"},
              "Machine": {"Advanced Micro Devices X86-64": "X86_64", "AArch64": "AARCH64"}}
    for name, allowed in labels.items():
        values = re.findall(r"^\s*"+name+r":\s*([^\n]+)$", output, re.M)
        metadata_require(len(values) == 1 and values[0].strip() in allowed, "ELF_FIELDS")
        fields[name.lower()] = allowed[values[0].strip()]
    values = re.findall(r"^\s*Type:\s*(EXEC|DYN|REL)(?: \([A-Za-z -]{1,80}\))?\s*$", output, re.M)
    metadata_require(len(values) == 1, "ELF_FIELDS"); fields["type"] = values[0]
    interpreters = re.findall(r"\[Requesting program interpreter: ([^\]\n]+)\]", output)
    metadata_require(len(interpreters) <= 1 and output.count("Requesting program interpreter") == len(interpreters), "ELF_INTERPRETER")
    if interpreters:
        loader = Path(interpreters[0]); metadata_require(system_path(loader), "ELF_INTERPRETER")
        leaf, info, watches, hops = system_leaf(loader, deadline); check_system_watches(watches)
        fields["interpreter"] = {"path_sha256": hashlib.sha256(str(loader).encode()).hexdigest(),
                                 "identity": metadata_record(info), "link_hops": hops}
    else: fields["interpreter"] = None
    needed = re.findall(r"\(NEEDED\).*\[([^\]\n]+)\]", output)
    metadata_require(len(needed) <= 32 and output.count("(NEEDED") == len(needed) and
                     all(re.fullmatch(r"[A-Za-z0-9_.+-]{1,128}", value) for value in needed), "ELF_NEEDED")
    fields["needed_sha256"] = [hashlib.sha256(value.encode()).hexdigest() for value in needed]
    return {"result": "OBSERVED_METADATA_NOT_LOADED", **fields}


def installed_kernel(deadline):
    metadata_tick(deadline); info = os.uname()
    metadata_require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+[A-Za-z0-9_.+-]{0,96}", info.release) and
                     info.machine in {"x86_64", "aarch64"}, "KERNEL_FIELDS")
    with held_system_leaf(Path("/usr/lib/os-release"), deadline) as (fd, leaf, before, watches, hops):
        data = os.read(fd, 4097); metadata_require(len(data) <= 4096, "OS_RELEASE_BYTES")
        text = data.decode("ascii"); fields = {}
        for name in ("ID", "VERSION_ID"):
            values = re.findall(r"^"+name+r'="?([A-Za-z0-9_.+-]{1,64})"?$', text, re.M)
            metadata_require(len(values) == 1 and (values[0] == "ubuntu" if name == "ID" else
                bool(re.fullmatch(r"[0-9]{2}\.[0-9]{2}", values[0]))), "OS_RELEASE_FIELDS")
            fields[name.lower()] = values[0]
    return {"result": "VISIBLE_KERNEL_ONLY", "release": info.release, "machine": info.machine,
            "version_sha256": hashlib.sha256(info.version.encode()).hexdigest(), **fields}


def installed_stat_metadata():
    """One metadata-only observation. Never calls the collector or stat/loader."""
    deadline = time.monotonic() + 30; queries = []
    receipt = {"scope": "INSTALLED_STAT_METADATA_ONLY_NOT_ADMITTED_NOT_IMAGE_QUALIFIED",
               "reader_sha": READER_SHA, "reader_source_sha256": READER_SOURCE_SHA256,
               "start_utc": datetime.now(timezone.utc).isoformat(), "queries": queries}
    # Verify the same immutable source/line map, without executing its module.
    try:
        path = READER_PATH
        metadata_require(not path.is_symlink() and path.resolve() == path and path.is_file(), "READER_SOURCE")
        data = path.read_bytes()
        metadata_require(hashlib.sha256(data).hexdigest() == READER_SOURCE_SHA256 and
            {n.name: (n.lineno, n.end_lineno) for n in ast.walk(ast.parse(data)) if isinstance(n, ast.FunctionDef) and n.name in FUNCTION_RANGES} == FUNCTION_RANGES,
            "READER_SOURCE")
    except Exception:
        return 1, {**receipt, "result": "OBSERVER_SOURCE_UNVERIFIED"}
    before = {}; stage = "STAT_LSTAT"
    try:
        tool = Path("/usr/bin/stat"); info = tool.lstat(); before[tool] = metadata_identity(info)
        receipt["stat"] = {"identity": metadata_record(info), **stat_predicate(info)}
        stage = "PARENT_LSTAT"; parents = []
        for parent in tool.parents:
            metadata_tick(deadline); item = parent.lstat(); before[parent] = metadata_identity(item)
            parents.append({"identity": metadata_record(item), "original_parent_predicate":
                stat.S_ISDIR(item.st_mode) and item.st_uid == 0 and not bool(item.st_mode & 0o022)})
        receipt["parents"] = parents
        stage = "SYSTEM_PROVENANCE"
        leaf, leaf_info, watches, hops = system_leaf(tool, deadline)
        receipt["provenance"] = {"result": "OBSERVED_NOT_ADOPTED", "leaf_path_sha256": hashlib.sha256(str(leaf).encode()).hexdigest(),
                                 "identity": metadata_record(leaf_info), "link_hops": hops}
        paths = [tool] if leaf == tool else [tool, leaf]
        try: receipt["packages"] = installed_packages(paths, deadline, queries)
        except Exception as error: receipt["packages"] = metadata_error(error)
        try:
            with held_system_leaf(tool, deadline) as (fd, held, held_info, held_watches, held_hops):
                receipt["leaf_sha256"] = metadata_hash(fd, deadline)
                receipt["elf"] = installed_elf(fd, deadline, queries)
                metadata_require(metadata_hash(fd, deadline) == receipt["leaf_sha256"], "IDENTITY_DRIFT")
        except Exception as error: receipt["elf"] = metadata_error(error)
        stage = "CLOSING_PROVENANCE"; check_system_watches(watches)
    except Exception as error:
        receipt["provenance_failure"] = {"primitive": stage, **metadata_error(error)}
    try: receipt["kernel"] = installed_kernel(deadline)
    except Exception as error: receipt["kernel"] = metadata_error(error)
    try:
        metadata_tick(deadline); check_system_watches(before)
        metadata_require("stat" in receipt and "provenance_failure" not in receipt, "STAT_METADATA_MISSING")
        metadata_require(all(item["pid"] is None or item["reaped"] for item in queries), "CHILD_NOT_REAPED")
        receipt["result"] = "METADATA_OBSERVED_NOT_ADMITTED"; code = 0
    except Exception as error:
        receipt["closing"] = metadata_error(error); receipt["result"] = "METADATA_MISSING_NOT_ADMITTED"; code = 1
    receipt["end_utc"] = datetime.now(timezone.utc).isoformat()
    return code, receipt


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if args == ["--installed-stat-metadata"]:
        code, receipt = installed_stat_metadata()
    elif args:
        code, receipt = 1, {"result": "OBSERVER_ARGUMENT_REFUSED", "scope": "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED"}
    else:
        try:
            reader, filename = load_reader()
        except Exception:
            code, receipt = 1, {"result": "OBSERVER_SOURCE_UNVERIFIED", "scope": "LAUNCHER_CGROUP_ONLY_NOT_IMAGE_QUALIFIED"}
        else:
            code, receipt = observe(reader, filename)
    print(json.dumps(receipt, sort_keys=True, separators=(",", ":")))
    return code


if __name__ == "__main__":
    raise SystemExit(main())
