//! Validation rules for blocks, boards, card properties, and authentication requests.

use fvoci_model::{
    BLOCK_FIELDS_MAX_CHARS, BLOCK_TITLE_MAX_CHARS, Block, BlockError, BlockType, Body,
    CARD_FIELDS_MAX_CHARS, CardPropertyDefinition, ChangePasswordRequest, Container,
    ContainerError, ContainerSearchField, NewAccount, PropertyType, RegisterRequest,
    is_email_valid,
};
use serde_json::json;

fn block() -> Block {
    Block {
        container_id: "board-id".to_owned(),
        ..Block::default()
    }
}

#[test]
fn block_validate_requires_a_board() {
    assert_eq!(
        Block::default().validate(),
        Err(BlockError::EmptyContainerId)
    );
    assert_eq!(block().validate(), Ok(()));
    assert_eq!(
        BlockError::EmptyContainerId.to_string(),
        "the block names no board"
    );
}

/// A comment's body has no file block, though it may have an image; a block of a type
/// without a body has none at all.
#[test]
fn block_validate_keeps_file_blocks_out_of_comments() {
    let body = |kind: &str| {
        Body::parse(json!({"type": "doc", "content": [{"type": kind, "attrs": {"fileId": "7a"}}]}))
            .expect("an allowed body")
    };
    let of = |block_type, kind: &str| Block {
        block_type,
        body: body(kind),
        ..block()
    };
    assert_eq!(
        of(BlockType::Comment, "file").validate(),
        Err(BlockError::FileBlockInComment)
    );
    assert_eq!(of(BlockType::Comment, "image").validate(), Ok(()));
    assert_eq!(of(BlockType::Card, "file").validate(), Ok(()));
    assert_eq!(
        of(BlockType::View, "file").validate(),
        Err(BlockError::BodyNotAllowed)
    );
}

#[test]
fn block_validate_limits_the_title_in_characters() {
    // The limit counts characters, not bytes: a three-byte character counts once.
    let at_limit = Block {
        title: "가".repeat(BLOCK_TITLE_MAX_CHARS),
        ..block()
    };
    assert_eq!(at_limit.validate(), Ok(()));

    let over_limit = Block {
        title: "a".repeat(BLOCK_TITLE_MAX_CHARS + 1),
        ..block()
    };
    assert_eq!(
        over_limit.validate(),
        Err(BlockError::TitleSizeLimitExceeded)
    );
    assert_eq!(BLOCK_TITLE_MAX_CHARS, 16383);
}

#[test]
fn block_validate_limits_the_serialized_fields_by_type() {
    // {"k":"<value>"} is the value plus eight characters of JSON syntax; for View,
    // {"defaultTemplateId":"<value>"} is the value plus twenty-four characters.
    let with_value_len = |block_type: &BlockType, key: &str, len: usize| -> Block {
        let mut fields = fvoci_model::JsonMap::new();
        fields.insert(key.to_owned(), json!("a".repeat(len)));
        Block {
            block_type: *block_type,
            fields,
            ..block()
        }
    };
    assert_eq!(CARD_FIELDS_MAX_CHARS, 64_000);
    assert_eq!(BLOCK_FIELDS_MAX_CHARS, 800_000);
    let cases = [
        (BlockType::Card, "k", 8, CARD_FIELDS_MAX_CHARS),
        (BlockType::Comment, "k", 8, CARD_FIELDS_MAX_CHARS),
        (BlockType::Attachment, "k", 8, CARD_FIELDS_MAX_CHARS),
        (
            BlockType::View,
            "defaultTemplateId",
            24,
            BLOCK_FIELDS_MAX_CHARS,
        ),
    ];
    for (block_type, key, overhead, limit) in cases {
        assert_eq!(block_type.fields_max_chars(), limit, "{block_type}");
        assert_eq!(
            with_value_len(&block_type, key, limit - overhead).validate(),
            Ok(()),
            "{block_type} at the limit"
        );
        assert_eq!(
            with_value_len(&block_type, key, limit - overhead + 1).validate(),
            Err(BlockError::FieldsSizeLimitExceeded),
            "{block_type} over the limit"
        );
    }
}

/// A new block is a card, a view, a comment or an attachment.
#[test]
fn only_four_types_are_current() {
    for block_type in [
        BlockType::Card,
        BlockType::View,
        BlockType::Comment,
        BlockType::Attachment,
    ] {
        assert!(block_type.is_current(), "{block_type}");
    }
    for block_type in [
        BlockType::Page,
        BlockType::Entry,
        BlockType::Unit,
        BlockType::Chapter,
    ] {
        assert!(!block_type.is_current(), "{block_type}");
    }
}

/// Container-specific block types map to their dedicated refusal errors.
#[test]
fn non_board_block_types_name_their_operations() {
    assert_eq!(
        BlockError::type_not_current(&BlockType::Page),
        BlockError::PageByBlocks
    );
    assert_eq!(
        BlockError::type_not_current(&BlockType::Entry),
        BlockError::EntryByBlocks
    );
    assert_eq!(
        BlockError::type_not_current(&BlockType::Unit),
        BlockError::UnitByBlocks
    );
    assert_eq!(
        BlockError::type_not_current(&BlockType::Chapter),
        BlockError::UnitByBlocks
    );
}

#[test]
fn container_validate_requires_a_workspace() {
    assert_eq!(
        Container::default().validate(),
        Err(ContainerError::EmptyWorkspaceId)
    );
    assert_eq!(
        ContainerError::EmptyWorkspaceId.to_string(),
        "empty-workspace-id"
    );
    let board = Container {
        workspace_id: "0".to_owned(),
        ..Container::default()
    };
    assert_eq!(board.validate(), Ok(()));
}

#[test]
fn container_search_field_parses_the_two_wire_names() {
    assert_eq!("title".parse(), Ok(ContainerSearchField::Title));
    assert_eq!(
        "property_name".parse(),
        Ok(ContainerSearchField::PropertyName)
    );
    for other in ["", "Title", "name"] {
        let error = other.parse::<ContainerSearchField>().expect_err(other);
        assert_eq!(error.to_string(), "invalid board search field");
    }
}

fn register(username: &str, email: &str, password: &str) -> Result<(), String> {
    RegisterRequest {
        username: username.to_owned(),
        email: email.to_owned(),
        password: password.to_owned(),
        display_name: String::new(),
        organization_name: "The Firm".to_owned(),
        language: String::new(),
    }
    .validate()
    .map_err(|error| error.to_string())
}

/// What a registration and an account asked for are checked into is what they are
/// once tidied, and only what is then accepted is: the name and address without the
/// white space around them, the organization as it is stored, none for a blank one.
#[test]
fn a_checked_registration_is_the_tidied_request_that_is_accepted() {
    let request = |username: &str, organization_name: &str| RegisterRequest {
        username: username.to_owned(),
        email: " alice@example.com ".to_owned(),
        password: "12345678".to_owned(),
        display_name: String::new(),
        organization_name: organization_name.to_owned(),
        language: "ko".to_owned(),
    };
    let registration = request(" alice ", "  우리 팀  ")
        .check_as_first()
        .expect("a first registration that names its organization");
    assert_eq!(registration.account().username(), "alice");
    assert_eq!(registration.account().email(), "alice@example.com");
    assert_eq!(registration.organization(), Some("우리 팀"));
    assert_eq!(registration.language(), "ko");
    let alone = request("alice", "   ")
        .check()
        .expect("a registration of no organization");
    assert_eq!(alone.organization(), None);
    assert!(
        request("alice", "   ").check_as_first().is_err(),
        "the first names one"
    );
    assert!(
        request("al@ice", "").check().is_err(),
        "a name with @ is refused"
    );
    assert!(
        !format!("{:?}", alone.account()).contains("12345678"),
        "the password is not debugged"
    );

    let account = NewAccount {
        username: " bob ".to_owned(),
        email: "bob@example.com".to_owned(),
        password: "12345678".to_owned(),
        display_name: String::new(),
    };
    assert_eq!(
        account.clone().check().expect("an account").username(),
        "bob"
    );
    let short = NewAccount {
        password: "1234".to_owned(),
        ..account
    };
    assert_eq!(
        short.clone().check().map(|_| ()),
        short.tidied().validate(),
        "refused as the tidied account is"
    );
}

/// A registration names the organization it starts, or none: a name that is left out
/// or blank is no organization, and the account starts with its own space alone. The
/// first registration of a server starts the server's organization and must name it.
#[test]
fn a_registration_names_its_organization_or_none() {
    let request = |organization_name: &str| RegisterRequest {
        username: "alice".to_owned(),
        email: "alice@example.com".to_owned(),
        password: "12345678".to_owned(),
        display_name: String::new(),
        organization_name: organization_name.to_owned(),
        language: String::new(),
    };
    let named = |organization_name: &str| {
        request(organization_name)
            .validate()
            .map_err(|error| error.to_string())
    };
    let first = |organization_name: &str| {
        request(organization_name)
            .validate_as_first()
            .map_err(|error| error.to_string())
    };
    assert_eq!(first("  우리 팀  "), Ok(()));
    for none in ["", "   "] {
        assert_eq!(named(none), Ok(()), "no organization is started");
        assert_eq!(request(none).organization(), None);
        assert_eq!(
            first(none),
            Err("organization name is required".to_owned()),
            "the first registration names the server's organization"
        );
    }
    assert!(request("  우리 팀  ").organization().is_some());
    assert_eq!(named("  우리 팀  "), Ok(()));
    assert_eq!(
        fvoci_model::organization_name("  우리 팀  "),
        Ok("우리 팀".to_owned())
    );
    assert_eq!(named(&"가".repeat(64)), Ok(()));
    assert_eq!(
        named(&"가".repeat(65)),
        Err("an organization name has at most 64 characters".to_owned())
    );
    assert!(
        named("a\u{202E}b").is_err(),
        "nothing that turns the text around"
    );
    // Every member that is refused is named, the organization after the account: of
    // a first registration the missing organization with the rest, of any other the
    // rest alone.
    let all = RegisterRequest::default()
        .validate_as_first()
        .expect_err("nothing is there");
    let details: Vec<&str> = all.iter().map(|refusal| refusal.detail.as_str()).collect();
    assert_eq!(
        details,
        [
            "username is required",
            "email is required",
            "password is required",
            "organization name is required"
        ]
    );
    let later = RegisterRequest::default()
        .validate()
        .expect_err("nothing is there");
    let details: Vec<&str> = later
        .iter()
        .map(|refusal| refusal.detail.as_str())
        .collect();
    assert_eq!(
        details,
        [
            "username is required",
            "email is required",
            "password is required"
        ]
    );
}

#[test]
fn register_request_validate() {
    assert_eq!(register("alice", "alice@example.com", "12345678"), Ok(()));

    let cases = [
        (
            ("", "alice@example.com", "12345678"),
            "username is required",
        ),
        (
            ("  ", "alice@example.com", "12345678"),
            "username is required",
        ),
        (("alice", "", "12345678"), "email is required"),
        (
            ("alice", "not-an-email", "12345678"),
            "invalid email format",
        ),
        (("alice", "alice@example.com", ""), "password is required"),
        (
            ("alice", "alice@example.com", "1234567"),
            "password must be at least 8 characters",
        ),
    ];
    for ((username, email, password), message) in cases {
        assert_eq!(
            register(username, email, password),
            Err(message.to_owned()),
            "{username:?} {email:?} {password:?}"
        );
    }
}

#[test]
fn the_minimum_password_length_is_counted_in_characters() {
    // Four two-byte characters are eight bytes, and four characters: too short.
    assert_eq!(
        register("alice", "alice@example.com", "éééé"),
        Err("password must be at least 8 characters".to_owned())
    );
    // Eight characters, of whatever width, meet the minimum.
    assert_eq!(
        register("alice", "alice@example.com", "비밀번호여덟글자"),
        Ok(())
    );
    assert_eq!(register("alice", "alice@example.com", "éééééééé"), Ok(()));
}

#[test]
fn the_maximum_password_length_is_counted_in_bytes() {
    let longest = "a".repeat(fvoci_model::MAXIMUM_PASSWORD_BYTES);
    assert_eq!(register("alice", "alice@example.com", &longest), Ok(()));
    let too_long = "a".repeat(fvoci_model::MAXIMUM_PASSWORD_BYTES + 1);
    assert_eq!(
        register("alice", "alice@example.com", &too_long),
        Err("password must be at most 1024 bytes".to_owned())
    );
    // 342 three-byte characters are 1026 bytes.
    let wide = "가".repeat(342);
    assert_eq!(
        register("alice", "alice@example.com", &wide),
        Err("password must be at most 1024 bytes".to_owned())
    );
    assert_eq!(
        fvoci_model::check_new_password(&too_long).map_err(|error| error.to_string()),
        Err("password must be at most 1024 bytes".to_owned()),
        "the same rule for every way a password is set"
    );
}

#[test]
fn change_password_request_validate() {
    let check = |old: &str, new: &str| -> Result<(), String> {
        ChangePasswordRequest {
            old_password: old.to_owned(),
            new_password: new.to_owned(),
        }
        .validate()
        .map_err(|error| error.to_string())
    };
    assert_eq!(check("old", "12345678"), Ok(()));
    assert_eq!(
        check("", "12345678"),
        Err("old password is required".to_owned())
    );
    assert_eq!(check("old", ""), Err("new password is required".to_owned()));
    assert_eq!(
        check("old", "short"),
        Err("password must be at least 8 characters".to_owned())
    );
}

#[test]
fn email_validation_matches_expected_pattern() {
    let label63 = "a".repeat(63);
    let valid = [
        "a@b".to_owned(),
        "test@mail.com".to_owned(),
        "first.last+tag@sub.example.co".to_owned(),
        "!#$%&'*+/=?^_`{|}~-@example.com".to_owned(),
        "user@a-b.example".to_owned(),
        "user@1.2".to_owned(),
        format!("user@{label63}.com"),
    ];
    for email in &valid {
        assert!(is_email_valid(email), "{email} should be valid");
    }

    let label64 = "a".repeat(64);
    let invalid = [
        String::new(),
        "a@".to_owned(),
        "@b".to_owned(),
        "ab".to_owned(),
        "a b@example.com".to_owned(),
        "a@b@c".to_owned(),
        "user@-example.com".to_owned(),
        "user@example-.com".to_owned(),
        "user@example..com".to_owned(),
        "user@example.com.".to_owned(),
        "user@.example.com".to_owned(),
        "user@exa_mple.com".to_owned(),
        "üser@example.com".to_owned(),
        "user@example.com\n".to_owned(),
        format!("user@{label64}.com"),
        format!("{}@example.com", "a".repeat(243)),
    ];
    for email in &invalid {
        assert!(!is_email_valid(email), "{email:?} should be invalid");
    }

    // 254 bytes is the longest address accepted.
    let longest = format!("{}@{}.{}.{}.com", "a".repeat(58), label63, label63, label63);
    assert_eq!(longest.len(), 254);
    assert!(is_email_valid(&longest), "a 254-byte address is valid");
}

/// A new order of the card properties names each of them once: one left out, named twice
/// or unknown is refused and changes nothing; a board without properties takes the empty
/// order.
#[test]
fn card_properties_are_reordered_only_by_a_whole_order() {
    let property = |id: &str| CardPropertyDefinition::new(id, id, PropertyType::Text);
    let ids = |board: &Container| -> Vec<String> {
        board
            .card_properties
            .iter()
            .map(|property| property.id.clone())
            .collect()
    };
    let order = |ids: &[&str]| -> Vec<String> { ids.iter().map(|&id| id.to_owned()).collect() };
    let mut board = Container {
        card_properties: vec![property("a"), property("b"), property("c")],
        ..Container::default()
    };

    board
        .reorder_card_properties(&order(&["c", "a", "b"]))
        .expect("a whole order");
    assert_eq!(ids(&board), ["c", "a", "b"]);
    assert_eq!(
        board.reorder_card_properties(&order(&["a", "b"])),
        Err(ContainerError::PropertiesLeftOut { named: 2, total: 3 })
    );
    assert_eq!(
        board.reorder_card_properties(&order(&["a", "a", "b", "c"])),
        Err(ContainerError::PropertyNamedTwice { id: "a".to_owned() })
    );
    assert_eq!(
        board.reorder_card_properties(&order(&["a", "b", "c", "d"])),
        Err(ContainerError::PropertyUnknown { id: "d".to_owned() })
    );
    assert_eq!(ids(&board), ["c", "a", "b"], "nothing refused changed it");

    let mut empty = Container::default();
    empty
        .reorder_card_properties(&[])
        .expect("no properties, no order");
}
