//! Erasing an account that is alone in an organization: the organization goes with it
//! (except the instance's own), unless somebody comes in before or past the mark; and
//! an organization put out of use for one account goes with that account alone.

use fvoci_model::{FileRecord, LoginOrigin, OrgRole, Session, WorkspaceRole, new_id};
use fvoci_store::{DbType, DeletionAsking, Erasing, ErasureEnd, ErasureStart, LeftFrom, Store};

use crate::account_erasure::{
    NOBODY, T0, WAIT, board_of, count, erase, erase_from_organizations, founder, holding, link_by,
    rows_of, rows_of_every_table, work_on,
};
use crate::common::{self, TestStore};

fn file_of(board_id: &str) -> FileRecord {
    FileRecord {
        id: new_id(),
        container_id: board_id.to_owned(),
        stored_name: "stored.png".to_owned(),
        name: "a picture".to_owned(),
        extension: ".png".to_owned(),
        size: 2048,
        path: format!("boards/{board_id}/stored.png"),
        created_at: 1,
        version: 1,
        created_by: String::new(),
    }
}

/// Tables that hold rows of an organization in use: of the one that goes, they hold
/// none afterwards.
const HELD_BY_AN_ORGANIZATION: [&str; 11] = [
    "audit_log",
    "blocks",
    "containers",
    "container_members",
    "deleted_containers",
    "files",
    "invitations",
    "oidc_providers",
    "organization_members",
    "workspace_members",
    "workspaces",
];

/// Where the identifier of an organization that went is still found: what the file
/// storage is yet to be told, which its job takes away once the objects are removed.
/// (SQLite, which holds one organization, does not name it there.)
fn keeps_the_organization() -> &'static [&'static str] {
    match common::db_type() {
        DbType::Postgres => &["loose_objects.org_id"],
        DbType::Sqlite => &[],
    }
}

/// An account that is alone in an organization it administers.
struct Alone {
    leaver: String,
    org: String,
    /// The organization's default workspace, which the leaver administers.
    workspace: String,
    /// On PostgreSQL, an organization of others; the test's own is somebody else's
    /// too, with the leaver a member of it. SQLite holds one organization, the
    /// leaver's.
    third: Option<String>,
}

/// Makes the account that is alone in an organization, and somebody who administers
/// the instance from outside of it: the instance keeps an administrator.
async fn alone_in_an_organization(test: &TestStore) -> Alone {
    let system = test.system();
    let operator = common::user(&system, "operator").await;
    system
        .execute_unchecked(&format!(
            "UPDATE users SET is_admin = true WHERE id = '{operator}'"
        ))
        .await
        .expect("set");
    let several = common::db_type() == DbType::Postgres;
    let (alone, alone_workspace, third) = if several {
        // The test's organization is somebody else's, the leaver a member of it; a
        // second is the leaver's alone, and a third nobody's of them.
        founder(test).await;
        let (alone, alone_workspace) = common::organization(&system, "Alone").await;
        let (third, third_workspace) = common::organization(&system, "Third").await;
        let third_store = test.for_org(&third).expect("an organization");
        let elsewhere = common::user(&third_store, "elsewhere").await;
        board_of(&third_store, &third_workspace, &elsewhere).await;
        link_by(
            &third_store,
            &third_workspace,
            &elsewhere,
            "a link of the third",
        )
        .await;
        (alone, alone_workspace, Some(third))
    } else {
        (test.org_id.clone(), test.workspace_id.clone(), None)
    };
    let alone_store = test.for_org(&alone).expect("an organization");
    let leaver = common::user(test, "leaver").await;
    if several {
        board_of(test, &test.workspace_id, &leaver).await;
        alone_store
            .add_organization_member(&leaver, OrgRole::Admin)
            .await
            .expect("its administrator");
        alone_store
            .add_workspace_member(&alone_workspace, &leaver, WorkspaceRole::Admin)
            .await
            .expect("let in");
    } else {
        // The one organization is the leaver's alone, who administers it and not the
        // instance: the operator, who is in no organization, does that.
        for statement in [
            format!("UPDATE users SET is_admin = false WHERE id = '{leaver}'"),
            format!("UPDATE organization_members SET role = 'admin' WHERE user_id = '{leaver}'"),
            format!("UPDATE workspace_members SET role = 'admin' WHERE user_id = '{leaver}'"),
            format!(
                "DELETE FROM workspace_members WHERE user_id = '{operator}' \
                 AND workspace_id = '{alone_workspace}'"
            ),
            format!(
                "DELETE FROM organization_members WHERE user_id = '{operator}' \
                 AND org_id = '{alone}'"
            ),
        ] {
            system.execute_unchecked(&statement).await.expect("set");
        }
    }
    Alone {
        leaver,
        org: alone,
        workspace: alone_workspace,
        third,
    }
}

/// Begins erasing the account that is alone in `org` at `now`, as its wait running out
/// begins it. SQLite's one organization is the instance's own, which goes with no
/// account: the first step refuses there, and what it would write is written here, so
/// that the statements that take an organization away are held to both databases.
async fn begin_alone(test: &TestStore, leaver: &str, org: &str, now: i64) {
    if common::db_type() == DbType::Postgres {
        let begun = test
            .begin_account_erasure(leaver, now, Erasing::AfterTheWait)
            .await
            .expect("asked");
        assert!(
            matches!(begun, ErasureStart::Begun(_)),
            "the erasing begins"
        );
        return;
    }
    // Its asking was refused too: the operator's word is refused the same way.
    let begun = test
        .begin_account_erasure(leaver, now, Erasing::AtOnce)
        .await
        .expect("asked");
    let ErasureStart::Refused(standing) = begun else {
        panic!("the instance's own organization was to go: {begun:?}");
    };
    assert_eq!(standing.sole_admin_of[0].id, org, "{standing:?}");
    for statement in [
        format!(
            "UPDATE users SET deleted_at = {now}, delete_after = {now}, \
             erase_started_at = {now} WHERE id = '{leaver}'"
        ),
        format!("UPDATE organizations SET deleted_at = {now} WHERE id = '{org}'"),
    ] {
        test.system()
            .execute_unchecked(&statement)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
}

/// Fills an organization with what one holds: a board with a card and a file, what
/// its person wrote on the card and a group of people they are in, a board and a
/// workspace in the trash, a link, a provider, and a record.
async fn fill(alone_store: &Store, alone: &str, alone_workspace: &str, leaver: &str) {
    let (board, card) = board_of(alone_store, alone_workspace, leaver).await;
    work_on(alone_store, &board, &card, leaver).await;
    let people = alone_store
        .create_team(alone_workspace, "some people", leaver)
        .await
        .expect("a team of the workspace");
    alone_store
        .add_team_member(alone_workspace, &people, leaver, leaver)
        .await
        .expect("the leaver is in it");
    alone_store
        .save_file_record(&file_of(&board))
        .await
        .expect("a file");
    let (in_the_trash, _) = board_of(alone_store, alone_workspace, leaver).await;
    alone_store
        .delete_container(&in_the_trash, leaver)
        .await
        .expect("to the trash");
    let trashed = common::workspace(alone_store).await;
    alone_store
        .add_workspace_member(&trashed, leaver, WorkspaceRole::Admin)
        .await
        .expect("let in");
    board_of(alone_store, &trashed, leaver).await;
    alone_store
        .execute_unchecked(&format!(
            "UPDATE workspaces SET deleted_at = {T0} WHERE id = '{trashed}'"
        ))
        .await
        .expect("the workspace is in the trash");
    link_by(alone_store, alone_workspace, leaver, "a link into it").await;
    alone_store
        .execute_unchecked(&format!(
            "INSERT INTO oidc_providers (id, org_id, issuer, client_id, client_secret, \
             auto_join, enabled, created_at, updated_at) VALUES ('{}', '{alone}', \
             'https://provider.example', 'client', '', true, true, {T0}, {T0})",
            new_id()
        ))
        .await
        .expect("a provider");
    alone_store
        .execute_unchecked(&format!(
            "INSERT INTO audit_log (id, org_id, actor_id, actor_kind, action, target_id, \
             created_at) VALUES ('{}', '{alone}', '{leaver}', 'person', \
             'oidc_provider_configured', '{alone}', {T0})",
            new_id()
        ))
        .await
        .expect("a record");
}

/// Fills the own space of `owner` with what one holds, a board with a card and a file
/// and what its owner wrote on the card, and returns the space's organization.
async fn fill_the_space_of(test: &TestStore, owner: &str) -> String {
    let query = format!("SELECT CAST(id AS TEXT) FROM organizations WHERE owner_id = '{owner}'");
    let found = test.system().texts_unchecked(&query).await;
    let org = found.expect("read").remove(0);
    let workspace = common::own_workspace(test, owner).await;
    let space = test.for_org(&org).expect("an organization");
    let (board, card) = board_of(&space, &workspace, owner).await;
    work_on(&space, &board, &card, owner).await;
    let saved = space.save_file_record(&file_of(&board)).await;
    saved.expect("a file of the space");
    // A diary of the space: the entry of a day, one in the trash, and a file.
    let diary = crate::diaries::new_diary(&workspace);
    let made = space.create_diary(diary, owner, fvoci_model::DiaryMaking::Another);
    let diary = made.await.expect("a diary of the space").diary.id;
    crate::diaries::written(&space, &diary, "2024-02-29", owner).await;
    let gone = crate::diaries::written(&space, &diary, "2024-03-01", owner).await;
    let deleted = space.delete_block(&gone.id, owner).await;
    deleted.expect("an entry in the trash");
    let saved = space.save_file_record(&file_of(&diary)).await;
    saved.expect("a file of the diary");
    org
}

/// No column of any table names the space `org` any more, but what the storage is
/// still to be told.
async fn check_nothing_names(system: &Store, org: &str) {
    assert_eq!(
        holding(system, org).await,
        keeps_the_organization(),
        "nothing names the space but what the storage is to be told"
    );
}

/// The organization of others, and the test's own, hold the rows they held (`kept`,
/// in that order), but for the leaver's own membership of the test's.
async fn check_the_others_keep(test: &TestStore, third: &str, kept: Vec<Vec<(String, i64)>>) {
    let after = vec![
        rows_of(&test.system(), third).await,
        rows_of(&test.system(), &test.org_id).await,
    ];
    // The leaver's own membership of the test's organization went with the account.
    let mut expected = kept;
    for (table, rows) in &mut expected[1] {
        if [
            "organization_members",
            "workspace_members",
            "container_members",
        ]
        .contains(&table.as_str())
        {
            *rows -= 1;
        }
    }
    assert_eq!(after, expected, "the other organizations keep every row");
    assert!(
        test.count_rows("blocks").await.expect("counted") > 0,
        "what the account wrote in the organization that stays, stays"
    );
}

/// An organization that has nobody but the account goes with it, and everything in
/// it: no column of any table names it afterwards, but what the storage is still to be
/// told, and no table holds more rows than before it was filled. The other
/// organizations of the account, and those of others, keep every row. SQLite's one
/// organization is the instance's own, which is not asked to go: it is taken away
/// here all the same ([`begin_alone`]).
#[tokio::test]
async fn an_organization_that_has_nobody_else_goes_with_the_account() {
    let test = common::setup().await;
    let system = test.system();
    let Alone {
        leaver,
        org: alone,
        workspace: alone_workspace,
        third,
    } = alone_in_an_organization(&test).await;
    let several = third.is_some();
    let alone_store = test.for_org(&alone).expect("an organization");
    let before = rows_of_every_table(&system).await;
    fill(&alone_store, &alone, &alone_workspace, &leaver).await;
    let space = fill_the_space_of(&test, &leaver).await;
    for table in HELD_BY_AN_ORGANIZATION {
        assert!(
            alone_store.count_rows(table).await.expect("counted") > 0,
            "{table} holds rows of it before"
        );
    }
    let kept = match &third {
        Some(third) => vec![
            rows_of(&system, third).await,
            rows_of(&system, &test.org_id).await,
        ],
        None => Vec::new(),
    };

    let standing = test.account_deletion_standing(&leaver).await.expect("read");
    // The instance's own organization stays, whoever is in it.
    let (goes, stays) = if several {
        (&standing.alone_in, &standing.sole_admin_of)
    } else {
        (&standing.sole_admin_of, &standing.alone_in)
    };
    assert_eq!(goes.len(), 1, "{standing:?}");
    assert_eq!(goes[0].id, alone, "{standing:?}");
    assert!(stays.is_empty(), "{standing:?}");
    let counted = alone_store
        .leaving_organization(goes[0].clone())
        .await
        .expect("counted")
        .expect("still there");
    assert_eq!(
        (counted.boards, counted.wikis, counted.files),
        (Some(1), Some(0), Some(1)),
        "what is in use is counted"
    );
    let asked = test
        .request_account_deletion(&leaver, "", T0, T0 + WAIT)
        .await
        .expect("asked");
    assert_eq!(
        matches!(asked, DeletionAsking::Asked(..)),
        several,
        "asked where the organization goes: {asked:?}"
    );

    begin_alone(&test, &leaver, &alone, T0 + WAIT).await;
    let erased = erase(&test, &leaver, Erasing::AfterTheWait, T0 + WAIT).await;

    let removed: Vec<&LeftFrom> = erased
        .iter()
        .filter(|from| from.removed.is_some())
        .collect();
    // The organization, and the leaver's own space with what it holds.
    assert_eq!(removed.len(), 2, "two organizations went: {erased:?}");
    let files: Vec<_> = removed
        .iter()
        .flat_map(|from| &from.removed.as_ref().expect("removed").purged.files)
        .collect();
    // The file of the organization's board, of the space's board and of its diary.
    assert_eq!(files.len(), 3, "the files of each are doomed: {files:?}");
    check_nothing_names(&system, &space).await;
    assert_eq!(
        holding(&system, &alone).await,
        keeps_the_organization(),
        "nothing names the organization but what the storage is to be told"
    );
    for table in HELD_BY_AN_ORGANIZATION {
        // SQLite holds the system's organization too, which has a workspace, and
        // the space of the operator, who is in it and in its workspace. The leaver's
        // own space went with the account.
        let others = match (several, table) {
            (false, "workspaces") => 2,
            (false, "organization_members" | "workspace_members") => 1,
            _ => 0,
        };
        assert_eq!(
            alone_store.count_rows(table).await.expect("counted"),
            others,
            "{table} holds nothing of it afterwards"
        );
    }
    // Nothing of what filled it is left, in a table that names an organization or in
    // one that does not: but for the records of the leaving where the account was a
    // member besides, and what the storage is still to be told, no table has grown.
    for (table, rows) in rows_of_every_table(&system).await {
        if ["audit_log", "loose_objects"].contains(&table.as_str()) {
            continue;
        }
        assert!(
            rows <= before[&table],
            "{table} holds {rows} rows, and held {} before the organization was filled",
            before[&table]
        );
    }
    if let Some(third) = &third {
        check_the_others_keep(&test, third, kept).await;
    }
    test.teardown().await;
}

/// Somebody who came into the organization during the wait keeps it: the account is
/// then the only administrator of an organization with other people, and is not
/// erased.
#[tokio::test]
async fn an_organization_somebody_came_into_during_the_wait_stays() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let system = test.system();
    founder(&test).await;
    let (alone, _) = common::organization(&system, "Alone").await;
    let alone_store = test.for_org(&alone).expect("an organization");
    let leaver = common::user(&test, "leaver").await;
    alone_store
        .add_organization_member(&leaver, OrgRole::Admin)
        .await
        .expect("its administrator");
    assert!(matches!(
        test.request_account_deletion(&leaver, "", T0, T0 + WAIT)
            .await
            .expect("asked"),
        DeletionAsking::Asked(..)
    ));
    let newcomer = common::user(&system, "newcomer").await;
    alone_store
        .add_organization_member(&newcomer, OrgRole::Member)
        .await
        .expect("let in");

    let ErasureStart::Refused(standing) = test
        .begin_account_erasure(&leaver, T0 + WAIT, Erasing::AfterTheWait)
        .await
        .expect("asked")
    else {
        panic!("the only administrator of an organization with a newcomer was erased");
    };
    assert_eq!(standing.sole_admin_of[0].id, alone);
    assert_eq!(
        alone_store
            .count_rows("organizations")
            .await
            .expect("counted"),
        1,
        "the organization stays"
    );
    assert_eq!(
        count(
            &alone_store,
            "SELECT CAST(count(*) AS TEXT) FROM organizations WHERE deleted_at IS NULL"
        )
        .await,
        1,
        "in use"
    );
    test.teardown().await;
}

/// Somebody who comes into an organization after it was put out of use to go, past the
/// question the erasing began with, keeps it: it is in use again, the account leaves
/// it as it leaves any other, and whoever runs the server is to be told that it has
/// no administrator.
#[tokio::test]
async fn an_organization_somebody_came_into_past_the_mark_is_in_use_again() {
    let test = common::setup().await;
    let system = test.system();
    let Alone { leaver, org, .. } = alone_in_an_organization(&test).await;
    let in_use = format!(
        "SELECT CAST(count(*) AS TEXT) FROM organizations \
         WHERE deleted_at IS NULL AND CAST(id AS TEXT) = '{org}'"
    );
    if common::db_type() == DbType::Postgres {
        assert!(matches!(
            test.request_account_deletion(&leaver, "", T0 - WAIT, T0)
                .await
                .expect("asked"),
            DeletionAsking::Asked(..)
        ));
    }
    begin_alone(&test, &leaver, &org, T0).await;
    assert_eq!(count(&system, &in_use).await, 0, "out of use, to go");
    let newcomer = common::user(&system, "newcomer").await;
    system
        .execute_unchecked(&format!(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ('{org}', '{newcomer}', 'member', {T0}) ON CONFLICT DO NOTHING"
        ))
        .await
        .expect("somebody comes in");

    let erased = erase_from_organizations(&test, &leaver, Erasing::AfterTheWait).await;

    let kept: Vec<&LeftFrom> = erased
        .iter()
        .filter(|from| from.kept_for_newcomer)
        .collect();
    assert_eq!(kept.len(), 1, "the organization is kept: {erased:?}");
    assert_eq!(kept[0].removed, None, "nothing of it is removed");
    assert_eq!(count(&system, &in_use).await, 1, "in use again");
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 1)
            .await
            .expect("finished"),
        ErasureEnd::Erased,
        "the account is erased all the same"
    );
    assert_eq!(
        system
            .texts_unchecked(&format!(
                "SELECT CAST(user_id AS TEXT) FROM organization_members \
                 WHERE CAST(org_id AS TEXT) = '{org}'"
            ))
            .await
            .expect("read"),
        [newcomer],
        "the newcomer is in it, alone"
    );
    test.teardown().await;
}

/// The instance's own organization goes with no account, whoever is in it: the
/// organization after it would be the instance's then, and its administrators the
/// instance's. Its only administrator is not erased, alone in it or not.
#[tokio::test]
async fn the_instances_own_organization_does_not_go_with_an_account() {
    let test = common::setup().await;
    let system = test.system();
    let (alone, others) = if common::db_type() == DbType::Postgres {
        // The founder is alone in the test's organization, the oldest; somebody of
        // another administers the instance too.
        let founder = founder(&test).await;
        let (elsewhere, _) = common::organization(&system, "Elsewhere").await;
        let elsewhere_store = test.for_org(&elsewhere).expect("an organization");
        let operator = common::user(&elsewhere_store, "operator").await;
        system
            .execute_unchecked(&format!(
                "UPDATE users SET is_admin = true WHERE id = '{operator}'"
            ))
            .await
            .expect("set");
        (founder, vec![elsewhere])
    } else {
        (alone_in_an_organization(&test).await.leaver, Vec::new())
    };

    let standing = test.account_deletion_standing(&alone).await.expect("read");
    assert!(!standing.last_instance_admin, "{standing:?}");
    assert_eq!(standing.alone_in, [], "it does not go: {standing:?}");
    assert_eq!(standing.sole_admin_of.len(), 1, "{standing:?}");
    assert_eq!(standing.sole_admin_of[0].id, test.org_id, "{standing:?}");
    assert!(matches!(
        test.request_account_deletion(&alone, "", T0, T0 + WAIT)
            .await
            .expect("asked"),
        DeletionAsking::Refused(_)
    ));
    assert!(matches!(
        test.begin_account_erasure(&alone, T0, Erasing::AtOnce)
            .await
            .expect("asked"),
        ErasureStart::Refused(_)
    ));
    // The organization the instance's administrators are of is the one it was.
    let in_use = system
        .texts_unchecked(
            "SELECT CAST(id AS TEXT) FROM organizations \
             WHERE deleted_at IS NULL AND is_system = false AND owner_id IS NULL \
             ORDER BY created_at, id",
        )
        .await
        .expect("read");
    assert_eq!(in_use[0], test.org_id, "the oldest in use");
    assert_eq!(in_use.len(), 1 + others.len(), "none went");
    test.teardown().await;
}

/// An organization that is out of use to go with one account is not taken along, nor
/// put back in use, by the erasing of another that has something left in it: it is
/// the first's to go with.
#[tokio::test]
async fn an_organization_goes_with_the_account_it_was_put_out_of_use_for() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let system = test.system();
    let Alone {
        leaver,
        org,
        workspace,
        ..
    } = alone_in_an_organization(&test).await;
    let org_store = test.for_org(&org).expect("an organization");
    let (board, _) = board_of(&org_store, &workspace, &leaver).await;
    // Somebody of another organization kept a role on a board of this one.
    let other = common::user(&test, "other").await;
    system
        .execute_unchecked(&format!(
            "INSERT INTO container_members (container_id, user_id, org_id, role) \
             VALUES ('{board}', '{other}', '{org}', 'editor')"
        ))
        .await
        .expect("a role is left");
    // Somebody who once came in through its provider, and left, kept the session.
    let former = common::user(&test, "former").await;
    let through_it = Session {
        id: new_id(),
        user_id: former.clone(),
        created_at: T0,
        last_used_at: T0,
        expires_at: T0 + 2 * WAIT,
        user_agent: "a browser".to_owned(),
        login: Some(LoginOrigin {
            provider_id: new_id(),
            org_id: org.clone(),
            issuer: "https://idp.example".to_owned(),
            subject: Some("former".to_owned()),
            revision: Some(1),
        }),
        ..Session::default()
    };
    assert!(
        test.create_session(&through_it, "made through its provider", "")
            .await
            .expect("asked"),
        "stored"
    );
    assert!(matches!(
        test.request_account_deletion(&leaver, "", T0 - WAIT, T0)
            .await
            .expect("asked"),
        DeletionAsking::Asked(..)
    ));
    begin_alone(&test, &leaver, &org, T0).await;
    let out_of_use = format!(
        "SELECT CAST(count(*) AS TEXT) FROM organizations \
         WHERE deleted_at IS NOT NULL AND CAST(id AS TEXT) = '{org}'"
    );

    let of_other = erase(&test, &other, Erasing::AtOnce, T0 + 1).await;

    // The other's own space goes with the other, and holds nothing.
    let gone: Vec<_> = of_other
        .iter()
        .filter_map(|from| from.removed.as_ref())
        .collect();
    assert!(
        matches!(gone[..], [own] if own.purged.files.is_empty())
            && of_other.iter().all(|from| !from.kept_for_newcomer),
        "the other's erasing neither takes the organization nor keeps it: {of_other:?}"
    );
    assert_eq!(count(&system, &out_of_use).await, 1, "still to go");
    let of_leaver = erase(&test, &leaver, Erasing::AfterTheWait, T0 + 2).await;
    assert_eq!(
        of_leaver
            .iter()
            .filter(|from| from.removed.is_some())
            .count(),
        2,
        "it goes with the account it was put out of use for, as the account's own \
         space does: {of_leaver:?}"
    );
    // The session made through its provider goes with it, and is named to be closed.
    let ended: Vec<&str> = of_leaver
        .iter()
        .filter_map(|from| from.removed.as_ref())
        .flat_map(|removed| removed.ended.session_ids())
        .map(String::as_str)
        .collect();
    assert_eq!(ended, [through_it.id.as_str()], "{of_leaver:?}");
    assert!(
        test.get_session("made through its provider", T0 + 2, 0)
            .await
            .is_err(),
        "the session is gone"
    );
    assert_eq!(
        holding(&system, &org).await,
        NOBODY,
        "nothing names the organization: it held no file"
    );
    test.teardown().await;
}

/// An erasure cut off before the account's own space is removed cannot finish yet
/// (`StillPlaced`), and resuming removes the own space and its contents; an erasure
/// cut off after the own space is removed finishes idempotently.
#[tokio::test]
async fn an_erasure_cut_off_around_the_own_space_resumes_and_finishes() {
    let test = common::setup().await;
    let system = test.system();
    founder(&test).await;
    let leaver = common::user(&test, "cut-off-space").await;
    let space = fill_the_space_of(&test, &leaver).await;

    assert!(matches!(
        test.begin_account_erasure(&leaver, T0, Erasing::AtOnce)
            .await
            .expect("begun"),
        ErasureStart::Begun(_)
    ));
    // Cut off after erasing the shared organization, before reaching the own space.
    let orgs = test
        .organizations_of_account(&leaver)
        .await
        .expect("listed");
    assert!(orgs.contains(&test.org_id), "shared org listed: {orgs:?}");
    assert!(orgs.contains(&space), "own space listed: {orgs:?}");
    test.erase_account_from_organization(&leaver, Erasing::AtOnce)
        .await
        .expect("left shared org");
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 1)
            .await
            .expect("attempted finish"),
        ErasureEnd::StillPlaced,
        "cannot finish while the own space still exists"
    );
    assert_eq!(
        test.organizations_of_account(&leaver)
            .await
            .expect("listed"),
        [space.as_str()],
        "only the own space remains to erase"
    );
    assert_eq!(
        test.accounts_to_erase(T0 + 1).await.expect("due"),
        [leaver.as_str()],
        "stays listed as unfinished erasure"
    );

    // Resuming removes the own space and dooms its files; cutting off again before
    // `finish_account_erasure` leaves no organizations and finishes cleanly on retry.
    let space_store = test.for_org(&space).expect("an identifier");
    let removed = space_store
        .erase_account_from_organization(&leaver, Erasing::AtOnce)
        .await
        .expect("erased own space")
        .removed
        .expect("own space removed");
    assert_eq!(
        removed.purged.files.len(),
        2,
        "board and diary files doomed: {:?}",
        removed.purged.files
    );
    check_nothing_names(&system, &space).await;
    assert_eq!(
        test.organizations_of_account(&leaver)
            .await
            .expect("listed"),
        NOBODY,
        "no organizations left after the own space went"
    );
    assert_eq!(
        test.accounts_to_erase(T0 + 2).await.expect("due"),
        [leaver.as_str()],
        "still due until finish runs"
    );
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 2)
            .await
            .expect("finished"),
        ErasureEnd::Erased
    );
    assert_eq!(
        test.finish_account_erasure(&leaver, T0 + 3)
            .await
            .expect("idempotent finish"),
        ErasureEnd::ErasedBefore
    );
    assert_eq!(test.accounts_to_erase(T0 + 3).await.expect("due"), NOBODY);
    test.teardown().await;
}
