# Composer session revisions R3 — post-submit reconnect checkpoint

## Dispatch

- Task: `task_9a2e69bd662f` / `ctx_da56473b093e`
- Prior worker_done SHA: `8c227539a367de796f3eabd47ce9100c62f74c22` (R3 primary path)
- Coordinator correction: per-connection join admission witness; session-revision persist barrier held through leave persist await; release only after rejoin is queued on the room mailbox; preserve last-leave snapshot across reconnect (clear prior session rows, re-leave dedupe).

## Fixed HEAD

- Branch: `fvoci/rust-session-revisions`
- SHA: `0f3a4679e8a19980c053d871936b8170698295af` (branch tip; substantive reconnect fix in `ea69c54d`)

## Test delta (narrow)

Command (exit 0):

```sh
export CARGO_HOME=/home/kinesis/orca/toolchains/fvoci-rust/cargo
export RUSTUP_HOME=/home/kinesis/orca/toolchains/fvoci-rust/rustup
export PATH="$CARGO_HOME/bin:$PATH"
ROOT=/home/kinesis/orca/workspaces/fvoci/rust-session-revisions
export FVOCI_COLLAB_ENGINE="$ROOT/crates/collab-engine/target/debug/collab-engine"
CARGO_TARGET_DIR="$ROOT/target/task_9a2e69bd662f"
bash "$ROOT/scripts/start-test-postgres.sh" env \
  CARGO_TARGET_DIR="$CARGO_TARGET_DIR" \
  FVOCI_COLLAB_ENGINE="$FVOCI_COLLAB_ENGINE" \
  cargo test --locked --offline --features db-tests --test revision_integration \
  session_revision_persists_through_immediate_reconnect -- --nocapture
```

Result: `1 passed; 0 failed` (2026-09-27).

## Code notes

- `tests/revision_integration.rs`: `session_revision_persists_through_immediate_reconnect` uses `arm_join_channel_admission_witness(rejoin_conn)` (per-connection), not a global next-join witness.
- `src/collab/room.rs`: removed unused `NEXT_JOIN_MAILBOX_WITNESS` hooks to avoid global next-join races alongside per-connection admission.

## Not run (checkpoint scope)

- Full collaboration DB bundle / heavy revision_integration matrix (prior R3 run; Grok review of `8c227539` in flight).
- Scheduled retention / new architecture (out of scope).
