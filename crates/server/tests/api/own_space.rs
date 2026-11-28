//! A user's own space through the routes: every account has one, however it was made,
//! works in it as in any workspace, and reaches it alone.

use fvoci_model::{Card, Container, ContainerVisibility, OrgKind, UserWorkspace, WorkspaceRole};
use reqwest::StatusCode;
use serde_json::json;

use crate::boards::new_board;
use crate::common::{Client, ROOT_WS, Setup, TEST_WS};

/// The workspace of the client's own space: the last the user is listed, and the one
/// of its kind.
pub(crate) async fn own_workspace(client: &Client) -> UserWorkspace {
    workspaces_and_own(client).await.1
}

/// The workspaces the client's user is listed, of the organizations people work in,
/// and the one of the user's own space, which is listed after them.
pub(crate) async fn workspaces_and_own(client: &Client) -> (Vec<UserWorkspace>, UserWorkspace) {
    let mut workspaces: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
    let own = workspaces.pop().expect("the list is never empty");
    assert_eq!(
        (own.kind, own.role, own.name.as_str(), own.sso_required),
        (OrgKind::Personal, WorkspaceRole::Admin, "", false),
        "the user's own space, last: {own:?}"
    );
    assert!(
        workspaces
            .iter()
            .all(|workspace| workspace.kind == OrgKind::Shared && workspace.org_id != own.org_id),
        "one space of the user's own: {workspaces:?}"
    );
    (workspaces, own)
}

fn ids(containers: &[Container]) -> Vec<&str> {
    containers.iter().map(|board| board.id.as_str()).collect()
}

/// The account that starts a server's organization and the account a link lets into
/// it each have a space of their own. The first makes a board and a card there and
/// reads them back, by the list, by the identifier and by a search; the second, who
/// works beside the first in the organization, finds nothing of it by any of them,
/// from the first's workspace or from one of their own, and makes nothing there.
#[tokio::test]
async fn an_account_makes_a_board_in_its_own_space_and_nobody_else_finds_it() {
    let th = Setup::native().start().await.init_basic().await;
    let own = own_workspace(&th.client).await;
    let others = own_workspace(&th.client2).await;
    assert_ne!(own.org_id, others.org_id, "an organization for each");
    assert_ne!(own.id, others.id, "a workspace for each");

    let here = format!("/w/{}", own.id);
    th.client.get(&here).await.check_ok();
    let board: Container = th
        .client
        .create_board(&new_board(
            &own.id,
            ContainerVisibility::Private,
            "mine alone",
        ))
        .await
        .created_json();
    assert_eq!(board.workspace_id, own.id);
    let cards = format!("{here}/boards/{}/cards", board.id);
    let card = Card {
        title: "a card of mine".to_owned(),
        ..Card::default()
    };
    let card: Card = th.client.post(&cards, &card).await.created_json();

    let listed: Vec<Container> = th.client.get(&format!("{here}/boards")).await.ok_json();
    assert_eq!(ids(&listed), [board.id.as_str()], "listed");
    let by_id = format!("{here}/containers/{}", board.id);
    let read: Container = th.client.get(&by_id).await.ok_json();
    assert_eq!(read.title, "mine alone", "read by its identifier");
    let read: Vec<Card> = th.client.get(&cards).await.ok_json();
    assert_eq!(read, [card], "its card");
    let search = format!("{here}/boards/search?q=alone");
    let found: Vec<Container> = th.client.get(&search).await.ok_json();
    assert_eq!(ids(&found), [board.id.as_str()], "found");

    // The other account: in the first's workspace nothing is there.
    for path in [&here, &format!("{here}/boards"), &by_id, &cards, &search] {
        th.client2.get(path).await.check_not_found();
    }
    let theirs = new_board(&own.id, ContainerVisibility::Private, "theirs");
    th.client2.create_board(&theirs).await.check_not_found();
    th.client2.post(&cards, &json!({})).await.check_not_found();
    // And from the workspaces they work in, the board is not one of them.
    for theirs in [ROOT_WS, TEST_WS, &format!("/w/{}", others.id)] {
        let by_id = format!("{theirs}/containers/{}", board.id);
        th.client2.get(&by_id).await.check_not_found();
        let found: Vec<Container> = th
            .client2
            .get(&format!("{theirs}/boards/search?q=alone"))
            .await
            .ok_json();
        assert_eq!(ids(&found), [""; 0], "{theirs}");
    }
    // Nor is it the first's to reach from the organization's workspace.
    let elsewhere = format!("{ROOT_WS}/containers/{}", board.id);
    th.client.get(&elsewhere).await.check_not_found();
    th.teardown().await;
}

/// Nobody administers a user's own space, and it has no people: to its owner the
/// routes of an organization answer 404 for it, and under its workspace the routes of
/// invitations, of guests and of the groups of people do. The same routes answer the same user in the
/// organization they started.
#[tokio::test]
async fn an_own_space_has_no_organization_to_administer_and_no_people() {
    let th = Setup::native().start().await.init_basic().await;
    let own = own_workspace(&th.client).await;
    let started: Vec<UserWorkspace> = th.client.get("/users/me/workspaces").await.ok_json();
    let organization = format!("/org/{}", started[0].org_id);
    let space = format!("/org/{}", own.org_id);
    for route in ["", "/members", "/workspaces", "/audit"] {
        th.client
            .get(&format!("{organization}{route}"))
            .await
            .check_ok();
        th.client
            .get(&format!("{space}{route}"))
            .await
            .check_not_found();
    }
    // The organization has no provider and says so; the space says what it says of
    // everything else.
    let provider = th.client.get(&format!("{organization}/oidc")).await;
    provider.check_error(StatusCode::NOT_FOUND, "oidc provider not found");
    let provider = th.client.get(&format!("{space}/oidc")).await;
    provider.check_error(StatusCode::NOT_FOUND, "organization not found");
    let renamed = json!({"name": "renamed"});
    th.client.patch(&space, &renamed).await.check_not_found();
    let workspaces = format!("{space}/workspaces");
    let made = th.client.post(&workspaces, &renamed).await;
    made.check_not_found();

    let here = format!("/w/{}", own.id);
    for route in ["/invitations", "/teams"] {
        th.client.get(&format!("{ROOT_WS}{route}")).await.check_ok();
        th.client
            .get(&format!("{here}{route}"))
            .await
            .check_not_found();
    }
    let nothing = json!({});
    for route in ["/invitations", "/invitations/mail", "/teams"] {
        let posted = th.client.post(&format!("{here}{route}"), &nothing).await;
        posted.check_not_found();
    }
    let guests = format!("{here}/guest-invitations");
    th.client.put(&guests, &nothing).await.check_not_found();
    // Nobody's role changes in a space and nobody is taken out of one: asked of its
    // owner about themselves, the workspace is not there, where the same requests
    // about somebody of the organization are taken.
    let role = json!({"role": "member"});
    let themselves = format!("/members/{}", th.client.me().await.id);
    let owner_put = th.client.put(&format!("{here}{themselves}"), &role).await;
    owner_put.check_not_found();
    let owner_removed = th.client.delete(&format!("{here}{themselves}")).await;
    owner_removed.check_not_found();
    let other = format!("{ROOT_WS}/members/{}", th.client2.me().await.id);
    th.client.put(&other, &role).await.check_ok();
    th.client.delete(&other).await.check_no_content();
    assert_eq!(
        own_workspace(&th.client).await,
        own,
        "the owner is what they were in their space"
    );
    // The workspace itself is there, and its people are its owner.
    th.client.get(&here).await.check_ok();
    th.teardown().await;
}

/// `restoreOrgWorkspace` filters by organization even when an own space's workspace is
/// actually in the trash (`deleted_at IS NOT NULL`): neither the shared organization's admin nor
/// the space's owner can restore it, while the same admin restores a trashed workspace
/// of the shared organization.
#[tokio::test]
async fn restore_org_workspace_refuses_a_trashed_own_space_workspace() {
    let th = Setup::native().start().await.init_basic().await;
    let (org_workspaces, own) = workspaces_and_own(&th.client).await;
    let shared_org_id = &org_workspaces[0].org_id;
    let other_own = own_workspace(&th.client2).await;

    // Positive control: create, trash, and restore a workspace of the shared organization.
    let made: serde_json::Value = th
        .client
        .post(
            &format!("/org/{shared_org_id}/workspaces"),
            &json!({"name": "Extra"}),
        )
        .await
        .created_json();
    let extra_id = made["id"].as_str().expect("workspace id");
    th.client
        .delete(&format!("/org/{shared_org_id}/workspaces/{extra_id}"))
        .await
        .check_no_content();
    let restored = th
        .client
        .post_empty(&format!(
            "/org/{shared_org_id}/workspaces/{extra_id}/restore"
        ))
        .await;
    restored.check_ok();
    assert_eq!(restored.value().get("deletedAt"), None);

    // Put both own spaces' workspaces in the trash directly in the database so a 404
    // cannot be explained by `deleted_at IS NULL`.
    for ws_id in [&own.id, &other_own.id] {
        th.app
            .store()
            .system()
            .execute_unchecked(&format!(
                "UPDATE workspaces SET deleted_at = 1 WHERE id = '{ws_id}'"
            ))
            .await
            .expect("trashed own space workspace");
    }

    // Shared org admin asking under shared_org_id, and owners asking under their space org_id:
    // every restore is 404 and leaves deleted_at = 1.
    for (client, org_id, ws_id) in [
        (&th.client, shared_org_id.as_str(), own.id.as_str()),
        (&th.client, shared_org_id.as_str(), other_own.id.as_str()),
        (&th.client, own.org_id.as_str(), own.id.as_str()),
        (
            &th.client2,
            other_own.org_id.as_str(),
            other_own.id.as_str(),
        ),
    ] {
        client
            .post_empty(&format!("/org/{org_id}/workspaces/{ws_id}/restore"))
            .await
            .check_not_found();
    }
    for ws_id in [&own.id, &other_own.id] {
        let deleted_at = th
            .app
            .store()
            .system()
            .texts_unchecked(&format!(
                "SELECT CAST(deleted_at AS TEXT) FROM workspaces WHERE id = '{ws_id}'"
            ))
            .await
            .expect("read");
        assert_eq!(deleted_at, ["1"], "{ws_id} stayed in the trash");
    }
    th.teardown().await;
}

async fn start_webhook_receiver() -> (
    String,
    tokio::sync::mpsc::UnboundedReceiver<String>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (received_tx, received_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let router = axum::Router::new().route(
        "/hook",
        axum::routing::post(move |body: String| {
            let tx = received_tx.clone();
            async move {
                let _ = tx.send(body);
                axum::http::StatusCode::OK
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("bind webhook receiver");
    let url = format!("http://{}/hook", listener.local_addr().expect("local addr"));
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(async move {
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(async move {
                let _ = stop_rx.await;
            })
            .await;
    });
    (url, received_rx, stop_tx, serving)
}

fn make_views(board_id: &str, count: usize) -> Vec<fvoci_model::Block> {
    (0..count)
        .map(|i| fvoci_model::Block {
            id: fvoci_model::new_id(),
            container_id: board_id.to_owned(),
            block_type: fvoci_model::BlockType::View,
            title: format!("view {i}"),
            ..fvoci_model::Block::default()
        })
        .collect()
}

#[derive(Debug, Clone, Default)]
struct ReadCount(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl fvoci_store::Observer for ReadCount {
    fn called(&self, call: fvoci_store::Call) {
        if call.access == fvoci_store::Access::Read {
            self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
    fn waited_for_connection(&self, _: fvoci_store::DbType, _: std::time::Duration) {}
}

async fn insert_views_counting_reads(
    app: &fvoci_server::app::App,
    board_id: &str,
    user_id: &str,
    count: usize,
    reads: &ReadCount,
) -> usize {
    let mut batch = make_views(board_id, count);
    reads.0.store(0, std::sync::atomic::Ordering::Relaxed);
    app.insert_blocks_unchecked(
        &mut batch,
        user_id,
        fvoci_server::app::ChangeNotification::Suppress,
    )
    .await
    .expect("inserted views");
    assert!(
        app.finish_background(std::time::Duration::from_secs(5))
            .await,
        "background webhooks finished"
    );
    reads.0.load(std::sync::atomic::Ordering::Relaxed)
}

/// When `block_change_webhooks` is configured, an `App` handle entered through
/// `enter_workspace` already knows its organization's kind (0 extra DB reads on
/// `tell_webhooks`), a handle created via `scoped` reads it at most once across many
/// block changes, and re-scoping to a personal organization does not reuse the previous
/// organization's kind.
#[tokio::test]
async fn webhooks_cache_organization_kind_per_handle_and_reset_on_scoped() {
    use std::time::Duration;

    use fvoci_store::Tenant;

    let (url, mut received_rx, stop_tx, serving) = start_webhook_receiver().await;
    let th = Setup::native().start().await.init_basic().await;
    let mut config = th.app.config().clone();
    config.block_change_webhooks = vec![url];
    let (settings, _) = config.check().expect("the configuration with the hook");
    let hooked =
        fvoci_server::app::App::new(settings, th.app.store().clone(), th.app.files().clone())
            .expect("a server with the hook");
    let reads = ReadCount::default();
    assert!(
        th.app.store().observe(Box::new(reads.clone())),
        "install store observer"
    );

    let (org_workspaces, own) = workspaces_and_own(&th.client).await;
    let org_ws = &org_workspaces[0];
    let user_id = th.client.me().await.id;
    let board = th
        .create_board(&org_ws.id, ContainerVisibility::Private)
        .await;

    // 1. A scoped handle reads get_container once + org kind once across 6 blocks (2 reads).
    let scoped = hooked.scoped(Tenant::org(&org_ws.org_id).expect("org id"));
    assert_eq!(
        insert_views_counting_reads(&scoped, &board.id, &user_id, 6, &reads).await,
        2,
        "scoped handle reads get_container once + org kind once for 6 blocks"
    );
    for _ in 0..6 {
        tokio::time::timeout(Duration::from_secs(5), received_rx.recv())
            .await
            .expect("webhook delivered")
            .expect("channel open");
    }

    // 2. A handle from enter_workspace already knows org kind (only 1 get_container read).
    let entered = hooked
        .enter_workspace_as(
            fvoci_server::app::Asker {
                user_id: &user_id,
                making: fvoci_store::Making::with_password(),
                delegation: None,
            },
            &org_ws.id,
        )
        .await
        .expect("enter")
        .expect("member")
        .app;
    assert_eq!(
        insert_views_counting_reads(&entered, &board.id, &user_id, 6, &reads).await,
        1,
        "entered handle already knows org kind (only 1 get_container read)"
    );
    for _ in 0..6 {
        tokio::time::timeout(Duration::from_secs(5), received_rx.recv())
            .await
            .expect("webhook delivered")
            .expect("channel open");
    }

    // 3. Re-scoping the shared-org handle to the personal org resets the cached kind.
    let rescoped_own = entered.scoped(Tenant::org(&own.org_id).expect("own org id"));
    let own_board: Container = th
        .client
        .create_board(&new_board(&own.id, ContainerVisibility::Private, "private"))
        .await
        .created_json();
    assert_eq!(
        insert_views_counting_reads(&rescoped_own, &own_board.id, &user_id, 3, &reads).await,
        2,
        "rescoped own handle reads get_container once + org kind once"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(200), received_rx.recv())
            .await
            .is_err(),
        "no webhook sent for personal organization after rescoping a shared-org handle"
    );

    let _ = stop_tx.send(());
    let _ = serving.await;
    th.teardown().await;
}
