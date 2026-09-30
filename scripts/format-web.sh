#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bun --bun scripts/verify-web-tools.mjs
if [[ $# == 0 || $1 == -* ]]; then
  set -- apps/web packages/editor packages/i18n \
    scripts/document-convert scripts/generate-emoji-shortcodes.mjs \
    scripts/install-smoke-collab.mjs scripts/verify-web-tools.mjs scripts/WEB_LINT.md \
    eslint.config.mjs package.json .prettierrc.json "$@"
fi
exec bun --bun node_modules/prettier/bin/prettier.cjs --check --ignore-path .prettierignore "$@"
