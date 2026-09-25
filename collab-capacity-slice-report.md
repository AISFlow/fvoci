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
