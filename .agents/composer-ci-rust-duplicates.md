# composer-ci-rust-duplicates evidence

## Checkpoint 1 (24b838d8820ec48e1a6daa60de9b4650c0d59270)

- Model: Composer 2.5 (cursor-agent implement worker).
- Inlined `verify_native_admission_log` into `scripts/run-rust-collaboration-ci-tests.sh` and removed `scripts/verify-collaboration-native-admission-log.sh`.
- Added stub-cargo fixture `scripts/fixtures/rust-ci/run-collaboration-admission-fixture-test.sh` covering pass, fail, duplicate, and missing admission output paths against the production runner only.

## Checkpoint 2 (fixture hardening + fast CI hook)

- Model: Composer 2.5 (`composer-2.5`, cursor-agent dispatch `task_9494da2383ec`).
- Fixture: single `mktemp` run directory for fake `cargo` and `RUNNER_TEMP` logs; dropped `chmod` on tracked runner; fail stub prints admission `ok` then exits `101` with assertion on propagated status; added `failed` and `ignored` stub modes with a valid admission `ok` line so FAILED/ignored verifier branches are exercised through the runner; records one `cargo` invocation and asserts all nine `--test` targets parsed from the runner script.
- CI: `Rust` workflow `fast` job runs the fixture immediately after toolchain install, before Cargo cache restore and builds.
- Verification: `bash scripts/fixtures/rust-ci/run-collaboration-admission-fixture-test.sh` → `run-collaboration-admission-fixture-test: ok` (exit 0).
