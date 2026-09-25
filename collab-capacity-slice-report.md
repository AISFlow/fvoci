# Collab capacity slice report

## 2026-09-25 — latency root cause and merge-bar pass

Evidence log: `/home/kinesis/orca/fvoci-evidence/collab-capacity-probe-20260925T070429Z.log`  
Branch: `fvoci/rust-collab-engine-capacity`

### Sampler discrepancy (fixed)

The pre-fix probe reported client apply→broadcast p50 ≈ 25 ms while `collab.stage` server stages summed to ≈ 340 ms p50 at ~64 updates/s. Root causes:

1. **Survivorship bias** — an 800 ms client wait dropped slow samples.
2. **Subset sampling** — only 10 rotating rooms per tick were measured, not all 64 load edits.
3. **Post-send drain** — the latency task drained the reader *after* the edit marker, often consuming the broadcast before `wait_for_sync_update` (fixed: parallel pre-send drain per tick).

The harness now samples **all 64 rooms every tick** (11,520 samples / 180 s), uses a 5 s `LATENCY_WAIT`, asserts `latency_missed == 0`, and reports client p50/p95/p99 in `PROBE_SUMMARY`.

### Client latency (ordinary load edits)

| Percentile | ms |
| --- | ---: |
| p50 | 73 |
| p95 | 108 |
| p99 | 209 |

Merge bar: p95 ≤ 300 ms, rate ≥ 0.95/s/room, 0 writer loss, 0×1011, hostile 5/5, room 65→1013, slot reuse — **pass**.

### Server `collab.stage` percentiles (scripted from log)

Computed with `scripts/collab-capacity-stage-percentiles.py` (n ≈ 11,595 per stage):

| Stage | p50 ms | p95 ms | p99 ms |
| --- | ---: | ---: | ---: |
| validate | 5.8 | 17.0 | 24.0 |
| auth_tx | — | — | — |
| append_tx | 34.3 | 53.1 | 74.6 |
| apply | 0.9 | 3.7 | 32.1 |
| broadcast | 0.0 | 0.0 | 0.3 |

**validate sub-stages (p50 ms):** `spawn_us` 0, `load_us` 2.8 (primary Apply), `snapshot_us` 1.7, `slot_wait_us` 0.

**append_tx sub-stages (p50 ms):** `pool_wait_us` 0, `advisory_lock_us` 1.1, `row_lock_us` 11.3, `stmt_us` 13.9, `commit_us` 2.8.

`auth_tx` is folded into `append_tx` (`authorize_wiki_collab_write` inside the append transaction).

### Dominant causes and fixes

| Cause | Symptom | Fix |
| --- | --- | --- |
| Ephemeral validator spawn per edit | validate p50 ≈ 158 ms, `spawn_us` ≈ 92 ms | **Primary-engine admission** — Apply + Snapshot on the room primary (no validator child) |
| App pool size 10 vs 64 rooms | `pool_wait_us` on auth/append | `derive_app_pool_max_connections(max_rooms)` + room dedicated PG connection for hot-path append |
| Redundant auth_tx transaction | ~70 ms p50 duplicate authorization | Removed separate `auth_tx`; append authorizes in one transaction |
| Biased client sampler | Client p50 ≪ server stages | Full-room per-edit measurement (above) |

### Warm per-room validator (design note — not in hot path)

`RoomAdmissionValidator` (I1–I5) remains in `validation.rs` for compaction/fallback documentation:

- **I1** — one admission child per room at steady state  
- **I2** — full committed bundle reloaded each edit  
- **I3** — child killed on reject or engine unavailable  
- **I4** — no cross-room shared mutable validator state  
- **I5** — admission limits unchanged  

Attempts to enable warm validators on the hot path (lazy spawn, cap raised to 64) collapsed load (~192 edits, mass writer failures) — likely 128 live children (64 primary + 64 validator) plus slot contention. **Primary admission** achieves the latency goal with fewer moving parts; warm validators stay a documented fallback for environments that cannot share primary for admission.

### Configuration

- `collab_pg_connections_required(max_rooms)` checked at startup against PostgreSQL `max_connections`.
- Probe/test PG default `max_connections` raised to 150 for 64 room guards + app pool + reserve.

### ARM64 `collab_operational_project_failure_recovers_primary` flake

`collab_operational_project_failure_recovers_primary` passed 5/5 locally with `--test-threads=4` (matching CI parallelism). No branch change targets projection persist-failed timing; primary admission does not alter project-helper caps. If ARM64 CI still flakes, suspect pre-existing helper-slot contention under parallel `collab_projection` rather than this slice.

## 2026-09-25 — advisor capacity-3 gate fixes and re-verification

Evidence log: `/home/kinesis/orca/fvoci-evidence/collab-capacity-probe-20260925T074123Z.log`  
Branch: `fvoci/rust-collab-engine-capacity` (post-fix commit pending)

### Advisor gate changes (room.rs)

| Issue | Fix |
| --- | --- |
| `writer_generation` None after primary Apply | Check before `validate_candidate_on_primary` |
| `room_guard` `.expect()` panic | Fatal fence loss: close 1013, drop guard, actor exits when empty |
| Ignored reload failures on reject paths | `reload_primary_or_close_room` → 1013 room close; post-commit unhealthy paths still 1011 |
| Redundant per-edit Snapshot | Dropped; `Apply` already enforces `complete_v1` cap |
| Double Apply after commit | `integrate_committed_update(..., admission_applied=true)` skips re-apply unless recycle |
| Hostile reload amplification | Per-connection reject counter (8 / 30 s) closes offender 1008 |
| Fence connection I/O error | SQLx error on room PG conn → fatal fence loss + actor exit |
| Cold-restart proof | `collab_cold_reload_fragmented_document_fits_rlimits` (48-tail bundle) |

**Op budget recycling:** hot path now **1 engine op/edit** (admission Apply only; integrate is metadata). Prior path was ~3 ops/edit (Apply + Snapshot + integrate Apply) → recycle threshold moves from ~85 edits to **~256** per primary child (`MAX_OPS=256`).

### New regression tests (collab_product.rs)

- `collab_reject_reload_failure_closes_room_without_serving_rejected_to_peer` — forced reload fail after reject closes 1013; peer SyncStep2 never contains hostile bytes
- `collab_room_fence_connection_loss_closes_room_and_recovers` — `pg_terminate_backend` on room fence conn; reconnect resumes edits
- `collab_cold_reload_fragmented_document_fits_rlimits` — fresh child cold Load of fragmented snapshot+tail

Probe harness: **mass reconnect** phase (drop all 128 sockets, idle-evict, reopen 64 rooms, verify writers).

### Client latency (64 rooms × 180 s load, post-fix)

| Percentile | ms |
| --- | ---: |
| p50 | 68 |
| p95 | 103 |
| p99 | 160 |

Merge bar: p95 ≤ 300 ms, rate ≥ 0.95/s/room (achieved **1.000**), 0 writer loss, 0×1011, hostile 5/5 + victim recovery, mass reconnect 64/64, room 65→1013, slot reuse — **pass**.

### Server `collab.stage` percentiles (scripted; n ≈ 11,659)

| Stage | p50 ms | p95 ms | p99 ms |
| --- | ---: | ---: | ---: |
| validate | 1.2 | 6.9 | 11.8 |
| append_tx | 33.8 | 54.9 | 77.0 |
| apply | 0.0 | 0.0 | 0.0 |
| broadcast | 0.0 | 0.0 | 0.1 |

`snapshot_us` p50 = 0 (Snapshot removed from admission). `apply` p50 = 0 (integrate no longer re-applies).

### CI / integration (local, `--test-threads=4` where noted)

| Suite | Result |
| --- | --- |
| `cargo fmt --check`, `clippy --all-targets --features db-tests,api-schema -D warnings` | pass |
| `collab_projection` (19 tests) | pass — ARM64 child-cap flake **unchanged** by this branch |
| `revision_integration` (8 tests) | pass |
| `collab_lifecycle` (20), `collab_shutdown` (7), `document_collab_lifecycle` (3) | pass |
| `collab-engine` crate tests | pass |
| New `collab_product` gate tests (3) | pass |

SIGTERM drain at 64 live rooms: covered by existing `collab_shutdown` process tests (normal SIGTERM exit 0, guard release); not duplicated inside the capacity probe.
