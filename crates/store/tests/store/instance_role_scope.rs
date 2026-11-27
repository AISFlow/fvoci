//! Which organization an administrator's change of an instance role reaches: only the
//! instance's own, whichever organization's handle the request came through.

use fvoci_model::{InstanceRole, OrgRole};
use fvoci_store::{AdminChange, AuditPlace, DbType, Paging};

use crate::common;

const PAGING: Paging = Paging {
    page: 0,
    per_page: std::num::NonZeroU32::new(100).expect("a page size"),
};

/// Through another organization's handle, an administrator of the instance makes a
/// member of that organization an administrator of the instance. The organization's
/// role does not follow there, and nothing is recorded there: the change is the
/// instance's, whose own organization is the one that follows. Before this bundle the
/// handle's organization followed (`scope.has_org()`), and its log named the change.
/// One organization only on SQLite: what is held here is PostgreSQL's.
#[tokio::test]
async fn an_instance_role_change_through_another_organization_leaves_it_alone() {
    let test = common::setup().await;
    if common::db_type() != DbType::Postgres {
        test.teardown().await;
        return;
    }
    let admin = common::user(&test, "instance-admin").await;
    test.make_admin_by_operator(&admin)
        .await
        .expect("an administrator of the instance");
    let user = common::user(&test, "member-elsewhere").await;
    let (org, _) = common::organization(&test.system(), "Another organization").await;
    let other = test.for_org(&org).expect("organization");
    other
        .execute_unchecked(&format!(
            "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ('{org}', '{user}', 'member', 1)"
        ))
        .await
        .expect("a member of the other organization");

    let change = other
        .set_user_admin(&user, InstanceRole::Administrator, &admin)
        .await
        .expect("asked");
    assert!(matches!(change, AdminChange::Done(_)), "{change:?}");

    assert_eq!(
        test.org_role_of_user(&user, &org).await.expect("read"),
        Some(OrgRole::Member),
        "the other organization's role stays"
    );
    assert_eq!(
        other
            .audit_records(AuditPlace::Organization, PAGING)
            .await
            .expect("records"),
        [],
        "nothing in the other organization's log"
    );
    test.teardown().await;
}
