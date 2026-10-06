#!/usr/bin/env python3
"""One pinned launcher reader observation; never invoke the image producer."""

import ast
import hashlib
import types
import json
import sys
from pathlib import Path

READER_SHA = "05c9611c5c140030df582dd99fd518a022948a31"
READER_SOURCE_SHA256 = "4712f6e06d653c50e04020fdabfbfdd48e7b2ea090876b277b3aeb317698d038"
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
