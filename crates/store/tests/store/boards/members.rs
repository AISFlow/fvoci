//! Who is a member of a board, with what role, and how a membership is added, changed and
//! taken away, also by many at once.

use super::*;

#[tokio::test]
async fn save_member() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    // "should correctly create a member"
    {
        let member = ContainerMember {
            user_id: user_id.clone(),
            container_id: board_id.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        };

        test.save_member(&member)
            .await
            .expect("the member is saved");
        // Read back the stored membership to verify its role.
        let stored = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect("the member exists");
        assert_eq!(user_id, stored.user_id, "the member");
        assert_eq!(board_id, stored.container_id, "the member's board");
        assert_eq!(
            stored.role,
            fvoci_model::ContainerRole::Admin,
            "the member administers the board"
        );
    }

    // "should correctly update a member"
    {
        let member = ContainerMember {
            user_id: user_id.clone(),
            container_id: board_id.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        };

        test.save_member(&member)
            .await
            .expect("the member is saved");
        let stored = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect("the member exists");
        assert_eq!(user_id, stored.user_id, "the member");
        assert_eq!(board_id, stored.container_id, "the member's board");
        assert_eq!(
            stored.role,
            fvoci_model::ContainerRole::Editor,
            "an editor now"
        );

        // Updating an existing membership replaces it in place rather than duplicating it.
        let members = test
            .get_members_for_container(&board_id)
            .await
            .expect("the members are read");
        assert_eq!(members, [stored], "one membership, the updated one");
    }

    test.teardown().await;
}

#[tokio::test]
async fn get_member_for_container() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    // "should return an error not found for nonexisting membership"
    {
        let error = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect_err("there is no such membership");
        assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
        assert!(
            error.is_not_found(),
            "Should be ErrNotFound compatible error"
        );
        assert_eq!(
            error.to_string(),
            format!("{{board member BoardID={board_id} UserID={user_id}}} not found"),
            "the error names the board and the user"
        );
    }

    // "should return the membership if exists"
    {
        let member = ContainerMember {
            user_id: user_id.clone(),
            container_id: board_id.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        };
        test.save_member(&member)
            .await
            .expect("the member is saved");

        let stored = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect("the member exists");
        assert_eq!(user_id, stored.user_id, "the member");
        assert_eq!(board_id, stored.container_id, "the member's board");
        assert_eq!(
            stored.role,
            fvoci_model::ContainerRole::Admin,
            "the member administers the board"
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn get_members_for_container() {
    let test = common::setup().await;
    let user_id_1 = common::user(&test, "user-11").await;
    let user_id_2 = common::user(&test, "user-12").await;
    let user_id_3 = common::user(&test, "user-13").await;
    let workspace_id = common::workspace(&test).await;
    let board_id_1 = stored_board(&test, &workspace_id, &user_id_1).await;
    let board_id_2 = stored_board(&test, &workspace_id, &user_id_1).await;

    // "should return empty list if there are no members on a board"
    {
        let members = test
            .get_members_for_container(&board_id_1)
            .await
            .expect("the query succeeds");
        assert!(members.is_empty(), "got {members:?}");
    }

    // "should return the members of the board"
    {
        let member1 = ContainerMember {
            container_id: board_id_1.clone(),
            user_id: user_id_1.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        };
        test.save_member(&member1).await.expect("member 1 is saved");

        let member2 = ContainerMember {
            container_id: board_id_1.clone(),
            user_id: user_id_2.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        };
        test.save_member(&member2).await.expect("member 2 is saved");

        let member3 = ContainerMember {
            container_id: board_id_2.clone(),
            user_id: user_id_3.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        };
        test.save_member(&member3).await.expect("member 3 is saved");

        let board1_members = test
            .get_members_for_container(&board_id_1)
            .await
            .expect("the query succeeds");
        assert_eq!(board1_members.len(), 2, "got {board1_members:?}");
        assert_eq!(
            sorted_user_ids(&board1_members),
            sorted([&user_id_1, &user_id_2]),
            "the members of board 1"
        );

        let board2_members = test
            .get_members_for_container(&board_id_2)
            .await
            .expect("the query succeeds");
        assert_eq!(board2_members.len(), 1, "got {board2_members:?}");
        assert_eq!(
            sorted_user_ids(&board2_members),
            [user_id_3],
            "the member of board 2"
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn get_members_for_user() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;

    // "should return empty list if there are no memberships for a user"
    {
        let members = test
            .get_members_for_user(&user_id)
            .await
            .expect("the query succeeds");
        assert!(members.is_empty(), "got {members:?}");
    }

    test.teardown().await;
}

#[tokio::test]
async fn delete_member() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    // "should return nil if deleting a nonexistent member"
    {
        test.delete_member(&board_id, &user_id)
            .await
            .expect("deleting a member that does not exist succeeds");
    }

    // "should correctly delete a member"
    {
        let member = ContainerMember {
            user_id: user_id.clone(),
            container_id: board_id.clone(),
            role: fvoci_model::ContainerRole::Admin,
            synthetic: false,
            inherited_role: None,
        };
        test.save_member(&member)
            .await
            .expect("the member is saved");

        test.delete_member(&board_id, &user_id)
            .await
            .expect("the member is deleted");

        let error = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect_err("the membership is gone");
        assert!(
            error.is_not_found(),
            "Should be ErrNotFound compatible error"
        );
    }

    test.teardown().await;
}

/// Saving a member stores its direct `role` and ignores transient `synthetic` and
/// `inherited_role` annotations.
#[tokio::test]
async fn save_member_stores_the_direct_role_only() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let member_id = common::user(&test, "member").await;
    let workspace_id = common::workspace_of(&test, &[&user_id, &member_id]).await;
    let board_id = new_id();

    let mut board = Container {
        minimum_role: MinimumRole::Editor,
        ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container(&mut board, &user_id)
        .await
        .expect("the board is inserted");

    let member = ContainerMember {
        container_id: board_id.clone(),
        user_id: member_id.clone(),
        role: fvoci_model::ContainerRole::Commenter,
        synthetic: true,
        inherited_role: Some(MinimumRole::Editor),
    };
    test.save_member(&member)
        .await
        .expect("the member is saved");

    let stored = test
        .get_member_for_container(&board_id, &member_id)
        .await
        .expect("the member exists");
    assert_eq!(
        stored,
        ContainerMember {
            container_id: board_id,
            user_id: member_id.clone(),
            role: fvoci_model::ContainerRole::Commenter,
            synthetic: false,
            inherited_role: None,
        },
        "the direct role alone"
    );
    let of_user = test
        .get_members_for_user(&member_id)
        .await
        .expect("the memberships are read");
    assert_eq!(of_user, vec![stored], "the user's memberships");

    test.teardown().await;
}

// Membership changes that report what changed and enforce the last-administrator invariant
// atomically inside the store.

#[tokio::test]
async fn add_member_stores_a_new_membership_and_returns_none() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    let member = ContainerMember {
        role: fvoci_model::ContainerRole::Commenter,
        ..editor_of(&board_id, &user_id)
    };
    let existing = test.add_member(&member).await.expect("the member is added");
    assert_eq!(existing, None, "there was no membership before");

    assert_eq!(
        members_of(&test, &board_id).await,
        [member],
        "the membership as it was given"
    );

    test.teardown().await;
}

#[tokio::test]
async fn add_member_returns_the_existing_membership_and_changes_nothing() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;
    let stored = editor_of(&board_id, &user_id);
    test.save_member(&stored)
        .await
        .expect("the member is stored");

    // The same role again, and others: more rights and fewer.
    let offers = [
        editor_of(&board_id, &user_id),
        admin_of(&board_id, &user_id),
        ContainerMember {
            role: fvoci_model::ContainerRole::Viewer,
            ..editor_of(&board_id, &user_id)
        },
    ];
    for offer in offers {
        let existing = test
            .add_member(&offer)
            .await
            .expect("adding a member again succeeds");
        assert_eq!(
            existing.as_ref(),
            Some(&stored),
            "the membership as it stands, whatever was offered: {offer:?}"
        );
        assert_eq!(
            members_of(&test, &board_id).await,
            std::slice::from_ref(&stored),
            "the stored membership is unchanged by the offer {offer:?}"
        );
    }

    test.teardown().await;
}

#[tokio::test]
async fn add_member_to_a_board_or_of_a_user_that_does_not_exist_is_an_error() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    // An identifier nothing has is not found, and the refusal says what is missing.
    for (member, missing) in [
        (editor_of(&new_id(), &user_id), "board"),
        (editor_of(&board_id, &new_id()), "user"),
    ] {
        let error = test
            .add_member(&member)
            .await
            .expect_err("a membership needs its board and its user");
        assert!(
            matches!(&error, StoreError::NotFound(what) if what.contains(missing)),
            "{member:?}: no such {missing}: got {error:?}"
        );
        assert!(error.is_not_found(), "{member:?}: got {error:?}");
    }
    // Text that is no identifier names no board, as an identifier nothing has. For
    // the user of a membership it is refused as what it is.
    let error = test
        .add_member(&editor_of(NOT_AN_ID, &user_id))
        .await
        .expect_err("text that is no identifier names no board");
    assert!(
        matches!(&error, StoreError::NotFound(what) if what.contains("board")),
        "got {error:?}"
    );
    let error = test
        .add_member(&editor_of(&board_id, NOT_AN_ID))
        .await
        .expect_err("text that is no identifier is no user");
    assert!(
        matches!(&error, StoreError::InvalidId(invalid) if invalid == NOT_AN_ID),
        "got {error:?}"
    );

    let members = members_of(&test, &board_id).await;
    assert!(members.is_empty(), "the board got a member: {members:?}");
    let memberships = test
        .get_members_for_user(&user_id)
        .await
        .expect("the memberships are read");
    assert!(
        memberships.is_empty(),
        "the user got a membership: {memberships:?}"
    );

    test.teardown().await;
}

/// How many requests add the same user to the same board at once.
const RACING_ADDERS: usize = 8;

/// Two requests that add the same user at once end in one membership, the one that was
/// stored first; every other request is told of it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_add_member_calls_end_in_one_membership_and_one_of_them_added_it() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    for round in 0..RACE_ROUNDS {
        let board_id = stored_board(&test, &workspace_id, &user_id).await;
        let mut adders = JoinSet::new();
        for number in 0..RACING_ADDERS {
            let role = fvoci_model::ContainerRole::ALL[number % 4];
            let offer = ContainerMember {
                container_id: board_id.clone(),
                user_id: user_id.clone(),
                role,
                synthetic: false,
                inherited_role: None,
            };
            let store = test.store.clone();
            adders.spawn(async move {
                let existing = store
                    .add_member(&offer)
                    .await
                    .expect("adding a member succeeds");
                (offer, existing)
            });
        }
        let outcomes = adders.join_all().await;

        let members = members_of(&test, &board_id).await;
        assert_eq!(
            members.len(),
            1,
            "round {round}: one membership: {members:?}"
        );
        let stored = &members[0];
        let added: Vec<&ContainerMember> = outcomes
            .iter()
            .filter(|(_, existing)| existing.is_none())
            .map(|(offer, _)| offer)
            .collect();
        assert_eq!(
            added,
            [stored],
            "round {round}: exactly one request added the membership, and it is the stored one"
        );
        for (offer, existing) in &outcomes {
            if let Some(existing) = existing {
                assert_eq!(
                    existing, stored,
                    "round {round}: the request offering {offer:?} was told of the stored one"
                );
            }
        }
    }

    test.teardown().await;
}

#[tokio::test]
async fn change_member_replaces_the_flags_of_a_membership_and_of_no_one_else() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let member_id = common::user(&test, "member").await;
    let stranger_id = common::user(&test, "stranger").await;
    let workspace_id = common::workspace_of(&test, &[&admin_id, &member_id, &stranger_id]).await;
    let board_id = stored_board(&test, &workspace_id, &admin_id).await;
    for member in [
        admin_of(&board_id, &admin_id),
        editor_of(&board_id, &member_id),
    ] {
        test.save_member(&member)
            .await
            .expect("the member is stored");
    }

    // Up to administrator and down again: there is another one.
    let viewer = ContainerMember {
        role: fvoci_model::ContainerRole::Viewer,
        ..editor_of(&board_id, &member_id)
    };
    for change in [admin_of(&board_id, &member_id), viewer] {
        let outcome = test
            .change_member(&change)
            .await
            .expect("the membership is changed");
        assert_eq!(outcome, MemberChange::Done, "the change to {change:?}");
        let stored = test
            .get_member_for_container(&board_id, &member_id)
            .await
            .expect("the membership exists");
        assert_eq!(stored, change, "the role is the given one");
    }

    // Who is no member is not made one: on this board, on a board that does not exist
    // and on text that is no identifier.
    for change in [
        admin_of(&board_id, &stranger_id),
        admin_of(&new_id(), &member_id),
        admin_of(NOT_AN_ID, &member_id),
    ] {
        let outcome = test
            .change_member(&change)
            .await
            .expect("changing no membership succeeds");
        assert_eq!(
            outcome,
            MemberChange::NotAMember,
            "the change to {change:?}"
        );
    }
    assert_eq!(
        sorted_user_ids(&members_of(&test, &board_id).await),
        sorted([&admin_id, &member_id]),
        "the board has the members it had"
    );
    let of_stranger = test
        .get_members_for_user(&stranger_id)
        .await
        .expect("the memberships are read");
    assert!(of_stranger.is_empty(), "got {of_stranger:?}");

    test.teardown().await;
}

#[tokio::test]
async fn change_member_refuses_to_take_the_role_from_the_only_administrator() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let editor_id = common::user(&test, "editor").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &admin_id).await;
    // Other members do not count, only other administrators.
    for member in [
        admin_of(&board_id, &admin_id),
        editor_of(&board_id, &editor_id),
    ] {
        test.save_member(&member)
            .await
            .expect("the member is stored");
    }

    let outcome = test
        .change_member(&editor_of(&board_id, &admin_id))
        .await
        .expect("the change is answered");
    assert_eq!(outcome, MemberChange::LastAdmin, "stepping down is refused");
    let stored = test
        .get_member_for_container(&board_id, &admin_id)
        .await
        .expect("the membership exists");
    assert_eq!(stored, admin_of(&board_id, &admin_id), "nothing changed");

    // A change that leaves the admin role alone is made.
    let still_admin = admin_of(&board_id, &admin_id);
    let outcome = test
        .change_member(&still_admin)
        .await
        .expect("the membership is changed");
    assert_eq!(outcome, MemberChange::Done, "the administrator stays one");
    let stored = test
        .get_member_for_container(&board_id, &admin_id)
        .await
        .expect("the membership exists");
    assert_eq!(stored, still_admin, "the role is unchanged");

    test.teardown().await;
}

#[tokio::test]
async fn remove_member_removes_a_member_and_refuses_the_only_administrator() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let editor_id = common::user(&test, "editor").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &admin_id).await;
    for member in [
        admin_of(&board_id, &admin_id),
        editor_of(&board_id, &editor_id),
    ] {
        test.save_member(&member)
            .await
            .expect("the member is stored");
    }

    let outcome = test
        .remove_member(&board_id, &admin_id)
        .await
        .expect("the removal is answered");
    assert_eq!(outcome, MemberChange::LastAdmin, "leaving is refused");
    assert_eq!(
        sorted_user_ids(&members_of(&test, &board_id).await),
        sorted([&admin_id, &editor_id]),
        "nobody left"
    );

    let outcome = test
        .remove_member(&board_id, &editor_id)
        .await
        .expect("the member is removed");
    assert_eq!(outcome, MemberChange::Done, "an editor may leave");
    assert_eq!(
        members_of(&test, &board_id).await,
        [admin_of(&board_id, &admin_id)],
        "the administrator is left"
    );

    // Who is no member (any more) is not removed: from this board, from a board that
    // does not exist and from text that is no identifier.
    for board in [board_id.clone(), new_id(), NOT_AN_ID.to_owned()] {
        let outcome = test
            .remove_member(&board, &editor_id)
            .await
            .expect("removing no member succeeds");
        assert_eq!(outcome, MemberChange::NotAMember, "board {board}");
    }

    test.teardown().await;
}

#[tokio::test]
async fn an_administrator_may_step_down_or_leave_while_another_one_exists() {
    let test = common::setup().await;
    let first_id = common::user(&test, "first").await;
    let second_id = common::user(&test, "second").await;
    let third_id = common::user(&test, "third").await;
    let workspace_id = common::workspace_of(&test, &[&first_id, &second_id, &third_id]).await;
    let board_id = stored_board(&test, &workspace_id, &first_id).await;
    for user_id in [&first_id, &second_id, &third_id] {
        test.save_member(&admin_of(&board_id, user_id))
            .await
            .expect("the administrator is stored");
    }

    let outcome = test
        .change_member(&editor_of(&board_id, &first_id))
        .await
        .expect("the membership is changed");
    assert_eq!(outcome, MemberChange::Done, "two administrators are left");
    let outcome = test
        .remove_member(&board_id, &second_id)
        .await
        .expect("the member is removed");
    assert_eq!(outcome, MemberChange::Done, "one administrator is left");
    assert_eq!(
        members_of(&test, &board_id).await,
        sorted_members(vec![
            editor_of(&board_id, &first_id),
            admin_of(&board_id, &third_id)
        ]),
        "an editor and the administrator who stayed"
    );

    // The one who stayed is the last, and is held to it either way.
    let outcome = test
        .change_member(&editor_of(&board_id, &third_id))
        .await
        .expect("the change is answered");
    assert_eq!(outcome, MemberChange::LastAdmin, "stepping down is refused");
    let outcome = test
        .remove_member(&board_id, &third_id)
        .await
        .expect("the removal is answered");
    assert_eq!(outcome, MemberChange::LastAdmin, "leaving is refused");

    test.teardown().await;
}

#[tokio::test]
async fn remove_member_ends_the_subscriptions_of_the_user_to_the_blocks_of_the_board() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let member_id = common::user(&test, "member").await;
    let workspace_id = common::workspace(&test).await;
    let (left_board_id, left_card_id, _) =
        board_with_card_and_text(&test, &workspace_id, &admin_id).await;
    let (other_board_id, other_card_id, _) =
        board_with_card_and_text(&test, &workspace_id, &admin_id).await;
    for board_id in [&left_board_id, &other_board_id] {
        test.save_member(&admin_of(board_id, &admin_id))
            .await
            .expect("the administrator is stored");
        test.save_member(&editor_of(board_id, &member_id))
            .await
            .expect("the member is stored");
    }
    for card_id in [&left_card_id, &other_card_id] {
        subscribe(&test, card_id, &member_id).await;
        subscribe(&test, card_id, &admin_id).await;
    }

    let outcome = test
        .remove_member(&left_board_id, &member_id)
        .await
        .expect("the member is removed");
    assert_eq!(outcome, MemberChange::Done, "the member left the board");

    assert_eq!(
        subscribed_block_ids(&test, &member_id).await,
        [other_card_id.as_str()],
        "the subscription to the card of the board the user is still on"
    );
    assert_eq!(
        subscribed_block_ids(&test, &admin_id).await,
        sorted([&left_card_id, &other_card_id]),
        "the subscriptions of who stayed are untouched"
    );

    test.teardown().await;
}

/// How an administrator gives up the role.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GiveUp {
    /// Stays a member, as an editor.
    StepDown,
    /// Leaves the board.
    Leave,
}

impl GiveUp {
    async fn run(self, store: &Store, board_id: &str, user_id: &str) -> MemberChange {
        let outcome = match self {
            Self::StepDown => store.change_member(&editor_of(board_id, user_id)).await,
            Self::Leave => store.remove_member(board_id, user_id).await,
        };
        outcome.expect("giving up the role is answered")
    }
}

/// The check for another administrator and the write are one step: of the two
/// administrators of a board who give up the role at once, however each does it, one is
/// refused and stays the administrator.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_administrators_who_give_up_the_role_at_once_one_is_refused() {
    let test = common::setup().await;
    let first_id = common::user(&test, "first").await;
    let second_id = common::user(&test, "second").await;
    let editor_id = common::user(&test, "editor").await;
    let workspace_id = common::workspace_of(&test, &[&first_id, &second_id, &editor_id]).await;

    let pairs = [
        (GiveUp::StepDown, GiveUp::StepDown),
        (GiveUp::Leave, GiveUp::Leave),
        (GiveUp::StepDown, GiveUp::Leave),
    ];
    for (round, (first_way, second_way)) in
        pairs.into_iter().cycle().take(3 * RACE_ROUNDS).enumerate()
    {
        let board_id = stored_board(&test, &workspace_id, &first_id).await;
        for member in [
            admin_of(&board_id, &first_id),
            admin_of(&board_id, &second_id),
            editor_of(&board_id, &editor_id),
        ] {
            test.save_member(&member)
                .await
                .expect("the member is stored");
        }

        let mut racers = JoinSet::new();
        for (way, user_id) in [(first_way, &first_id), (second_way, &second_id)] {
            let store = test.store.clone();
            let board_id = board_id.clone();
            let user_id = user_id.clone();
            racers.spawn(async move {
                let outcome = way.run(&store, &board_id, &user_id).await;
                (user_id, outcome)
            });
        }
        let outcomes = racers.join_all().await;

        let what = format!("round {round}, {first_way:?} against {second_way:?}");
        let refused: Vec<&str> = outcomes
            .iter()
            .filter(|(_, outcome)| *outcome == MemberChange::LastAdmin)
            .map(|(user_id, _)| user_id.as_str())
            .collect();
        let done = outcomes
            .iter()
            .filter(|(_, outcome)| *outcome == MemberChange::Done)
            .count();
        assert_eq!(
            (done, refused.len()),
            (1, 1),
            "{what}: one gave up the role and one was refused: {outcomes:?}"
        );
        let members = members_of(&test, &board_id).await;
        let admins: Vec<&str> = members
            .iter()
            .filter(|member| member.role == fvoci_model::ContainerRole::Admin)
            .map(|member| member.user_id.as_str())
            .collect();
        assert_eq!(
            admins, refused,
            "{what}: the one who was refused is the administrator: {members:?}"
        );
    }

    test.teardown().await;
}

// -- joining with the minimum role ------------------------------------------------------

/// Joining gives the board's minimum role to a user who is no member, raises a member
/// who is below it, and leaves alone a member who has it or more. It says which.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "every role a user may hold against every minimum role, on one board"
)]
async fn join_container_gives_the_minimum_role_raises_to_it_and_never_lowers() {
    use fvoci_model::ContainerRole;
    let test = common::setup().await;
    let owner_id = common::user(&test, "owner").await;
    let user_id = common::user(&test, "joiner").await;
    let workspace_id = common::workspace_of(&test, &[&owner_id]).await;

    for (minimum_role, expected) in [
        (MinimumRole::Viewer, ContainerRole::Viewer),
        (MinimumRole::Commenter, ContainerRole::Commenter),
        (MinimumRole::Editor, ContainerRole::Editor),
    ] {
        let board_id = new_id();
        let mut board = Container {
            minimum_role,
            ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
        };
        test.insert_container_with_admin(&mut board, &owner_id)
            .await
            .expect("the board is stored");

        let joined = test
            .join_container(&board_id, &user_id)
            .await
            .expect("joined");
        assert!(
            matches!(joined, Joined::New(_)),
            "{minimum_role:?}: {joined:?}"
        );
        assert_eq!(joined.into_member().role, expected, "{minimum_role:?}");
        let again = test
            .join_container(&board_id, &user_id)
            .await
            .expect("joined");
        assert!(
            matches!(again, Joined::Already(_)),
            "{minimum_role:?}: {again:?}"
        );
        let stored = test
            .get_member_for_container(&board_id, &user_id)
            .await
            .expect("the membership is stored");
        assert_eq!(stored.role, expected, "{minimum_role:?}: as stored");

        if expected > ContainerRole::Viewer {
            test.save_member(&ContainerMember {
                container_id: board_id.clone(),
                user_id: user_id.clone(),
                role: ContainerRole::Viewer,
                synthetic: false,
                inherited_role: None,
            })
            .await
            .expect("lowered to viewer");
            let raised = test
                .join_container(&board_id, &user_id)
                .await
                .expect("joined");
            assert!(
                matches!(raised, Joined::Raised(_)),
                "{minimum_role:?}: {raised:?}"
            );
            assert_eq!(raised.into_member().role, expected, "{minimum_role:?}");
        }

        // The owner is the administrator and stays it.
        let owner = test
            .join_container(&board_id, &owner_id)
            .await
            .expect("joined");
        assert!(matches!(owner, Joined::Already(_)), "{owner:?}");
        assert_eq!(
            owner.into_member().role,
            ContainerRole::Admin,
            "not lowered"
        );
    }

    // A board whose minimum role is commenter: a viewer is raised, an editor is not.
    let board_id = new_id();
    let mut board = Container {
        minimum_role: MinimumRole::Commenter,
        ..new_board(&board_id, &workspace_id, ContainerVisibility::Open)
    };
    test.insert_container_with_admin(&mut board, &owner_id)
        .await
        .expect("the board is stored");
    for (before, outcome_is_raise, after) in [
        (ContainerRole::Viewer, true, ContainerRole::Commenter),
        (ContainerRole::Commenter, false, ContainerRole::Commenter),
        (ContainerRole::Editor, false, ContainerRole::Editor),
    ] {
        test.save_member(&ContainerMember {
            container_id: board_id.clone(),
            user_id: user_id.clone(),
            role: before,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("the member is stored");
        let joined = test
            .join_container(&board_id, &user_id)
            .await
            .expect("joined");
        assert_eq!(
            matches!(joined, Joined::Raised(_)),
            outcome_is_raise,
            "{before:?}: {joined:?}"
        );
        assert_eq!(joined.into_member().role, after, "{before:?}");
    }

    // What does not exist is not joined.
    let error = test
        .join_container(&new_id(), &user_id)
        .await
        .expect_err("no such board");
    assert!(error.is_not_found(), "{error:?}");
    let error = test
        .join_container(&board_id, &new_id())
        .await
        .expect_err("no such user");
    assert!(
        matches!(&error, StoreError::NotFound(what) if what == "user"),
        "{error:?}"
    );

    test.teardown().await;
}

/// The schema keeps what the model promises: a board's minimum role is one of the three,
/// never empty and never admin, whoever writes the row.
#[tokio::test]
async fn the_schema_refuses_a_minimum_role_that_is_none() {
    let test = common::setup().await;
    let user_id = common::user(&test, "owner").await;
    let workspace_id = common::workspace(&test).await;
    let boards = common::create_test_boards(&test, &workspace_id, &user_id, 1).await;
    let board_id = &boards[0].id;

    for role in ["", "admin", "owner"] {
        let refused = common::try_execute_raw(
            &test.database_connection(),
            &format!("UPDATE containers SET minimum_role = '{role}' WHERE id = '{board_id}'"),
        )
        .await;
        assert!(refused.is_err(), "{role:?} is stored: {refused:?}");
    }
    for role in ["viewer", "commenter", "editor"] {
        common::execute_raw(
            &test.database_connection(),
            &format!("UPDATE containers SET minimum_role = '{role}' WHERE id = '{board_id}'"),
        )
        .await;
        let board = test
            .get_container(board_id)
            .await
            .expect("the board is read");
        assert_eq!(board.minimum_role.as_str(), role, "as written");
    }

    test.teardown().await;
}

/// Reading the membership and writing it are one step: of users who join a board at
/// once, each ends with one membership with the minimum role, and one call made it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn joining_at_once_makes_one_membership() {
    let test = common::setup().await;
    let owner_id = common::user(&test, "owner").await;
    let user_id = common::user(&test, "joiner").await;
    let workspace_id = common::workspace(&test).await;

    for round in 0..RACE_ROUNDS {
        let board_id = stored_board(&test, &workspace_id, &owner_id).await;
        let mut racers = JoinSet::new();
        for _ in 0..6 {
            let store = test.store.clone();
            let (board_id, user_id) = (board_id.clone(), user_id.clone());
            racers.spawn(async move { store.join_container(&board_id, &user_id).await });
        }
        let outcomes: Vec<Joined> = racers
            .join_all()
            .await
            .into_iter()
            .map(|outcome| outcome.expect("joining succeeds"))
            .collect();

        let new = outcomes
            .iter()
            .filter(|outcome| matches!(outcome, Joined::New(_)))
            .count();
        assert_eq!(new, 1, "round {round}: one call made the membership");
        assert!(
            outcomes
                .iter()
                .all(|outcome| matches!(outcome, Joined::New(_) | Joined::Already(_))),
            "round {round}: {outcomes:?}"
        );
        let members = test
            .get_members_for_container(&board_id)
            .await
            .expect("the members are read");
        assert_eq!(
            members
                .iter()
                .filter(|member| member.user_id == user_id)
                .count(),
            1,
            "round {round}: one membership"
        );
    }

    test.teardown().await;
}
