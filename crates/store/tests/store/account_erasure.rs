//! Erasing an account: nothing of the person is left, what they wrote and the records
//! about them stay, nothing is left without somebody to run it, an organization that
//! has nobody else goes with it, and an erasing that was cut off is taken up again.

use fvoci_model::{
    AttendanceStatus, AuditAction, Block, BlockType, Category, CompletionChange, Container,
    ContainerKind, ContainerMember, ContainerVisibility, CourseLimits, Credential,
    LastPositionChange, NameShown, NewCourseExam, NewCourseMeeting, NewUnit, NewUnitNote,
    NewUnitReview, OrgRole, PersonName, Session, Title, UnitNoteKind, UnitProgressChange, User,
    ValidatedMyExamScore, WorkspaceRole, new_id,
};
use fvoci_store::{
    AuditPlace, DbType, DeletionAsking, Erasing, ErasureEnd, ErasureStart, Identity, LeftFrom,
    Looking, NewLink, Paging, Reach, Sight, Store, StoreError, Tenant,
};

use crate::common::{self, TestStore};

/// A time in milliseconds that the tests count from.
pub(crate) const T0: i64 = 1_800_000_000_000;

/// No column, and nobody.
pub(crate) const NOBODY: [String; 0] = [];

/// Every record, on one page.
const EVERY: Paging = Paging {
    page: 0,
    per_page: std::num::NonZeroU32::new(1000).expect("a page size"),
};

/// The wait the tests ask for.
pub(crate) const WAIT: i64 = 30 * 24 * 60 * 60 * 1000;

/// What is the person's of the account [`person`] makes: none of it is anywhere once
/// the account is erased.
const OF_THE_PERSON: [&str; 16] = [
    "leaver-shelf",
    "Leaver-First",
    "Leaver-Last",
    "Leaver-Latin-First",
    "Leaver-Latin-Last",
    "kr_patent_attorney",
    "kr_korean_medicine_doctor",
    // The choice of the name shown; erased to the one every new account has.
    "latin_name",
    "nder-leaver-mail@example.com",
    "leaver-name",
    "leaver-mail@example.com",
    "Leaver Shown",
    "leaver-password-hash",
    "leaver-subject",
    "leaver-at-the-provider@example.com",
    "leaver-browser",
];

/// Where the identifier of the account [`person`] makes stays once it is erased, with
/// what the first test writes as it: the row itself, what it made and changed, the
/// records about it, and shared course meeting/attendance history (intentionally
/// preserved as a tombstone pointing at the erased account).
const KEEPS_THE_IDENTIFIER: [&str; 19] = [
    "audit_log.actor_id",
    "audit_log.subject_id",
    "blocks.created_by",
    "blocks.fields",
    "blocks.modified_by",
    "body_version_editors.user_id",
    "comment_versions.written_by",
    "containers.created_by",
    "containers.modified_by",
    "course_meetings.host_id",
    "course_meetings.presenter_id",
    "deleted_blocks.block",
    "deleted_blocks.deleted_by",
    "deleted_containers.container",
    "deleted_containers.deleted_by",
    "invitations.created_by",
    "meeting_attendance.user_id",
    "uploads.user_id",
    "users.id",
];

/// Gives a member of the handle's organization the role in it, past the routes.
async fn set_org_role(store: &Store, user_id: &str, role: OrgRole) {
    store
        .execute_unchecked(&format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{user_id}' \
             AND org_id IN (SELECT id FROM organizations WHERE owner_id IS NULL)",
            role.as_str()
        ))
        .await
        .expect("the role is set");
}

/// The first user of the instance, who administers it and the test's organization.
pub(crate) async fn founder(test: &TestStore) -> String {
    common::user(test, "founder").await
}

/// An account with everything a person leaves in the tables that are not an
/// organization's: a name, an address, a shown name, a password hash, an identity at a
/// provider, a session, a setting and a credential to set the password anew. A member
/// of the handle's organization and its default workspace.
async fn person(store: &Store) -> String {
    let mut user = User {
        id: new_id(),
        username: "leaver-name".to_owned(),
        // A capital that is no ASCII letter: the databases fold such a letter each in
        // its own way, and the address is found as it is written all the same.
        email: "Ünder-leaver-mail@example.com".to_owned(),
        email_verified_at: T0,
        name: PersonName {
            first_name: "Leaver-First".to_owned(),
            last_name: "Leaver-Last".to_owned(),
            latin_first_name: "Leaver-Latin-First".to_owned(),
            latin_last_name: "Leaver-Latin-Last".to_owned(),
            custom_name: "Leaver Shown".to_owned(),
            name_shown: NameShown::LatinName,
            title: Some(Title::Prof),
            credentials: vec![
                Credential::KrPatentAttorney,
                Credential::KrKoreanMedicineDoctor,
            ],
        },
        password_hash: "leaver-password-hash".to_owned(),
        ..User::default()
    };
    store.create_user(&mut user).await.expect("stored");
    let identity = Identity {
        issuer: "https://provider.example",
        subject: "leaver-subject",
    };
    store
        .add_identity(&user.id, identity, "leaver-at-the-provider@example.com")
        .await
        .expect("tied");
    // An address a provider gave that is somebody else's: the provider's word is
    // checked by nobody, and what was mailed there is not the account's.
    let other = Identity {
        issuer: "https://other.example",
        subject: "leaver-subject",
    };
    store
        .add_identity(&user.id, other, "somebody-else@example.com")
        .await
        .expect("tied");
    let session = Session {
        id: new_id(),
        user_id: user.id.clone(),
        created_at: T0,
        last_used_at: T0,
        expires_at: T0 + 2 * WAIT,
        user_agent: "leaver-browser".to_owned(),
        ..Session::default()
    };
    assert!(
        store
            .create_session(&session, "a session's digest", "leaver-password-hash")
            .await
            .expect("stored"),
        "the session is stored"
    );
    assert!(
        store
            .create_password_reset(&user.id, &user.email, "a credential's digest", T0)
            .await
            .expect("stored"),
        "the credential is stored"
    );
    store
        .execute_unchecked(&format!(
            "INSERT INTO user_settings (user_id, theme, language) \
             VALUES ('{}', 'dark', 'ko')",
            user.id
        ))
        .await
        .expect("stored");
    store
        .execute_unchecked(&format!(
            "INSERT INTO user_client_state (user_id, key, value) \
             VALUES ('{}', 'seen', 'yes')",
            user.id
        ))
        .await
        .expect("stored");
    user.id
}

/// A private board of the workspace whose admin is `user_id`, with a card they wrote.
pub(crate) async fn board_of(store: &Store, workspace_id: &str, user_id: &str) -> (String, String) {
    let mut board = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        title: "a board".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut board, user_id)
        .await
        .expect("the board is stored");
    let mut card = Block {
        id: new_id(),
        container_id: board.id.clone(),
        block_type: BlockType::Card,
        title: "a card".to_owned(),
        ..Block::default()
    };
    // Quietly, so that its writer does not watch it: the tests that want the card
    // followed say so (`work_on`).
    store
        .telling_nobody()
        .insert_block(&mut card, user_id)
        .await
        .expect("the card is stored");
    (board.id, card.id)
}

/// A link into the workspace made by `user_id`, under the digest.
pub(crate) async fn link_by(store: &Store, workspace_id: &str, user_id: &str, digest: &str) {
    let link = NewLink {
        workspace_id,
        guest_of: None,
        token_hash: digest,
        created_by: user_id,
        now: T0,
        expires_at: T0 + WAIT,
        max_uses: None,
        mailed: None,
    };
    store
        .create_invitation(&link)
        .await
        .expect("the link is made");
}

/// Writes down that the account made the organization, as the start of one does.
async fn made_by(test: &TestStore, org_id: &str, user_id: &str) {
    test.execute_unchecked(&format!(
        "UPDATE organizations SET created_by = '{user_id}' WHERE id = '{org_id}'"
    ))
    .await
    .expect("the organization is the account's making");
}

/// Erases the account through every step, on the operator's word, and returns what
/// became of it in each organization.
pub(crate) async fn erase(
    test: &TestStore,
    user_id: &str,
    erasing: Erasing,
    now: i64,
) -> Vec<LeftFrom> {
    let begun = test
        .begin_account_erasure(user_id, now, erasing)
        .await
        .expect("begun");
    assert!(
        matches!(begun, ErasureStart::Begun(_)),
        "the erasing begins"
    );
    let erased = erase_from_organizations(test, user_id, erasing).await;
    assert_eq!(
        test.finish_account_erasure(user_id, now)
            .await
            .expect("finished"),
        ErasureEnd::Erased,
        "the erasing ends"
    );
    erased
}

pub(crate) async fn erase_from_organizations(
    test: &TestStore,
    user_id: &str,
    erasing: Erasing,
) -> Vec<LeftFrom> {
    let mut erased = Vec::new();
    for org_id in test
        .organizations_of_account(user_id)
        .await
        .expect("listed")
    {
        let store = test.for_org(&org_id).expect("an organization");
        erased.push(
            store
                .erase_account_from_organization(user_id, erasing)
                .await
                .expect("erased from the organization"),
        );
    }
    erased
}

/// A column of the schema, as the database's own catalog lists it.
struct Column {
    table: String,
    name: String,
    /// Whether it holds bytes, which PostgreSQL shows as hexadecimal unless told.
    bytes: bool,
}

/// Every column of every table of the schema, read from the database's own catalog:
/// a table or a column that is added later is swept with the others.
async fn columns(system: &Store) -> Vec<Column> {
    let listed = match common::db_type() {
        DbType::Sqlite => {
            system
                .texts_unchecked(
                    "SELECT m.name || '.' || c.name || '.' || lower(c.type) \
                     FROM sqlite_master AS m JOIN pragma_table_info(m.name) AS c \
                     WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'",
                )
                .await
        }
        DbType::Postgres => {
            system
                .texts_unchecked(
                    "SELECT table_name || '.' || column_name || '.' || data_type \
                     FROM information_schema.columns WHERE table_schema = current_schema()",
                )
                .await
        }
    }
    .expect("the catalog is read");
    assert!(
        listed.len() > 200,
        "the columns are found: {}",
        listed.len()
    );
    listed
        .iter()
        .map(|name| {
            let mut parts = name.splitn(3, '.');
            let mut part = || {
                parts
                    .next()
                    .expect("a table, a column and a type")
                    .to_owned()
            };
            let (table, name, kind) = (part(), part(), part());
            Column {
                table,
                name,
                bytes: kind == "bytea" || kind == "blob",
            }
        })
        .collect()
}

/// The columns that hold `needle` in some row, of any organization, in any letter
/// case: as text, a value of any type, JSON included; in a column of bytes, the bytes
/// of the needle as it is written and in small letters, whatever bytes are around
/// them (SQLite reads bytes as text up to the first zero byte only).
pub(crate) async fn holding(system: &Store, needle: &str) -> Vec<String> {
    let small = needle.to_lowercase();
    let mut found = Vec::new();
    for column in columns(system).await {
        let (table, name) = (&column.table, &column.name);
        let holds = match (common::db_type(), column.bytes) {
            (DbType::Postgres, true) => {
                format!("lower(encode(\"{name}\", 'escape')) LIKE '%{small}%'")
            }
            (DbType::Postgres, false) => {
                format!("lower(CAST(\"{name}\" AS TEXT)) LIKE '%{small}%'")
            }
            (DbType::Sqlite, true) => format!(
                "instr(hex(\"{name}\"), hex('{needle}')) > 0 \
                 OR instr(hex(\"{name}\"), hex('{small}')) > 0"
            ),
            (DbType::Sqlite, false) => {
                format!("instr(lower(CAST(\"{name}\" AS TEXT)), '{small}') > 0")
            }
        };
        let count = system
            .texts_unchecked(&format!(
                "SELECT CAST(count(*) AS TEXT) FROM \"{table}\" WHERE {holds}"
            ))
            .await
            .unwrap_or_else(|error| panic!("{table}.{name} is searched: {error}"));
        if count != ["0"] {
            found.push(format!("{table}.{name}"));
        }
    }
    found.sort();
    found
}

/// How many rows each table holds, of every organization.
pub(crate) async fn rows_of_every_table(system: &Store) -> std::collections::BTreeMap<String, i64> {
    let mut rows = std::collections::BTreeMap::new();
    for column in columns(system).await {
        if rows.contains_key(&column.table) {
            continue;
        }
        let query = format!("SELECT CAST(count(*) AS TEXT) FROM \"{}\"", column.table);
        let counted = count(system, &query).await;
        rows.insert(column.table, counted);
    }
    rows
}

/// What somebody leaves on a card of theirs besides the card: a comment with a version
/// they wrote, a kept version of the body they edited, and that they follow the card.
pub(crate) async fn work_on(store: &Store, board: &str, card: &str, user_id: &str) {
    let mut comment = Block {
        id: new_id(),
        container_id: board.to_owned(),
        parent_id: card.to_owned(),
        block_type: BlockType::Comment,
        ..Block::default()
    };
    store
        .telling_nobody()
        .insert_block(&mut comment, user_id)
        .await
        .expect("the comment is stored");
    let digest = match common::db_type() {
        DbType::Postgres => "'\\x00'::bytea",
        DbType::Sqlite => "x'00'",
    };
    for statement in [
        format!(
            "INSERT INTO comment_versions (block_id, version, files, written_by, written_at) \
             VALUES ('{}', 1, '[]', '{user_id}', {T0})",
            comment.id
        ),
        format!(
            "INSERT INTO body_versions (block_id, version, body_digest, files, kept_at) \
             VALUES ('{card}', 1, {digest}, '[]', {T0})"
        ),
        format!(
            "INSERT INTO body_version_editors (block_id, version, user_id) \
             VALUES ('{card}', 1, '{user_id}')"
        ),
        format!(
            "INSERT INTO subscriptions (block_type, block_id, user_id, \
             created_at) VALUES ('card', '{card}', '{user_id}', {T0})"
        ),
        format!(
            "INSERT INTO reactions (block_id, user_id, emoji, created_at) \
             VALUES ('{card}', '{user_id}', '👍', {T0})"
        ),
    ] {
        store
            .execute_unchecked(&statement)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
}

/// How many rows of each table that names its organization (`org_id`) are the
/// organization's.
pub(crate) async fn rows_of(system: &Store, org_id: &str) -> Vec<(String, i64)> {
    let mut rows = Vec::new();
    for column in columns(system).await {
        if column.name != "org_id" {
            continue;
        }
        let query = format!(
            "SELECT CAST(count(*) AS TEXT) FROM \"{}\" WHERE CAST(org_id AS TEXT) = '{org_id}'",
            column.table
        );
        rows.push((column.table, count(system, &query).await));
    }
    rows.sort();
    assert!(
        rows.len() > 10,
        "the tables of an organization are found: {rows:?}"
    );
    rows
}

pub(crate) async fn count(store: &Store, query: &str) -> i64 {
    store.texts_unchecked(query).await.expect("counted")[0]
        .parse()
        .expect("a number")
}

/// What a person leaves behind besides what [`person`] stores, in the test's
/// organization: links they made and links mailed to them by `founder`, used and not;
/// a sign-in of theirs held at a provider, and a record that says who their identity
/// is; a board of theirs in the trash; an upload they began on `board`; and what the
/// operator did about their account.
async fn leave_traces(test: &TestStore, founder: &str, leaver: &str, board: &str) {
    link_by(test, &test.workspace_id, leaver, "the leaver's unused link").await;
    // Links mailed to the person's addresses by somebody else: one unused, one used.
    link_by(test, &test.workspace_id, founder, "mailed, unused").await;
    link_by(test, &test.workspace_id, founder, "mailed, used").await;
    link_by(
        test,
        &test.workspace_id,
        founder,
        "mailed to the other address",
    )
    .await;
    // A sign-in through a provider held for somebody of their address to say that the
    // account is theirs (it is tied to no account), and a record that says who their
    // identity is.
    let provider_id = new_id();
    for statement in [
        "UPDATE invitations SET recipient = 'Ünder-Leaver-Mail@example.com' \
         WHERE token_hash = 'mailed, unused'"
            .to_owned(),
        "UPDATE invitations SET recipient = 'Ünder-leaver-mail@example.com', uses = 1 \
         WHERE token_hash = 'mailed, used'"
            .to_owned(),
        "UPDATE invitations SET recipient = 'somebody-else@example.com' \
         WHERE token_hash = 'mailed to the other address'"
            .to_owned(),
        format!(
            "INSERT INTO oidc_providers (id, org_id, issuer, client_id, client_secret, \
             auto_join, enabled, created_at, updated_at) VALUES ('{provider_id}', '{org}', \
             'https://provider.example', 'client', '', true, true, {T0}, {T0})",
            org = test.org_id
        ),
        format!(
            "INSERT INTO oidc_held_sign_ins (held_hash, org_id, provider_id, revision, issuer, \
             subject, email, email_unverified, name, preferred_username, created_at, expires_at) \
             VALUES ('held', '{org}', '{provider_id}', 1, 'https://provider.example', \
             'somebody at the provider', 'Ünder-LEAVER-MAIL@example.com', false, 'Leaver Shown', \
             'leaver-name', {T0}, {later})",
            org = test.org_id,
            later = T0 + WAIT
        ),
        format!(
            "INSERT INTO audit_log (id, org_id, actor_id, actor_kind, action, target_id, \
             subject_id, created_at, detail, identity) VALUES ('{id}', '{org}', '{leaver}', \
             'person', 'oidc_identity_bound', '{org}', '{leaver}', {T0}, \
             'https://provider.example', 'leaver-at-the-provider@example.com')",
            id = new_id(),
            org = test.org_id
        ),
        // The link back to an address they had before.
        format!(
            "INSERT INTO email_reverts (token_hash, user_id, email, created_at, expires_at) \
             VALUES ('back', '{leaver}', 'Earlier-Leaver-Mail@example.com', {T0}, {T0})"
        ),
    ] {
        test.execute_unchecked(&statement)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    // A link of theirs that somebody used, a board of theirs in the trash with a card,
    // and an upload they had begun: what stays names them by identifier alone.
    link_by(test, &test.workspace_id, leaver, "the leaver's used link").await;
    test.execute_unchecked(
        "UPDATE invitations SET uses = 1 WHERE token_hash = 'the leaver''s used link'",
    )
    .await
    .expect("used");
    // Links of theirs that others were made like, for the recipients of their mail:
    // one whose copy was used, one whose copy was not.
    for (like, copy, uses) in [
        ("made like, the copy used", "a copy that was used", 1),
        ("made like, the copy unused", "a copy nobody used", 0),
    ] {
        link_by(test, &test.workspace_id, leaver, like).await;
        link_by(test, &test.workspace_id, leaver, copy).await;
        let statement = format!(
            "UPDATE invitations SET uses = {uses}, recipient = 'somebody@example.com', \
             parent_id = (SELECT id FROM invitations WHERE token_hash = '{like}') \
             WHERE token_hash = '{copy}'"
        );
        test.execute_unchecked(&statement)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
    let (thrown_away, _) = board_of(test, &test.workspace_id, leaver).await;
    test.delete_container(&thrown_away, leaver)
        .await
        .expect("to the trash");
    test.execute_unchecked(&format!(
        "INSERT INTO uploads (id, user_id, container_id, name, size, content_type, part_size, \
         created_at, expires_at, closed_at, multipart_id) VALUES ('an upload', '{leaver}', \
         '{board}', 'a file', 1, 'text/plain', 1, {T0}, {later}, 0, '')",
        later = T0 + WAIT
    ))
    .await
    .expect("an upload is begun");
    // What the operator did about the account before is on the record.
    assert!(
        test.create_password_reset_by_operator(leaver, "the operator's digest", T0)
            .await
            .expect("stored"),
        "the operator's credential is stored"
    );
}

/// What the person wrote stays, under their identifier; so does the record about
/// them, without who their identity is; and of the links, the used ones, without the
/// address they were mailed to.
async fn check_what_stays(test: &TestStore, leaver: &str, board: &str, card: &str) {
    // What they wrote stays, under their identifier; so does the record about them,
    // without who their identity is.
    let written = test.get_block(card).await.expect("the card stays");
    assert_eq!(written.created_by, leaver, "the card names its writer");
    assert_eq!(
        test.get_container(board)
            .await
            .expect("the board stays")
            .created_by,
        leaver,
        "the board names its maker"
    );
    let records = test
        .audit_records(AuditPlace::Organization, EVERY)
        .await
        .expect("read");
    let bound = records
        .iter()
        .find(|record| record.action == AuditAction::OidcIdentityBound)
        .expect("the record stays");
    assert_eq!(bound.identity, None, "without who the identity is");
    assert_eq!(
        bound.detail.as_deref(),
        Some("https://provider.example"),
        "with the provider it was of"
    );
    assert_eq!(
        bound.subject.as_ref().map(|subject| subject.id.as_str()),
        Some(leaver),
        "about the account, by its identifier"
    );
    // The links: theirs that nobody used and the unused one mailed to them are gone;
    // the used ones stay, the mailed one without the address, and with a copy that
    // was used the link it was made like.
    let mut links = test
        .texts_unchecked("SELECT token_hash FROM invitations")
        .await
        .expect("read");
    links.sort();
    assert_eq!(
        links,
        [
            "a copy that was used",
            "made like, the copy used",
            "mailed to the other address",
            "mailed, used",
            "the leaver's used link"
        ],
        "the links that were used stay, and the one mailed to an address a provider gave"
    );
    assert_eq!(
        test.texts_unchecked(
            "SELECT recipient FROM invitations \
             WHERE token_hash = 'mailed to the other address'"
        )
        .await
        .expect("read"),
        ["somebody-else@example.com"],
        "which may be somebody else's: it keeps its address"
    );
}

/// Records a personal unit progress row for `learner` in a course owned by `owner`.
async fn record_course_progress(store: &Store, workspace_id: &str, owner: &str, learner: &str) {
    let mut course = Container {
        id: new_id(),
        workspace_id: workspace_id.to_owned(),
        visibility: ContainerVisibility::Private,
        kind: ContainerKind::Course,
        title: "erasure course".to_owned(),
        ..Container::default()
    };
    store
        .insert_container_with_admin(&mut course, owner)
        .await
        .expect("a course is stored");
    let unit = store
        .create_unit(
            &course.id,
            &NewUnit {
                title: "lesson".to_owned(),
                ..NewUnit::default()
            },
            CourseLimits::DEFAULT,
            owner,
        )
        .await
        .expect("a unit is stored")
        .first;
    store
        .save_member(&ContainerMember {
            container_id: course.id.clone(),
            user_id: learner.to_owned(),
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("learner enrolled in course");
    store
        .upsert_unit_progress(
            &course.id,
            &unit.id,
            learner,
            &UnitProgressChange {
                completion: CompletionChange::Complete,
                last_position: LastPositionChange::Set("leaver-shelf".to_owned()),
            },
        )
        .await
        .expect("progress is stored");
    record_course_meetings_and_exams(store, &course.id, &unit.id, learner).await;
    store
        .schedule_unit_review(
            &course.id,
            &unit.id,
            learner,
            &NewUnitReview {
                scheduled_date: "2026-11-25".to_owned(),
                interval_days: 3,
            },
            None,
        )
        .await
        .expect("review is stored");
    store
        .create_unit_note(
            &course.id,
            &unit.id,
            learner,
            &NewUnitNote {
                kind: UnitNoteKind::Mistake,
                content: "leaver-shelf personal mistake".to_owned(),
            },
        )
        .await
        .expect("note is stored");
}

/// Seeds shared course meeting/attendance tombstones and personal exam scores for erasure tests.
async fn record_course_meetings_and_exams(
    store: &Store,
    course_id: &str,
    unit_id: &str,
    learner: &str,
) {
    let meeting = store
        .create_course_meeting(
            course_id,
            &NewCourseMeeting {
                title: "erasure study".to_owned(),
                starts_at: T0 + 1000,
                location: String::new(),
                unit_ids: vec![unit_id.to_owned()],
                presenter_id: Some(learner.to_owned()),
                host_id: Some(learner.to_owned()),
                notes: String::new(),
            }
            .check_request()
            .expect("valid meeting"),
        )
        .await
        .expect("meeting is stored");
    store
        .upsert_meeting_attendance(course_id, &meeting.id, learner, AttendanceStatus::Present)
        .await
        .expect("attendance is stored");
    let exam = store
        .create_course_exam(
            course_id,
            &NewCourseExam {
                title: "erasure exam".to_owned(),
                exam_date: Some("2026-12-01".to_owned()),
                max_score: 100.0,
                description: String::new(),
            }
            .check_request()
            .expect("valid exam"),
        )
        .await
        .expect("exam is stored");
    store
        .upsert_my_exam_score(
            course_id,
            &exam.id,
            learner,
            &ValidatedMyExamScore {
                score: 88.0,
                note: "leaver-shelf".to_owned(),
            },
        )
        .await
        .expect("exam score is stored");
}

/// Nothing of the person is in any column of any table once the account is erased;
/// the row keeps its identifier, and what the person wrote and the records about them
/// stay, pointing at it.
#[tokio::test]
async fn an_erased_account_leaves_its_identifier_and_nothing_of_the_person() {
    let test = common::setup().await;
    let system = test.system();
    let founder = founder(&test).await;
    let leaver = person(&test).await;
    let (board, card) = board_of(&test, &test.workspace_id, &leaver).await;
    work_on(&test, &board, &card, &leaver).await;
    // A shelf of their sidebar with the board on it, and a card of somebody else's
    // that names them in a property of people.
    let shelf = Category {
        id: new_id(),
        name: "leaver-shelf".to_owned(),
        user_id: leaver.clone(),
        workspace_id: test.workspace_id.clone(),
        ..Category::default()
    };
    test.create_category(&shelf).await.expect("a shelf");
    test.add_update_category_container(&leaver, &shelf.id, std::slice::from_ref(&board))
        .await
        .expect("the board is on it");
    let (_, assigned) = board_of(&test, &test.workspace_id, &founder).await;
    let statement = format!(
        "UPDATE blocks SET fields = '{{\"properties\":{{\"who\":\"{leaver}\"}}}}' \
         WHERE id = '{assigned}'"
    );
    test.execute_unchecked(&statement)
        .await
        .unwrap_or_else(|error| panic!("{statement}: {error}"));
    let people = test
        .create_team(&test.workspace_id, "some people", &founder)
        .await
        .expect("a team of the workspace");
    test.add_team_member(&test.workspace_id, &people, &leaver, &founder)
        .await
        .expect("the leaver is in it");
    record_course_progress(&test, &test.workspace_id, &founder, &leaver).await;
    leave_traces(&test, &founder, &leaver, &board).await;
    // The organization is of the leaver's making, as one they started is.
    made_by(&test, &test.org_id, &leaver).await;
    for needle in OF_THE_PERSON {
        assert!(
            !holding(&system, needle).await.is_empty(),
            "{needle} is stored before the account is erased"
        );
    }
    assert!(
        holding(&system, &leaver)
            .await
            .contains(&"organizations.created_by".to_owned()),
        "the organization names who made it before the account is erased"
    );

    erase(&test, &leaver, Erasing::AtOnce, T0 + 1).await;

    for needle in OF_THE_PERSON {
        assert_eq!(
            holding(&system, needle).await,
            NOBODY,
            "{needle} is stored nowhere"
        );
    }
    let left = test
        .texts_unchecked(&format!(
            "SELECT username || '|' || email || '|' || display_name || '|' || password_hash \
             || '|' || CAST(erased_at AS TEXT) FROM users WHERE id = '{leaver}'"
        ))
        .await
        .expect("read");
    assert_eq!(
        left,
        [format!("||||{}", T0 + 1)],
        "the row is its identifier"
    );
    // The identifier stays where it says who wrote or did something, and nowhere else:
    // no membership, nothing that was the person's own.
    assert_eq!(holding(&system, &leaver).await, KEEPS_THE_IDENTIFIER);
    check_what_stays(&test, &leaver, &board, &card).await;
    // The name and the address are free for somebody else.
    let mut again = User {
        id: new_id(),
        username: "leaver-name".to_owned(),
        email: "Ünder-leaver-mail@example.com".to_owned(),
        ..User::default()
    };
    test.create_user(&mut again)
        .await
        .expect("the name is free");
    test.teardown().await;
}

/// An organization names who made it no more once that account is erased, also where
/// the account has no place in it any more: it is found by its maker, and stays with
/// its people.
#[tokio::test]
async fn an_organization_forgets_a_maker_who_left_it_and_is_erased() {
    let test = common::setup().await;
    let system = test.system();
    let founder = founder(&test).await;
    // In no organization: its own space is all it has.
    let maker = common::user(&test.scoped(Tenant::Unset), "maker").await;
    made_by(&test, &test.org_id, &maker).await;
    let places = test.organizations_of_account(&maker).await.expect("listed");
    assert!(
        places.contains(&test.org_id),
        "the organization it made is among what it has something in: {places:?}"
    );
    let people_before = test
        .count_rows("organization_members")
        .await
        .expect("counted");

    erase(&test, &maker, Erasing::AtOnce, T0 + 1).await;

    let named = holding(&system, &maker).await;
    assert_eq!(named, ["users.id"], "nothing names the account but its row");
    let still: Vec<String> = system
        .texts_unchecked(&format!(
            "SELECT CAST(id AS TEXT) FROM organizations WHERE id = '{}' AND created_by IS NULL",
            test.org_id
        ))
        .await
        .expect("read");
    assert_eq!(
        still,
        std::slice::from_ref(&test.org_id),
        "the organization stays"
    );
    assert_eq!(
        test.count_rows("organization_members")
            .await
            .expect("counted"),
        people_before - i64::from(common::db_type() == DbType::Sqlite),
        "with its people: only the maker's own space went, where that is counted"
    );
    assert!(
        test.account_deletion_standing(&founder).await.is_ok(),
        "and its administrator is who they were"
    );
    test.teardown().await;
}

/// The guard of the audit log lets who an identity is be taken out of the records
/// about an account that is being erased, and nothing else: no other column, no other
/// record, no removal.
#[tokio::test]
async fn the_audit_log_gives_up_who_an_identity_is_and_nothing_else() {
    let test = common::setup().await;
    founder(&test).await;
    let leaver = person(&test).await;
    let stays = common::user(&test, "stays").await;
    for user_id in [&leaver, &stays] {
        test.execute_unchecked(&format!(
            "INSERT INTO audit_log (id, org_id, actor_id, actor_kind, action, target_id, \
             subject_id, created_at, identity) VALUES ('{id}', '{org}', '{user_id}', 'person', \
             'oidc_identity_bound', '{org}', '{user_id}', {T0}, 'who@example.com')",
            id = new_id(),
            org = test.org_id
        ))
        .await
        .expect("recorded");
    }
    let of = |user_id: &str| format!("actor_id = '{user_id}' AND identity IS NOT NULL");
    let refused = |result: Result<u64, StoreError>, what: &str| {
        let error = result.expect_err(what).to_string();
        assert!(
            error.contains("a record of the audit log is not changed or removed"),
            "{what}: {error}"
        );
    };

    // Nobody is being erased: no record gives anything up.
    for user_id in [&leaver, &stays] {
        refused(
            test.execute_unchecked(&format!(
                "UPDATE audit_log SET identity = NULL WHERE {}",
                of(user_id)
            ))
            .await,
            "an identity of an account that is not being erased",
        );
    }
    assert!(matches!(
        test.begin_account_erasure(&leaver, T0, Erasing::AtOnce)
            .await
            .expect("begun"),
        ErasureStart::Begun(_)
    ));
    // Of the record about the account being erased, every other change is refused: of
    // each column to another value, of one that is NULL to a value, and with the
    // identity taken out in the same statement.
    let workspace = &test.workspace_id;
    for change in [
        "identity = 'somebody.else@example.com'".to_owned(),
        "identity = ''".to_owned(),
        format!("id = '{}'", new_id()),
        format!("actor_id = '{stays}'"),
        format!("subject_id = '{stays}'"),
        "subject_id = NULL".to_owned(),
        "actor_id = NULL, actor_kind = 'server'".to_owned(),
        "action = 'oidc_identity_carried_over'".to_owned(),
        format!("target_id = '{}'", new_id()),
        "created_at = 1".to_owned(),
        "detail = 'https://another.example'".to_owned(),
        format!("workspace_id = '{workspace}', action = 'org_admin_joined'"),
    ] {
        for with_identity in ["", ", identity = NULL"] {
            let statement = format!(
                "UPDATE audit_log SET {change}{with_identity} WHERE {}",
                of(&leaver)
            );
            if change.starts_with("identity") && !with_identity.is_empty() {
                continue;
            }
            refused(test.execute_unchecked(&statement).await, &statement);
        }
    }
    refused(
        test.execute_unchecked(&format!("DELETE FROM audit_log WHERE {}", of(&leaver)))
            .await,
        "a removal",
    );
    refused(
        test.execute_unchecked(&format!(
            "UPDATE audit_log SET identity = NULL WHERE {}",
            of(&stays)
        ))
        .await,
        "the identity of somebody else",
    );

    let given_up = test
        .execute_unchecked(&format!(
            "UPDATE audit_log SET identity = NULL WHERE {}",
            of(&leaver)
        ))
        .await
        .expect("the identity of the account being erased is taken out");
    assert_eq!(given_up, 1);
    assert_eq!(
        count(
            &test,
            "SELECT CAST(count(*) AS TEXT) FROM audit_log WHERE identity IS NOT NULL"
        )
        .await,
        1,
        "the other record keeps its identity"
    );
    test.teardown().await;
}

/// Where a leaver is in the test's organization, and who is there with them.
struct Places {
    leaver: String,
    /// The administrator of the default workspace who has been one longest on their
    /// own account, and one who has been one for less long.
    heir: String,
    younger: String,
    /// Boards of the default workspace the leaver is the admin of: alone, and with
    /// `younger`.
    passes: String,
    shared: String,
    /// A board of `alone`, a workspace the leaver administers alone.
    orphan: String,
    alone: String,
    /// A workspace in the trash, with a board of the leaver's in the trash too.
    trashed: String,
}

async fn places_of_a_leaver(test: &TestStore, founder: &str) -> Places {
    let leaver = common::user(test, "leaver").await;
    let heir = common::user(test, "heir").await;
    let younger = common::user(test, "younger").await;
    // The default workspace: the founder joined it as the organization's
    // administrator, which makes them no heir; two administrators on their own account.
    for statement in [
        format!("UPDATE workspace_members SET via_org_admin = true WHERE user_id = '{founder}'"),
        format!(
            "UPDATE workspace_members SET role = 'admin', created_at = 10 WHERE user_id = '{heir}'"
        ),
        format!(
            "UPDATE workspace_members SET role = 'admin', created_at = 20 \
             WHERE user_id = '{younger}'"
        ),
    ] {
        test.execute_unchecked(&statement).await.expect("set");
    }
    let (passes, _) = board_of(test, &test.workspace_id, &leaver).await;
    let (shared, _) = board_of(test, &test.workspace_id, &leaver).await;
    let other_admin = fvoci_model::ContainerMember::new(
        shared.clone(),
        younger.clone(),
        fvoci_model::ContainerRole::Admin,
    );
    test.save_member(&other_admin)
        .await
        .expect("a second admin");
    // A second workspace, of the leaver alone, and a third in the trash.
    let alone = common::workspace(test).await;
    let trashed = common::workspace(test).await;
    for workspace_id in [&alone, &trashed] {
        test.add_workspace_member(workspace_id, &leaver, WorkspaceRole::Admin)
            .await
            .expect("let in");
    }
    let (orphan, _) = board_of(test, &alone, &leaver).await;
    let (in_the_trash, _) = board_of(test, &trashed, &leaver).await;
    test.delete_container(&in_the_trash, &leaver)
        .await
        .expect("to the trash");
    test.execute_unchecked(&format!(
        "UPDATE workspaces SET deleted_at = {T0} WHERE id = '{trashed}'"
    ))
    .await
    .expect("the workspace is in the trash");

    Places {
        leaver,
        heir,
        younger,
        passes,
        shared,
        orphan,
        alone,
        trashed,
    }
}

/// A board whose only admin is erased passes to the administrator of its workspace who
/// has been one longest on their own account, and that is recorded as the server's; a
/// board of a workspace without one is left for whoever administers it next. The
/// account goes from a workspace in the trash too, and from what its boards in the
/// trash would come back with.
#[tokio::test]
async fn a_board_whose_only_admin_is_erased_passes_to_an_administrator_of_its_workspace() {
    let test = common::setup().await;
    let founder = founder(&test).await;
    let Places {
        leaver,
        heir,
        younger,
        passes,
        shared,
        orphan,
        alone,
        trashed,
    } = places_of_a_leaver(&test, &founder).await;

    assert!(matches!(
        test.request_account_deletion(&leaver, "", T0, T0 + WAIT)
            .await
            .expect("asked"),
        DeletionAsking::Asked(..)
    ));
    let erased = erase(&test, &leaver, Erasing::AfterTheWait, T0 + WAIT).await;

    let handed: Vec<(&str, &str)> = erased
        .iter()
        .flat_map(|from| &from.handed_over)
        .map(|passed| (passed.container_id.as_str(), passed.heir_id.as_str()))
        .collect();
    assert_eq!(handed, [(passes.as_str(), heir.as_str())]);
    let mut left: Vec<&str> = erased
        .iter()
        .flat_map(|from| &from.left)
        .map(String::as_str)
        .collect();
    left.sort_unstable();
    let mut workspaces = [test.workspace_id.as_str(), alone.as_str(), trashed.as_str()];
    workspaces.sort_unstable();
    assert_eq!(
        left, workspaces,
        "every workspace, the one in the trash too"
    );
    let admins = |container_id: String| {
        let test = &test;
        async move {
            test.texts_unchecked(&format!(
                "SELECT CAST(user_id AS TEXT) FROM container_members \
                 WHERE container_id = '{container_id}' AND role = 'admin' ORDER BY 1"
            ))
            .await
            .expect("read")
        }
    };
    assert_eq!(admins(passes.clone()).await, std::slice::from_ref(&heir));
    assert_eq!(
        admins(shared).await,
        [younger],
        "an admin it had stays its admin"
    );
    assert_eq!(admins(orphan).await, NOBODY, "nobody to pass to");
    let recorded = test
        .audit_records(AuditPlace::Workspace(&test.workspace_id), EVERY)
        .await
        .expect("read");
    let passing = recorded
        .iter()
        .find(|record| record.target_id == passes)
        .expect("the passing is recorded");
    assert_eq!(passing.action, AuditAction::AddedToContainer);
    assert_eq!(passing.actor, fvoci_model::AuditRecordActor::Server);
    assert_eq!(
        passing.subject.as_ref().map(|subject| subject.id.as_str()),
        Some(heir.as_str())
    );
    assert_eq!(
        count(
            &test,
            &format!(
                "SELECT CAST(count(*) AS TEXT) FROM deleted_containers \
                 WHERE CAST(members AS TEXT) LIKE '%{leaver}%' \
                 OR CAST(places AS TEXT) LIKE '%{leaver}%'"
            )
        )
        .await,
        0,
        "a board of the trash comes back without the account"
    );
    test.teardown().await;
}

/// The question of the asking is asked again when the erasing begins: an account that
/// has become the only administrator of an organization with other people is not
/// erased, and nothing of it changes; once somebody else administers it, it is.
#[tokio::test]
async fn an_account_that_became_the_only_administrator_meanwhile_is_not_erased() {
    let test = common::setup().await;
    let founder = founder(&test).await;
    let second = common::user(&test, "second").await;
    set_org_role(&test, &second, OrgRole::Admin).await;
    test.execute_unchecked(&format!(
        "UPDATE users SET is_admin = true WHERE id = '{second}'"
    ))
    .await
    .expect("set");
    let asked = test
        .request_account_deletion(&founder, "", T0, T0 + WAIT)
        .await
        .expect("asked");
    assert!(matches!(asked, DeletionAsking::Asked(..)));

    // Not before its wait ran out.
    let early = test
        .begin_account_erasure(&founder, T0 + WAIT - 1, Erasing::AfterTheWait)
        .await
        .expect("asked");
    assert_eq!(early, ErasureStart::Waits);
    assert_eq!(
        test.accounts_to_erase(T0 + WAIT - 1).await.expect("listed"),
        NOBODY
    );
    assert_eq!(
        test.accounts_to_erase(T0 + WAIT).await.expect("listed"),
        std::slice::from_ref(&founder)
    );

    // The other administrator is one no more by then.
    set_org_role(&test, &second, OrgRole::Member).await;
    test.execute_unchecked(&format!(
        "UPDATE users SET is_admin = false WHERE id = '{second}'"
    ))
    .await
    .expect("set");
    let ErasureStart::Refused(standing) = test
        .begin_account_erasure(&founder, T0 + WAIT, Erasing::AfterTheWait)
        .await
        .expect("asked")
    else {
        panic!("the only administrator was erased");
    };
    assert!(standing.last_instance_admin);
    assert_eq!(standing.sole_admin_of.len(), 1);
    assert_eq!(
        test.account_by_id(&founder)
            .await
            .expect("it waits still")
            .username,
        "founder"
    );
    // The operator's word does not go past it either.
    assert!(matches!(
        test.begin_account_erasure(&founder, T0 + WAIT, Erasing::AtOnce)
            .await
            .expect("asked"),
        ErasureStart::Refused(_)
    ));

    // With an administrator again, the next run erases it; and then it is not taken back.
    set_org_role(&test, &second, OrgRole::Admin).await;
    test.execute_unchecked(&format!(
        "UPDATE users SET is_admin = true WHERE id = '{second}'"
    ))
    .await
    .expect("set");
    assert!(matches!(
        test.begin_account_erasure(&founder, T0 + WAIT + 1, Erasing::AfterTheWait)
            .await
            .expect("asked"),
        ErasureStart::Begun(_)
    ));
    assert!(
        !test
            .cancel_account_deletion(&founder, T0 + WAIT + 2)
            .await
            .expect("asked"),
        "an erasing that began is not taken back"
    );
    test.teardown().await;
}

/// An erasing that was cut off after its first step, or between two organizations, is
/// found again by its mark and ends when it is run again; it does not end while the
/// account still has a place somewhere.
#[tokio::test]
async fn an_erasing_that_was_cut_off_is_taken_up_and_ends() {
    let test = common::setup().await;
    founder(&test).await;
    let leaver = person(&test).await;
    board_of(&test, &test.workspace_id, &leaver).await;

    assert!(matches!(
        test.begin_account_erasure(&leaver, T0, Erasing::AtOnce)
            .await
            .expect("begun"),
        ErasureStart::Begun(_)
    ));
    assert!(
        test.is_address_held("Ünder-LEAVER-mail@example.com")
            .await
            .expect("asked"),
        "the address is the account's until the erasing ends"
    );
    // Cut off here: the account has its places still, and is not ended.
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 1)
            .await
            .expect("asked"),
        ErasureEnd::StillPlaced,
        "an account with a place somewhere is not ended"
    );
    assert_eq!(
        test.accounts_to_erase(T0 + 1).await.expect("listed"),
        std::slice::from_ref(&leaver),
        "the mark finds it again, whatever its wait"
    );
    // Taken up: the first step says it began, and the rest is done once.
    assert!(matches!(
        test.begin_account_erasure(&leaver, T0 + 2, Erasing::AfterTheWait)
            .await
            .expect("asked"),
        ErasureStart::Begun(_)
    ));
    erase_from_organizations(&test, &leaver, Erasing::AfterTheWait).await;
    assert_eq!(
        erase_from_organizations(&test, &leaver, Erasing::AfterTheWait).await,
        Vec::<LeftFrom>::new(),
        "no organization is left to go from"
    );
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 3)
            .await
            .expect("ended"),
        ErasureEnd::Erased,
        "ended"
    );
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 4)
            .await
            .expect("asked"),
        ErasureEnd::ErasedBefore,
        "ended once"
    );
    assert!(
        !test
            .is_address_held("Ünder-LEAVER-mail@example.com")
            .await
            .expect("asked"),
        "and the address is nobody's"
    );
    assert_eq!(
        test.accounts_to_erase(T0 + 5).await.expect("listed"),
        NOBODY
    );
    assert_eq!(
        test.begin_account_erasure(&leaver, T0 + 5, Erasing::AtOnce)
            .await
            .expect("asked"),
        ErasureStart::NotThere
    );
    test.teardown().await;
}

/// What is left of an erased account is answered to whoever asks by its identifier,
/// whether they saw the account before or not: a mention of it in what they read is a
/// deleted user's. By name it is found by nobody; an account in use that the seer
/// does not see is still not answered.
#[tokio::test]
async fn an_erased_account_is_answered_by_its_identifier_to_whoever_asks() {
    let test = common::setup().await;
    let seer = founder(&test).await;
    let leaver = person(&test).await;
    // Somebody the seer shares no workspace with, and wrote nothing the seer reads.
    let elsewhere = common::workspace(&test).await;
    let unseen = common::user(&test, "unseen").await;
    test.execute_unchecked(&format!(
        "DELETE FROM workspace_members WHERE user_id = '{unseen}'"
    ))
    .await
    .expect("out of the default workspace");
    test.add_workspace_member(&elsewhere, &unseen, WorkspaceRole::Member)
        .await
        .expect("into another");
    erase(&test, &leaver, Erasing::AtOnce, T0).await;

    let sight = |looking| Sight::InWorkspace {
        seer_id: &seer,
        workspace_id: &test.workspace_id,
        reach: Reach::Workspace,
        looking,
    };
    let asked = [leaver.clone(), unseen.clone()];
    let found = test
        .get_users_in_sight(sight(Looking::ByName), &asked)
        .await
        .expect("asked");
    assert_eq!(found.len(), 1, "the one that was erased alone: {found:?}");
    assert_eq!(found[0].id, leaver, "by its identifier");
    assert_eq!(
        (
            found[0].username.as_str(),
            found[0].email.as_str(),
            found[0].display_name.as_str()
        ),
        ("", "", ""),
        "and nothing else"
    );
    assert_eq!(
        test.search_users_in_sight(sight(Looking::AmongPeople), "")
            .await
            .expect("searched")
            .iter()
            .filter(|user| user.id == leaver)
            .count(),
        0,
        "among the people it is not"
    );
    if common::db_type() == DbType::Postgres {
        // Across the wall too, and there alone: somebody of another organization is
        // answered what is left of the erased account, and nobody of this one.
        let system = test.system();
        let (other_org, other_workspace) = common::organization(&system, "Other").await;
        let other_store = test.for_org(&other_org).expect("an organization");
        let outsider = common::user(&other_store, "outsider").await;
        let from_outside = Sight::InWorkspace {
            seer_id: &outsider,
            workspace_id: &other_workspace,
            reach: Reach::Workspace,
            looking: Looking::ByName,
        };
        let asked = [leaver.clone(), unseen, seer.clone()];
        let found = other_store
            .get_users_in_sight(from_outside, &asked)
            .await
            .expect("asked");
        assert_eq!(
            found
                .iter()
                .map(|user| user.id.as_str())
                .collect::<Vec<_>>(),
            [leaver.as_str()],
            "what is left of the erased account, and nobody in use"
        );
        assert_eq!(found[0].username, "", "no name");
    }
    test.teardown().await;
}

/// The guard of the audit log names every column but `identity`, the one that may be
/// set to nothing: a column added to the table later is to be named in it too, or it
/// could be changed with `identity`.
#[tokio::test]
async fn the_audit_logs_guard_names_every_column_of_the_table() {
    let test = common::setup().await;
    let mut names: Vec<String> = columns(&test.system())
        .await
        .into_iter()
        .filter(|column| column.table == "audit_log")
        .map(|column| column.name)
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "action",
            "actor_id",
            "actor_kind",
            "created_at",
            "detail",
            "id",
            "identity",
            "org_id",
            "subject_id",
            "target_id",
            "workspace_id"
        ],
        "a new column of the audit log is added to its guard as well"
    );
    for dialect in ["sqlite", "postgres"] {
        let guard = std::fs::read_to_string(format!(
            "{}/schema/{dialect}/0001_baseline.sql",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("the baseline");
        for name in names.iter().filter(|name| *name != "identity") {
            // As a whole word: `OLD.id` is not found in `OLD.identity`.
            let old_value = format!("OLD.{name}");
            let compared = guard.match_indices(&old_value).any(|(at, _)| {
                !guard[at + old_value.len()..]
                    .starts_with(|next: char| next.is_ascii_alphanumeric() || next == '_')
            });
            assert!(compared, "{dialect}: the guard compares {name}");
        }
    }
    test.teardown().await;
}
