//! Transactions that write the links of invitations, and those that delete what the
//! links hang from, wait for each other instead of each holding what the other waits
//! for (`crates/store/src/link_locks.rs`). On PostgreSQL each test stops one
//! transaction once it holds a link, a workspace or a board, starts the other, lets
//! the first go once the other waits for it, and asks that both end as they should; on
//! SQLite, whose writers take turns, the two run one after the other.

use fvoci_model::{ContainerRole, DeleteContainersAndBlocks, GuestInvitationsChange, Invitation};
use fvoci_store::{DbType, MailedTo, NewLink, StoreError};

use crate::common::{self, TestStore};
use crate::invitations::{LATER, NOW, link};
use crate::lock_gate::{Gate, ended};

const OFF: GuestInvitationsChange = GuestInvitationsChange { enabled: false };

/// The link made like `parent` for one recipient of its mail, with the token's digest
/// `token_hash`.
fn mailed_like<'a>(
    workspace_id: &'a str,
    parent: &'a Invitation,
    token_hash: &'a str,
) -> NewLink<'a> {
    NewLink {
        workspace_id,
        guest_of: parent
            .container_id
            .as_deref()
            .map(|id| (id, ContainerRole::Viewer)),
        token_hash,
        created_by: &parent.created_by,
        now: NOW,
        expires_at: LATER,
        max_uses: Some(1),
        mailed: Some(MailedTo {
            parent_id: &parent.id,
            recipient: "someone@example.test",
        }),
    }
}

/// Whether the row of the link `first` comes before that of `second` in its table, as
/// a scan of the whole table meets them.
async fn comes_before(test: &TestStore, first: &str, second: &str) -> bool {
    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    sqlx::query_scalar(
        "SELECT (SELECT ctid FROM invitations WHERE id = $1::uuid) \
         < (SELECT ctid FROM invitations WHERE id = $2::uuid)",
    )
    .bind(first)
    .bind(second)
    .fetch_one(&mut conn)
    .await
    .expect("the rows are found")
}

/// The digests of the tokens of the links that are there and not revoked.
async fn unrevoked(test: &TestStore) -> Vec<String> {
    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    sqlx::query_scalar("SELECT token_hash FROM invitations WHERE revoked_at = 0 ORDER BY 1")
        .fetch_all(&mut conn)
        .await
        .expect("the links are read")
}

/// Revoking a mailed link holds it and then its children; deleting its board deletes
/// them in the order the table holds them, here the child first: the link was used
/// after it was mailed, and its row moved behind the child's.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoking_a_link_and_deleting_its_board_wait_for_each_other() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let board_id = boards[0].id.clone();
    let guest_of = Some((board_id.as_str(), ContainerRole::Viewer));
    let parent = link(&test, &admin, "parent", guest_of, None).await;
    let child = test
        .create_invitation(&mailed_like(&test.workspace_id, &parent, "child"))
        .await
        .expect("the child is stored");
    if common::db_type() != DbType::Postgres {
        let revoked = test.revoke_invitation(&test.workspace_id, &parent.id, None, NOW);
        assert!(revoked.await.expect("revoked"));
        test.delete_container(&board_id, &admin)
            .await
            .expect("deleted");
        let child = test.unended_invitation("child", NOW).await.expect("asked");
        assert_eq!(child, None, "the child went with its board");
        test.teardown().await;
        return;
    }
    let used = format!(
        "UPDATE invitations SET uses = uses + 1 WHERE id = '{}'",
        parent.id
    );
    common::execute_raw(&test.database_connection(), &used).await;
    assert!(
        comes_before(&test, &child.id, &parent.id).await,
        "the deletion meets the child first"
    );

    let mut gate = Gate::before(
        &test,
        "UPDATE",
        "invitations",
        "OLD.parent_id IS NULL AND OLD.revoked_at = 0 AND NEW.revoked_at <> 0",
    )
    .await;
    let deleter = gate.second_store(&test).await;
    let revoker = test.store.clone();
    let (workspace_id, parent_id) = (test.workspace_id.clone(), parent.id.clone());
    let revoke = tokio::spawn(async move {
        revoker
            .revoke_invitation(&workspace_id, &parent_id, None, NOW)
            .await
    });
    let revoking = gate.stopped(&revoke).await;
    let by = admin.clone();
    let delete = tokio::spawn(async move { deleter.delete_container(&board_id, &by).await });
    gate.waits_for(revoking, &delete).await;
    gate.open().await;

    assert!(ended(revoke).await.expect("the link is revoked"));
    ended(delete).await.expect("the board is deleted");
    assert_eq!(
        test.unended_invitation("child", NOW).await.expect("asked"),
        None,
        "the child went with its board"
    );
    test.teardown().await;
}

/// Turning a workspace's guest links off holds them in the order the table holds them;
/// deleting two boards at once deletes the links of each in the order the boards are
/// asked for, here the other way round.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_guest_links_off_and_deleting_their_boards_wait_for_each_other() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 2).await;
    let on_board = |at: usize| Some((boards[at].id.as_str(), ContainerRole::Viewer));
    let first = link(&test, &admin, "first", on_board(0), None).await;
    let second = link(&test, &admin, "second", on_board(1), None).await;
    let both = DeleteContainersAndBlocks {
        containers: vec![boards[1].id.clone(), boards[0].id.clone()],
        blocks: Vec::new(),
    };
    if common::db_type() != DbType::Postgres {
        let turned = test.set_guest_invitations(&test.workspace_id, OFF, &admin);
        turned.await.expect("turned off");
        let deleted = test.delete_containers_and_blocks(&both, &admin);
        deleted.await.expect("deleted");
        test.teardown().await;
        return;
    }
    assert!(
        comes_before(&test, &first.id, &second.id).await,
        "turning off meets the first link first"
    );

    let mut gate = Gate::before(
        &test,
        "UPDATE",
        "invitations",
        "OLD.revoked_at = 0 AND NEW.revoked_at <> 0",
    )
    .await;
    let deleter = gate.second_store(&test).await;
    let turner = test.store.clone();
    let (workspace_id, by) = (test.workspace_id.clone(), admin.clone());
    let turn_off =
        tokio::spawn(async move { turner.set_guest_invitations(&workspace_id, OFF, &by).await });
    let turning = gate.stopped(&turn_off).await;
    let by = admin.clone();
    let delete =
        tokio::spawn(async move { deleter.delete_containers_and_blocks(&both, &by).await });
    gate.waits_for(turning, &delete).await;
    gate.open().await;

    let workspace = ended(turn_off).await.expect("turned off");
    assert!(!workspace.guest_invitations);
    ended(delete).await.expect("the boards are deleted");
    assert_eq!(unrevoked(&test).await, Vec::<String>::new());
    test.teardown().await;
}

/// A link made for one recipient of a mailed link holds the link it is made like;
/// turning the workspace's guest links off holds the workspace and then its links. The
/// new link holds the workspace first: asked for while the links are turned off, it
/// waits, and is not made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_mailed_while_guest_links_are_turned_off_waits_and_is_not_made() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let guest_of = Some((boards[0].id.as_str(), ContainerRole::Viewer));
    let parent = link(&test, &admin, "parent", guest_of, None).await;
    if common::db_type() != DbType::Postgres {
        let turned = test.set_guest_invitations(&test.workspace_id, OFF, &admin);
        turned.await.expect("turned off");
        let made = test
            .create_invitation(&mailed_like(&test.workspace_id, &parent, "child"))
            .await;
        assert!(
            made.as_ref().is_err_and(StoreError::is_not_found),
            "{made:?}"
        );
        test.teardown().await;
        return;
    }

    let mut gate = Gate::before(
        &test,
        "UPDATE",
        "workspaces",
        "OLD.guest_invitations AND NOT NEW.guest_invitations",
    )
    .await;
    let maker = gate.second_store(&test).await;
    let turner = test.store.clone();
    let (workspace_id, by) = (test.workspace_id.clone(), admin.clone());
    let turn_off =
        tokio::spawn(async move { turner.set_guest_invitations(&workspace_id, OFF, &by).await });
    let turning = gate.stopped(&turn_off).await;
    let workspace_id = test.workspace_id.clone();
    let making = tokio::spawn(async move {
        let child = mailed_like(&workspace_id, &parent, "child");
        maker.create_invitation(&child).await
    });
    gate.waits_for(turning, &making).await;
    gate.open().await;

    ended(turn_off).await.expect("turned off");
    let made = ended(making).await;
    assert!(
        made.as_ref().is_err_and(StoreError::is_not_found),
        "{made:?}"
    );
    assert_eq!(unrevoked(&test).await, Vec::<String>::new());
    test.teardown().await;
}

/// A link made for one recipient of a mailed link names the board of the link it is
/// made like; deleting the board holds it, and then deletes that link. The new link
/// holds the board before the link it is made like: asked for while the board is
/// being deleted, it waits, and is not made. So does a guest's link of its own.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_made_while_its_board_is_deleted_waits_and_is_not_made() {
    for made in [Made::Mailed, Made::OfItsOwn] {
        made_while_the_board_is_deleted(made).await;
    }
}

/// How a guest's link to the board of another is made.
#[derive(Debug, Clone, Copy)]
enum Made {
    /// Like the other, for one recipient of its mail.
    Mailed,
    /// Like the other, but for no recipient of mail (`mailed` is none).
    OfItsOwn,
}

/// A guest's link to the board of `parent`, made as `made` says.
fn on_the_board<'a>(workspace_id: &'a str, parent: &'a Invitation, made: Made) -> NewLink<'a> {
    let child = mailed_like(workspace_id, parent, "child");
    match made {
        Made::Mailed => child,
        Made::OfItsOwn => NewLink {
            mailed: None,
            ..child
        },
    }
}

async fn made_while_the_board_is_deleted(made: Made) {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let board_id = boards[0].id.clone();
    let guest_of = Some((board_id.as_str(), ContainerRole::Viewer));
    let parent = link(&test, &admin, "parent", guest_of, None).await;
    if common::db_type() != DbType::Postgres {
        test.delete_container(&board_id, &admin)
            .await
            .expect("deleted");
        let answer = test
            .create_invitation(&on_the_board(&test.workspace_id, &parent, made))
            .await;
        assert!(
            answer.as_ref().is_err_and(StoreError::is_not_found),
            "{answer:?}"
        );
        test.teardown().await;
        return;
    }

    // The deletion stops once it holds the board, as it puts the board in the trash.
    let mut gate = Gate::before(&test, "INSERT", "deleted_containers", "true").await;
    let maker = gate.second_store(&test).await;
    let deleter = test.store.clone();
    let by = admin.clone();
    let delete = tokio::spawn(async move { deleter.delete_container(&board_id, &by).await });
    let deleting = gate.stopped(&delete).await;
    let workspace_id = test.workspace_id.clone();
    let making = tokio::spawn(async move {
        let child = on_the_board(&workspace_id, &parent, made);
        maker.create_invitation(&child).await
    });
    gate.waits_for(deleting, &making).await;
    gate.open().await;

    ended(delete).await.expect("the board is deleted");
    let answer = ended(making).await;
    assert!(
        answer.as_ref().is_err_and(StoreError::is_not_found),
        "{answer:?}"
    );
    assert_eq!(
        unrevoked(&test).await,
        Vec::<String>::new(),
        "no link is left"
    );
    test.teardown().await;
}

/// A link made for one recipient of a mailed link before the workspace's guest links
/// are turned off is held by them: turning them off waits for it, and then revokes it
/// with the others.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_mailed_before_guest_links_are_turned_off_is_revoked_with_them() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let guest_of = Some((boards[0].id.as_str(), ContainerRole::Viewer));
    let parent = link(&test, &admin, "parent", guest_of, None).await;
    if common::db_type() != DbType::Postgres {
        let child = mailed_like(&test.workspace_id, &parent, "child");
        test.create_invitation(&child).await.expect("made");
        let turned = test.set_guest_invitations(&test.workspace_id, OFF, &admin);
        turned.await.expect("turned off");
        let child = test.unended_invitation("child", NOW).await.expect("asked");
        assert_eq!(child, None, "revoked with the others");
        test.teardown().await;
        return;
    }

    // The new link stops once it holds what it hangs from, before it is stored.
    let mut gate = Gate::before(&test, "INSERT", "invitations", "NEW.parent_id IS NOT NULL").await;
    let turner = gate.second_store(&test).await;
    let maker = test.store.clone();
    let workspace_id = test.workspace_id.clone();
    let making = tokio::spawn(async move {
        let child = mailed_like(&workspace_id, &parent, "child");
        maker.create_invitation(&child).await
    });
    let holding = gate.stopped(&making).await;
    let (workspace_id, by) = (test.workspace_id.clone(), admin.clone());
    let turn_off =
        tokio::spawn(async move { turner.set_guest_invitations(&workspace_id, OFF, &by).await });
    gate.waits_for(holding, &turn_off).await;
    gate.open().await;

    ended(making).await.expect("the link is made");
    ended(turn_off).await.expect("turned off");
    assert_eq!(
        unrevoked(&test).await,
        Vec::<String>::new(),
        "revoked with the others"
    );
    test.teardown().await;
}
