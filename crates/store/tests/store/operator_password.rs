//! What the server's operator does to a person's password is recorded in every
//! organization the person is a member of, one record each, as the operator's, and in
//! no other organization.

use fvoci_model::{AuditAction, AuditRecord, AuditRecordActor};
use fvoci_store::{AuditPlace, Paging, Store, StoreError};

use crate::common;

async fn records(store: &Store) -> Vec<AuditRecord> {
    let paging = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };
    store
        .audit_records(AuditPlace::Organization, paging)
        .await
        .expect("records")
}

/// The actions of `store`'s organization's records of the operator to `user_id`,
/// newest first, each checked to name the organization and no person who did it.
async fn operators_records(store: &Store, org_id: &str, user_id: &str) -> Vec<AuditAction> {
    records(store)
        .await
        .into_iter()
        .map(|record| {
            assert_eq!(
                record.actor,
                AuditRecordActor::ServerOperator,
                "the operator's"
            );
            assert_eq!(record.target_id, org_id, "the organization's own record");
            assert_eq!(
                record.subject.as_ref().map(|subject| subject.id.as_str()),
                Some(user_id),
                "done to the person"
            );
            record.action
        })
        .collect()
}

#[tokio::test]
async fn the_operators_password_acts_are_recorded_in_each_organization_of_the_person() {
    let test = common::setup().await;
    let user = common::user(&test, "pat").await;
    let nobodys = common::user(&test.system(), "nobody-of-an-organization").await;
    // A second organization of theirs, and a third that is not, where SQLite holds one.
    let others = if common::db_type() == fvoci_store::DbType::Postgres {
        let (second, _) = common::organization(&test.system(), "Second").await;
        let second = test.for_org(&second).expect("organization");
        second
            .add_organization_member(&user, fvoci_model::OrgRole::Member)
            .await
            .expect("a member of the second as well");
        let (third, _) = common::organization(&test.system(), "Third").await;
        let third = test.for_org(&third).expect("organization");
        common::user(&third, "a-member-of-the-third").await;
        // An organization of theirs that is deleted records nothing.
        let (deleted, _) = common::organization(&test.system(), "Deleted").await;
        let deleted = test.for_org(&deleted).expect("organization");
        deleted
            .add_organization_member(&user, fvoci_model::OrgRole::Member)
            .await
            .expect("a member of the deleted one");
        deleted
            .execute_unchecked("UPDATE organizations SET deleted_at = 1")
            .await
            .expect("deleted");
        let held = deleted.count_rows("audit_log").await.expect("counted");
        Some((second, third, deleted, held))
    } else {
        None
    };

    // A member of the organization without a password: no link, and no record.
    let passwordless = common::user(&test, "no-password").await;

    let _ = test
        .reset_user_password("PAT", "replacement hash")
        .await
        .expect("set");
    let linked = test
        .create_password_reset_by_operator(&user, "pat's link", 100)
        .await
        .expect("asked");
    let not_found = test.reset_user_password("nobody-at-all", "hash").await;
    let _ = test
        .reset_user_password("nobody-of-an-organization", "hash")
        .await
        .expect("set");
    let in_none = test
        .create_password_reset_by_operator(&nobodys, "nobody's link", 100)
        .await
        .expect("asked");
    let refused = test
        .create_password_reset_by_operator(&passwordless, "no link", 100)
        .await
        .expect("asked");

    let both = [
        AuditAction::PasswordResetLinkMade,
        AuditAction::PasswordSetByOperator,
    ];
    assert_eq!(operators_records(&test, &test.org_id, &user).await, both);
    assert_eq!(
        common::rows_of_own_spaces(&test, "audit_log").await,
        0,
        "no record in the account's own space, which has no administrators to read one"
    );
    assert!(matches!(not_found, Err(StoreError::UserNotFound(_))));
    assert!(linked, "a link for an account with a password");
    assert!(
        in_none,
        "a person of no organization has a link, recorded nowhere"
    );
    assert!(
        !refused,
        "no link for an account without one, and no record"
    );
    if let Some((second, third, deleted, held)) = others {
        assert_eq!(
            deleted.count_rows("audit_log").await.expect("counted"),
            held,
            "no record in the deleted organization"
        );
        let second_id = second.tenant().org_id().expect("an organization");
        assert_eq!(operators_records(&second, &second_id, &user).await, both);
        assert_eq!(
            records(&third).await,
            [],
            "nothing in an organization not theirs"
        );
    }
    test.teardown().await;
}
