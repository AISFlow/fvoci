#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."
bun --bun scripts/verify-web-tools.mjs
bun --bun scripts/prepare-vue-lint-types.mjs
if [[ $# == 0 || $1 == -* ]]; then
  set -- 'apps/web/**/*.{js,mjs,cjs,ts,tsx,vue}' \
    'packages/editor/**/*.{js,mjs,cjs,ts,tsx,vue}' 'packages/i18n/**/*.{js,mjs,cjs,ts,tsx,vue}' \
    'scripts/**/*.mjs' 'scripts/run-selected-backend-e2e.ts' scripts/eslint-fixtures.test.ts \
    'tools/selected-backend-ci/**/*.ts' tools/ci/workflows.test.ts tools/ci/planner.test.ts 'tools/web-e2e/**/*.ts' \
    'tools/install-smoke/**/*.ts' \
    'scripts/schema-baseline/*.ts' eslint.config.mjs "$@"
fi
exec bun --bun node_modules/eslint/bin/eslint.js --max-warnings=0 "$@"
