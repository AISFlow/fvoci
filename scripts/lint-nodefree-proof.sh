#!/usr/bin/env bash
# Local reproducible proof: only Bun and the Python fixture runner are exposed.
# bwrap is a development verification tool, not a product dependency.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bun_path=$(command -v bun)
python_path=$(readlink -f "$(command -v python3)")
exec bwrap --unshare-net \
  --ro-bind /usr/lib /usr/lib --ro-bind /lib /lib --ro-bind /lib64 /lib64 \
  --ro-bind "$bun_path" /bin/bun --ro-bind "$python_path" /bin/python3 \
  --ro-bind /usr/bin/bash /bin/bash --ro-bind /usr/bin/env /bin/env \
  --bind "$PWD" "$PWD" --proc /proc --dev /dev --tmpfs /tmp \
  --setenv PATH /bin --setenv HOME /tmp --chdir "$PWD" \
  /bin/bash -c '
    set -euo pipefail
    if command -v node; then exit 1; fi
    python3 -c '\''from pathlib import Path; assert not list(Path("/bin").glob("node*")); assert not Path("/usr/bin").exists(); assert not Path("/home/kinesis/.nvm").exists(); print("isolated filesystem: Node absent, network isolated")'\''
    bun -e '\''console.log(JSON.stringify({bun:process.versions.bun,execPath:process.execPath}))'\''
    python3 scripts/test_eslint.py "$@"
  ' lint-nodefree-proof "$@"
