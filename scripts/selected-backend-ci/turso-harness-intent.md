# Turso harness T4 — intent and differences

The GitHub workflow still admits, freezes, and consumes through the same modes. TypeScript run by Bun replaces the three Python entrypoints. Security checks stay fail-closed: fixed error codes, no secret echo, manual dispatch only, and the pre-secret diagnostic unit.

| Old file | New file | Caller contract kept |
| --- | --- | --- |
| `scripts/selected-backend-ci/turso-test-fixtures.py` | `scripts/selected-backend-ci/turso-test-fixtures.test.ts` | No arguments. Exit 0 when the offline admission checks pass, non-zero otherwise. No Turso database and no credential lookup. |
| `scripts/selected-backend-ci/turso-test-guard.py` | `scripts/selected-backend-ci/turso-test-guard.ts` | Argv is exactly `--admit`, `--freeze`, `--diagnostic-unit`, or `--consume`. Success exit 0. Admission failures exit 78 with one fixed code line on stderr (`ADMISSION_FAILED` for unexpected faults). Stdout lines consumed by operators stay the same (`BOOTSTRAP_SOURCE_ADMISSION_OK_RUNTIME_NOT_RUN`, `ENVIRONMENT_ADMISSION_OK_RUNTIME_NOT_RUN`, `COMPILED_TEST_FROZEN_RUNTIME_NOT_RUN`, `TURSO_DIAGNOSTIC_UNIT_PASS`, connection/migration/inventory/reset receipts). `--admit` appends `environment_id=<id>` to `GITHUB_OUTPUT`. `--freeze` still writes `turso-connection-libtest` (mode `0700`) and `turso-connection-build.json`. |
| `scripts/selected-backend-ci/turso-ui.py` | `scripts/selected-backend-ci/turso-ui.ts` | Argv is exactly `--record-before`, `--freeze`, or `--actor`. Exit 78 with `UI_*` / `TURSO_UI_*` or `UI_CONSUMER_FAILED`. `--record-before` writes `source-before.json` and `physical-before.private.json` (mode `0600`). `--freeze` writes `current-build.json`. Baseline stdout is `TURSO_UI_BASELINE_PASS ...`. Ack stdout is `TURSO_UI_ACK_PASS on=1 restart=1 off=8 ...`. |

## Differences

| Topic | What changed | Why it is still the same contract |
| --- | --- | --- |
| Fixture stdout | `bun test` prints its own reporter instead of `unittest` text. | The workflow uses the exit code only. |
| Environment metadata | `fetch` with `redirect: "manual"`, no `Authorization`, 15s timeout, 256KiB cap. | Redirects and transport faults still become `ENVIRONMENT_METADATA_UNAVAILABLE`. The anonymous URL is unchanged. |
| Child pipes | Stdout and stderr chunks are kept in arrival order and then discarded except for fixed receipt lines. | Raw libtest/SDK text is still not printed. A split receipt fails closed. |
| Physical inputs and local lease | The TS harness calls the existing `inputs()` / `build_env()` and `load_local_allocation(..., consumer='turso-ui')` and drops their stderr. | Those collectors stay the single source. No new Python file is added. |
| Orca-local docker fifo | GitHub-hosted mode runs the native fixture, migrate, and Playwright path. Orca-local fixture execution refuses with `UI_EXECUTION_MODE_REFUSED` instead of the old pause/go fifo docker handshake. Capsule text, docker argv, creation identity, and cgroup cap checks remain. | The workflow job is `github-ci`. Local docker observation is not silently downgraded to a host process. |
| Actor wrapper | The generated member wrapper execs `bun` on `turso-ui.ts --actor`. | The mode, quoting, and `0700` mode are unchanged. |
| Freeze manifest JSON | Object encoding matches CPython `json.dumps` spacing so digests of tracked sources stay comparable. | Keys and hashes are the binding, not incidental whitespace outside that digest. |
