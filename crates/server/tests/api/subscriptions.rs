//! Integration tests for block subscriptions: validation, permissions, membership
//! cascade, and non-broadcasting behavior. Each test runs against its own server.

use std::time::Duration;

use fvoci_model::{
    Block, BlockType, Container, ContainerMember, ContainerVisibility, Subscription,
};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::board_broadcasts::WorkspaceSocket;
use crate::cards::{Roles, board_with_roles, check_error, insert_block, new_block};
use crate::common::TEST_WS;
use crate::common::{ApiResponse, Client, Setup, TEST_WORKSPACE_ID, UNKNOWN_ID};

/// How long background notifications may take to finish.
const BACKGROUND: Duration = Duration::from_secs(10);

const PATH: &str = "/w/00000000-0000-7000-8000-000000000001/subscriptions";

/// What a block the user cannot read answers, as one that does not exist.
const NO_BLOCK: &str = "block not found";

/// What ending a subscription the user does not have answers.
const NO_SUBSCRIPTION: &str = "subscription not found";

/// The refusal to subscribe somebody else.
const NOT_FOR_OTHERS: &str = "a user subscribes for themselves only";

// -- helpers --------------------------------------------------------------------------

/// The body that subscribes a user to a card.
fn request(block_id: &str, user_id: &str) -> Value {
    json!({
        "blockType": "card",
        "blockId": block_id,
        "userId": user_id,
    })
}

/// `CreateSubscription`, expecting success.
async fn subscribe(client: &Client, block_id: &str, user_id: &str) -> Subscription {
    client
        .post(PATH, &request(block_id, user_id))
        .await
        .created_json()
}

/// `GetSubscriptions`
async fn get_subscriptions(client: &Client, user_id: &str) -> ApiResponse {
    client.get(&format!("{PATH}/{user_id}")).await
}

/// `GetSubscriptions`, expecting success; sorted by block, because the list has no
/// defined order.
async fn subscriptions_of(client: &Client, user_id: &str) -> Vec<Subscription> {
    let mut subscriptions: Vec<Subscription> = get_subscriptions(client, user_id).await.ok_json();
    subscriptions.sort_by(|a, b| a.block_id.cmp(&b.block_id));
    subscriptions
}

/// `DeleteSubscription`
async fn unsubscribe(client: &Client, block_id: &str, user_id: &str) -> ApiResponse {
    client.delete(&format!("{PATH}/{block_id}/{user_id}")).await
}

fn sorted(mut subscriptions: Vec<Subscription>) -> Vec<Subscription> {
    subscriptions.sort_by(|a, b| a.block_id.cmp(&b.block_id));
    subscriptions
}

/// Creates a card on the board quietly (`disableNotify`) and ends its writer's watching
/// of it, which a quiet write makes all the same: these tests make and end every
/// subscription themselves.
async fn insert_card(client: &Client, board_id: &str) -> Block {
    let created: fvoci_model::Card = client
        .post(
            &format!("{TEST_WS}/boards/{board_id}/cards?disableNotify=true"),
            &json!({}),
        )
        .await
        .created_json();
    let stored: fvoci_model::Card = client
        .get(&format!("{TEST_WS}/cards/{}", created.id))
        .await
        .ok_json();
    let card = Block::from(stored);
    let writer = client.me().await.id;
    unsubscribe(client, &card.id, &writer)
        .await
        .check_no_content();
    card
}

/// A board of the client's user with `count` cards, and the
/// user's subscription to each. Returns the subscriptions as the server answered and
/// the user's identifier.
async fn create_test_subscriptions(client: &Client, count: usize) -> (Vec<Subscription>, String) {
    let user = client.me().await;
    let board = Container {
        workspace_id: TEST_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        created_at: 1,
        updated_at: 1,
        ..Container::default()
    };
    let board: Container = client.create_board(&board).await.created_json();

    let mut subscriptions = Vec::with_capacity(count);
    for _ in 0..count {
        let card = insert_card(client, &board.id).await;
        let created = subscribe(client, &card.id, &user.id).await;
        subscriptions.push(created);
    }
    (subscriptions, user.id)
}

/// Creating a valid subscription succeeds and returns the stored subscription.
#[tokio::test]
async fn create_subscription_create_valid_subscription() {
    let th = Setup::native().start().await.init_basic().await;

    let (subscriptions, user_id) = create_test_subscriptions(&th.client, 5).await;
    assert_eq!(subscriptions.len(), 5, "five subscriptions were created");

    // Fetch the newly created subscriptions and compare.
    let found = subscriptions_of(&th.client, &user_id).await;
    assert_eq!(found, sorted(subscriptions), "as they were answered");
    th.teardown().await;
}

/// Creating a subscription with empty required fields is rejected.
#[tokio::test]
async fn create_subscription_create_invalid_subscription() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;

    let response = th
        .client
        .post(
            PATH,
            &json!({
                "blockType": "",
                "blockId": "",
                "userId": user.id,
            }),
        )
        .await;

    // Everything it lacks is told at once.
    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "2 members of the request were refused",
    );
    assert_eq!(response.refused_places(), ["/blockId", "/blockType"]);
    th.teardown().await;
}

/// Creating a subscription on behalf of another user with missing required fields reports
/// missing fields first.
#[tokio::test]
async fn create_subscription_create_subscription_for_another_user() {
    let th = Setup::native().start().await.init_basic().await;
    let other = th.client2.me().await;

    let response = th
        .client
        .post(
            PATH,
            &json!({
                "blockType": "",
                "blockId": "",
                "userId": other.id,
            }),
        )
        .await;

    // Missing required fields are rejected before comparing the user with the session user.
    check_error(
        &response,
        StatusCode::BAD_REQUEST,
        "2 members of the request were refused",
    );
    assert_eq!(response.refused_places(), ["/blockId", "/blockType"]);
    th.teardown().await;
}

/// Listing subscriptions for a user returns their active subscriptions.
#[tokio::test]
async fn get_subscriptions_get_subscriptions_for_user() {
    let th = Setup::native().start().await.init_basic().await;

    let (my_subscriptions, user1_id) = create_test_subscriptions(&th.client, 5).await;
    assert_eq!(my_subscriptions.len(), 5, "five of the first user");
    // Create more subscriptions with a different user.
    let (other_subscriptions, _) = create_test_subscriptions(&th.client2, 10).await;
    assert_eq!(other_subscriptions.len(), 10, "ten of the second user");

    // Only the ones created for the current user are returned.
    let found = subscriptions_of(&th.client, &user1_id).await;
    assert_eq!(found, sorted(my_subscriptions), "the first user's own");
    th.teardown().await;
}

/// Deleting an existing subscription removes it from the user's subscriptions.
#[tokio::test]
async fn delete_subscription_delete_valid_subscription() {
    let th = Setup::native().start().await.init_basic().await;
    let (subscriptions, user_id) = create_test_subscriptions(&th.client, 3).await;
    assert_eq!(subscriptions.len(), 3, "three subscriptions were created");

    let response = unsubscribe(&th.client, &subscriptions[1].block_id, &user_id).await;
    response.check_no_content();

    // Fetch the subscriptions and ensure the list is correct.
    let found = subscriptions_of(&th.client, &user_id).await;
    assert_eq!(found.len(), 2, "two are left");
    assert!(found.contains(&subscriptions[0]), "the first is left");
    assert!(found.contains(&subscriptions[2]), "the third is left");
    assert!(!found.contains(&subscriptions[1]), "the second is gone");
    th.teardown().await;
}

/// Deleting a non-existent or invalid subscription is rejected.
#[tokio::test]
async fn delete_subscription_delete_invalid_subscription() {
    let th = Setup::native().start().await.init_basic().await;
    let user = th.client.me().await;

    for block_id in [UNKNOWN_ID, "bogus"] {
        let response = unsubscribe(&th.client, block_id, &user.id).await;
        check_error(&response, StatusCode::NOT_FOUND, NO_SUBSCRIPTION);
    }
    th.teardown().await;
}

// -- POST /subscriptions ------------------------------------------------------------------

/// What a subscription must say, in the order it is checked, and how a body that is no
/// subscription is answered.
#[tokio::test]
async fn create_subscription_validation() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let other = th.client2.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = insert_card(&th.client, &board.id).await;
    let without = |member: &str| {
        let mut body = request(&card.id, &me);
        body.as_object_mut()
            .expect("the request is an object")
            .remove(member);
        body
    };
    let with = |member: &str, value: Value| {
        let mut body = request(&card.id, &me);
        body[member] = value;
        body
    };

    let response = th.anonymous().post(PATH, &request(&card.id, &me)).await;
    response.check_unauthorized();

    let all_null = json!({
        "blockType": null,
        "blockId": null,
        "userId": null,
    });
    let bad_requests = [
        (without("blockId"), "missing block id"),
        (without("blockType"), "missing block type"),
        (without("userId"), "missing user id"),
        (with("blockId", json!("")), "missing block id"),
        // Legacy and uncontracted fields are rejected as unknown members, with no fallback.
        (
            with("subscriberType", json!("user")),
            "unknown field `subscriberType`, expected one of `blockType`, `blockId`, `userId`",
        ),
        (
            json!({"blockType": "card", "blockId": card.id, "subscriberId": me}),
            "unknown field `subscriberId`, expected one of `blockType`, `blockId`, `userId`",
        ),
        (
            with("createdAt", json!(6)),
            "unknown field `createdAt`, expected one of `blockType`, `blockId`, `userId`",
        ),
        // Nothing at all: everything missing is reported. `null` is no string.
        (json!({}), "3 members of the request were refused"),
        (all_null, "invalid type: null, expected a string"),
        (
            json!({"userId": other}),
            "2 members of the request were refused",
        ),
    ];
    for (body, message) in &bad_requests {
        let response = th.client.post(PATH, body).await;
        check_error(&response, StatusCode::BAD_REQUEST, message);
    }
    let response = th.client.post(PATH, &json!({})).await;
    assert_eq!(
        response.refused_places(),
        ["/blockId", "/blockType", "/userId"],
        "each member that is missing"
    );

    // Complete, but for someone else; whether the block exists is not looked at.
    for block_id in [card.id.as_str(), UNKNOWN_ID, "nope"] {
        let response = th.client.post(PATH, &request(block_id, &other)).await;
        check_error(&response, StatusCode::FORBIDDEN, NOT_FOR_OTHERS);
    }
    // A block that does not exist. Boards are no blocks.
    for block_id in [UNKNOWN_ID, "nope", board.id.as_str()] {
        let response = th.client.post(PATH, &request(block_id, &me)).await;
        check_error(&response, StatusCode::NOT_FOUND, NO_BLOCK);
    }

    // A body that is not a JSON object, or whose members have the wrong types, is a 400.
    let block_id_number = with("blockId", json!(5)).to_string();
    let time_text = with("notifiedAt", json!("now")).to_string();
    for body in [
        "{\"blockId\":",
        "",
        "[]",
        "null",
        "\"card\"",
        &block_id_number,
        &time_text,
    ] {
        let response = th.client.send_raw_body(Method::POST, PATH, body).await;
        response.check_bad_request();
        assert_eq!(response.problem_type(), "/problems/bad-request", "{body}");
    }

    assert_eq!(
        subscriptions_of(&th.client, &me).await,
        [],
        "nothing was stored"
    );
    th.teardown().await;
}

/// What a new subscription looks like, and what subscribing again does.
#[tokio::test]
async fn create_subscription_answer_and_subscribing_again() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = insert_card(&th.client, &board.id).await;
    let before = fvoci_server::auth::now_millis();

    let response = th.client.post(PATH, &request(&card.id, &me)).await;
    let first: Subscription = response.created_json();
    assert!(
        first.created_at >= before,
        "created now: {}",
        first.created_at
    );
    assert_eq!(
        response.value(),
        json!({
            "blockType": "card",
            "blockId": card.id,
            "userId": me,
            "notifiedAt": first.created_at,
            "createdAt": first.created_at,
        }),
        "the answer"
    );
    assert_eq!(
        subscriptions_of(&th.client, &me).await,
        std::slice::from_ref(&first)
    );

    // Subscribing again answers the subscription there is: it keeps its creation time
    // and is marked as notified now.
    tokio::time::sleep(Duration::from_millis(5)).await;
    let second = subscribe(&th.client, &card.id, &me).await;
    assert_eq!(second.created_at, first.created_at, "the same subscription");
    assert!(second.notified_at > first.notified_at, "notified now");
    assert_eq!(
        subscriptions_of(&th.client, &me).await,
        std::slice::from_ref(&second),
        "what is stored is what was answered"
    );

    // Unsubscribing ends the subscription; subscribing after that starts a new one.
    unsubscribe(&th.client, &card.id, &me)
        .await
        .check_no_content();
    assert_eq!(subscriptions_of(&th.client, &me).await, [], "unsubscribed");
    tokio::time::sleep(Duration::from_millis(5)).await;
    let third = subscribe(&th.client, &card.id, &me).await;
    assert!(third.created_at > first.created_at, "created anew");
    assert_eq!(
        subscriptions_of(&th.client, &me).await,
        std::slice::from_ref(&third),
        "what is stored is what was answered"
    );

    // Shared-organization subscriptions accept `card`, `page`, and `unit`, while
    // rejecting non-subscribable or unknown block types with 400.
    for block_type in ["page", "unit"] {
        let card = insert_card(&th.client, &board.id).await;
        let mut body = request(&card.id, &me);
        body["blockType"] = json!(block_type);
        let response = th.client.post(PATH, &body).await;
        response.check_created();
        let answer = response.value();
        assert_eq!(answer["blockType"], json!(block_type), "{answer}");
        let listed = get_subscriptions(&th.client, &me).await.value();
        let listed = listed.as_array().expect("a list");
        assert!(listed.contains(&answer), "{answer} is among {listed:?}");
    }

    for block_type in [
        "view",
        "comment",
        "attachment",
        "chapter",
        "text",
        "whatever",
    ] {
        let mut body = request(&card.id, &me);
        body["blockType"] = json!(block_type);
        th.client.post(PATH, &body).await.check_bad_request();
    }

    th.teardown().await;
}

// -- GET /subscriptions/{user_id}, DELETE /subscriptions/{block_id}/{user_id} --

/// A user reads and ends their own subscriptions only.
#[tokio::test]
async fn subscriptions_of_another_user_are_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let user1 = th.client.me().await.id;
    let user2 = th.client2.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = insert_card(&th.client, &board.id).await;
    let subscription = subscribe(&th.client, &card.id, &user1).await;

    let response = get_subscriptions(&th.anonymous(), &user1).await;
    response.check_unauthorized();
    let response = unsubscribe(&th.anonymous(), &card.id, &user1).await;
    response.check_unauthorized();

    // The subscriptions of another user are nobody's business: the answer is the same
    // for a user who has some, a user who has none, and nobody at all.
    for user_id in [user1.as_str(), UNKNOWN_ID, "nobody"] {
        let response = get_subscriptions(&th.client2, user_id).await;
        check_error(&response, StatusCode::NOT_FOUND, "user not found");
        for block_id in [card.id.as_str(), UNKNOWN_ID, "nope"] {
            let response = unsubscribe(&th.client2, block_id, user_id).await;
            check_error(&response, StatusCode::NOT_FOUND, NO_SUBSCRIPTION);
        }
    }

    let response = get_subscriptions(&th.client2, &user2).await;
    response.check_ok();
    assert_eq!(response.text(), "[]", "no subscriptions are the empty list");
    assert_eq!(
        subscriptions_of(&th.client, &user1).await,
        [subscription],
        "the first user's subscription is still there"
    );
    th.teardown().await;
}

/// A subscription can be ended once.
#[tokio::test]
async fn delete_subscription_twice() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = insert_card(&th.client, &board.id).await;
    let response = unsubscribe(&th.client, &card.id, &me).await;
    check_error(&response, StatusCode::NOT_FOUND, NO_SUBSCRIPTION);

    subscribe(&th.client, &card.id, &me).await;
    unsubscribe(&th.client, &card.id, &me)
        .await
        .check_no_content();
    let response = unsubscribe(&th.client, &card.id, &me).await;
    check_error(&response, StatusCode::NOT_FOUND, NO_SUBSCRIPTION);

    th.teardown().await;
}

/// A subscription goes into the trash with its block: while the block is there it is
/// not listed and there is nothing to subscribe to or end. Restoring the block brings
/// the subscription back as it was.
#[tokio::test]
async fn a_subscription_goes_to_the_trash_with_its_block_and_comes_back_with_it() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = insert_card(&th.client, &board.id).await;
    let created = subscribe(&th.client, &card.id, &me).await;
    crate::blocks::trash_card(&th.client, &board.id, &card.id)
        .await
        .check_no_content();

    assert_eq!(
        subscriptions_of(&th.client, &me).await,
        [],
        "the subscription went into the trash with the block"
    );
    let response = th.client.post(PATH, &request(&card.id, &me)).await;
    check_error(&response, StatusCode::NOT_FOUND, NO_BLOCK);
    let response = unsubscribe(&th.client, &card.id, &me).await;
    check_error(&response, StatusCode::NOT_FOUND, NO_SUBSCRIPTION);

    crate::blocks::restore_card(&th.client, &board.id, &card.id)
        .await
        .check_ok();
    assert_eq!(
        subscriptions_of(&th.client, &me).await,
        [created],
        "and comes back with it as it was"
    );
    th.teardown().await;
}

// -- who is told --------------------------------------------------------------------------

/// Subscription mutations do not broadcast websocket messages.
#[tokio::test]
async fn subscription_changes_are_not_broadcast() {
    let th = Setup::native().start().await.init_basic().await;
    let me = th.client.me().await.id;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card = insert_card(&th.client, &board.id).await;
    assert!(
        th.app.finish_background(BACKGROUND).await,
        "the card's broadcast is out before anyone listens"
    );
    let mut socket = WorkspaceSocket::subscribe(&th, &th.client, TEST_WORKSPACE_ID, &me).await;

    subscribe(&th.client, &card.id, &me).await;
    subscribe(&th.client, &card.id, &me).await;
    unsubscribe(&th.client, &card.id, &me)
        .await
        .check_no_content();

    assert!(
        th.app.finish_background(BACKGROUND).await,
        "background work finishes"
    );
    assert_eq!(socket.drain().await, [json!(null); 0], "nothing is said");
    th.teardown().await;
}

// -- security -----------------------------------------------------------------------------

/// Subscribing to a block requires permission to view the block's board.
#[tokio::test]
async fn subscription_to_a_block_of_a_board_the_user_cannot_view_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let outsider = th.client2.me().await.id;
    let private_board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let open_board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;

    // A private board's block answers an uninvited member as not found, while an open
    // board's block can be viewed and subscribed to by workspace members (but not guests).
    let private_card = insert_card(&th.client, &private_board.id).await;
    let response = th
        .client2
        .post(PATH, &request(&private_card.id, &outsider))
        .await;
    check_error(&response, StatusCode::NOT_FOUND, NO_BLOCK);
    assert_eq!(
        subscriptions_of(&th.client2, &outsider).await,
        [],
        "nothing was stored"
    );

    let guest = th.add_user("sub.guest").await;
    let guest_id = guest.me().await.id;
    crate::content_search::set_role(&th, &guest_id, "guest").await;
    let open_card = insert_card(&th.client, &open_board.id).await;
    let guest_response = guest.post(PATH, &request(&open_card.id, &guest_id)).await;
    check_error(&guest_response, StatusCode::NOT_FOUND, NO_BLOCK);

    for block_id in [UNKNOWN_ID, "nope"] {
        let response = th.client2.post(PATH, &request(block_id, &outsider)).await;
        check_error(&response, StatusCode::NOT_FOUND, NO_BLOCK);
    }

    // A workspace member can subscribe to an open board's cards directly.
    let subscription = subscribe(&th.client2, &open_card.id, &outsider).await;
    assert_eq!(
        subscriptions_of(&th.client2, &outsider).await,
        [subscription]
    );
    th.teardown().await;
}

/// The check leaves alone what worked before: every member of a board, whatever their
/// role, subscribes to its blocks. A user who is removed from the board cannot subscribe
/// any more, and has no subscription left to end.
#[tokio::test]
async fn subscription_by_every_member_of_the_board_still_works() {
    let th = Setup::native().start().await.init_basic().await;
    let Roles {
        board,
        editor,
        commenter,
        viewer,
    } = board_with_roles(&th).await;
    let card = insert_card(&th.client, &board.id).await;
    let text = insert_block(
        &th.client,
        &new_block(&board.id, &card.id, BlockType::Attachment),
    )
    .await;

    for client in [&th.client, &editor, &commenter, &viewer] {
        let user_id = client.me().await.id;
        let on_card = subscribe(client, &card.id, &user_id).await;
        let on_text = subscribe(client, &text.id, &user_id).await;
        assert_eq!(
            subscriptions_of(client, &user_id).await,
            sorted(vec![on_card, on_text]),
            "the member's subscriptions"
        );
    }

    let viewer_id = viewer.me().await.id;
    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{viewer_id}",
            board.id
        ))
        .await
        .check_no_content();
    let response = viewer.post(PATH, &request(&card.id, &viewer_id)).await;
    check_error(&response, StatusCode::NOT_FOUND, NO_BLOCK);
    assert_eq!(
        subscriptions_of(&viewer, &viewer_id).await,
        [],
        "ended with the membership"
    );
    for block in [&card, &text] {
        unsubscribe(&viewer, &block.id, &viewer_id)
            .await
            .check_not_found();
    }
    th.teardown().await;
}

/// A member who leaves or is removed from a board loses their subscriptions to that
/// board's blocks.
/// Subscriptions on other boards, and those of members who stay, are untouched.
#[tokio::test]
async fn leaving_or_being_removed_from_a_board_ends_the_subscriptions_to_its_blocks() {
    let th = Setup::native().start().await.init_basic().await;
    let admin_id = th.client.me().await.id;
    let user2_id = th.client2.me().await.id;
    let mut cards = Vec::new();
    let mut boards = Vec::new();
    for _ in 0..2 {
        let board = th
            .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
            .await;
        let member = ContainerMember {
            container_id: board.id.clone(),
            user_id: user2_id.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        };
        th.client.add_member(&member).await.check_ok();
        cards.push(insert_card(&th.client, &board.id).await);
        boards.push(board);
    }
    let on_first = subscribe(&th.client2, &cards[0].id, &user2_id).await;
    let on_second = subscribe(&th.client2, &cards[1].id, &user2_id).await;
    let admins = subscribe(&th.client, &cards[0].id, &admin_id).await;

    // A removal that is refused ends nothing: an editor may not remove a member.
    th.client2
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{admin_id}",
            boards[0].id
        ))
        .await
        .check_forbidden();
    assert_eq!(
        subscriptions_of(&th.client, &admin_id).await,
        std::slice::from_ref(&admins),
        "the subscription of the member who was not removed"
    );

    // Removed by the admin.
    th.client
        .delete(&format!(
            "{TEST_WS}/containers/{}/members/{user2_id}",
            boards[0].id
        ))
        .await
        .check_no_content();
    assert_eq!(
        subscriptions_of(&th.client2, &user2_id).await,
        [on_second],
        "only the subscription on the other board is left"
    );
    assert_eq!(
        subscriptions_of(&th.client, &admin_id).await,
        [admins],
        "the subscription of a member who stays"
    );
    let _ = on_first;

    // Leaving by oneself.
    th.client2
        .post_empty(&format!("{TEST_WS}/containers/{}/leave", boards[1].id))
        .await
        .check_no_content();
    assert_eq!(
        subscriptions_of(&th.client2, &user2_id).await,
        [],
        "nothing is left after leaving the other board too"
    );

    // A member again, the user subscribes again.
    let member = ContainerMember {
        container_id: boards[0].id.clone(),
        user_id: user2_id.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    th.client.add_member(&member).await.check_ok();
    subscribe(&th.client2, &cards[0].id, &user2_id).await;
    // The revived subscription keeps its first creation time, so compare the blocks.
    let subscribed: Vec<String> = subscriptions_of(&th.client2, &user2_id)
        .await
        .into_iter()
        .map(|subscription| subscription.block_id)
        .collect();
    assert_eq!(
        subscribed,
        [cards[0].id.clone()],
        "subscribed again as a member"
    );
    th.teardown().await;
}
