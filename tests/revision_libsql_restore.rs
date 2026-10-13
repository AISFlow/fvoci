#![cfg(feature = "db-tests")]

#[path = "revision_restore/fixture.rs"]
#[allow(dead_code)]
mod restore_fixture;

// Explicitly selected with the pinned official sqld; never in CI cohorts.
// Missing FVOCI_TEST_SQLD fails the fixture instead of skipping SDK coverage.
use restore_fixture::{contract, Case};

#[tokio::test]
async fn libsql_on_restore_commit_replay_peer_and_restart() {
    contract(true, Case::Commit).await;
}
#[tokio::test]
async fn libsql_on_restore_committed_ambiguity() {
    contract(true, Case::Ambiguous).await;
}
#[tokio::test]
async fn libsql_on_restore_revoked_before_apply() {
    contract(true, Case::Revoke).await;
}
#[tokio::test]
async fn libsql_on_restore_late_failure_rolls_back() {
    contract(true, Case::Rollback).await;
}
