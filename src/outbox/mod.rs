//! Outbox relay: delivers committed `fvoci.events` to each consumer in
//! `(xact, seq)` order for PostgreSQL, one cursor per consumer. A PG event
//! is read only once its xact is below snapshot xmin. SQLite-family events
//! use their real committed writer-transaction sequence, with no XID/xmin
//! surrogate; readers cannot skip a later commit behind their cursor.
//!
//! Each server runs one dispatcher task that serves every registered
//! consumer in turn, with one lease per consumer. Only the lease holder
//! advances a cursor or records a failure. The task owns its leases and
//! releases them on shutdown; after a crash they expire on their own.
//!
//! Order per event:
//! - `DatabaseAtomic` (legacy spelling `PgOnly`): one transaction writes the
//!   processed mark (where the consumer
//!   keeps marks), applies the effect only when that mark is new, and
//!   advances the cursor; a rejected advance rolls it all back.
//! - `External`: the effect, then the processed mark (it needs no lease),
//!   then a lease renewal, then one advance over the delivered prefix, then
//!   the failure rows are cleared. A requeued retry applies the effect and
//!   marks unless the mark is already there, then clears its failure row; it
//!   does not move the cursor.
//!
//! Guaranteed: a `PgOnly` effect is applied once, and `External` delivery is
//! at least once. The cursor passes an event the consumer has not reported
//! done only as a dead letter, or through `fvoci-migrate --outbox-reset` or
//! `--recover-outbox`.
//!
//! Not guaranteed: exactly-once `External` delivery (the effect is repeated
//! when a crash, a dropped call or a database error comes between it and its
//! mark, or when a lease runs out mid-call and a second owner takes it; see
//! [`DeliveryMode::External`]), any order across consumers, and redelivery
//! of a dead letter. An event that fails `OUTBOX_MAX_ATTEMPTS` times (with
//! the default backoff about 15 s plus attempt time after its first failure)
//! is dead-lettered and passed. It is delivered again in only two cases.
//! After a manual SQL call to `fvoci.app_outbox_requeue` (there is no CLI
//! or UI), it is retried out of order. When `--recover-outbox`, which
//! deletes every failure row, replays a window that contains it, it is
//! delivered in `(xact, seq)` order with the rest of that window.

mod dispatcher;

pub use dispatcher::{
    spawn_outbox_dispatcher, spawn_outbox_dispatcher_backend, DeliveryMode, OutboxConsumer,
    OutboxDispatcherHandle, OutboxDispatcherSettings, OutboxProcessError,
};
