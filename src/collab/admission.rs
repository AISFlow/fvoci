use collab_engine::limits::room_memory_reservation_bytes;
use collab_engine::process::sum_live_children_rss_bytes;
use uuid::Uuid;

/// True when starting a room would exceed the configured aggregate helper RSS budget.
pub fn memory_budget_exceeded(budget_bytes: u64, persisted_bytes: u64) -> bool {
    let reservation = room_memory_reservation_bytes(persisted_bytes);
    sum_live_children_rss_bytes().saturating_add(reservation) > budget_bytes
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
