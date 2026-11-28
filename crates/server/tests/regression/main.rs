//! Regression tests: fixed request sequences whose answers are pinned in `golden/`.
//!
//! Recorded HTTP answers are stored under `golden/`. A test fails when an answer differs
//! from its recording. To change an answer on purpose, rerun with
//! `FVOCI_REGRESSION_UPDATE=1` and commit the rewritten golden files with the change.
//!
//! * One test per scenario: 19 scenarios, 517 steps (`golden/<scenario>.json`).
//! * The permission table: 52 routes × 6 kinds of caller × 2 board types = 624 cells
//!   (`golden/permissions_private.txt`, `golden/permissions_open.txt`).
//!
//! Every test starts a server of its own, on SQLite or PostgreSQL as the environment of
//! the store's test support says. The golden files are the same for both.

#[path = "../api/common.rs"]
mod common;

mod accounts;
mod blocks;
mod boards;
mod content;
mod fixtures;
mod golden;
mod harness;
mod permissions;
mod sidebar;
mod trash;

/// One test per scenario; the test's name is the name of its golden file.
macro_rules! scenarios {
    ($($module:ident :: $scenario:ident),* $(,)?) => {
        $(
            #[tokio::test]
            async fn $scenario() {
                harness::record(stringify!($scenario), $module::$scenario).await;
            }
        )*
    };
}

scenarios![
    accounts::system,
    accounts::auth,
    accounts::workspaces,
    accounts::admins,
    accounts::settings,
    accounts::cookies,
    accounts::login_limits,
    accounts::password_limits,
    boards::boards,
    boards::members,
    blocks::cards,
    sidebar::categories,
    sidebar::public_links,
    sidebar::subscriptions,
    sidebar::search,
    content::templates,
    content::files,
    content::archive,
    trash::trash,
];

#[tokio::test]
async fn permissions_private() {
    permissions::record("permissions_private", "private").await;
}

#[tokio::test]
async fn permissions_open() {
    permissions::record("permissions_open", "open").await;
}
