//! The wall between organizations as the schema has it, table by table: the policies
//! and the role they hold, the keys that hold a row to the organization of what it
//! hangs on, and what two organizations store in every table.

use std::collections::BTreeMap;
use std::path::Path;

use fvoci_model::{
    AttendanceStatus, AuditAction, BlockType, Category, CompletionChange, Container, ContainerKind,
    ContainerMember, ContainerVisibility, CourseLimits, LastPositionChange, NewCourseExam,
    NewCourseMeeting, NewUnit, NewUnitNote, NewUnitReview, PublicLinkChange, STUDY_DAYS_WEEKDAYS,
    StudyDays, Subscription, UnitNoteKind, UnitProgressChange, Upload, UploadPart,
    ValidatedCourseSchedule, ValidatedMyExamScore, WorkspaceRole, new_id,
};
use fvoci_store::testing::{GLOBAL_TABLES, ORG_TABLES};
use fvoci_store::{
    AuditEntry, DbType, GrantedWorkspace, Issuance, SYSTEM_ORG_ID, Store, Tenant, TokenToIssue,
};
use sqlx::Connection as _;

use crate::common;
use crate::tenancy::board;

/// The policies hold the role the tests connect as, on tables it owns: a table whose
/// row-level security is forced and that has no policy reads as empty to it. A
/// superuser, a role with `BYPASSRLS`, and the owner of a table whose row-level
/// security is not forced read every row; as one of those, every test of the wall
/// would pass with the wall gone.
#[tokio::test]
async fn the_policies_hold_the_role_the_tests_connect_as() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let database = common::empty_database().await;
    let mut conn = common::connect_past_the_store(database.database_connection()).await;
    sqlx::raw_sql(
        "CREATE TABLE probe (x INT); INSERT INTO probe VALUES (1); \
         ALTER TABLE probe ENABLE ROW LEVEL SECURITY",
    )
    .execute(&mut conn)
    .await
    .expect("the probe is made");
    let not_forced: i64 = sqlx::query_scalar("SELECT count(*) FROM probe")
        .fetch_one(&mut conn)
        .await
        .expect("the probe is read");
    sqlx::raw_sql("ALTER TABLE probe FORCE ROW LEVEL SECURITY")
        .execute(&mut conn)
        .await
        .expect("row-level security is forced");
    let forced: i64 = sqlx::query_scalar("SELECT count(*) FROM probe")
        .fetch_one(&mut conn)
        .await
        .expect("the probe is read");
    conn.close().await.expect("the connection closes");
    database.remove().await;

    assert_eq!(
        not_forced, 1,
        "the role owns its tables: without FORCE the policies do not hold it"
    );
    assert_eq!(
        forced, 0,
        "with FORCE and no policy the table reads as empty"
    );
}

/// The tables of the test's schema, by name.
async fn tables(database_connection: &str) -> Vec<String> {
    let mut conn = common::connect_past_the_store(database_connection).await;
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables \
         WHERE table_schema = current_schema() ORDER BY table_name",
    )
    .fetch_all(&mut conn)
    .await
    .expect("the tables are listed");
    conn.close().await.expect("the connection closes");
    tables
}

/// A policy as the catalog has it: its command, whether it is permissive, its
/// condition, and its condition for new rows.
type Policy = (String, String, Option<String>, Option<String>);

/// A row of `pg_policies`: the table and the name of a policy, and the policy.
type PolicyRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
);

/// The policies of the test's schema, by table and name.
async fn policies(conn: &mut sqlx::postgres::PgConnection) -> BTreeMap<(String, String), Policy> {
    let rows: Vec<PolicyRow> = sqlx::query_as(
        "SELECT tablename::text, policyname::text, cmd::text, permissive::text, qual, \
         with_check FROM pg_policies WHERE schemaname = current_schema()",
    )
    .fetch_all(conn)
    .await
    .expect("the policies are read");
    rows.into_iter()
        .map(|(table, policy, cmd, permissive, qual, check)| {
            ((table, policy), (cmd, permissive, qual, check))
        })
        .collect()
}

/// Checks that a table is behind the wall: row-level security enabled and forced on
/// the owner, the policy `org_wall` as `wall` is, and a column `org_id` that takes no
/// NULL and, where the server does not write it, defaults to the organization of the
/// transaction.
async fn assert_behind_the_wall(
    conn: &mut sqlx::postgres::PgConnection,
    table: &str,
    wall: &Policy,
    policies: &BTreeMap<(String, String), Policy>,
) {
    let (enabled, forced): (bool, bool) = sqlx::query_as(
        "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = to_regclass($1)",
    )
    .bind(table)
    .fetch_one(&mut *conn)
    .await
    .expect("the table is in the catalog");
    assert!(enabled, "{table}: row-level security is enabled");
    assert!(forced, "{table}: and forced on the owner");
    let policy = policies
        .get(&(table.to_owned(), "org_wall".to_owned()))
        .unwrap_or_else(|| panic!("{table} has the policy org_wall"));
    if table == "organizations" {
        // The same wall, by the organization's own identifier.
        let (command, permissive, using, check) = wall;
        let using = using.as_deref().map(|using| using.replace("org_id", "id"));
        assert_eq!(
            policy,
            &(command.clone(), permissive.clone(), using, check.clone()),
            "{table}: the policy is the one of boards, by its identifier"
        );
        return;
    }
    assert_eq!(policy, wall, "{table}: the policy is the one of boards");
    let (nullable, default): (String, Option<String>) = sqlx::query_as(
        "SELECT is_nullable::text, column_default::text FROM information_schema.columns \
         WHERE table_schema = current_schema() AND table_name = $1 AND column_name = 'org_id'",
    )
    .bind(table)
    .fetch_one(&mut *conn)
    .await
    .unwrap_or_else(|error| panic!("{table} has the column org_id: {error}"));
    assert_eq!(nullable, "NO", "{table}: a row is some organization's");
    // The server names the organization itself in two tables; elsewhere a row gets
    // the organization of the transaction that stores it.
    if ["workspaces", "organization_members"].contains(&table) {
        assert_eq!(default, None, "{table}: the server writes the organization");
    } else {
        let default = default.unwrap_or_default();
        assert!(default.contains("fvoci.org"), "{table}: {default}");
    }
}

/// Every table of the schema is one of the few that are no organization's, or is
/// behind the wall: its row-level security is enabled and forced on its owner, and its
/// policy `org_wall` is the one policy that lets rows be changed, the same on every
/// table. A table that a later migration adds and forgets here fails this test.
#[tokio::test]
async fn every_table_is_no_organizations_or_behind_the_wall() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let all = tables(&test.database_connection()).await;
    for global in GLOBAL_TABLES {
        assert!(
            all.iter().any(|table| table == global),
            "{global} is a table"
        );
    }
    let walled: Vec<&str> = all
        .iter()
        .map(String::as_str)
        .filter(|table| !GLOBAL_TABLES.contains(table))
        .collect();
    assert_eq!(
        walled, ORG_TABLES,
        "every table that is not nobody's is in the list of the organizations' tables"
    );

    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    let policies = policies(&mut conn).await;
    let wall = policies
        .get(&("containers".to_owned(), "org_wall".to_owned()))
        .expect("boards has the policy")
        .clone();
    let (command, permissive, using, check) = &wall;
    let using = using.as_deref().expect("the policy has a condition");
    assert!(
        using.contains("fvoci.org") && using.contains("fvoci.system") && using.contains("org_id"),
        "{using}"
    );
    assert_eq!(command, "ALL", "for every command");
    assert_eq!(permissive, "PERMISSIVE");
    assert_eq!(check, &None, "new rows are held to the same condition");
    for table in &walled {
        assert_behind_the_wall(&mut conn, table, &wall, &policies).await;
    }

    // Besides the wall, only the reading of the system organization's templates: their
    // boards, their blocks and the records of the files they show.
    for ((table, policy), (command, permissive, using, _)) in &policies {
        if policy == "org_wall" {
            continue;
        }
        assert_eq!(policy, "org_templates", "{table} has the policy {policy}");
        assert!(
            ["containers", "blocks", "files"].contains(&table.as_str()),
            "{table} lets the templates be read"
        );
        assert_eq!(
            (command.as_str(), permissive.as_str()),
            ("SELECT", "PERMISSIVE")
        );
        assert!(
            using.as_deref().unwrap_or_default().contains(SYSTEM_ORG_ID),
            "{table}: {using:?}"
        );
    }
    for global in GLOBAL_TABLES {
        let enabled: bool =
            sqlx::query_scalar("SELECT relrowsecurity FROM pg_class WHERE oid = to_regclass($1)")
                .bind(global)
                .fetch_one(&mut conn)
                .await
                .expect("the table is in the catalog");
        assert!(!enabled, "{global} is nobody's and has no wall");
    }
    conn.close().await.expect("the connection closes");
    test.teardown().await;
}

/// Stores, through `store`, an upload with a part, an object being written and a file's version.
async fn in_the_files(store: &Store, user_id: &str, board_id: &str, username: &str) {
    let upload = Upload {
        id: new_id(),
        user_id: user_id.to_owned(),
        container_id: board_id.to_owned(),
        name: "upload.bin".to_owned(),
        size: 1,
        part_size: 1,
        created_at: 1,
        expires_at: i64::MAX,
        ..Upload::default()
    };
    let opened = store.create_upload(&upload).await;
    opened.expect("an upload is opened");
    store
        .put_upload_part(
            &upload.id,
            &UploadPart {
                number: 1,
                size: 1,
                part_id: String::new(),
            },
        )
        .await
        .expect("a part is recorded");
    store
        .mark_writing(&format!("boards/{username}/being-written"), i64::MAX)
        .await
        .expect("an object is being written");
    let added = store.add_file_version(board_id, "7first.png", 1, "7second.png", user_id);
    added.await.expect("a second version of a file");
}

/// Makes the user an administrator of the workspace: a role is recorded as given by a
/// board's manager only while they administer its workspace.
async fn administer(store: &Store, workspace_id: &str, user_id: &str) {
    let made = store
        .add_workspace_member(workspace_id, user_id, WorkspaceRole::Admin)
        .await;
    if made.is_err() {
        store
            .execute_unchecked(&format!(
                "UPDATE workspace_members SET role = 'admin' WHERE user_id = '{user_id}' \
                 AND workspace_id = '{workspace_id}'"
            ))
            .await
            .expect("the user administers the workspace");
    }
}

/// Two cards of the board that name the user, written on behalf of nobody: the user
/// is told of one, and still to be told of the other.
async fn told_and_to_be_told(store: &Store, board_id: &str, user_id: &str) {
    let body = fvoci_model::Body::parse(serde_json::json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "mention", "attrs": {"userId": user_id}}]},
    ]}))
    .expect("a document");
    for _ in 0..2 {
        let mut card = fvoci_model::Block {
            id: new_id(),
            container_id: board_id.to_owned(),
            block_type: BlockType::Card,
            body: body.clone(),
            ..fvoci_model::Block::default()
        };
        store
            .insert_block(&mut card, "")
            .await
            .expect("a card that names the user is stored");
    }
    let noted = store
        .due_notification_events(i64::MAX, 10)
        .await
        .expect("the list is read");
    let limits = fvoci_store::NotificationLimits {
        same_within_millis: 0,
        most_made_by_one: 10,
        made_within_millis: 0,
        most_of_a_person: 10,
    };
    let of_the_user = noted
        .iter()
        .find(|event| event.recipient_id == user_id)
        .expect("a row of the user");
    let told = store.tell_of(of_the_user, 1, limits).await.expect("asked");
    assert!(
        matches!(told, fvoci_store::Told::Made(_)),
        "the user is told: {told:?}"
    );
}

/// Stores, through `store`, something in every table of an organization: a workspace, a
/// board with a member and the record of a change of its roles, a card and a comment
/// whose text was changed, files, a public link, a subscription, a category with the board in
/// it, an upload with a part, an object being written, a notification and one still to
/// be made, and a board and a card in the trash. The user is made through `store`
/// too, and so joins its organization. Returns the identifiers of the card that went to
/// the trash and of the user.
async fn something_in_every_table(store: &Store, username: &str) -> (String, String) {
    // As on a server that has a search server: what changes is noted for it.
    store.keep_search_outbox();
    let user_id = common::user(store, username).await;
    let comment = crate::files::posted_comment(store, &user_id).await;
    let board_id = comment.container_id.clone();
    let board = store.get_container(&board_id).await.expect("the board");
    store
        .patch_block(
            &comment.id,
            &crate::files::title_and_body(Some("v2")),
            &user_id,
        )
        .await
        .expect("the comment is changed");
    store
        .save_member(&ContainerMember {
            container_id: board_id.clone(),
            user_id: user_id.clone(),
            role: fvoci_model::ContainerRole::Editor,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("a member is stored");
    let as_viewer = ContainerMember {
        container_id: board_id.clone(),
        user_id: user_id.clone(),
        role: fvoci_model::ContainerRole::Viewer,
        synthetic: false,
        inherited_role: None,
    };
    let entry = AuditEntry::new(
        &board.workspace_id,
        &user_id,
        AuditAction::SelfAddedToContainer,
        &board_id,
    );
    administer(store, &board.workspace_id, &user_id).await;
    a_token_reaching(store, &board.workspace_id, &user_id).await;
    store
        .change_member_recorded(&as_viewer, &entry)
        .await
        .expect("a change of roles is recorded");
    store
        .set_public_link(&board_id, PublicLinkChange::On, &new_id(), &user_id)
        .await
        .expect("the board is shared");
    store
        .create_subscription(Subscription {
            block_type: BlockType::Card,
            block_id: comment.parent_id.clone(),
            user_id: user_id.clone(),
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        })
        .await
        .expect("a subscription is stored");
    let category = Category {
        id: new_id(),
        name: "a category".to_owned(),
        user_id: user_id.clone(),
        workspace_id: board.workspace_id.clone(),
        ..Category::default()
    };
    store
        .create_category(&category)
        .await
        .expect("a category is stored");
    store
        .add_update_category_container(&user_id, &category.id, std::slice::from_ref(&board_id))
        .await
        .expect("the board is filed");
    in_the_files(store, &user_id, &board_id, username).await;
    a_body_edited_together(store, &board_id, &user_id).await;
    a_team_on_board(store, &board, &user_id).await;
    a_course_with_progress(store, &board.workspace_id, &user_id).await;
    told_and_to_be_told(store, &board_id, &user_id).await;
    a_time_entry(store, &board, &comment.parent_id, &user_id).await;
    a_card_assignee_and_due_notice(store, &board, &comment.parent_id, &user_id).await;
    // The trash: a card of the live board, and a whole board.
    let mut card = fvoci_model::Block {
        id: new_id(),
        container_id: board_id.clone(),
        block_type: BlockType::Card,
        ..fvoci_model::Block::default()
    };
    common::insert_blocks(store, std::slice::from_mut(&mut card), &user_id).await;
    store
        .delete_block(&card.id, &user_id)
        .await
        .expect("a card goes to the trash");
    let gone = common::create_test_boards(store, &board.workspace_id, &user_id, 1).await;
    store
        .delete_container(&gone[0].id, &user_id)
        .await
        .expect("a board goes to the trash");
    common::sign_in_under_way(store, username).await;
    a_link_for_a_guest(store, &board, &user_id).await;
    (card.id, user_id)
}

/// A personal API token of the user that reaches the workspace, which they work in.
async fn a_token_reaching(store: &Store, workspace_id: &str, user_id: &str) {
    let org_id = store
        .tenant()
        .org_id()
        .expect("a handle of an organization");
    let (id, digest) = (new_id(), new_id());
    let session = fvoci_model::Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: 1,
        last_used_at: 1,
        expires_at: i64::MAX,
        ..fvoci_model::Session::default()
    };
    assert!(
        store
            .create_session(&session, &new_id(), "")
            .await
            .expect("a session is stored"),
        "a new session"
    );
    let granted = [GrantedWorkspace {
        workspace_id,
        org_id: &org_id,
    }];
    let issued = store
        .issue_api_token(&TokenToIssue {
            id: &id,
            user_id,
            session_id: &session.id,
            name: "agent",
            token_prefix: "fvoci_pat_abcdef",
            token_digest: &digest,
            scope: fvoci_model::ApiTokenScope::Write,
            workspaces: &granted,
            allow_personal_space: false,
            notify_self: true,
            created_at: 1,
            expires_at: i64::MAX,
        })
        .await
        .expect("a token is stored");
    assert!(matches!(issued, Issuance::Issued(_)), "{issued:?}");
}

/// A link that invites a guest to the board, made by the user.
async fn a_link_for_a_guest(store: &Store, board: &Container, user_id: &str) {
    store
        .create_invitation(&fvoci_store::NewLink {
            workspace_id: &board.workspace_id,
            guest_of: Some((&board.id, fvoci_model::ContainerRole::Viewer)),
            token_hash: &format!("link of {user_id}"),
            created_by: user_id,
            now: 0,
            expires_at: i64::MAX,
            max_uses: Some(1),
            mailed: None,
        })
        .await
        .expect("a link is stored");
}

/// The document of a card's body that is edited together: an update logged, a version
/// kept. The body names a block, which is a row of what bodies name.
async fn a_body_edited_together(store: &Store, board_id: &str, user_id: &str) {
    let mut edited = fvoci_model::Block {
        id: new_id(),
        container_id: board_id.to_owned(),
        block_type: BlockType::Card,
        body: crate::body_links::body_naming("kept", &[&new_id()]),
        ..fvoci_model::Block::default()
    };
    common::insert_blocks(store, std::slice::from_mut(&mut edited), user_id).await;
    let body = edited.body.clone();
    crate::body_versions::type_body(store, &edited.id, body, user_id).await;
    crate::body_versions::keep(store, &edited.id).await;
}

/// What two organizations store stays apart in every table of theirs, whatever the
/// statement asks for: counted with no condition, each handle reaches its own rows, one
/// that names no organization reaches none, and the system's reaches all.
#[tokio::test]
async fn what_two_organizations_store_stays_apart_in_every_table() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, _) = common::organization(&unset, "the other").await;
    let ours = &test.store;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let system_org = test.for_org(SYSTEM_ORG_ID).expect("an identifier");

    something_in_every_table(ours, "ours").await;
    // Twice as much of everything for them, so that no count is the same by chance.
    something_in_every_table(&theirs, "theirs").await;
    something_in_every_table(&theirs, "theirs.too").await;

    let walled: Vec<String> = tables(&test.database_connection())
        .await
        .into_iter()
        .filter(|table| !GLOBAL_TABLES.contains(&table.as_str()))
        .collect();
    let accounts = test.system().count_rows("users").await.expect("counted");
    assert_eq!(accounts, 3, "one account for each filling");
    for table in &walled {
        let count = async |store: &Store| store.count_rows(table).await.expect("counted");
        let (our_rows, their_rows) = (count(ours).await, count(&theirs).await);
        // The days of diaries are of own spaces alone, and no organization of people
        // has a row of them: `diaries::the_days_of_a_diary_stay_in_its_space` asks.
        if table == "diary_days" {
            let all = count(&test.system()).await;
            assert_eq!((our_rows, their_rows, all), (0, 0, 0), "{table}");
            continue;
        }
        assert!(our_rows > 0, "{table}: the test stores a row of ours");
        // An organization, its provider of sign-ins, the one it moved away from and
        // its pending settings are one of each.
        let one_of_each = [
            "organizations",
            "oidc_providers",
            "oidc_former_providers",
            "oidc_pending_providers",
        ];
        if one_of_each.contains(&table.as_str()) {
            assert_eq!((our_rows, their_rows), (1, 1), "each reaches its own alone");
        } else {
            assert!(
                their_rows > our_rows,
                "{table}: theirs are more ({their_rows}) than ours ({our_rows})"
            );
        }
        assert_eq!(count(&unset).await, 0, "{table}: no organization named");
        // What is of neither is the system's, or the own space of an account: an
        // organization and its workspace for each account the test made, the account
        // in both, and not a row of any other table.
        let of_spaces = if common::OWN_SPACE_TABLES.contains(&table.as_str()) {
            accounts
        } else {
            0
        };
        assert_eq!(
            common::rows_of_own_spaces(ours, table).await,
            of_spaces,
            "{table}: what the own spaces of the {accounts} accounts hold"
        );
        assert_eq!(
            count(&test.system()).await,
            our_rows + their_rows + count(&system_org).await + of_spaces,
            "{table}: the system's handle reaches all, and nothing is nobody's"
        );
    }

    test.teardown().await;
}

/// The trash of one organization is not purged by another: what is not there for a
/// handle is not gone. The server purges as the system, which reaches all of it.
#[tokio::test]
async fn one_organization_does_not_purge_the_trash_of_another() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let unset = test.scoped(Tenant::Unset);
    let (other_org, _) = common::organization(&unset, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let (our_card, _) = something_in_every_table(&test, "ours").await;
    let (their_card, _) = something_in_every_table(&theirs, "theirs").await;

    // Everything in the trash is old enough to go.
    let whenever = i64::MAX;
    let purged = unset.purge_trash(whenever).await.expect("purged");
    assert_eq!(
        purged.records, 0,
        "a handle of no organization purges nothing"
    );
    let purged = test.purge_trash(whenever).await.expect("purged");
    assert!(purged.records > 0, "ours is purged: {purged:?}");
    let gone = test
        .get_deleted_block(&our_card)
        .await
        .expect_err("our card is gone for good");
    assert!(gone.is_not_found(), "got {gone:?}");
    theirs
        .get_deleted_block(&their_card)
        .await
        .expect("their card is still in their trash");
    assert!(
        theirs.count_rows("file_refs").await.expect("counted") > 0,
        "and what their blocks show is still put down"
    );
    assert!(
        theirs.count_rows("body_links").await.expect("counted") > 0,
        "and what their bodies name"
    );

    let purged = test.system().purge_trash(whenever).await.expect("purged");
    assert!(purged.records > 0, "the system purges theirs: {purged:?}");
    let gone = theirs
        .get_deleted_block(&their_card)
        .await
        .expect_err("their card is gone for good");
    assert!(gone.is_not_found(), "got {gone:?}");

    test.teardown().await;
}

/// A completed time entry of the user on a card of the organization.
async fn a_time_entry(store: &Store, board: &Container, card_id: &str, user_id: &str) {
    let entry = fvoci_store::NewStoredTimeEntry {
        workspace_id: Some(&board.workspace_id),
        container_id: Some(&board.id),
        block_id: Some(card_id),
        target_kind: Some(fvoci_model::TimeTargetKind::Card),
        target_title: Some("tenancy card"),
        kind: fvoci_model::TimeEntryKind::Work,
        note: "tenancy entry",
    };
    store
        .create_time_entry(user_id, &entry, 1000, 2000, 2000)
        .await
        .expect("a time entry is stored");
}

async fn a_card_assignee_and_due_notice(
    store: &Store,
    board: &Container,
    card_id: &str,
    user_id: &str,
) {
    let assignee_prop = new_id();
    let due_prop = new_id();
    store
        .patch_container(
            &board.id,
            &fvoci_model::ContainerPatch {
                updated_card_properties: vec![
                    fvoci_model::CardPropertyDefinition::new(
                        assignee_prop.clone(),
                        "Assignee",
                        fvoci_model::PropertyType::Person,
                    ),
                    fvoci_model::CardPropertyDefinition::new(
                        due_prop.clone(),
                        "Due",
                        fvoci_model::PropertyType::Date,
                    ),
                ],
                ..fvoci_model::ContainerPatch::default()
            },
            user_id,
        )
        .await
        .expect("board properties patched");
    let mut updated_props = fvoci_model::JsonMap::new();
    updated_props.insert(assignee_prop, serde_json::Value::String(user_id.to_owned()));
    updated_props.insert(due_prop, serde_json::json!({"from": "2026-11-26"}));
    store
        .patch_block(
            card_id,
            &fvoci_model::BlockPatch {
                property_changes: fvoci_model::PropertyChanges {
                    updated: updated_props,
                    deleted: Vec::new(),
                },
                ..fvoci_model::BlockPatch::default()
            },
            user_id,
        )
        .await
        .expect("card assigned with due date");
    let org_id = store
        .tenant()
        .org_id()
        .expect("organization-scoped store handle");
    store
        .notify_due_soon_card(
            &fvoci_store::DueSoonCardCandidate {
                org_id,
                workspace_id: board.workspace_id.clone(),
                board_id: board.id.clone(),
                card_id: card_id.to_owned(),
                user_id: user_id.to_owned(),
                due_at: "2026-11-26".to_owned(),
                due_millis: None,
                user_time_zone: "auto".to_owned(),
            },
            3000,
            100,
        )
        .await
        .expect("due notice stored");
}

/// Real rows in all three group tables, so every organization wall is exercised.
async fn a_team_on_board(store: &Store, board: &Container, user_id: &str) {
    let group = store
        .create_team(&board.workspace_id, user_id, user_id)
        .await
        .expect("a team is stored");
    store
        .add_team_member(&board.workspace_id, &group, user_id, user_id)
        .await
        .expect("a team member is stored");
    store.execute_unchecked(&format!(
        "INSERT INTO container_teams (container_id, team_id, workspace_id, role) VALUES ('{}', '{group}', '{}', 'editor')",
        board.id, board.workspace_id
    )).await.expect("the team has a board role");
}

/// A course with one unit and a personal progress row, so `unit_progress` sits behind
/// the organization wall in `what_two_organizations_store_stays_apart_in_every_table`.
async fn a_course_with_progress(store: &Store, workspace_id: &str, user_id: &str) {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Course,
        title: "a course".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut course, user_id)
        .await
        .expect("a course is stored");
    let unit = store
        .create_unit(
            &course.id,
            &NewUnit {
                title: "unit 1".to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            user_id,
        )
        .await
        .expect("a unit is stored")
        .unit;
    store
        .upsert_unit_progress(
            &course.id,
            &unit.id,
            user_id,
            &UnitProgressChange {
                completion: CompletionChange::Complete,
                last_position: LastPositionChange::Set("p1".to_owned()),
            },
        )
        .await
        .expect("unit progress is stored");
    seed_course_meetings_and_exams(store, &course.id, &unit.id, user_id).await;
    store
        .schedule_unit_review(
            &course.id,
            &unit.id,
            user_id,
            &NewUnitReview {
                scheduled_date: "2026-11-25".to_owned(),
                interval_days: 1,
            },
            None,
        )
        .await
        .expect("unit review is stored");
    store
        .create_unit_note(
            &course.id,
            &unit.id,
            user_id,
            &NewUnitNote {
                kind: UnitNoteKind::Question,
                content: "why does RLS hold?".to_owned(),
            },
        )
        .await
        .expect("unit note is stored");
    store
        .upsert_course_schedule(
            &course.id,
            &ValidatedCourseSchedule {
                start_date: "2026-11-01".parse().expect("start date"),
                target_date: "2026-11-30".parse().expect("target date"),
                study_days: StudyDays::try_new(STUDY_DAYS_WEEKDAYS).expect("weekdays"),
                auto_assign: false,
            },
            user_id,
        )
        .await
        .expect("course schedule is stored");
}

async fn seed_course_meetings_and_exams(
    store: &Store,
    course_id: &str,
    unit_id: &str,
    user_id: &str,
) {
    let meeting = store
        .create_course_meeting(
            course_id,
            &NewCourseMeeting {
                title: "weekly sync".to_owned(),
                starts_at: 1_800_000_000_000,
                location: "room 1".to_owned(),
                unit_ids: vec![unit_id.to_owned()],
                presenter_id: Some(user_id.to_owned()),
                host_id: Some(user_id.to_owned()),
                notes: "agenda".to_owned(),
            }
            .check_request()
            .expect("valid meeting"),
        )
        .await
        .expect("course meeting is stored");
    store
        .upsert_meeting_attendance(course_id, &meeting.id, user_id, AttendanceStatus::Present)
        .await
        .expect("meeting attendance is stored");
    let exam = store
        .create_course_exam(
            course_id,
            &NewCourseExam {
                title: "quiz 1".to_owned(),
                exam_date: Some("2026-12-01".to_owned()),
                max_score: 100.0,
                description: "chapter 1".to_owned(),
            }
            .check_request()
            .expect("valid exam"),
        )
        .await
        .expect("course exam is stored");
    store
        .upsert_my_exam_score(
            course_id,
            &exam.id,
            user_id,
            &ValidatedMyExamScore {
                score: 92.5,
                note: "solid".to_owned(),
            },
        )
        .await
        .expect("exam score is stored");
}

/// The store tells whether the wall holds the role it connects as: the role of the
/// tests it does, a superuser it does not, and SQLite has no role to ask about.
#[tokio::test]
async fn the_store_names_a_role_that_is_past_the_wall() {
    let test = common::setup().await;
    assert_eq!(
        test.role_past_the_wall().await.expect("the role is read"),
        None,
        "the role of the tests is held"
    );
    test.teardown().await;

    if common::db_type() != DbType::Postgres {
        return;
    }
    let directory = Path::new(env!("CARGO_TARGET_TMPDIR")).join("store-tests");
    let database = fvoci_store::testing::TestDatabase::create_as_superuser(&directory).await;
    let store = Store::connect(database.db_type(), database.database_connection())
        .await
        .expect("a store connects as the superuser");
    let role = store.role_past_the_wall().await.expect("the role is read");
    assert!(role.is_some(), "a superuser is past the wall");
    // Past it indeed: what a handle of no organization must not reach, it reaches.
    common::organization(&store, "anybody's").await;
    assert_eq!(
        store.count_rows("workspaces").await.expect("counted"),
        2,
        "the superuser reads the workspace and the system's through a handle of no organization"
    );
    store.close().await;
    database.remove().await;
}

/// The foreign keys of the test's schema: the table, the table it refers to, and the
/// columns of the key in the table, in the key's order.
async fn foreign_keys(conn: &mut sqlx::PgConnection) -> Vec<(String, String, Vec<String>)> {
    sqlx::query_as(
        "SELECT c.conrelid::regclass::text, c.confrelid::regclass::text, \
         (SELECT array_agg(a.attname::text ORDER BY k.position) \
          FROM unnest(c.conkey) WITH ORDINALITY AS k (number, position) \
          JOIN pg_attribute AS a ON a.attrelid = c.conrelid AND a.attnum = k.number) \
         FROM pg_constraint AS c \
         WHERE c.contype = 'f' AND c.connamespace = current_schema()::regnamespace \
         ORDER BY 1, 2, 3",
    )
    .fetch_all(conn)
    .await
    .expect("the keys are in the catalog")
}

/// The wall looks at a row's own organization. That the row and what it hangs on are
/// one organization's is held by keys: wherever a table of an organization refers to
/// another by a key of one column, a key of that column and the organization refers to
/// the same row. A table that a later migration adds with the one and without the
/// other fails this test.
#[tokio::test]
async fn a_row_is_held_to_the_organization_of_what_it_hangs_on() {
    // A category takes a built-in template, which is the system organization's: a
    // trigger checks the board when the row is stored (organization_boundary_round2.rs).
    const NOT_HELD: [(&str, &str); 1] = [("category_containers", "container_id")];
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    let keys = foreign_keys(&mut conn).await;
    conn.close().await.expect("the connection closes");
    test.teardown().await;

    let mut held = Vec::new();
    for (table, parent, columns) in &keys {
        let [column] = columns.as_slice() else {
            continue;
        };
        let of_organizations = |name: &String| ORG_TABLES.contains(&name.as_str());
        // A member of an organization hangs on the organization by its own org_id.
        if !of_organizations(table) || !of_organizations(parent) || parent == "organizations" {
            continue;
        }
        if NOT_HELD.contains(&(table.as_str(), column.as_str())) {
            continue;
        }
        // An organization is itself the organization of its default workspace.
        let organization = if table == "organizations" {
            "id"
        } else {
            "org_id"
        };
        let with_the_organization = vec![column.clone(), organization.to_owned()];
        assert!(
            keys.contains(&(table.clone(), parent.clone(), with_the_organization)),
            "{table}.{column} refers to {parent} without the organization: {keys:?}"
        );
        held.push(format!("{table}.{column}"));
    }
    assert_eq!(
        held,
        [
            "blocks.container_id",
            "categories.workspace_id",
            "category_containers.category_id",
            "container_members.container_id",
            "containers.workspace_id",
            "files.container_id",
            "organizations.default_workspace_id",
            "public_links.container_id",
            "subscriptions.block_id",
            "upload_parts.upload_id",
            "workspace_members.workspace_id",
        ],
        "the keys that are held to the organization"
    );
}

/// The lookups that reach across every organization do not take the keys on trust:
/// with the keys gone, a member's row that one organization stored under another's
/// workspace lets nobody into it, and a public link that one stored under another's
/// board opens nothing.
#[tokio::test]
async fn a_row_of_one_organization_under_anothers_parent_counts_for_nothing() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "anybody").await;
    let (other_org, their_workspace) = common::organization(&test, "the other").await;
    let theirs = test.for_org(&other_org).expect("an identifier");
    let their_board = board(&theirs, &their_workspace, &user_id, "theirs").await;

    // Each row is the first organization's: stored by its handle, with its org_id.
    let forgeries = [
        format!(
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) \
             VALUES ('{their_workspace}', '{user_id}', 'admin', 0)"
        ),
        format!(
            "INSERT INTO public_links (container_id, enabled, token, updated_at) \
             VALUES ('{their_board}', true, 'a-forged-token', 0)"
        ),
    ];
    for forged in &forgeries {
        let refused = test
            .execute_unchecked(forged)
            .await
            .expect_err("the key holds the row to its parent's organization");
        assert!(refused.to_string().contains("foreign key"), "{refused}");
    }

    let mut conn = common::connect_past_the_store(&test.database_connection()).await;
    sqlx::raw_sql(
        "ALTER TABLE workspace_members DROP CONSTRAINT workspace_members_workspace_org_fkey; \
         ALTER TABLE public_links DROP CONSTRAINT public_links_container_org_fkey",
    )
    .execute(&mut conn)
    .await
    .expect("the keys are dropped in the test's own schema");
    conn.close().await.expect("the connection closes");

    for forged in &forgeries {
        assert_eq!(
            test.execute_unchecked(forged)
                .await
                .expect("without the key the row is stored"),
            1
        );
    }
    assert_eq!(
        test.resolve_workspace(&user_id, &their_workspace)
            .await
            .expect("the membership is looked up"),
        None,
        "the row lets nobody into their workspace"
    );
    let listed = test
        .workspaces_of_user(&user_id, fvoci_store::Making::with_password())
        .await
        .expect("the workspaces are listed");
    assert!(
        listed
            .iter()
            .all(|workspace| workspace.id != their_workspace),
        "and their workspace is not among the user's: {listed:?}"
    );
    assert_eq!(
        test.shared_container(&their_board, "a-forged-token")
            .await
            .expect("the public link is looked up"),
        None,
        "the public link opens nothing"
    );
    test.teardown().await;
}
