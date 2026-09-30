#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
# Always use this workspace's locked CLI, including when invoked from elsewhere.
# BIOME_BINARY would replace the npm package's binary lookup.
unset BIOME_BINARY
expected=$(bun -e 'process.stdout.write(require("./package.json").devDependencies["@biomejs/biome"])')
actual=$(bun --bun node_modules/@biomejs/biome/bin/biome --version)
if [[ "$expected" != "2.5.14" || "$actual" != "Version: $expected" ]]; then
  printf 'Expected pinned Biome 2.5.14, manifest=%s, CLI=%s\n' "$expected" "$actual" >&2
  exit 1
fi
exec bun --bun node_modules/@biomejs/biome/bin/biome ci \
  --config-path=biome.json --error-on-warnings "$@"
