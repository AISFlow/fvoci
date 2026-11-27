//! The row's organization must agree with its workspace, even below server checks.

#[expect(
    dead_code,
    reason = "the existing store harness is shared with other suites"
)]
#[path = "store/common.rs"]
mod common;

use fvoci_model::{WorkspaceRole, new_id};
use fvoci_store::{DbType, Tenant};

/// An explicit WITH CHECK is optional for an ALL policy: test the resulting UPDATE
/// behavior, then exercise the error and cancellation paths of pooled transactions.
#[tokio::test]
async fn failed_and_cancelled_transactions_do_not_leave_an_organization_on_the_pool() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "member").await;
    let (other_org, _) = common::organization(&test, "other").await;
    let mut board = fvoci_model::Container {
        id: new_id(),
        workspace_id: test.workspace_id.clone(),
        visibility: fvoci_model::ContainerVisibility::Private,
        ..fvoci_model::Container::default()
    };
    test.insert_container(&mut board, &user_id)
        .await
        .expect("the organization's board exists");
    let moved = test
        .execute_unchecked(&format!(
            "UPDATE containers SET org_id = '{other_org}' WHERE id = '{}'",
            board.id
        ))
        .await
        .expect_err("WITH CHECK must reject a move to another organization");
    assert!(
        moved.to_string().contains("row-level security"),
        "the policy rejected the update: {moved}"
    );

    let system = test.system();
    let cancelled = tokio::time::timeout(
        std::time::Duration::from_millis(20),
        system.execute_unchecked("SELECT pg_sleep(0.2)"),
    )
    .await;
    assert!(
        cancelled.is_err(),
        "the operation was cancelled during its query"
    );
    for (store, count) in [
        (test.store.clone(), 1),
        (test.for_org(&other_org).expect("the other organization"), 0),
        (test.scoped(Tenant::Unset), 0),
        (system, 1),
    ] {
        assert_eq!(
            store
                .count_rows("containers")
                .await
                .expect("a new transaction reads"),
            count,
            "the next operation reaches only {:?}",
            store.tenant()
        );
    }
    let settings = test.settings_left_on_the_connections().await;
    assert!(
        !settings.is_empty()
            && settings
                .iter()
                .all(|(org, system)| { org.is_empty() && (system.is_empty() || system == "off") }),
        "neither failed nor cancelled transactions leave tenant settings: {settings:?}"
    );
    test.teardown().await;
}

#[tokio::test]
async fn an_org_cannot_forge_membership_in_another_orgs_workspace() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "member").await;
    let (other_org, other_workspace) = common::organization(&test, "other").await;
    assert!(
        test.resolve_workspace(&user_id, &other_workspace)
            .await
            .expect("initial lookup")
            .is_none(),
        "the workspace is initially inaccessible"
    );

    // This uses only organization A's handle, its default org_id and ordinary INSERT.
    // No System handle, session SET, or RLS bypass is involved in the forged row.
    let inserted = test
        .execute_unchecked(&format!(
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at) \
         VALUES ('{other_workspace}', '{user_id}', 'member', 0)"
        ))
        .await;
    let access = test
        .resolve_workspace(&user_id, &other_workspace)
        .await
        .expect("the directory resolves membership");
    let role = access.as_ref().map(|access| access.workspace_role);
    let resolved_org = access.as_ref().map(|access| access.org_id.clone());
    test.teardown().await;

    assert!(
        inserted.is_err() && access.is_none(),
        "a row owned by the first org granted entry to the second: insert={inserted:?}, org={resolved_org:?}, role={role:?}; second org={other_org}, member role={:?}",
        WorkspaceRole::Member
    );
}

#[tokio::test]
async fn an_org_cannot_attach_a_board_to_another_orgs_workspace() {
    if common::db_type() != DbType::Postgres {
        return;
    }
    let test = common::setup().await;
    let user_id = common::user(&test, "member").await;
    let (_, other_workspace) = common::organization(&test, "other").await;
    let mut board = fvoci_model::Container {
        id: new_id(),
        workspace_id: other_workspace,
        visibility: fvoci_model::ContainerVisibility::Private,
        ..fvoci_model::Container::default()
    };
    let inserted = test.insert_container(&mut board, &user_id).await;
    test.teardown().await;
    assert!(
        inserted.is_err(),
        "a board of the first org was attached to another org's workspace: {inserted:?}"
    );
}
