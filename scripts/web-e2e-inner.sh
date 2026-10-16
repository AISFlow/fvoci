#!/usr/bin/env bash
exec bun --no-env-file "${ROOT:?ROOT is required}/tools/web-e2e/run-web-e2e.ts" group-runtime "$@"
