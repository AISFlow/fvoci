//! Who is in a workspace, in the store: the last administrator stays, whoever asks and
//! however many ask at once, and a board is not left without an admin.

use std::sync::Arc;

use fvoci_model::{ContainerMember, ContainerRole, WorkspaceRole};
use fvoci_store::{MembershipChange, Store};
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common::{self, TestStore};

/// Makes a member of the test's workspace an administrator of it.
async fn administrator(test: &TestStore, username: &str) -> String {
    let user_id = common::user(test, username).await;
    let made = test
        .change_workspace_role(
            &test.workspace_id,
            &user_id,
            (WorkspaceRole::Member, WorkspaceRole::Admin),
            &user_id,
        )
        .await
        .expect("the role is changed");
    assert!(matches!(made, MembershipChange::Done { .. }), "{made:?}");
    user_id
}

async fn roles(test: &TestStore) -> Vec<(String, WorkspaceRole)> {
    test.get_workspace_members(&test.workspace_id)
        .await
        .expect("the members")
        .into_iter()
        .map(|member| (member.username, member.role))
        .collect()
}

/// Of two administrators who take each other out of the workspace at once, or make each
/// other a member, one is refused: the workspace keeps an administrator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_administrators_who_take_each_other_away_at_once_one_stays() {
    const ROUNDS: usize = 8;
    for round in 0..ROUNDS {
        let test = common::setup().await;
        // The first user of the instance administers the workspace already.
        let first = common::user(&test, "first").await;
        let second = administrator(&test, "second").await;
        let barrier = Arc::new(Barrier::new(2));
        let mut racers = JoinSet::new();
        for (target, by) in [
            (first.clone(), second.clone()),
            (second.clone(), first.clone()),
        ] {
            let store: Store = test.store.clone();
            let workspace_id = test.workspace_id.clone();
            let barrier = Arc::clone(&barrier);
            racers.spawn(async move {
                barrier.wait().await;
                if round % 2 == 0 {
                    store
                        .remove_workspace_member(&workspace_id, &target, &by)
                        .await
                } else {
                    store
                        .change_workspace_role(
                            &workspace_id,
                            &target,
                            (WorkspaceRole::Admin, WorkspaceRole::Member),
                            &by,
                        )
                        .await
                }
            });
        }
        let outcomes: Vec<MembershipChange> = racers
            .join_all()
            .await
            .into_iter()
            .map(|outcome| outcome.expect("the store answers"))
            .collect();
        let done = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, MembershipChange::Done { .. }))
            .count();
        assert_eq!(done, 1, "round {round}: {outcomes:?}");
        assert!(
            outcomes.contains(&MembershipChange::LastAdmin),
            "round {round}: {outcomes:?}"
        );
        let administrators = roles(&test)
            .await
            .into_iter()
            .filter(|(_, role)| *role == WorkspaceRole::Admin)
            .count();
        assert_eq!(administrators, 1, "round {round}: one is left");

        test.teardown().await;
    }
}

/// Who is taken out leaves no role on the boards of the workspace, and a board they
/// alone administered has who took them out as its admin; somebody who is no member,
/// and a workspace that is not there, change nothing.
#[tokio::test]
async fn who_is_taken_out_leaves_their_boards_with_an_admin() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let leaving = common::user(&test, "leaving").await;
    let other = common::user(&test, "other").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &leaving, 3).await;
    let (alone, with_another, with_a_guest_admin) = (&boards[0].id, &boards[1].id, &boards[2].id);
    let add = async |board_id: &str, user_id: &str, role: ContainerRole| {
        let member = ContainerMember::new(board_id.to_owned(), user_id.to_owned(), role);
        test.add_member(&member)
            .await
            .expect("a member of the board");
    };
    for board_id in [alone, with_another, with_a_guest_admin] {
        add(board_id, &leaving, ContainerRole::Admin).await;
    }
    add(with_another, &other, ContainerRole::Admin).await;
    add(alone, &other, ContainerRole::Editor).await;
    // An admin flag on a guest of the workspace is no admin who can act as one.
    let guest = common::user(&test, "guest").await;
    add(with_a_guest_admin, &guest, ContainerRole::Admin).await;
    test.execute_unchecked(&format!(
        "UPDATE workspace_members SET role = 'guest' WHERE user_id = '{guest}'"
    ))
    .await
    .expect("a guest of the workspace");

    for (workspace_id, user_id) in [
        (test.workspace_id.clone(), fvoci_model::new_id()),
        (fvoci_model::new_id(), leaving.clone()),
    ] {
        let nothing = test
            .remove_workspace_member(&workspace_id, &user_id, &admin)
            .await
            .expect("the store answers");
        assert_eq!(nothing, MembershipChange::NotAMember);
    }

    let removed = test
        .remove_workspace_member(&test.workspace_id, &leaving, &admin)
        .await
        .expect("the member is taken out");
    let mut expected = vec![alone.clone(), with_a_guest_admin.clone()];
    expected.sort();
    assert_eq!(
        removed,
        MembershipChange::Done {
            taken_over: expected
        }
    );
    let role = async |board_id: &str, user_id: &str| {
        test.get_member_for_container(board_id, user_id)
            .await
            .ok()
            .map(|member| member.role)
    };
    for board_id in [alone, with_another, with_a_guest_admin] {
        assert_eq!(role(board_id, &leaving).await, None, "no role is left");
    }
    assert_eq!(role(alone, &admin).await, Some(ContainerRole::Admin));
    assert_eq!(
        role(with_a_guest_admin, &admin).await,
        Some(ContainerRole::Admin)
    );
    assert_eq!(role(with_another, &admin).await, None, "it had an admin");
    assert_eq!(role(alone, &other).await, Some(ContainerRole::Editor));
    assert_eq!(
        roles(&test).await,
        [
            ("admin".to_owned(), WorkspaceRole::Admin),
            ("other".to_owned(), WorkspaceRole::Member),
            ("guest".to_owned(), WorkspaceRole::Guest),
        ],
        "the oldest membership first"
    );
    // Taken out, they are still an account, and a member of the organization.
    test.get_user_by_id(&leaving).await.expect("the account");

    test.teardown().await;
}

/// A change of role is made of what the member is when it is stored: asked from
/// something they are not any more, it changes nothing and says what they are.
#[tokio::test]
async fn a_change_of_role_from_what_a_member_is_no_longer_changes_nothing() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let member = common::user(&test, "member").await;
    let made_a_guest = test
        .change_workspace_role(
            &test.workspace_id,
            &member,
            (WorkspaceRole::Member, WorkspaceRole::Guest),
            &admin,
        )
        .await
        .expect("the role is changed");
    assert!(matches!(made_a_guest, MembershipChange::Done { .. }));
    // Somebody who read "a member" before that asks to make them an administrator.
    let stale = test
        .change_workspace_role(
            &test.workspace_id,
            &member,
            (WorkspaceRole::Member, WorkspaceRole::Admin),
            &admin,
        )
        .await
        .expect("the store answers");
    assert_eq!(stale, MembershipChange::RoleIs(WorkspaceRole::Guest));
    assert_eq!(
        roles(&test).await,
        [
            ("admin".to_owned(), WorkspaceRole::Admin),
            ("member".to_owned(), WorkspaceRole::Guest),
        ]
    );

    test.teardown().await;
}

/// A board in the trash comes back with an admin who can act as one: who brings it
/// back, where its own are gone from the workspace or are its guests by then; and a
/// guest comes back on it as an editor at most.
#[tokio::test]
async fn a_board_comes_back_from_the_trash_with_an_admin() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let leaving = common::user(&test, "leaving").await;
    let lowered = common::user(&test, "lowered").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 3).await;
    let (of_the_leaver, of_the_guest, of_both_and_the_admin) =
        (&boards[0].id, &boards[1].id, &boards[2].id);
    let add = async |board_id: &str, user_id: &str| {
        let member = ContainerMember::new(
            board_id.to_owned(),
            user_id.to_owned(),
            ContainerRole::Admin,
        );
        test.add_member(&member)
            .await
            .expect("an admin of the board");
    };
    add(of_the_leaver, &leaving).await;
    add(of_the_guest, &lowered).await;
    for user_id in [&leaving, &lowered, &admin] {
        add(of_both_and_the_admin, user_id).await;
    }
    for board_id in [of_the_leaver, of_the_guest, of_both_and_the_admin] {
        test.delete_container(board_id, &admin)
            .await
            .expect("the board goes to the trash");
    }
    test.remove_workspace_member(&test.workspace_id, &leaving, &admin)
        .await
        .expect("the member is taken out");
    test.change_workspace_role(
        &test.workspace_id,
        &lowered,
        (WorkspaceRole::Member, WorkspaceRole::Guest),
        &admin,
    )
    .await
    .expect("the member is made a guest");
    for board_id in [of_the_leaver, of_the_guest, of_both_and_the_admin] {
        test.undelete_container(board_id, &admin)
            .await
            .expect("the board comes back");
    }

    let role = async |board_id: &str, user_id: &str| {
        test.get_member_for_container(board_id, user_id)
            .await
            .ok()
            .map(|member| member.role)
    };
    assert_eq!(role(of_the_leaver, &leaving).await, None);
    assert_eq!(
        role(of_the_leaver, &admin).await,
        Some(ContainerRole::Admin),
        "its admin left: who brought it back"
    );
    assert_eq!(
        role(of_the_guest, &lowered).await,
        Some(ContainerRole::Editor)
    );
    assert_eq!(
        role(of_the_guest, &admin).await,
        Some(ContainerRole::Admin),
        "its admin is a guest now: who brought it back"
    );
    assert_eq!(role(of_both_and_the_admin, &leaving).await, None);
    assert_eq!(
        role(of_both_and_the_admin, &lowered).await,
        Some(ContainerRole::Editor)
    );
    assert_eq!(
        role(of_both_and_the_admin, &admin).await,
        Some(ContainerRole::Admin)
    );

    test.teardown().await;
}
