# Search index throughput report

Advisor rejected b190073 (process-wide `MEILI_WRITE_BATCH`, last-uid-only flush,
Meili-named outbox hooks). This rework uses generic `OutboxConsumer::deliver_batch`
and `SearchIndexConsumer` batch refresh with per-call state.

## Correctness changes

- Removed `MEILI_WRITE_BATCH`, `begin_meili_write_batch`, `finish_meili_write_batch`,
  `external_batch_meili_writes`, and `meili_batch_flush_config`.
- External dispatcher: renew lease, call `deliver_batch`, `mark_processed` + cursor
  advance for the returned prefix only, `handle_failure` on the first failed event.
- `SearchIndexConsumer::deliver_batch`: coalesce resources (last wins), enqueue all
  Meili writes, `wait_meili_tasks` via `GET /tasks?uids=…` checking every uid; any
  failure returns `(0, err)` for idempotent retry.
- Tests: failed middle oversized doc, parallel dispatchers (no static cross-talk),
  lease-bound chunks, burst convergence, throughput probe.

## `SEARCH_INDEX_EXCLUSIVE`

Kept. It serializes tests against one shared Meili CE + Postgres, not the removed
write-batch static (`meili_down_retries_*` timing).

## Throughput (`scripts/measure-search-index-throughput.sh`)

| Probe | b190073 (rejected) | deliver_batch rework |
| --- | ---: | ---: |
| `ensure_ms` | 41 | 39 |
| `single_event_ms` | 2008 | 1897 |
| `batch5_ms` | 3299 | 3190 |

Batch of five events stays ~3.2s (one coalesced Meili wait chain) vs ~10s if paid
per event at ~2s/task. Raw Meili upserts unchanged (~1.6–2.6s/task_wait).

Measured 2026-09-25 on WSL2 with worktree `CARGO_TARGET_DIR` under `/tmp`.
