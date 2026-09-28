#!/usr/bin/env bash
# Prints the measurement environment as JSON (no secrets, no user data).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SERVER_BIN="${1:?server binary}"
COLLAB_BIN="${2:?collab engine binary}"
python3 - "$ROOT" "$SERVER_BIN" "$COLLAB_BIN" <<'PY'
import json, os, platform, subprocess, sys
root, server, collab = sys.argv[1:4]
def run(*cmd, cwd=None):
    try:
        return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True, timeout=30).stdout.strip()
    except Exception as exc:  # recorded, not hidden
        return f"unavailable: {type(exc).__name__}"
def meminfo():
    for line in open("/proc/meminfo"):
        if line.startswith("MemTotal"):
            return line.split(":")[1].strip()
cpu = next((l.split(":", 1)[1].strip() for l in run("lscpu").splitlines() if l.startswith("Model name")), "")
os_name = next((l.split("=", 1)[1].strip().strip('"') for l in open("/etc/os-release") if l.startswith("PRETTY_NAME")), "")
print(json.dumps({
    "git_head": run("git", "rev-parse", "HEAD", cwd=root),
    "git_dirty_paths": len(run("git", "status", "--porcelain", cwd=root).splitlines()),
    "build_kind": "source build (release cargo + production vite); not a published image",
    "published_image_digest": None,
    "kernel": platform.release(),
    "os": os_name,
    "cpu_model": cpu,
    "logical_cpus": os.cpu_count(),
    "mem_total": meminfo(),
    "loadavg_at_start": os.getloadavg(),
    "docker_server": run("docker", "version", "--format", "{{.Server.Version}}"),
    "docker_ncpu_mem": run("docker", "info", "--format", "{{.NCPU}} cpus {{.MemTotal}} bytes"),
    "docker_container_limits": "none set (containers share the host)",
    "node": run("node", "--version"),
    "playwright": run(os.path.join(root, "apps/web/node_modules/.bin/playwright"), "--version"),
    "server_binary_bytes": os.path.getsize(server),
    "collab_engine_binary_bytes": os.path.getsize(collab),
    "rustc": run("rustc", "--version"),
}, indent=1))
PY
