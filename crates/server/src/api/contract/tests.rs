use serde_json::{Value, json};

use super::{
    Members, document, finish_inline_schemas, finish_operations, open_free_objects,
    path_parameters, rename_paths, require_members, sorted, strip_defaults,
};

#[test]
fn the_document_can_be_made() {
    let text = document().expect("the routes and schemas are fit for the contract");
    let document: Value = serde_json::from_str(&text).expect("the document is JSON");
    assert_eq!(
        document["openapi"], "3.1.0",
        "the version of OpenAPI the document is written in"
    );
    assert!(text.ends_with("}\n"), "the text ends with one line break");
}

/// The operations that answer some refusals in a protocol of their own, besides the
/// problem: the MCP server answers a body that is no JSON-RPC message with the JSON-RPC
/// error, as the transport has it.
const SPOKEN_IN_THEIR_PROTOCOL: [(&str, &str, &str); 1] = [("post", "/api/v1/mcp", "400")];

#[test]
fn every_operation_answers_a_refusal_with_the_problem() {
    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let paths = document["paths"].as_object().expect("paths");
    let mut operations = 0;
    for (path, item) in paths {
        for (method, operation) in item.as_object().expect("a path item") {
            operations += 1;
            let content = operation["responses"]["default"]["content"]
                .as_object()
                .expect("the content of the refusal");
            assert_eq!(content.len(), 1, "one media type: {method} {path}");
            assert_eq!(
                content["application/problem+json"]["schema"]["$ref"],
                "#/components/schemas/Problem",
                "the refusal of {method} {path}"
            );
            // A status an operation describes by itself is no other kind of body.
            for (status, response) in operation["responses"].as_object().expect("responses") {
                if !status.starts_with('4') && !status.starts_with('5') {
                    continue;
                }
                let content = response["content"]
                    .as_object()
                    .expect("an error has a body");
                let mut types: Vec<&str> = content.keys().map(String::as_str).collect();
                types.sort_unstable();
                let spoken = SPOKEN_IN_THEIR_PROTOCOL.contains(&(
                    method.as_str(),
                    path.as_str(),
                    status.as_str(),
                ));
                let expected: &[&str] = if spoken {
                    &["application/json", "application/problem+json"]
                } else {
                    &["application/problem+json"]
                };
                assert_eq!(types, expected, "the {status} of {method} {path}");
            }
        }
    }
    assert!(operations > 70, "the document has {operations} operations");
}

/// Every `reason` is named in the description of the API or of an operation: no
/// reason comes without a word of when. (That the committed contract lists the
/// kinds the code has is checked where the file is held against this document, and
/// by the API test that asks the server for the page of every kind it lists.)
#[test]
fn the_contract_says_when_each_reason_comes() {
    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let mut descriptions = vec![document["info"]["description"].as_str().expect("text")];
    for item in document["paths"].as_object().expect("paths").values() {
        for operation in item.as_object().expect("a path item").values() {
            descriptions.extend(operation["description"].as_str());
            let responses = operation["responses"].as_object().expect("responses");
            descriptions.extend(
                responses
                    .values()
                    .filter_map(|response| response["description"].as_str()),
            );
        }
    }
    for reason in crate::error::Reason::ALL {
        let name = format!("`{}`", reason.as_str());
        assert!(
            descriptions.iter().any(|text| text.contains(&name)),
            "no description says when the reason {name} comes"
        );
    }

    // The causes an entry of `errors` can have are named where the entry is
    // described.
    let entry_reason =
        document["components"]["schemas"]["InvalidInput"]["properties"]["reason"]["description"]
            .as_str()
            .expect("the description of the reason of an entry");
    for reason in fvoci_model::RefusalReason::ALL {
        assert!(
            entry_reason.contains(&format!("`{}`", reason.as_str())),
            "the contract does not name the cause {}",
            reason.as_str()
        );
    }
}

/// The operations that need no session at all.
const NO_SESSION: [&str; 17] = [
    "completeEmailVerification",
    "completePasswordReset",
    "confirmEmailChange",
    "finishHeldOidcLogin",
    "finishOidcLogin",
    "getClientConfig",
    "getLoginOptions",
    "login",
    "ping",
    "previewInvitation",
    "ready",
    "register",
    "requestEmailVerification",
    "requestPasswordReset",
    "revertEmailChange",
    "startOidcCarryOver",
    "startOidcLogin",
];
/// The operations that read a shared container with its read token, and look at no session.
const READ_TOKEN: [&str; 9] = [
    "getSharedBlocks",
    "getSharedCards",
    "getSharedContainer",
    "getSharedFile",
    "getSharedFileRecord",
    "getSharedPage",
    "getSharedPageTree",
    "getSharedUnit",
    "getSharedUnitTree",
];
/// The operations that take a session and do without one: the acceptance of an
/// invitation, which makes an account for whoever has none.
const SESSION_OR_NONE: [&str; 1] = ["acceptInvitation"];

#[test]
fn what_needs_no_session_says_so() {
    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let mut no_session = Vec::new();
    let mut read_token = Vec::new();
    let mut session_or_none = Vec::new();
    for (path, item) in document["paths"].as_object().expect("paths") {
        for operation in item.as_object().expect("a path item").values() {
            let name = operation["operationId"].as_str().expect("a name");
            let security = &operation["security"];
            if *security == json!([{}]) {
                if path.starts_with("/api/v1/shared/") {
                    read_token.push(name);
                } else {
                    no_session.push(name);
                }
            } else if *security == json!([{}, { "sessionCookie": [] }, { "bearerToken": [] }]) {
                session_or_none.push(name);
            } else {
                assert_eq!(
                    *security,
                    Value::Null,
                    "{name}: what says nothing needs what the document says for all"
                );
            }
        }
    }
    no_session.sort_unstable();
    read_token.sort_unstable();
    session_or_none.sort_unstable();
    assert_eq!(
        no_session, NO_SESSION,
        "these need no session, and no others"
    );
    assert_eq!(
        read_token, READ_TOKEN,
        "these read a shared board or wiki with its token, and are all under /shared"
    );
    assert_eq!(
        session_or_none, SESSION_OR_NONE,
        "these take a session and do without one, and no others"
    );
    assert_eq!(
        document["security"],
        json!([{ "sessionCookie": [] }, { "bearerToken": [] }]),
        "what every operation needs unless it says otherwise"
    );
}

#[test]
fn an_answer_has_every_member_and_a_patch_needs_none() {
    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let schemas = &document["components"]["schemas"];
    let workspace = &schemas["Workspace"];
    assert_eq!(
        workspace["required"].as_array().map(Vec::len),
        workspace["properties"]
            .as_object()
            .map(serde_json::Map::len),
        "every member of a workspace is always there"
    );
    let board = &schemas["Container"];
    assert_eq!(
        board["required"].as_array().map(Vec::len),
        board["properties"]
            .as_object()
            .map(|members| members.len() - 3),
        "every member of a board but deletedAt, dueDatePropertyId and doneWhen is always there"
    );
    assert_eq!(
        schemas["ContainerPatch"]["required"],
        Value::Null,
        "a patch needs no member"
    );
    assert_eq!(
        schemas["PublicLink"]["required"],
        json!(["enabled"]),
        "the token and the time are left out while a board has no public link"
    );
    assert_eq!(
        schemas["CategoryContainers"]["allOf"][1]["required"],
        json!(["containerMetadata"]),
        "the members of a category with its boards, beside those of the category"
    );
    assert!(board["required"].is_array() && board["properties"].is_object());
    assert_eq!(
        defaults_in(schemas, "components.schemas"),
        Vec::<String>::new(),
        "no schema carries the defaults of the decoding"
    );
    assert_eq!(
        defaults_in(&document["paths"], "paths"),
        Vec::<String>::new(),
        "nor does a schema that is written where it is used"
    );
}

/// Where below `value` something has a `default` that is no member of a schema's
/// `properties`, and no `default` answer of an operation.
fn defaults_in(value: &Value, place: &str) -> Vec<String> {
    match value {
        Value::Object(members) => members
            .iter()
            .flat_map(|(name, member)| {
                let inner = format!("{place}.{name}");
                let is_schema_default = name == "default"
                    && !place.ends_with(".properties")
                    && !place.ends_with(".responses");
                if is_schema_default {
                    vec![inner]
                } else {
                    defaults_in(member, &inner)
                }
            })
            .collect(),
        Value::Array(items) => items
            .iter()
            .flat_map(|item| defaults_in(item, place))
            .collect(),
        _ => Vec::new(),
    }
}

/// The members a value is written with.
fn sent<T: serde::Serialize>(value: &T) -> Vec<String> {
    serde_json::to_value(value)
        .expect("JSON")
        .as_object()
        .expect("an object")
        .keys()
        .cloned()
        .collect()
}

/// The members a schema says are always there, with those of the schemas it has
/// inline.
fn required_of(schemas: &Value, schema: &Value) -> Vec<String> {
    let mut required: Vec<String> = schema["required"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect();
    for part in schema["allOf"].as_array().into_iter().flatten() {
        let part = match part["$ref"].as_str() {
            Some(reference) => &schemas[reference.rsplit('/').next().expect("a name")],
            None => part,
        };
        required.extend(required_of(schemas, part));
    }
    required.sort_unstable();
    required
}

/// The table of schemas says by hand which members are always there. What the
/// server writes says it too: a value with nothing in it is written with exactly
/// the members that are never left out. A member that is given a
/// `skip_serializing_if` and forgotten in the table fails here.
#[test]
fn what_is_always_there_is_what_the_server_writes() {
    use fvoci_model::{
        Block, Card, CardPropertyDefinition, Category, CategoryContainerMetadata,
        CategoryContainers, ClientConfig, Container, ContainerMember, ContainersAndBlocks,
        FileRecord, PropertyOption, PublicLink, Subscription, User, Workspace,
    };

    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let schemas = &document["components"]["schemas"];
    let written = [
        ("Block", sent(&Block::default())),
        ("Card", sent(&Card::default())),
        (
            "CardPropertyDefinition",
            sent(&CardPropertyDefinition::default()),
        ),
        ("Category", sent(&Category::default())),
        (
            "CategoryContainerMetadata",
            sent(&CategoryContainerMetadata::default()),
        ),
        ("CategoryContainers", sent(&CategoryContainers::default())),
        ("ClientConfig", sent(&ClientConfig::default())),
        ("Container", sent(&Container::default())),
        (
            "ContainerMember",
            sent(&ContainerMember::new(
                "",
                "",
                fvoci_model::ContainerRole::Viewer,
            )),
        ),
        ("ContainersAndBlocks", sent(&ContainersAndBlocks::default())),
        ("FileRecord", sent(&FileRecord::default())),
        ("PropertyOption", sent(&PropertyOption::default())),
        ("PublicLink", sent(&PublicLink::default())),
        ("Subscription", sent(&Subscription::default())),
        ("Workspace", sent(&Workspace::default())),
        ("User", sent(&User::default())),
    ];
    for (name, mut members) in written {
        members.sort_unstable();
        assert_eq!(
            required_of(schemas, &schemas[name]),
            members,
            "the members of {name} that are always there"
        );
    }
}

/// The words of a closed set are those the server writes, letter for letter.
#[test]
fn a_closed_set_of_words_lists_the_words_as_they_are_written() {
    use fvoci_model::{
        BlockType, CardOpen, ContainerRole, Density, HourCycle, OrgKind, PropertyColor,
        PropertyType, Theme, UserSettings, WeekStart,
    };

    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let schemas = &document["components"]["schemas"];
    for (name, words) in [
        ("BlockType", BlockType::WORDS),
        ("CardOpen", CardOpen::WORDS),
        ("ContainerRole", ContainerRole::WORDS),
        ("Density", Density::WORDS),
        ("HourCycle", HourCycle::WORDS),
        ("OrgKind", OrgKind::WORDS),
        ("PropertyColor", PropertyColor::WORDS),
        ("PropertyType", PropertyType::WORDS),
        ("Theme", Theme::WORDS),
        ("WeekStart", WeekStart::WORDS),
    ] {
        assert_eq!(schemas[name]["enum"], json!(words), "{name}");
        assert!(schemas[name]["description"].is_string(), "{name}");
    }
    // Both the answer and the change name every setting there is.
    let mut every: Vec<&str> = fvoci_model::SettingName::ALL
        .iter()
        .map(|name| name.as_str())
        .collect();
    every.sort_unstable();
    for schema in ["UserSettings", "SettingsChange"] {
        let mut members: Vec<&str> = schemas[schema]["properties"]
            .as_object()
            .expect("members")
            .keys()
            .map(String::as_str)
            .collect();
        members.sort_unstable();
        assert_eq!(members, every, "{schema} names every setting");
    }
    // And what an answer carries is among them.
    let settings = serde_json::to_value(UserSettings::default()).expect("JSON");
    for (member, set) in [
        ("cardOpen", "CardOpen"),
        ("density", "Density"),
        ("hourCycle", "HourCycle"),
        ("theme", "Theme"),
        ("weekStart", "WeekStart"),
    ] {
        let words = schemas[set]["enum"].as_array().expect("words");
        assert!(
            words.contains(&settings[member]),
            "{member} is {}, which is none of {words:?}",
            settings[member]
        );
    }
    for (set, value) in [
        (
            "ContainerVisibility",
            json!(fvoci_model::ContainerVisibility::Open),
        ),
        (
            "ContainerVisibility",
            json!(fvoci_model::ContainerVisibility::Private),
        ),
        ("MinimumRole", json!(fvoci_model::MinimumRole::default())),
        ("CategoryType", json!(fvoci_model::CategoryType::default())),
        ("WorkspaceRole", json!(fvoci_model::WorkspaceRole::Admin)),
        ("WorkspaceRole", json!(fvoci_model::WorkspaceRole::Member)),
        ("WorkspaceRole", json!(fvoci_model::WorkspaceRole::Guest)),
    ] {
        let words = schemas[set]["enum"].as_array().expect("words");
        assert!(words.contains(&value), "{value} is none of {words:?}");
    }
}

/// A document with one operation, for what [`finish_operations`] refuses.
fn with_operation(path: &str, operation: &Value) -> Value {
    json!({
        "paths": { path: { "get": operation } },
        "components": { "securitySchemes": { "sessionCookie": {} } },
    })
}

#[test]
fn an_operation_that_is_not_fit_for_the_contract_is_refused_with_the_reason() {
    let good = json!({
        "operationId": "getThing",
        "summary": "A thing",
        "tags": ["things"],
        "parameters": [{ "in": "path", "name": "thingId" }],
        "responses": { "200": {} },
    });
    let path = "/things/{thingId}";
    finish_operations(&mut with_operation(path, &good)).expect("this one is fit");

    let without = |member: &str| {
        let mut operation = good.clone();
        operation.as_object_mut().expect("an object").remove(member);
        operation
    };
    let with = |member: &str, value: Value| {
        let mut operation = good.clone();
        operation[member] = value;
        operation
    };
    for (operation, reason) in [
        (without("operationId"), "has no operationId"),
        (without("summary"), "has no summary"),
        (without("tags"), "has no tags"),
        (
            without("parameters"),
            "does not describe its path parameter thingId",
        ),
        (
            with(
                "parameters",
                json!([{ "in": "path", "name": "thingId" }, { "in": "path", "name": "other" }]),
            ),
            "describes a path parameter other that its path does not have",
        ),
        (
            with("responses", json!({ "404": {} })),
            "has no answer of success",
        ),
        (
            with("security", json!([{ "sessionCooky": [] }])),
            "asks for sessionCooky, which is no security scheme",
        ),
    ] {
        let error = finish_operations(&mut with_operation(path, &operation))
            .expect_err(reason)
            .to_string();
        assert!(error.contains(reason), "{error}");
    }

    let mut twice = json!({
        "paths": { "/things": { "get": without("parameters"), "post": without("parameters") } },
    });
    let error = finish_operations(&mut twice)
        .expect_err("one name for two")
        .to_string();
    assert!(
        error.contains("two operations have the operationId getThing"),
        "{error}"
    );
}

#[test]
fn a_path_that_is_not_there_cannot_be_renamed() {
    let error = rename_paths(&mut json!({ "paths": {} }))
        .expect_err("nothing to rename")
        .to_string();
    assert!(error.contains("there is no path"), "{error}");
}

#[test]
fn a_schema_that_is_written_where_it_is_used_is_finished_too() {
    let mut paths = json!({
        "/things": { "post": {
            "parameters": [{ "name": "limit", "schema": { "type": "integer", "default": 100 } }],
            "requestBody": { "content": { "application/json": {
                "schema": { "type": "object", "default": {} },
            } } },
            "responses": { "200": { "content": { "application/json": {
                "schema": { "type": "array", "items": { "type": "object" } },
            } } } },
        } },
    });
    finish_inline_schemas(paths.as_object_mut().expect("an object"));
    let operation = &paths["/things"]["post"];
    assert_eq!(
        operation["parameters"][0]["schema"],
        json!({ "type": "integer" })
    );
    assert_eq!(
        operation["requestBody"]["content"]["application/json"]["schema"],
        json!({ "type": "object", "additionalProperties": true })
    );
    assert_eq!(
        operation["responses"]["200"]["content"]["application/json"]["schema"]["items"],
        json!({ "type": "object", "additionalProperties": true })
    );
}

/// What each operation that answers a list says about the order of the list: words
/// of its description, or of the description of its answer.
const ORDERS: &[(&str, &str)] = &[
    ("batchPatchCards", "in the order of `cardIds`"),
    (
        "duplicateContainer",
        "in the order of the source board's, oldest first",
    ),
    ("getApiTokens", "the newest first"),
    ("getBoardViews", "Oldest first"),
    ("getCardActivity", "the newest first"),
    ("getCardAttachments", "Oldest first"),
    ("getCardBodyVersions", "The newest first"),
    ("getCardCommentVersions", "the first one first"),
    ("getCardComments", "oldest first"),
    ("getCards", "Oldest first; the pages cut that order"),
    ("getCategories", "by `sortOrder`, lowest first"),
    (
        "getContainerTeams",
        "Ordered by team name in any letter case, by code point, then by identifier",
    ),
    ("getCourses", "Oldest first"),
    (
        "getDeletedCards",
        "The last deleted first; the pages cut that order",
    ),
    (
        "getDeletedContainers",
        "The last deleted first; the pages cut that order",
    ),
    ("getDeletedDiaryEntries", "the last deleted first"),
    (
        "getDeletedPages",
        "The last deleted first; the pages cut that order",
    ),
    (
        "getDeletedUnits",
        "The last deleted first; the pages cut that order",
    ),
    ("getDiaries", "oldest first"),
    ("getDiaryDays", "the earliest first"),
    ("getDiaryEntriesOn", "the latest year first"),
    ("getDiaryEntryBodyVersions", "the newest first"),
    ("getDiaryRuns", "the earliest first"),
    ("getFavorites", "in their order"),
    ("getFileVersions", "The newest first"),
    ("getInvitations", "The newest first"),
    ("getMembers", "By the identifier of the user"),
    ("getMyCourseProgress", "ordered by `unitId`"),
    ("getMyIdentities", "Oldest first"),
    ("getMyMemberships", "By the identifier of the board"),
    ("getMyOrganizations", "Oldest first"),
    ("getMyWorkspaces", "the oldest workspace first"),
    ("getNotifications", "the newest first"),
    ("getOrgApiTokens", "the newest first"),
    ("getOrgAudit", "the newest first"),
    ("getOrgMembers", "the oldest member first"),
    ("getOrgWorkspaces", "the oldest first"),
    ("getPageBodyVersions", "the newest first"),
    ("getPageCommentVersions", "the first one first"),
    ("getPageComments", "oldest first"),
    (
        "getPageTree",
        "By `order` as strings compare by code unit, then by `id`",
    ),
    ("getReferencesTo", "Oldest first"),
    (
        "getSharedBlocks",
        "Oldest first; the blocks that one request made come in the order that request had them in",
    ),
    ("getSharedCards", "Oldest first; the pages cut that order"),
    (
        "getSharedPageTree",
        "By `order` as strings compare by code unit, then by `id`",
    ),
    (
        "getSharedUnitTree",
        "By `order` as strings compare by code unit, then by `id`",
    ),
    (
        "getSubscriptions",
        "Oldest first, and by the identifier of the block",
    ),
    (
        "getTeams",
        "ordered by name in any letter case, by code point",
    ),
    ("getTemplates", "Each of the two oldest first"),
    ("getUnitBodyVersions", "the newest first"),
    ("getUnitCommentVersions", "the first one first"),
    ("getUnitComments", "oldest first"),
    (
        "getUnitTree",
        "By `order` as strings compare by code unit, then by `id`",
    ),
    ("getWikis", "Oldest first"),
    ("getWorkspaceAudit", "the newest first"),
    ("getWorkspaceBoards", "Oldest first"),
    ("getWorkspaceMembers", "The oldest membership first"),
    (
        "getWorkspaceTodayReviews",
        "Ordered by `scheduledDate` ascending",
    ),
    ("getWorkspaceUsers", "Oldest account first"),
    (
        "getWorkspaceUsersById",
        "Oldest account first, not in the order of the request",
    ),
    ("listCardFiles", "oldest first"),
    ("listContainerFiles", "oldest first"),
    (
        "listCourseExams",
        "ordered by `examDate` ascending (exams without a date last), then by `createdAt` and `id`",
    ),
    (
        "listCourseMeetings",
        "ordered by `startsAt` ascending, then by `id`",
    ),
    (
        "listMyCourseReviews",
        "Ordered by `scheduledDate` ascending",
    ),
    ("listMyUnitNotes", "Ordered by `createdAt` descending"),
    ("listWorkspaceContainers", "Oldest first"),
    ("reorderCardProperties", "in their new order"),
    ("reorderCategories", "in their new order"),
    ("reorderCategoryContainers", "in their new order"),
    ("resolveReferences", "in the order they were first named"),
    ("searchReferences", "the one changed last first"),
    ("searchWorkspaceBoards", "Oldest first"),
    ("searchWorkspaceContent", "The best match first"),
];

/// Whether what an operation answers is a list, or boards with their blocks.
fn answers_a_list(responses: &Value) -> bool {
    responses
        .as_object()
        .expect("responses")
        .iter()
        .filter(|(status, _)| status.starts_with('2'))
        .any(|(_, response)| {
            let schema = &response["content"]["application/json"]["schema"];
            schema["type"] == "array"
                || [
                    "#/components/schemas/ContainersAndBlocks",
                    "#/components/schemas/CardsOfPage",
                ]
                .contains(&schema["$ref"].as_str().unwrap_or_default())
        })
}

#[test]
fn every_list_says_its_order() {
    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let mut lists = Vec::new();
    for item in document["paths"].as_object().expect("paths").values() {
        for operation in item.as_object().expect("a path item").values() {
            if !answers_a_list(&operation["responses"]) {
                continue;
            }
            let id = operation["operationId"].as_str().expect("an operation id");
            let said: String = [&operation["description"]]
                .into_iter()
                .chain(
                    operation["responses"]
                        .as_object()
                        .expect("responses")
                        .values()
                        .map(|response| &response["description"]),
                )
                .filter_map(Value::as_str)
                .flat_map(str::split_whitespace)
                .collect::<Vec<_>>()
                .join(" ");
            let order = ORDERS
                .iter()
                .find(|(operation, _)| *operation == id)
                .map(|(_, order)| *order);
            let Some(order) = order else {
                panic!("{id} answers a list and ORDERS does not say in which order");
            };
            assert!(
                said.contains(order),
                "{id} is to say its list comes {order:?}; it says: {said}"
            );
            lists.push(id.to_owned());
        }
    }
    lists.sort_unstable();
    let named: Vec<&str> = ORDERS.iter().map(|(operation, _)| *operation).collect();
    assert_eq!(lists, named, "the operations that answer a list");
}

#[test]
fn the_routes_that_share_a_position_are_named_apart() {
    let text = document().expect("the document");
    let document: Value = serde_json::from_str(&text).expect("JSON");
    let paths = document["paths"].as_object().expect("paths");
    assert!(
        paths.contains_key("/api/v1/w/{workspaceId}/subscriptions/{blockId}/{userId}"),
        "the path that ends a subscription"
    );
    assert!(
        paths.contains_key("/api/v1/w/{workspaceId}/subscriptions/{userId}"),
        "the path that lists subscriptions"
    );
    assert!(
        !text.contains("blockOrUserId"),
        "the router's name for the shared position is not in the document"
    );
}

#[test]
fn path_parameters_are_the_names_in_braces() {
    let names: Vec<&str> = path_parameters("/boards/{boardId}/views/{viewId}").collect();
    assert_eq!(names, ["boardId", "viewId"], "the names in the path");
    assert_eq!(
        path_parameters("/boards").count(),
        0,
        "a path without parameters"
    );
}

#[test]
fn defaults_are_removed_at_every_depth_but_a_member_named_default_stays() {
    let mut schema = json!({
        "default": {},
        "properties": {
            "default": { "type": "string", "default": "" },
            "list": { "type": "array", "default": [], "items": { "default": 0 } },
        },
        "allOf": [{ "default": 1 }],
    });
    strip_defaults(&mut schema);
    assert_eq!(
        schema,
        json!({
            "properties": {
                "default": { "type": "string" },
                "list": { "type": "array", "items": {} },
            },
            "allOf": [{}],
        }),
        "the schema without the defaults"
    );
}

#[test]
fn an_object_without_members_may_have_any() {
    let mut schema = json!({
        "type": "object",
        "properties": {
            "fields": { "type": "object" },
            "maybe": { "type": ["object", "null"] },
            "list": { "type": "array", "items": { "type": "object" } },
            "counts": { "type": "object", "additionalProperties": { "type": "integer" } },
            "title": { "type": "string" },
        },
    });
    open_free_objects(&mut schema);
    assert_eq!(
        schema,
        json!({
            "type": "object",
            "properties": {
                "fields": { "type": "object", "additionalProperties": true },
                "maybe": { "type": ["object", "null"], "additionalProperties": true },
                "list": {
                    "type": "array",
                    "items": { "type": "object", "additionalProperties": true },
                },
                "counts": { "type": "object", "additionalProperties": { "type": "integer" } },
                "title": { "type": "string" },
            },
        }),
        "only the objects that say nothing about their members are opened"
    );
}

#[test]
fn a_member_that_is_not_there_cannot_be_required() {
    let mut schema = json!({ "properties": { "a": {}, "b": {} } });
    let error =
        require_members("Thing", &mut schema, Members::Only(&["c"])).expect_err("no member c");
    assert_eq!(
        error.to_string(),
        "the API contract cannot be made: the schema Thing has no member c",
        "what is wrong"
    );
    require_members("Thing", &mut schema, Members::AllBut(&["b"])).expect("a member b");
    assert_eq!(schema["required"], json!(["a"]), "all members but b");
}

#[test]
fn members_are_written_in_the_order_of_their_names() {
    let value = sorted(json!({ "b": [{ "d": 1, "c": 2 }], "a": 0 }));
    assert_eq!(
        serde_json::to_string(&value).expect("JSON"),
        r#"{"a":0,"b":[{"c":2,"d":1}]}"#,
        "the members in order"
    );
}
