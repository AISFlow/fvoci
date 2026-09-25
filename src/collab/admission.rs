use collab_engine::limits::room_memory_reservation_bytes;
use collab_engine::process::sum_live_children_rss_bytes;

/// True when starting a room would exceed the configured aggregate helper RSS budget.
pub fn memory_budget_exceeded(budget_bytes: u64, persisted_bytes: u64) -> bool {
    let reservation = room_memory_reservation_bytes(persisted_bytes);
    sum_live_children_rss_bytes().saturating_add(reservation) > budget_bytes
}
