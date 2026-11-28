//! Pins the JSON form of the wire types, and what decoding takes and refuses.

use fvoci_model::{
    Block, BlockPatch, BlockPatchBatch, BlockType, Card, CardPropertyDefinition, Category,
    CategoryContainerMetadata, CategoryContainers, CategoryType, ClientConfig, ClientStatePatch,
    Container, ContainerKind, ContainerMember, ContainerMemberPatch, ContainerPatch, ContainerRole,
    ContainerVisibility, Credential, InvalidInput, LoginRequest, MinimumRole, NameShown,
    OrgWorkspace, PersonName, Problem, PropertyColor, PropertyOption, PropertyType, PublicLink,
    Subscriber, Subscription, Title, User, Workspace,
};
use serde_json::{Value, json};

fn to_json<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).expect("wire types serialize to JSON")
}

#[test]
fn block_json_members() {
    let block = Block {
        id: "a1".to_owned(),
        parent_id: "c1".to_owned(),
        created_by: "u1".to_owned(),
        modified_by: "u2".to_owned(),
        block_type: BlockType::Comment,
        title: "hello".to_owned(),
        fields: serde_json::from_value(json!({ "z": 1, "a": [true] })).expect("object"),
        body: None,
        created_at: 10,
        updated_at: 20,
        deleted_at: None,
        container_id: "b1".to_owned(),
        number: None,
        edited_at: 0,
    };
    let want = json!({
        "id": "a1", "parentId": "c1", "createdBy": "u1", "modifiedBy": "u2",
        "type": "comment", "title": "hello", "fields": { "a": [true], "z": 1 }, "body": null,
        "createdAt": 10, "updatedAt": 20, "containerId": "b1"
    });
    assert_eq!(
        to_json(&block),
        want,
        "a comment never changed has no editedAt, and a live block has no deletedAt"
    );
    assert_eq!(
        serde_json::from_value::<Block>(want).expect("decodes"),
        block
    );
    let deleted = Block {
        deleted_at: Some(25),
        ..block.clone()
    };
    assert_eq!(to_json(&deleted)["deletedAt"], json!(25));
    let edited = Block {
        edited_at: 30,
        ..block
    };
    assert_eq!(to_json(&edited)["editedAt"], json!(30), "one that was has");
    let card = Block {
        block_type: BlockType::Card,
        number: Some(12),
        ..edited
    };
    assert_eq!(to_json(&card)["number"], json!(12), "a card has its number");
    assert_eq!(
        serde_json::from_value::<Block>(to_json(&card)).expect("decodes"),
        card,
        "and keeps it when read back, as the trash does"
    );
}

/// What a client sends as a number, which the server does not take, is read as none
/// when it is no number a card can have, and is never refused.
#[test]
fn a_number_that_no_card_can_have_is_read_as_none() {
    for sent in [
        json!(null),
        json!(0),
        json!(-1),
        json!(1.5),
        json!("7"),
        json!(4_294_967_296_u64),
        json!([1]),
    ] {
        let block: Block =
            serde_json::from_value(json!({ "type": "card", "number": sent })).expect("a block");
        assert_eq!(block.number, None, "{sent}");
        let card: Card = serde_json::from_value(json!({ "number": sent })).expect("a card");
        assert_eq!(card.number, None, "{sent}");
    }
    let block: Block =
        serde_json::from_value(json!({ "type": "card", "number": 12 })).expect("a block");
    assert_eq!(block.number, Some(12));
}

#[test]
fn block_decodes_missing_members_as_empty_and_requires_a_valid_type() {
    let block: Block = serde_json::from_str(r#"{"id":"a1","type":"card"}"#).expect("decodes");
    assert_eq!(
        block,
        Block {
            id: "a1".to_owned(),
            block_type: BlockType::Card,
            ..Block::default()
        }
    );
    for invalid in [
        r#"{"id":"a1"}"#,
        r#"{"id":"a1","type":""}"#,
        r#"{"id":"a1","type":"text"}"#,
        r#"{"id":"a1","type":"h1"}"#,
        r#"{"id":"a1","type":"Card"}"#,
        r#"{"id":"a1","type":null}"#,
    ] {
        serde_json::from_str::<Block>(invalid)
            .expect_err("a missing or unknown block type is refused at deserialization");
    }
    serde_json::from_str::<Block>(r#"{"id":"a1","type":"card","fields":null}"#)
        .expect_err("fields is no member that may be null");
    let block: Block =
        serde_json::from_str(r#"{"id":"a1","type":"card","body":null}"#).expect("body may be null");
    assert_eq!(block.body, None, "a null body is none");
}

#[test]
fn block_type_is_a_closed_enum_rejecting_unknown_strings() {
    for block_type in BlockType::ALL {
        let name = block_type.as_str();
        assert_eq!(name.parse::<BlockType>(), Ok(block_type));
        assert_eq!(to_json(&block_type), json!(name));
        assert_eq!(
            serde_json::from_value::<BlockType>(json!(name)).expect("decodes"),
            block_type
        );
        name.to_uppercase()
            .parse::<BlockType>()
            .expect_err("a type is parsed as it is written, case and all");
        serde_json::from_value::<BlockType>(json!(name.to_uppercase()))
            .expect_err("uppercase is rejected by serde");
    }

    for unknown in ["", "Card", "text", "board", "list-item", "h1"] {
        serde_json::from_value::<BlockType>(json!(unknown))
            .expect_err("unknown block type strings are rejected at deserialization");
        let error = unknown
            .parse::<BlockType>()
            .expect_err("unknown block type strings are rejected by FromStr");
        assert_eq!(
            error.to_string(),
            "the text names no block type",
            "the text is not echoed"
        );
    }
}

#[test]
fn block_patch_json_members() {
    let patch: BlockPatch = serde_json::from_value(json!({
        "parentId": "p", "title": "t",
        "updatedFields": { "a": 1 }, "deletedFields": ["b"]
    }))
    .expect("decodes");
    assert_eq!(patch.parent_id.as_deref(), Some("p"));
    assert_eq!(patch.title.as_deref(), Some("t"));
    assert_eq!(patch.updated_fields["a"], json!(1));
    assert_eq!(patch.deleted_fields, ["b"]);

    // A member that may be null is none when it is; the lists may only be left out.
    let nulls: BlockPatch = serde_json::from_value(json!({
        "parentId": null, "title": null,
    }))
    .expect("decodes");
    assert_eq!(nulls, BlockPatch::default());
    for list in ["updatedFields", "deletedFields"] {
        serde_json::from_value::<BlockPatch>(json!({ list: null }))
            .expect_err("a null list is refused");
    }
    assert_eq!(
        to_json(&BlockPatch::default()),
        json!({ "updatedFields": {}, "deletedFields": [] })
    );

    let batch: BlockPatchBatch =
        serde_json::from_value(json!({ "blockIds": ["a"], "blockPatches": [{ "title": "t" }] }))
            .expect("decodes");
    assert_eq!(batch.block_ids, ["a"]);
    assert_eq!(batch.block_patches[0].title.as_deref(), Some("t"));
}

#[test]
fn block_patch_apply_updates_then_deletes() {
    let mut block = Block {
        title: "old".to_owned(),
        fields: serde_json::from_value(json!({ "keep": 1, "drop": 2, "both": 3 })).expect("object"),
        ..Block::default()
    };
    let patch: BlockPatch = serde_json::from_value(json!({
        "title": "new",
        "updatedFields": { "added": 4, "both": 5 },
        "deletedFields": ["drop", "both", "absent"]
    }))
    .expect("decodes");

    patch.apply(&mut block);

    assert_eq!(block.title, "new");
    assert_eq!(
        Value::Object(block.fields),
        json!({ "keep": 1, "added": 4 })
    );
}

#[test]
fn container_json_members() {
    let board = Container {
        id: "b1".to_owned(),
        workspace_id: "0".to_owned(),
        created_by: "u1".to_owned(),
        modified_by: "u1".to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Board,
        minimum_role: MinimumRole::Commenter,
        title: "Roadmap".to_owned(),
        description: "d".to_owned(),
        icon: "🚀".to_owned(),
        show_description: true,
        is_template: false,
        is_builtin: false,
        template_version: 6,
        properties: serde_json::from_value(json!({ "k": "v" })).expect("object"),
        card_properties: vec![CardPropertyDefinition {
            id: "p1".to_owned(),
            name: "Status".to_owned(),
            property_type: PropertyType::Select,
            options: vec![PropertyOption::new("o1", "Open", PropertyColor::Blue)],
        }],
        due_date_property_id: None,
        done_when: None,
        created_at: 1,
        updated_at: 2,
        deleted_at: None,
    };
    let want = json!({
        "id": "b1", "workspaceId": "0", "createdBy": "u1", "modifiedBy": "u1",
        "visibility": "private", "kind": "board", "minimumRole": "commenter", "title": "Roadmap", "description": "d",
        "icon": "🚀", "showDescription": true, "isTemplate": false, "isBuiltin": false, "templateVersion": 6,
        "properties": { "k": "v" },
        "cardProperties": [{
            "id": "p1",
            "name": "Status",
            "type": "select",
            "options": [{ "id": "o1", "label": "Open", "color": "blue" }]
        }],
        "createdAt": 1, "updatedAt": 2
    });
    assert_eq!(to_json(&board), want);
    assert_eq!(
        serde_json::from_value::<Container>(want).expect("decodes"),
        board
    );
    let trashed = Container {
        deleted_at: Some(99),
        ..board
    };
    assert_eq!(to_json(&trashed)["deletedAt"], json!(99));
}

#[test]
fn container_requires_a_valid_visibility_and_role() {
    let minimal: Container =
        serde_json::from_value(json!({ "visibility": "open" })).expect("decodes");
    assert_eq!(minimal, Container::default());
    // A board always has a minimum role. One that names none starts with editor.
    assert_eq!(minimal.minimum_role, MinimumRole::Editor);
    assert_eq!(to_json(&minimal)["minimumRole"], json!("editor"));
    serde_json::from_value::<Container>(json!({ "visibility": "open", "minimumRole": null }))
        .expect_err("a null minimum role is refused: it may only be left out");
    for role in MinimumRole::ALL {
        let board: Container =
            serde_json::from_value(json!({ "visibility": "open", "minimumRole": role.as_str() }))
                .expect("decodes");
        assert_eq!(board.minimum_role, role);
    }

    // Empty is no minimum role, and neither is admin: nobody becomes an administrator
    // of a board by joining it.
    for body in [
        json!({}),
        json!({ "visibility": "" }),
        json!({ "visibility": "X" }),
        json!({ "visibility": "Open" }),
        json!({ "visibility": "O" }),
        json!({ "visibility": "P" }),
        json!({ "type": "O" }),
        json!({ "type": "P" }),
        json!({ "type": "open" }),
        json!({ "visibility": "open", "minimumRole": "owner" }),
        json!({ "visibility": "open", "minimumRole": "" }),
        json!({ "visibility": "open", "minimumRole": "admin" }),
        json!({ "visibility": "open", "minimumRole": "Editor" }),
    ] {
        assert!(
            serde_json::from_value::<Container>(body.clone()).is_err(),
            "{body} must not decode"
        );
    }
}

#[test]
fn container_patch_apply_merges_card_properties_in_order() {
    let mut board = Container {
        properties: serde_json::from_value(json!({ "keep": 1, "drop": 2 })).expect("object"),
        card_properties: serde_json::from_value(json!([
            { "id": "a", "name": "A" },
            { "name": "no id" },
            { "id": "b", "name": "B" },
            { "id": "c", "name": "C" }
        ]))
        .expect("list of objects"),
        ..Container::default()
    };
    let patch: ContainerPatch = serde_json::from_value(json!({
        "visibility": "private", "minimumRole": "viewer", "title": "t", "description": "d", "icon": "i",
        "showDescription": true,
        "updatedProperties": { "new": 3 }, "deletedProperties": ["drop"],
        "updatedCardProperties": [{ "id": "b", "name": "B2" }, { "id": "d", "name": "D" }, { "name": "skipped" }],
        "deletedCardProperties": ["a"]
    }))
    .expect("decodes");

    patch.apply(&mut board);

    assert_eq!(board.visibility, ContainerVisibility::Private);
    assert_eq!(board.minimum_role, MinimumRole::Viewer);
    assert_eq!(
        (board.title.as_str(), board.description.as_str()),
        ("t", "d")
    );
    assert_eq!(board.icon, "i");
    assert!(board.show_description, "showDescription is set");
    assert_eq!(
        Value::Object(board.properties),
        json!({ "keep": 1, "new": 3 })
    );
    assert_eq!(
        to_json(&board.card_properties),
        json!([
            { "id": "b", "name": "B2", "type": "text", "options": [] },
            { "id": "c", "name": "C", "type": "text", "options": [] },
            { "id": "d", "name": "D", "type": "text", "options": [] }
        ])
    );
}

#[test]
fn container_patch_without_card_property_changes_keeps_them_untouched() {
    let card_properties = json!([
        { "id": "", "name": "no id", "type": "text", "options": [] },
        { "id": "a", "name": "", "type": "text", "options": [] }
    ]);
    let mut board = Container {
        card_properties: serde_json::from_value(card_properties.clone()).expect("list"),
        ..Container::default()
    };

    ContainerPatch {
        title: Some("t".to_owned()),
        ..ContainerPatch::default()
    }
    .apply(&mut board);

    assert_eq!(to_json(&board.card_properties), card_properties);
}

/// A property that the same patch deletes and updates comes out once, updated, at its
/// first position rather than duplicated.
#[test]
fn container_patch_deleting_and_updating_a_card_property_keeps_one() {
    let mut board = Container {
        card_properties: serde_json::from_value(
            json!([{ "id": "a", "name": "v1" }, { "id": "b" }]),
        )
        .expect("list"),
        ..Container::default()
    };
    let patch: ContainerPatch = serde_json::from_value(json!({
        "updatedCardProperties": [{ "id": "a", "name": "v2" }],
        "deletedCardProperties": ["a"]
    }))
    .expect("decodes");

    patch.apply(&mut board);

    assert_eq!(
        to_json(&board.card_properties),
        json!([
            { "id": "a", "name": "v2", "type": "text", "options": [] },
            { "id": "b", "name": "", "type": "text", "options": [] }
        ]),
        "one definition per identifier"
    );
}

#[test]
fn container_member_json_members() {
    let member = ContainerMember {
        container_id: "b".to_owned(),
        user_id: "u".to_owned(),
        role: ContainerRole::Viewer,
        synthetic: true,
        inherited_role: Some(MinimumRole::Editor),
    };
    // Direct role and inherited role remain distinguishable on the wire.
    assert_eq!(
        to_json(&member),
        json!({
            "containerId": "b",
            "userId": "u",
            "role": "viewer",
            "synthetic": true,
            "inheritedRole": "editor",
        })
    );
    assert_eq!(member.effective_role(), ContainerRole::Editor);

    // Every closed-set role decodes and serializes identically, and invalid or missing
    // roles are refused.
    for role in ContainerRole::ALL {
        let patch: ContainerMemberPatch =
            serde_json::from_value(json!({ "role": role.as_str() })).expect("decodes");
        assert_eq!(patch, ContainerMemberPatch { role });
        assert_eq!(to_json(&patch), json!({ "role": role.as_str() }));
    }
    for invalid in [
        json!({}),
        json!({ "role": "" }),
        json!({ "role": "owner" }),
        json!({ "role": "Admin" }),
        json!({ "role": null }),
    ] {
        assert!(
            serde_json::from_value::<ContainerMember>(invalid.clone()).is_err(),
            "{invalid} must not decode as ContainerMember"
        );
        assert!(
            serde_json::from_value::<ContainerMemberPatch>(invalid.clone()).is_err(),
            "{invalid} must not decode as ContainerMemberPatch"
        );
    }
}

#[test]
fn category_json_members() {
    let category = Category {
        id: "7c".to_owned(),
        name: "Boards".to_owned(),
        user_id: "u".to_owned(),
        workspace_id: "0".to_owned(),
        created_at: 1,
        updated_at: 2,
        deleted_at: None,
        collapsed: true,
        sort_order: 20,
        category_type: CategoryType::System,
    };
    let want = json!({
        "id": "7c", "name": "Boards", "userId": "u", "workspaceId": "0", "createdAt": 1,
        "updatedAt": 2, "collapsed": true, "sortOrder": 20,
        "type": "system"
    });
    assert_eq!(to_json(&category), want);
    assert_eq!(
        serde_json::from_value::<Category>(want).expect("decodes"),
        category
    );

    let defaulted: Category = serde_json::from_value(json!({ "name": "n" })).expect("decodes");
    assert_eq!(defaulted.category_type, CategoryType::Custom);
}

#[test]
fn category_containers_json_flattens_the_category() {
    let category_boards = CategoryContainers {
        category: Category {
            id: "7c".to_owned(),
            name: "Boards".to_owned(),
            sort_order: 30,
            ..Category::default()
        },
        container_metadata: vec![CategoryContainerMetadata {
            container_id: "b1".to_owned(),
            hidden: true,
        }],
    };
    assert_eq!(
        to_json(&category_boards),
        json!({
            "id": "7c", "name": "Boards", "userId": "", "workspaceId": "", "createdAt": 0,
            "updatedAt": 0, "collapsed": false, "sortOrder": 30,
            "type": "custom", "containerMetadata": [{ "containerId": "b1", "hidden": true }]
        })
    );
}

#[test]
fn category_hydrate_and_validate() {
    let mut category = Category {
        name: "n".to_owned(),
        user_id: "u".to_owned(),
        workspace_id: "t".to_owned(),
        sort_order: -5,
        ..Category::default()
    };
    assert_eq!(
        category.validate().expect_err("no id yet").to_string(),
        "the category has no id"
    );

    category.hydrate(77);

    assert!(
        fvoci_model::is_id(&category.id),
        "category id {}",
        category.id
    );
    assert_eq!((category.created_at, category.updated_at), (77, 77));
    assert_eq!(category.sort_order, 0);
    assert_eq!(category.validate(), Ok(()));

    let blank_name = Category {
        name: "  ".to_owned(),
        ..category.clone()
    };
    assert_eq!(
        blank_name.validate().expect_err("blank name").to_string(),
        "the category has no name"
    );
    let no_user = Category {
        user_id: String::new(),
        ..category.clone()
    };
    assert_eq!(
        no_user.validate().expect_err("no user").to_string(),
        "the category names no user"
    );
    let no_workspace = Category {
        workspace_id: String::new(),
        ..category
    };
    assert_eq!(
        no_workspace
            .validate()
            .expect_err("no workspace")
            .to_string(),
        "the category names no workspace"
    );
}

#[test]
fn user_json_hides_credentials_and_carries_the_name() {
    let user = User {
        id: "u1".to_owned(),
        username: "alice".to_owned(),
        display_name: "Alice A.".to_owned(),
        name: PersonName {
            first_name: "Alice".to_owned(),
            last_name: "A".to_owned(),
            custom_name: "Alice A.".to_owned(),
            name_shown: NameShown::CustomName,
            title: Some(Title::Dr),
            credentials: vec![Credential::Phd, Credential::KrAttorney],
            ..PersonName::default()
        },
        email: "alice@example.com".to_owned(),
        email_verified_at: 0,
        password_hash: "hash".to_owned(),
        password_locked_at: 7,
        created_at: 0,
        updated_at: 5,
        deleted_at: None,
        is_admin: true,
    };
    assert_eq!(
        to_json(&user),
        json!({
            "id": "u1", "username": "alice", "displayName": "Alice A.", "firstName": "Alice",
            "lastName": "A", "latinFirstName": "", "latinLastName": "",
            "customName": "Alice A.", "nameShown": "custom_name", "title": "dr",
            "credentials": ["phd", "kr_attorney"], "createdAt": 0, "updatedAt": 5,
            "isAdmin": true
        })
    );

    let debug = format!("{user:?}");
    assert!(!debug.contains("hash"), "Debug leaks: {debug}");

    let decoded: User = serde_json::from_value(json!({
        "id": "u1", "email": "x@example.com", "password": "p", "permissions": null
    }))
    .expect("decodes");
    assert_eq!(
        (decoded.email.as_str(), decoded.password_hash.as_str()),
        ("", "")
    );
}

#[test]
fn workspace_and_public_link_json_members() {
    let workspace = Workspace {
        id: "0".to_owned(),
        title: "t".to_owned(),
        settings: serde_json::from_value(json!({ "a": 1 })).expect("object"),
        modified_by: "u".to_owned(),
        updated_at: 3,
        guest_invitations: false,
    };
    assert_eq!(
        to_json(&workspace),
        json!({
            "id": "0", "title": "t", "settings": { "a": 1 },
            "modifiedBy": "u", "updatedAt": 3, "guestInvitations": false
        })
    );

    // What is answered about a public link: whether it is enabled, the token while
    // there is one, and when. Whose board it is and who changed it are not part of the answer.
    let link = PublicLink {
        container_id: "b".to_owned(),
        enabled: true,
        token: "tok".to_owned(),
        modified_by: "u".to_owned(),
        updated_at: 9,
    };
    assert_eq!(
        to_json(&link),
        json!({ "enabled": true, "token": "tok", "updatedAt": 9 })
    );
    assert_eq!(
        to_json(&PublicLink {
            container_id: "b".to_owned(),
            ..PublicLink::default()
        }),
        json!({ "enabled": false }),
        "a board that never had a public link"
    );
}

#[test]
fn subscription_json_members() {
    let subscription = Subscription {
        block_type: BlockType::Card,
        block_id: "c".to_owned(),
        user_id: "u".to_owned(),
        notified_at: 0,
        created_at: 1,
        deleted_at: None,
    };
    let want = json!({
        "blockType": "card", "blockId": "c", "userId": "u",
        "notifiedAt": 0, "createdAt": 1
    });
    assert_eq!(to_json(&subscription), want);
    assert_eq!(
        serde_json::from_value::<Subscription>(want).expect("decodes"),
        subscription
    );
    let cancelled = Subscription {
        deleted_at: Some(8),
        ..subscription
    };
    assert_eq!(to_json(&cancelled)["deletedAt"], json!(8));

    let subscriber = Subscriber {
        user_id: "u".to_owned(),
        notified_at: 4,
    };
    assert_eq!(
        to_json(&subscriber),
        json!({ "userId": "u", "notifiedAt": 4 })
    );
}

#[test]
fn config_error_and_request_json_members() {
    assert_eq!(
        to_json(&ClientConfig {
            max_file_bytes: 7,
            max_import_bytes: 9,
            allow_public_links: true,
            name_display: "username".to_owned(),
            ..ClientConfig::default()
        }),
        json!({
            "allowPublicLinks": true,
            "nameDisplay": "username", "featureFlags": {}, "maxFileBytes": 7,
            "maxImportBytes": 9, "searchEnabled": false, "trashRetentionDays": 0
        })
    );

    // A problem says its type, title, status and detail; the other members appear only
    // when set.
    assert_eq!(
        to_json(&Problem {
            problem_type: "/problems/not-found".to_owned(),
            title: "Not found".to_owned(),
            status: 404,
            detail: "board not found".to_owned(),
            ..Problem::default()
        }),
        json!({
            "type": "/problems/not-found", "title": "Not found", "status": 404,
            "detail": "board not found"
        })
    );
    assert_eq!(
        to_json(&Problem {
            problem_type: "/problems/r".to_owned(),
            title: "R".to_owned(),
            status: 413,
            detail: "the request body is too large".to_owned(),
            instance: Some("urn:uuid:id".to_owned()),
            reason: Some("r".to_owned()),
            errors: vec![
                InvalidInput {
                    pointer: Some("/f".to_owned()),
                    detail: "too long".to_owned(),
                    reason: "too_long".to_owned(),
                    ..InvalidInput::default()
                },
                InvalidInput {
                    parameter: Some("page".to_owned()),
                    detail: "no number".to_owned(),
                    reason: "invalid".to_owned(),
                    ..InvalidInput::default()
                },
            ],
            limit: Some(4_194_304),
            retry_after: Some(30),
            request_id: Some("id".to_owned()),
            org_id: Some("org".to_owned()),
        }),
        json!({
            "type": "/problems/r",
            "title": "R",
            "status": 413,
            "detail": "the request body is too large",
            "instance": "urn:uuid:id",
            "reason": "r",
            "errors": [
                { "pointer": "/f", "detail": "too long", "reason": "too_long" },
                { "parameter": "page", "detail": "no number", "reason": "invalid" },
            ],
            "limit": 4_194_304,
            "retryAfter": 30,
            "requestId": "id",
            "orgId": "org",
        })
    );

    // Extra fields sent on login requests are ignored rather than rejected.
    let login: LoginRequest = serde_json::from_value(json!({
        "type": "normal", "username": "u", "email": "e", "password": "p", "mfa_token": "m"
    }))
    .expect("decodes");
    assert_eq!((login.username.as_str(), login.email.as_str()), ("u", "e"));
    assert!(
        !format!("{login:?}").contains("\"p\""),
        "Debug leaks the password"
    );

    let client_state: ClientStatePatch =
        serde_json::from_value(json!({ "updatedFields": { "a": "b" } })).expect("decodes");
    assert_eq!(client_state.updated_fields["a"], "b");
    assert!(
        client_state.deleted_fields.is_empty(),
        "a list left out is empty"
    );
    serde_json::from_value::<ClientStatePatch>(
        json!({ "updatedFields": {}, "deletedFields": null }),
    )
    .expect_err("a null list is refused");
}

#[test]
fn subscription_validate() {
    let valid = Subscription {
        block_type: BlockType::Card,
        block_id: "c".to_owned(),
        user_id: "u".to_owned(),
        notified_at: 0,
        created_at: 0,
        deleted_at: None,
    };
    assert_eq!(valid.validate(), Ok(()));

    let cases = [
        (
            Subscription {
                block_id: String::new(),
                ..valid.clone()
            },
            "missing block id",
        ),
        (
            Subscription {
                user_id: String::new(),
                ..valid
            },
            "missing user id",
        ),
    ];
    for (subscription, message) in cases {
        let error = subscription.validate().expect_err(message);
        assert_eq!(error.to_string(), message);
    }
    for invalid in [
        json!({ "blockId": "c", "userId": "u" }),
        json!({ "blockType": "", "blockId": "c", "userId": "u" }),
        json!({ "blockType": "text", "blockId": "c", "userId": "u" }),
    ] {
        serde_json::from_value::<Subscription>(invalid)
            .expect_err("missing or unknown blockType is rejected at deserialization");
    }
}

/// `base` decodes; each of its members is refused when it is null, and may be left
/// out unless it is one of `required`.
fn refuses_null_members<T: serde::de::DeserializeOwned>(
    base: &Value,
    required: &[&str],
    what: &str,
) {
    assert!(
        serde_json::from_value::<T>(base.clone()).is_ok(),
        "{what} decodes"
    );
    let members = base.as_object().expect("an object");
    for member in members.keys() {
        let mut nulled = base.clone();
        nulled[member.as_str()] = Value::Null;
        assert!(
            serde_json::from_value::<T>(nulled).is_err(),
            "{what}: a null {member} is refused"
        );
        if required.contains(&member.as_str()) {
            continue;
        }
        let mut left_out = base.clone();
        left_out
            .as_object_mut()
            .expect("an object")
            .remove(member.as_str());
        assert!(
            serde_json::from_value::<T>(left_out).is_ok(),
            "{what}: {member} may be left out"
        );
    }
}

/// `null` is a value only for a member that may be `null` (an `Option`, `null` in the
/// contract's schema), and is then none. For every other member it is refused, as any
/// value of the wrong type is: a member that may be left out is left out instead.
#[test]
fn null_is_refused_for_members_that_are_not_nullable() {
    refuses_null_members::<Block>(
        &json!({
            "id": "a1", "parentId": "", "createdBy": "u1", "modifiedBy": "u1",
            "type": "card", "title": "t", "fields": {}, "createdAt": 1, "updatedAt": 1,
            "deletedAt": 2, "containerId": "b1",
        }),
        &["type"],
        "a block",
    );
    refuses_null_members::<Container>(
        &json!({
            "id": "b1", "workspaceId": "w1", "createdBy": "u1", "modifiedBy": "u1",
            "visibility": "private", "minimumRole": "editor", "title": "t", "description": "", "icon": "",
            "showDescription": false, "isTemplate": false, "templateVersion": 0,
            "properties": {}, "cardProperties": [], "createdAt": 1, "updatedAt": 1,
            "deletedAt": 2,
        }),
        &["visibility"],
        "a board",
    );
    refuses_null_members::<ContainerMember>(
        &json!({
            "containerId": "b1", "userId": "u1", "role": "viewer",
            "synthetic": false, "inheritedRole": "editor",
        }),
        &["role"],
        "a member",
    );
    refuses_null_members::<Category>(
        &json!({
            "id": "c1", "name": "n", "userId": "u1", "workspaceId": "w1", "createdAt": 1,
            "updatedAt": 1, "deletedAt": 2, "collapsed": false, "sortOrder": 0,
        }),
        &[],
        "a category",
    );
    refuses_null_members::<Card>(
        &json!({
            "id": "c1", "boardId": "b1", "createdBy": "u1",
            "modifiedBy": "u1", "title": "t", "icon": "", "isTemplate": false,
            "properties": {}, "hasBody": false, "tasksTotal": 0, "tasksDone": 0,
            "createdAt": 1, "updatedAt": 1, "deletedAt": 2, "lastCommentAt": 0,
        }),
        &[],
        "a card",
    );
    refuses_null_members::<Subscription>(
        &json!({
            "blockType": "card", "blockId": "c1", "userId": "u1",
            "notifiedAt": 0, "createdAt": 1, "deletedAt": 2,
        }),
        &["blockType"],
        "a subscription",
    );
    refuses_null_members::<OrgWorkspace>(
        &json!({
            "id": "w1", "name": "Design", "memberCount": 1, "isDefault": false,
            "deletedAt": 2, "joinedAsOrgAdmin": false,
        }),
        &["id", "name", "memberCount", "isDefault", "joinedAsOrgAdmin"],
        "an org workspace",
    );
    refuses_null_members::<User>(
        &json!({
            "id": "u1", "username": "ada", "displayName": "Ada",
            "createdAt": 1, "updatedAt": 1, "deletedAt": 2, "isAdmin": false,
        }),
        &[],
        "a user",
    );
    refuses_null_members::<LoginRequest>(
        &json!({ "username": "u", "password": "p" }),
        &[],
        "a login request",
    );
    refuses_null_members::<CardPropertyDefinition>(
        &json!({
            "id": "p1",
            "name": "Status",
            "type": "select",
            "options": [{ "id": "o1", "label": "Open", "color": "green" }],
        }),
        &[],
        "a card property definition",
    );
    refuses_null_members::<PropertyOption>(
        &json!({ "id": "o1", "label": "Open", "color": "green" }),
        &["id", "label"],
        "a property option",
    );

    // A member that may be null is none when it is.
    let block: Block = serde_json::from_value(json!({ "id": "a1", "type": "card", "body": null }))
        .expect("decodes");
    assert_eq!(block.body, None, "a null body is no body");
}
