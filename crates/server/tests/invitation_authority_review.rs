//! Independent PostgreSQL regressions for changes of admission during pending writes.

#[path = "api/common.rs"]
mod common;

use std::time::Duration;

use fvoci_model::{Container, Invitation, new_id};
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, Store, StoreError};
use reqwest::StatusCode;
use serde_json::json;
use tokio::task::JoinHandle;

use common::{Client, PASSWORD, ROOT_WORKSPACE_ID, Setup};

/// Waits for a database-observable synchronization point, rather than guessing how
/// long another request takes. Queries below concern only this test's own gate PID.
async fn wait_for_row(store: &Store, query: &str) -> String {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(found) = store
                .texts_unchecked(query)
                .await
                .expect("the synchronization point is inspected")
                .into_iter()
                .next()
            {
                return found;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the request reaches the synchronization point")
}

/// Holds one row while another request reaches its lock. Cancellation releases this
/// test's gate, not the request being tested. The sleep bounds an abandoned gate.
struct RowGate {
    store: Store,
    pid: String,
    task: JoinHandle<Result<Vec<String>, StoreError>>,
}

impl RowGate {
    async fn hold(store: &Store, table: &str, id: &str) -> Self {
        Self::hold_where(store, table, &format!("id = '{id}'")).await
    }

    /// Holds the rows of `table` that `condition` picks.
    async fn hold_where(store: &Store, table: &str, condition: &str) -> Self {
        let store = store.system();
        let marker = format!("review_gate_{}", new_id());
        let query = format!(
            "SELECT pg_sleep(30)::text FROM \
             (SELECT 1 FROM {table} WHERE {condition} FOR NO KEY UPDATE) AS held \
             /* {marker} */"
        );
        let connection = store.clone();
        let task = tokio::spawn(async move { connection.texts_unchecked(&query).await });
        let pid = wait_for_row(
            &store,
            &format!(
                "SELECT pid::text FROM pg_stat_activity \
                 WHERE wait_event = 'PgSleep' AND query LIKE '%{marker}%' \
                 AND pid <> pg_backend_pid()"
            ),
        )
        .await;
        Self { store, pid, task }
    }

    async fn wait_for_waiter(&self) {
        wait_for_row(
            &self.store,
            &format!(
                "SELECT pid::text FROM pg_stat_activity \
                 WHERE {} = ANY(pg_blocking_pids(pid))",
                self.pid
            ),
        )
        .await;
    }

    async fn release(self) {
        let cancelled = self
            .store
            .texts_unchecked(&format!("SELECT pg_cancel_backend({})::text", self.pid))
            .await
            .expect("this test's gate query is cancelled");
        assert_eq!(cancelled, ["true"], "the gate was still held");
        assert!(
            self.task.await.expect("the gate task completes").is_err(),
            "cancellation rolls back the gate transaction and releases its row"
        );
    }
}

fn member_path(user_id: &str) -> String {
    format!("/w/{ROOT_WORKSPACE_ID}/members/{user_id}")
}

async fn private_board(client: &Client, title: &str) -> Container {
    client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
            &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": title}),
        )
        .await
        .created_json()
}

#[tokio::test]
async fn acceptance_rechecks_the_inviter_after_waiting_for_its_link() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let maker = th.client2.me().await.id;
    th.client
        .put(&member_path(&maker), &json!({"role": "admin"}))
        .await
        .check_ok();
    let link: Invitation = th
        .client2
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role": "member"}),
        )
        .await
        .created_json();
    let token = link.token.expect("a token is handed out once");
    let gate = RowGate::hold(th.app.store(), "invitations", &link.id).await;
    let nobody = th.anonymous();
    let accepting_token = token.clone();
    let accepting = tokio::spawn(async move {
        nobody
            .post(
                "/invitations/accept",
                &json!({
                    "token": accepting_token,
                    "account": {
                        "username": "after-removal",
                        "email": "after-removal@example.com",
                        "password": PASSWORD,
                    },
                }),
            )
            .await
    });
    gate.wait_for_waiter().await;
    let removed = th.client.delete(&member_path(&maker)).await;
    let preview = th
        .anonymous()
        .post("/invitations/preview", &json!({"token": token}))
        .await;
    gate.release().await;
    let accepted = accepting.await.expect("the acceptance request completes");
    let memberships = th
        .app
        .workspace_members(ROOT_WORKSPACE_ID)
        .await
        .expect("members");
    th.teardown().await;

    removed.check_no_content();
    preview.check_status(StatusCode::NOT_FOUND);
    assert_eq!(
        accepted.status,
        StatusCode::NOT_FOUND,
        "a removed inviter's link must not admit a new account: {memberships:?}"
    );
    assert!(
        memberships
            .iter()
            .all(|member| member.username != "after-removal"),
        "the refused invitation must leave no new membership"
    );
}

#[derive(Debug, Clone, Copy)]
enum MembershipLoss {
    Removed,
    MadeGuest,
}

async fn a_new_board_keeps_an_admin_during(loss: MembershipLoss) {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let admin = th.client.me().await.id;
    let leaving = th.client2.me().await.id;
    let existing = private_board(&th.client2, "Existing board").await;
    let gate = RowGate::hold(th.app.store(), "containers", &existing.id).await;
    let manager = th.client.clone();
    let changing = tokio::spawn(async move {
        match loss {
            MembershipLoss::Removed => manager.delete(&member_path(&leaving)).await,
            MembershipLoss::MadeGuest => {
                manager
                    .put(&member_path(&leaving), &json!({"role": "guest"}))
                    .await
            }
        }
    });
    // The change holds the workspace and has already enumerated the user's boards.
    gate.wait_for_waiter().await;
    let created = private_board(&th.client2, "Created during the membership change").await;
    gate.release().await;
    let changed = changing.await.expect("the membership change completes");
    let members = th
        .app
        .store()
        .get_members_for_container(&created.id)
        .await
        .expect("the new board's roles are read");
    th.teardown().await;

    changed.check_status(match loss {
        MembershipLoss::Removed => StatusCode::NO_CONTENT,
        MembershipLoss::MadeGuest => StatusCode::OK,
    });
    assert!(
        members
            .iter()
            .any(|member| member.user_id == admin
                && member.role == fvoci_model::ContainerRole::Admin),
        "the workspace administrator must take over the new board during {loss:?}: {members:?}"
    );
}

#[tokio::test]
async fn a_board_created_during_member_removal_is_handed_over() {
    a_new_board_keeps_an_admin_during(MembershipLoss::Removed).await;
}

#[tokio::test]
async fn a_board_created_during_guest_demotion_is_handed_over() {
    a_new_board_keeps_an_admin_during(MembershipLoss::MadeGuest).await;
}

/// A board made once a removal has listed its maker's boards, while the removal still
/// holds the maker's membership: made at once, the removal's last sweep of the maker's
/// roles took its only admin away. It waits for the removal now and is refused, and
/// is never left without an admin who works in the workspace.
#[tokio::test]
async fn a_board_made_after_a_removal_listed_the_boards_keeps_an_admin() {
    const TITLE: &str = "Made after the boards were listed";
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let admin = th.client.me().await.id;
    let leaving = th.client2.me().await.id;
    let existing = private_board(&th.client2, "Existing board").await;
    // The removal lists the boards, then waits at the maker's role on one of them.
    let gate = RowGate::hold_where(
        th.app.store(),
        "container_members",
        &format!("container_id = '{}' AND user_id = '{leaving}'", existing.id),
    )
    .await;
    let manager = th.client.clone();
    let removing = tokio::spawn(async move { manager.delete(&member_path(&leaving)).await });
    gate.wait_for_waiter().await;

    let maker = th.client2.clone();
    let creating = tokio::spawn(async move {
        maker
            .post(
                &format!("/w/{ROOT_WORKSPACE_ID}/boards"),
                &json!({"workspaceId": ROOT_WORKSPACE_ID, "visibility": "private", "title": TITLE}),
            )
            .await
    });
    // Made at once, or waiting behind the removal, which waits for the gate.
    let store = th.app.store().system();
    let behind_the_gate = format!(
        "SELECT count(*)::text FROM pg_stat_activity AS a WHERE {gate} = \
         ANY(pg_blocking_pids(a.pid)) OR EXISTS (SELECT 1 FROM pg_stat_activity AS r \
         WHERE {gate} = ANY(pg_blocking_pids(r.pid)) AND r.pid = ANY(pg_blocking_pids(a.pid)))",
        gate = gate.pid
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !creating.is_finished() {
            let waiting = store
                .texts_unchecked(&behind_the_gate)
                .await
                .expect("the waiters are counted");
            let count = waiting.first().and_then(|count| count.parse::<u32>().ok());
            if count.is_some_and(|count| count >= 2) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the board is made, or waits");
    gate.release().await;
    let removed = removing.await.expect("the removal completes");
    let created = creating.await.expect("the making of the board completes");
    let boards = store
        .texts_unchecked(&format!(
            "SELECT id::text FROM containers WHERE title = '{TITLE}'"
        ))
        .await
        .expect("the boards of the title are read");
    let admins = match boards.first() {
        Some(board_id) => store
            .texts_unchecked(&format!(
                "SELECT user_id::text FROM container_members WHERE container_id = '{board_id}' \
                 AND role = 'admin'"
            ))
            .await
            .expect("the board's admins are read"),
        None => Vec::new(),
    };
    th.teardown().await;

    removed.check_status(StatusCode::NO_CONTENT);
    if created.status.is_success() {
        assert_eq!(
            admins,
            [admin],
            "the board passes to who took its maker out"
        );
    } else {
        created.check_status(StatusCode::NOT_FOUND);
        assert_eq!(
            boards,
            Vec::<String>::new(),
            "nothing of the refused board is stored"
        );
    }
}
