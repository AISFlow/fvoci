use std::sync::{Arc, Mutex, PoisonError};

use collab_engine::limits::room_memory_reservation_bytes;
use collab_engine::process::sum_live_children_rss_bytes;
use uuid::Uuid;

/// Aggregate helper memory budget for one hub's room starts.
///
/// A room's helper is spawned only at its first load, so live helper RSS does
/// not yet include a room that was admitted but has not loaded. The ledger
/// holds such a room's estimate ([`room_memory_reservation_bytes`]) from
/// admission until that first load, so overlapping starts for different
/// documents are checked against each other and not only against helpers that
/// already exist.
///
/// The check and the record happen under one lock, and the live RSS is read
/// inside it: a room returns its reservation only after its load finished, so
/// a read under the lock counts every room between admission and its first
/// load either as outstanding or through its helper's RSS. A loaded room whose
/// helper is being recycled, or whose respawn was refused, is counted by
/// neither until a helper loads again. The lock covers one synchronous `/proc`
/// read per live helper, never an await.
///
/// A room that was admitted but never loads (the join that started it was
/// denied, or every load failed) keeps its reservation until the room is torn
/// down, normally by idle eviction. That errs toward refusing.
pub struct MemoryLedger {
    budget_bytes: u64,
    outstanding_bytes: Mutex<u64>,
}

impl MemoryLedger {
    pub fn new(budget_bytes: u64) -> Arc<Self> {
        Arc::new(Self {
            budget_bytes,
            outstanding_bytes: Mutex::new(0),
        })
    }

    /// Admit a room whose persisted collab state is `persisted_bytes`, or
    /// return `None` when live helper RSS plus outstanding reservations plus
    /// this room's estimate would exceed the budget. Dropping the returned
    /// reservation gives the estimate back.
    pub fn try_reserve(self: &Arc<Self>, persisted_bytes: u64) -> Option<MemoryReservation> {
        self.try_reserve_against(persisted_bytes, sum_live_children_rss_bytes)
    }

    fn try_reserve_against(
        self: &Arc<Self>,
        persisted_bytes: u64,
        live_rss_bytes: impl FnOnce() -> u64,
    ) -> Option<MemoryReservation> {
        let bytes = room_memory_reservation_bytes(persisted_bytes);
        let mut outstanding = self
            .outstanding_bytes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let committed = live_rss_bytes().saturating_add(*outstanding);
        if committed.saturating_add(bytes) > self.budget_bytes {
            return None;
        }
        *outstanding = outstanding.saturating_add(bytes);
        Some(MemoryReservation {
            ledger: self.clone(),
            bytes,
        })
    }

    #[cfg(test)]
    fn outstanding(&self) -> u64 {
        *self
            .outstanding_bytes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// One admitted room's memory estimate, held by its actor until the first
/// successful helper load (or until the room is dropped).
pub struct MemoryReservation {
    ledger: Arc<MemoryLedger>,
    bytes: u64,
}

impl Drop for MemoryReservation {
    fn drop(&mut self) {
        let mut outstanding = self
            .ledger
            .outstanding_bytes
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *outstanding = outstanding.saturating_sub(self.bytes);
    }
}

/// Record why a join failed with an infrastructure (not access) error before the
/// error is collapsed into `JoinError`. Logs the sqlx error kind, the SQLSTATE and
/// server message for database errors, and the room ids; never bind values,
/// credentials or the connection string.
pub fn warn_join_db_error(
    site: &'static str,
    workspace_id: Uuid,
    document_id: Uuid,
    err: &sqlx::Error,
) {
    let kind = sqlx_error_kind(err);
    match err {
        sqlx::Error::Database(db) => tracing::warn!(
            site,
            %workspace_id,
            %document_id,
            kind,
            sqlstate = db.code().as_deref().unwrap_or(""),
            message = db.message(),
            "collab join database error"
        ),
        other => tracing::warn!(
            site,
            %workspace_id,
            %document_id,
            kind,
            error = %other,
            "collab join database error"
        ),
    }
}

fn sqlx_error_kind(err: &sqlx::Error) -> &'static str {
    match err {
        sqlx::Error::Configuration(_) => "configuration",
        sqlx::Error::Database(_) => "database",
        sqlx::Error::Io(_) => "io",
        sqlx::Error::Tls(_) => "tls",
        sqlx::Error::Protocol(_) => "protocol",
        sqlx::Error::RowNotFound => "row_not_found",
        sqlx::Error::TypeNotFound { .. } => "type_not_found",
        sqlx::Error::ColumnIndexOutOfBounds { .. } => "column_index_out_of_bounds",
        sqlx::Error::ColumnNotFound(_) => "column_not_found",
        sqlx::Error::ColumnDecode { .. } => "column_decode",
        sqlx::Error::Encode(_) => "encode",
        sqlx::Error::Decode(_) => "decode",
        sqlx::Error::AnyDriverError(_) => "any_driver",
        sqlx::Error::PoolTimedOut => "pool_timed_out",
        sqlx::Error::PoolClosed => "pool_closed",
        sqlx::Error::WorkerCrashed => "worker_crashed",
        sqlx::Error::Migrate(_) => "migrate",
        sqlx::Error::InvalidArgument(_) => "invalid_argument",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use collab_engine::limits::MIN_ROOM_MEMORY_RESERVATION_BYTES;

    use super::*;

    const MIB: u64 = 1024 * 1024;

    #[test]
    fn reservations_count_against_the_budget_until_dropped() {
        let ledger = MemoryLedger::new(2 * MIN_ROOM_MEMORY_RESERVATION_BYTES + MIB);
        let first = ledger.try_reserve_against(0, || 0).expect("first fits");
        let second = ledger.try_reserve_against(0, || 0).expect("second fits");
        assert_eq!(ledger.outstanding(), 2 * MIN_ROOM_MEMORY_RESERVATION_BYTES);
        assert!(
            ledger.try_reserve_against(0, || 0).is_none(),
            "a third start must see the two outstanding reservations"
        );
        drop(first);
        assert_eq!(ledger.outstanding(), MIN_ROOM_MEMORY_RESERVATION_BYTES);
        let third = ledger
            .try_reserve_against(0, || 0)
            .expect("a dropped reservation frees its share");
        drop((second, third));
        assert_eq!(ledger.outstanding(), 0);
    }

    #[test]
    fn live_rss_and_persisted_factor_count_toward_refusal() {
        let ledger = MemoryLedger::new(MIN_ROOM_MEMORY_RESERVATION_BYTES + MIB);
        assert!(
            ledger.try_reserve_against(0, || 2 * MIB).is_none(),
            "live helper RSS plus the floor exceeds the budget"
        );
        let persisted = 2 * MIB;
        assert!(room_memory_reservation_bytes(persisted) > MIN_ROOM_MEMORY_RESERVATION_BYTES);
        assert!(
            ledger.try_reserve_against(persisted, || 0).is_none(),
            "the reservation scales with persisted bytes"
        );
        assert_eq!(ledger.outstanding(), 0, "a refusal records nothing");
        assert!(ledger.try_reserve_against(0, || MIB).is_some());
    }
}
