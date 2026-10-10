#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
bun --bun scripts/verify-web-tools.mjs
if [[ $# == 0 || $1 == -* ]]; then
  set -- apps/web packages/editor packages/i18n \
    scripts/document-convert scripts/generate-emoji-shortcodes.mjs \
    scripts/install-smoke-collab.mjs scripts/verify-web-tools.mjs scripts/WEB_LINT.md \
    scripts/run-selected-backend-e2e.ts scripts/eslint-fixtures.test.ts \
    'tools/selected-backend-ci/**/*.ts' tools/ci/workflows.test.ts tools/ci/planner.test.ts 'tools/web-e2e/**/*.ts' \
    'scripts/schema-baseline/*.ts' 'scripts/schema-baseline/*.md' \
    eslint.config.mjs package.json .prettierrc.json "$@"
fi
exec bun --bun node_modules/prettier/bin/prettier.cjs --check --ignore-path .prettierignore "$@"
