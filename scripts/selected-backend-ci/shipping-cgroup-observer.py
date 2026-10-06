#!/usr/bin/env python3
"""One pinned launcher reader observation; never invoke the image producer."""

import ast
import hashlib
import types
import json
import sys
from pathlib import Path

READER_SHA = "288dacdcdbe1f76a7f885e6a13c17014dc4bb95d"
READER_SOURCE_SHA256 = "7abda2c2006ca4547811e243249f315e022b0505e2fc8b3238371999387ae644"
READER_PATH = Path(__file__).resolve().parents[2].parent / "reader/scripts/selected-backend-ci/shipping-image-producer.py"
CODES = {"CGROUP_METADATA_UNKNOWN", "CGROUP_ANCESTORS_HIDDEN", "CGROUP_HOST_ROOT_INVALID", "CGROUP_PATH_INVALID"}
STAGES = {
    168: "MOUNTINFO_READ", 169: "MOUNTINFO_PARSE", 170: "MOUNT_NAMESPACE_PREDICATE",
    171: "NAMESPACE_READLINK", 172: "CONTROLLERS_READ", 173: "HOST_ROOT_PREDICATE",
    175: "MEMBERSHIP_READ", 176: "MEMBERSHIP_PREDICATE", 178: "MEMBERSHIP_PATH_PREDICATE",
    182: "ANCESTOR_PATH_BIND", 183: "MEMORY_MAX_READ", 184: "MEMORY_MAX_PARSE",
    185: "MEMORY_CURRENT_READ_PARSE", 186: "MEMORY_PEAK_READ_PARSE", 187: "MEMORY_EVENTS_READ",
    188: "MEMORY_EVENTS_SHAPE", 189: "MEMORY_EVENTS_PARSE", 190: "MEMORY_COUNTERS_PREDICATE",
    191: "MEMORY_COUNTERS_PREDICATE", 192: "CPU_MAX_READ", 193: "CPU_MAX_PREDICATE", 194: "ANCESTOR_STAT",
}
FILES = {"memory.max": "MEMORY_MAX_READ", "memory.current": "MEMORY_CURRENT_READ_PARSE",
         "memory.peak": "MEMORY_PEAK_READ_PARSE", "memory.events": "MEMORY_EVENTS_READ", "cpu.max": "CPU_MAX_READ"}
PROC_FILES = {"/proc/self/mountinfo": "MOUNTINFO_READ", "/proc/self/ns/cgroup": "SELF_NAMESPACE_READLINK",
              "/proc/1/ns/cgroup": "INIT_NAMESPACE_READLINK", "/proc/self/cgroup": "MEMBERSHIP_READ",
              "/sys/fs/cgroup/cgroup.controllers": "CONTROLLERS_READ"}


def load_reader():
    path = READER_PATH
    if path.is_symlink() or not path.is_file() or path.resolve() != path:
        raise ValueError("OBSERVER_SOURCE_UNVERIFIED")
    data = path.read_bytes()
    if hashlib.sha256(data).hexdigest() != READER_SOURCE_SHA256:
        raise ValueError("OBSERVER_SOURCE_UNVERIFIED")
    function = next(n for n in ast.parse(data).body if isinstance(n, ast.FunctionDef) and n.name == "cgroup_chain")
    if (function.lineno, function.end_lineno) != (165, 201):
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
    line = None
    trace = cause.__traceback__
    while trace is not None:
        frame = trace.tb_frame
        if frame.f_code.co_filename == filename and frame.f_code.co_name == "cgroup_chain":
            line = trace.tb_lineno
        trace = trace.tb_next
    stage = STAGES.get(line, "UNKNOWN")
    number = cause.errno if isinstance(cause, OSError) and type(cause.errno) is int and 0 <= cause.errno <= 4095 else None
    if isinstance(cause, OSError) and line in STAGES and type(cause.filename) is str:
        # Classify only in memory. Never emit the filename or exception text.
        name = cause.filename
        if name in PROC_FILES:
            stage = PROC_FILES[name]
        elif name.startswith("/sys/fs/cgroup/") and ".." not in name.split("/"):
            stage = FILES.get(name.rsplit("/", 1)[-1], stage)
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


def main(argv=None):
    args = sys.argv[1:] if argv is None else argv
    if args:
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
