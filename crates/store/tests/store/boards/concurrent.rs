//! Writes of a board that race one another or its deletion.

use super::*;

// -- writes of one board at once ----------------------------------------------------------

/// A definition of a card property with the given identifier.
fn card_property(id: &str) -> fvoci_model::CardPropertyDefinition {
    fvoci_model::CardPropertyDefinition::new(
        id,
        format!("Property {id}"),
        fvoci_model::PropertyType::Text,
    )
}

/// The identifiers of the card property definitions of a board, in their order.
fn card_property_ids(board: &Container) -> Vec<&str> {
    board
        .card_properties
        .iter()
        .map(|definition| definition.id.as_str())
        .collect()
}

/// A patch is applied to the board as the patch before it left it: of two patches at
/// once that each add a card property, both definitions are there afterwards, next to
/// those of every round before.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_patches_of_a_board_at_once_both_take_effect() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &user_id).await;

    let mut expected = BTreeSet::new();
    for round in 0..2 * RACE_ROUNDS {
        let start = Arc::new(Barrier::new(2));
        let mut patchers = JoinSet::new();
        for side in ["left", "right"] {
            let property_id = format!("{side}-{round}");
            expected.insert(property_id.clone());
            let (store, start) = (test.store.clone(), Arc::clone(&start));
            let (board_id, user_id) = (board_id.clone(), user_id.clone());
            patchers.spawn(async move {
                let patch = ContainerPatch {
                    updated_card_properties: vec![card_property(&property_id)],
                    ..ContainerPatch::default()
                };
                start.wait().await;
                store.patch_container(&board_id, &patch, &user_id).await
            });
        }
        for outcome in patchers.join_all().await {
            let patched =
                outcome.unwrap_or_else(|error| panic!("round {round}: a patch failed: {error:?}"));
            assert!(
                card_property_ids(&patched).len() > 2 * round,
                "round {round}: a patch answered a board without the definitions of the \
                 rounds before: {:?}",
                card_property_ids(&patched)
            );
        }

        let board = test
            .get_container(&board_id)
            .await
            .expect("the board is read");
        let stored: BTreeSet<String> = card_property_ids(&board)
            .into_iter()
            .map(str::to_owned)
            .collect();
        assert_eq!(
            stored, expected,
            "round {round}: the definitions of both patches, and of every round before"
        );
        assert_eq!(
            board.card_properties.len(),
            expected.len(),
            "round {round}: each definition once"
        );
    }

    test.teardown().await;
}

/// A member is added again while being removed. What adding answers is true when both
/// are done: "new" only of a membership that exists (the one that was offered), "there
/// was one" only of the membership that the removal then took. Never "new" with no
/// membership.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_member_racing_remove_member_answers_what_is_true() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;
    let board_id = stored_board(&test, &workspace_id, &admin_id).await;
    test.save_member(&admin_of(&board_id, &admin_id))
        .await
        .expect("the administrator is stored");
    let there_before = editor_of(&board_id, &user_id);
    // Not the role of the membership that is there: which one is stored shows.
    let offer = ContainerMember {
        role: fvoci_model::ContainerRole::Viewer,
        ..editor_of(&board_id, &user_id)
    };

    let (mut new, mut existing) = (0, 0);
    for round in 0..4 * RACE_ROUNDS {
        test.save_member(&there_before)
            .await
            .expect("the member is there before the round");

        let start = Arc::new(Barrier::new(2));
        let adding = tokio::spawn({
            let (store, start, offer) = (test.store.clone(), Arc::clone(&start), offer.clone());
            async move {
                start.wait().await;
                store.add_member(&offer).await
            }
        });
        let removing = tokio::spawn({
            let (store, start) = (test.store.clone(), Arc::clone(&start));
            let (board_id, user_id) = (board_id.clone(), user_id.clone());
            async move {
                start.wait().await;
                store.remove_member(&board_id, &user_id).await
            }
        });
        let added = adding
            .await
            .expect("the adding task runs to its end")
            .unwrap_or_else(|error| panic!("round {round}: adding failed: {error:?}"));
        let removed = removing
            .await
            .expect("the removing task runs to its end")
            .unwrap_or_else(|error| panic!("round {round}: removing failed: {error:?}"));

        assert_eq!(
            removed,
            MemberChange::Done,
            "round {round}: there was a member to remove, before the addition or after it"
        );
        let stored = test.get_member_for_container(&board_id, &user_id).await;
        match added {
            None => {
                new += 1;
                let stored = stored.unwrap_or_else(|error| {
                    panic!("round {round}: \"new\" of a membership that does not exist: {error:?}")
                });
                assert_eq!(
                    stored, offer,
                    "round {round}: the membership that was offered"
                );
            }
            Some(told) => {
                existing += 1;
                assert_eq!(
                    told, there_before,
                    "round {round}: told of the membership that was there"
                );
                let error = stored.expect_err("the removal came after and took the membership");
                assert!(error.is_not_found(), "round {round}: got {error:?}");
            }
        }
    }
    assert_eq!(new + existing, 4 * RACE_ROUNDS, "every round was answered");

    test.teardown().await;
}

/// A user is added to a board while the board is deleted. Either the membership came
/// first, and then it is in the trash with the board and comes back with it, or the
/// board was gone, and then that is a not-found error and there is no membership. It is
/// never stored and lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_added_while_the_board_is_deleted_goes_with_the_board_or_is_not_found() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    for round in 0..2 * RACE_ROUNDS {
        let board_id = stored_board(&test, &workspace_id, &admin_id).await;
        test.save_member(&admin_of(&board_id, &admin_id))
            .await
            .expect("the administrator is stored");
        let late = editor_of(&board_id, &user_id);

        let start = Arc::new(Barrier::new(2));
        let deleting = tokio::spawn({
            let (store, start) = (test.store.clone(), Arc::clone(&start));
            let (board_id, admin_id) = (board_id.clone(), admin_id.clone());
            async move {
                start.wait().await;
                store.delete_container(&board_id, &admin_id).await
            }
        });
        let adding = tokio::spawn({
            let (store, start, late) = (test.store.clone(), Arc::clone(&start), late.clone());
            async move {
                start.wait().await;
                store.add_member(&late).await
            }
        });
        deleting
            .await
            .expect("the deleting task runs to its end")
            .unwrap_or_else(|error| panic!("round {round}: the board is deleted: {error:?}"));
        let added = adding.await.expect("the adding task runs to its end");

        let left = members_of(&test, &board_id).await;
        assert!(
            left.is_empty(),
            "round {round}: memberships of a deleted board are left: {left:?}"
        );
        let in_trash = test
            .get_deleted_container(&board_id)
            .await
            .expect("the board is in the trash")
            .members;
        let expected = match &added {
            Ok(None) => vec![admin_of(&board_id, &admin_id), late.clone()],
            Err(error) => {
                assert!(
                    matches!(error, StoreError::NotFound(_)),
                    "round {round}: a member who comes after the board is gone is a \
                     not-found error, got {error:?}"
                );
                vec![admin_of(&board_id, &admin_id)]
            }
            Ok(Some(member)) => {
                panic!("round {round}: told of a membership nobody stored: {member:?}")
            }
        };
        let expected = sorted_members(expected);
        assert_eq!(
            sorted_members(in_trash),
            expected,
            "round {round}: the memberships in the trash, after adding answered {added:?}"
        );

        test.undelete_container(&board_id, &admin_id)
            .await
            .unwrap_or_else(|error| panic!("round {round}: the board is restored: {error:?}"));
        assert_eq!(
            members_of(&test, &board_id).await,
            expected,
            "round {round}: the memberships that came back, after adding answered {added:?}"
        );
    }

    test.teardown().await;
}

/// A file record is stored for a board while the board is deleted. Either the record
/// came first, and then it is in the trash with the board and comes back with it, or
/// the board was gone, and then that is a not-found error and there is no record. It is
/// never stored and lost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_file_saved_while_the_board_is_deleted_goes_with_the_board_or_is_not_found() {
    let test = common::setup().await;
    let user_id = common::user(&test, "user").await;
    let workspace_id = common::workspace(&test).await;

    for round in 0..2 * RACE_ROUNDS {
        let board_id = stored_board(&test, &workspace_id, &user_id).await;
        let late = file_of(&board_id, "7late.png");

        let start = Arc::new(Barrier::new(2));
        let deleting = tokio::spawn({
            let (store, start) = (test.store.clone(), Arc::clone(&start));
            let (board_id, user_id) = (board_id.clone(), user_id.clone());
            async move {
                start.wait().await;
                store.delete_container(&board_id, &user_id).await
            }
        });
        let saving = tokio::spawn({
            let (store, start, late) = (test.store.clone(), Arc::clone(&start), late.clone());
            async move {
                start.wait().await;
                store.save_file_record(&late).await
            }
        });
        deleting
            .await
            .expect("the deleting task runs to its end")
            .unwrap_or_else(|error| panic!("round {round}: the board is deleted: {error:?}"));
        let saved = saving.await.expect("the saving task runs to its end");

        let error = test
            .get_file_record(&board_id, &late.stored_name)
            .await
            .expect_err("a deleted board has no file records");
        assert!(error.is_not_found(), "round {round}: got {error:?}");
        if let Err(error) = &saved {
            assert!(
                matches!(error, StoreError::NotFound(_)),
                "round {round}: a file record that comes after the board is gone is a \
                 not-found error, got {error:?}"
            );
        }

        test.undelete_container(&board_id, &user_id)
            .await
            .unwrap_or_else(|error| panic!("round {round}: the board is restored: {error:?}"));
        let back = test.get_file_record(&board_id, &late.stored_name).await;
        match (&saved, back) {
            (Ok(()), Ok(back)) => {
                assert_eq!(back, late, "round {round}: the record came back as it was");
            }
            (Err(_), Err(error)) => {
                assert!(error.is_not_found(), "round {round}: got {error:?}");
            }
            (Ok(()), Err(error)) => {
                panic!("round {round}: the record was stored and is lost: {error:?}")
            }
            (Err(refusal), Ok(back)) => {
                panic!("round {round}: the record was refused ({refusal:?}) and is there: {back:?}")
            }
        }
    }

    test.teardown().await;
}

// -- the trash keeps everything ---------------------------------------------------------

/// The members of `value`, by their names in its JSON form, that are what they are in
/// `default`.
fn members_at_their_default<T: Serialize>(value: &T, default: &T) -> Vec<String> {
    let members = |of: &T| match serde_json::to_value(of).expect("the value has a JSON form") {
        Value::Object(members) => members,
        other => panic!("the JSON form is an object, not {other}"),
    };
    let default = members(default);
    members(value)
        .into_iter()
        .filter(|(name, member)| default.get(name) == Some(member))
        .map(|(name, _)| name)
        .collect()
}

/// What goes to the trash comes back whole. The trash keeps a board, its memberships
/// and its blocks as JSON, so a member that the JSON form of one of them left out would
/// come back empty, and only here would anybody see it.
///
/// Every member of the board, of a block on it and of a membership is something other
/// than its default before the board is deleted, except what cannot be for a row that
/// exists: `deleted_at`, and of a membership `roles` and `synthetic`, which have no
/// column. The literals below name every member on purpose, without `..`: a member
/// that is added later does not compile until it is given a value here.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one board with every member of every record set, compared member for member"
)]
async fn a_board_comes_back_from_the_trash_with_every_member_it_had() {
    let test = common::setup().await;
    let author_id = common::user(&test, "author").await;
    let restorer_id = common::user(&test, "restorer").await;
    let workspace_id = common::workspace(&test).await;

    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.clone(),
        // Stamped by the store when the board is inserted.
        created_by: String::new(),
        modified_by: String::new(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Board,
        minimum_role: MinimumRole::Commenter,
        title: "Roadmap".to_owned(),
        description: "What comes when".to_owned(),
        icon: "🗺".to_owned(),
        show_description: true,
        is_template: true,
        // The store decides this, and only a template that no user made is built-in.
        // Set past the store below, so that this board has a creator as well.
        is_builtin: false,
        template_version: 3,
        properties: object(json!({ "color": "blue" })),
        card_properties: vec![
            fvoci_model::CardPropertyDefinition {
                id: "status".to_owned(),
                name: "Status".to_owned(),
                property_type: fvoci_model::PropertyType::Select,
                options: vec![fvoci_model::PropertyOption::new(
                    "open",
                    "Open",
                    fvoci_model::PropertyColor::Green,
                )],
            },
            fvoci_model::CardPropertyDefinition::new("due", "Due", fvoci_model::PropertyType::Date),
        ],
        created_at: 0,
        updated_at: 0,
        deleted_at: None,
        due_date_property_id: Some("due".to_owned()),
        done_when: Some(fvoci_model::DoneWhen {
            property_id: "status".to_owned(),
            option_ids: vec!["open".to_owned()],
        }),
    };
    test.insert_container(&mut board, &author_id)
        .await
        .expect("the board is stored");
    common::execute_raw(
        &test.database_connection(),
        &format!(
            "UPDATE containers SET is_builtin = TRUE WHERE id = '{}'",
            board.id
        ),
    )
    .await;
    for member in [
        admin_of(&board.id, &author_id),
        editor_of(&board.id, &restorer_id),
    ] {
        test.save_member(&member)
            .await
            .expect("the member is stored");
    }
    let card = new_block(&board.id, "", BlockType::Card);
    let mut note = Block {
        id: new_id(),
        parent_id: card.id.clone(),
        // Stamped by the store when the block is inserted.
        created_by: String::new(),
        modified_by: String::new(),
        number: None,
        // A card and no template, the one block with a body and a number: all is set.
        block_type: BlockType::Card,
        title: "a text".to_owned(),
        fields: object(json!({ "contentOrder": ["a", "b"], "isTemplate": false })),
        body: Body::parse(json!({"type": "doc", "content": [
            {"type": "paragraph", "content": [{"type": "text", "text": "a body"}]},
        ]}))
        .expect("a body that may be stored"),
        created_at: 0,
        updated_at: 0,
        deleted_at: None,
        container_id: board.id.clone(),
        // Only a comment whose body was changed has one.
        edited_at: 0,
    };
    test.insert_blocks(&mut [card], &author_id)
        .await
        .expect("the card is stored");
    test.insert_block(&mut note, &author_id)
        .await
        .expect("the text on the card is stored");

    // As stored, nothing that can be set is at its default; a wiki has no card properties,
    // and only a card has both a body and a number.
    let board = test
        .get_container(&board.id)
        .await
        .expect("the board is read");
    let members = members_of(&test, &board.id).await;
    let blocks = blocks_of(&test, &board.id).await;
    let note = test.get_block(&note.id).await.expect("the text is read");
    assert_eq!(
        members_at_their_default(&board, &Container::default()),
        ["kind"],
        "of the board: {board:?}"
    );
    assert_eq!(
        members_at_their_default(&note, &Block::default()),
        ["type"],
        "of the block: {note:?}"
    );
    let all_flags = members
        .iter()
        .find(|member| member.user_id == author_id)
        .expect("the author is a member");
    assert_eq!(
        members_at_their_default(
            all_flags,
            &ContainerMember::new("", "", fvoci_model::ContainerRole::Viewer),
        ),
        Vec::<String>::new(),
        "of the membership: {all_flags:?}"
    );

    test.delete_container(&board.id, &author_id)
        .await
        .expect("the board is deleted");
    let returned = test
        .undelete_container(&board.id, &restorer_id)
        .await
        .expect("the board is restored");

    // Restoring changes who modified the board and its blocks last, and when; nothing else.
    let restored = test
        .get_container(&board.id)
        .await
        .expect("the board is back");
    assert_eq!(returned, restored, "what is returned is what is stored");
    assert_eq!(
        restored,
        Container {
            modified_by: restorer_id.clone(),
            updated_at: restored.updated_at,
            ..board.clone()
        },
        "the board as it was"
    );
    assert_ne!(
        restored.modified_by, board.modified_by,
        "restored by another user than the one who wrote the board"
    );
    assert_eq!(
        members_of(&test, &board.id).await,
        members,
        "the memberships as they were"
    );
    let expected: Vec<Block> = blocks
        .iter()
        .map(|block| Block {
            modified_by: restorer_id.clone(),
            updated_at: restored.updated_at,
            ..block.clone()
        })
        .collect();
    assert_eq!(
        blocks_of(&test, &board.id).await,
        expected,
        "the blocks as they were"
    );

    test.teardown().await;
}
