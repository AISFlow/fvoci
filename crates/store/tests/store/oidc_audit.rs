//! Durable provider changes, operator attribution, and immutable records on both databases.
use std::time::Duration;

use crate::common::{self};
use fvoci_model::{AuditAction, AuditRecordActor, new_id};
use fvoci_store::{AuditPlace, Paging, StoredProvider};

/// A provider that takes no password: its organization opens to its sessions only.
pub(crate) fn settings() -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: "https://issuer.example.test".to_owned(),
        client_id: "client".to_owned(),
        sealed_client_secret: String::new(),
        auto_join: true,
        enabled: true,
        sso_required: true,
        revision: 0,
        created_at: 0,
        updated_at: 0,
    }
}

#[tokio::test]
async fn operator_records_are_distinct_immutable_and_made_with_the_release() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    test.put_oidc_provider_as_read(&settings(), None, &actor, Duration::ZERO)
        .await
        .expect("stored");
    assert!(
        test.release_sso_by_operator()
            .await
            .expect("released")
            .is_some()
    );
    assert!(
        test.release_sso_by_operator()
            .await
            .expect("already released")
            .is_none()
    );
    let records = test
        .audit_records(
            AuditPlace::Organization,
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(100).expect("a page size"),
            },
        )
        .await
        .expect("records");
    assert_eq!(records.len(), 3);
    let operators: Vec<_> = records
        .iter()
        .filter(|record| record.actor == AuditRecordActor::ServerOperator)
        .collect();
    assert_eq!(operators.len(), 1);
    assert_eq!(operators[0].action, AuditAction::SsoReleased);
    for record in &records {
        if let AuditRecordActor::Person(person) = &record.actor {
            assert_eq!(person.id, actor);
        }
    }
    for statement in [
        "UPDATE audit_log SET actor_kind = 'person' WHERE actor_kind = 'server_operator'",
        "UPDATE audit_log SET created_at = 0",
        "DELETE FROM audit_log WHERE actor_kind = 'server_operator'",
    ] {
        assert!(
            test.execute_unchecked(statement).await.is_err(),
            "{statement}"
        );
    }
    for (actor_id, kind) in [
        ("NULL".to_owned(), "person"),
        (format!("'{}'", new_id()), "server_operator"),
    ] {
        let statement = format!(
            "INSERT INTO audit_log (id, org_id, actor_id, actor_kind, action, target_id, created_at) \
             SELECT '{}', org_id, {actor_id}, '{kind}', 'sso_released', target_id, 0 FROM audit_log LIMIT 1",
            new_id()
        );
        assert!(
            test.execute_unchecked(&statement).await.is_err(),
            "inconsistent actor rejected"
        );
    }
    assert_eq!(
        test.audit_records(
            AuditPlace::Organization,
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(100).expect("a page size")
            }
        )
        .await
        .expect("unchanged"),
        records
    );
    test.teardown().await;
}

#[tokio::test]
async fn a_missing_actor_rolls_back_the_provider_change() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let fvoci_store::ProviderPut {
        provider: before, ..
    } = test
        .put_oidc_provider_as_read(&settings(), None, &actor, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let mut changed = before.clone();
    changed.client_id = "replacement".to_owned();
    assert!(
        test.put_oidc_provider_as_read(&changed, Some(&before), "", Duration::ZERO)
            .await
            .is_err()
    );
    assert_eq!(test.oidc_provider().await.expect("read"), Some(before));
    test.teardown().await;
}

#[tokio::test]
async fn a_release_or_removal_without_its_record_rolls_back() {
    for required in [false, true] {
        let test = common::setup().await;
        let actor = common::user(&test, "administrator").await;
        let mut provider = settings();
        provider.sso_required = required;
        let fvoci_store::ProviderPut {
            provider: before, ..
        } = test
            .put_oidc_provider_as_read(&provider, None, &actor, Duration::ZERO)
            .await
            .expect("stored")
            .expect("new");
        test.execute_unchecked("DROP TABLE audit_log")
            .await
            .expect("recording fails");
        if required {
            assert!(test.release_sso_by_operator().await.is_err());
        } else {
            assert!(test.delete_oidc_provider(&actor).await.is_err());
        }
        assert_eq!(test.oidc_provider().await.expect("read"), Some(before));
        test.teardown().await;
    }
}

#[tokio::test]
async fn an_operator_role_change_is_recorded_only_in_the_instances_organization() {
    let test = common::setup().await;
    common::user(&test, "first-admin").await;
    let user = common::user(&test, "next-admin").await;
    let other = if common::db_type() == fvoci_store::DbType::Postgres {
        let (org, _) = common::organization(&test.system(), "Another organization").await;
        let other = test.for_org(&org).expect("organization");
        other
            .execute_unchecked(&format!(
                "INSERT INTO organization_members (org_id, user_id, role, created_at) \
             VALUES ('{org}', '{user}', 'member', 1)"
            ))
            .await
            .expect("also a member elsewhere");
        Some(other)
    } else {
        None
    };
    test.make_admin_by_operator(&user).await.expect("promoted");
    test.make_admin_by_operator(&user)
        .await
        .expect("already an administrator");
    let paging = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };
    let records = test
        .audit_records(AuditPlace::Organization, paging)
        .await
        .expect("records");
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].actor, AuditRecordActor::ServerOperator);
    assert_eq!(records[0].action, AuditAction::OrgAdminGranted);
    assert_eq!(records[0].subject.as_ref().expect("subject").id, user);
    if let Some(other) = other {
        assert_eq!(
            other
                .audit_records(AuditPlace::Organization, paging)
                .await
                .expect("records"),
            []
        );
    }
    test.teardown().await;
}

#[tokio::test]
async fn a_save_records_only_what_it_changed() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let mut provider = settings();
    provider.sso_required = false;
    let fvoci_store::ProviderPut {
        provider: first, ..
    } = test
        .put_oidc_provider_as_read(&provider, None, &actor, Duration::ZERO)
        .await
        .expect("stored")
        .expect("new");
    let fvoci_store::ProviderPut {
        provider: unchanged,
        ..
    } = test
        .put_oidc_provider_as_read(&provider, Some(&first), &actor, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    provider.sso_required = true;
    let fvoci_store::ProviderPut {
        provider: enforced, ..
    } = test
        .put_oidc_provider_as_read(&provider, Some(&unchanged), &actor, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    provider.auto_join = false;
    let _ = test
        .put_oidc_provider_as_read(&provider, Some(&enforced), &actor, Duration::ZERO)
        .await
        .expect("stored")
        .expect("as read");
    let mut actions: Vec<AuditAction> = test
        .audit_records(
            AuditPlace::Organization,
            Paging {
                page: 0,
                per_page: std::num::NonZeroU32::new(100).expect("a page size"),
            },
        )
        .await
        .expect("records")
        .into_iter()
        .map(|record| record.action)
        .collect();
    test.teardown().await;
    // The newest first, as the list reads.
    actions.reverse();
    assert_eq!(
        actions,
        [
            AuditAction::OidcProviderConfigured,
            AuditAction::SsoRequired,
            AuditAction::OidcProviderConfigured,
        ],
        "the first save, the switch alone, and the change of joining; nothing for the save that changed nothing"
    );
}

/// Who an identity is is told in the organization's own records only: a record of a
/// workspace that carries one, whatever wrote it, is read without it.
#[tokio::test]
async fn a_workspaces_records_tell_no_identity_whatever_is_stored() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let (in_workspace, of_organization) = (new_id(), new_id());
    for (id, workspace) in [
        (&in_workspace, format!("'{}'", test.workspace_id)),
        (&of_organization, "NULL".to_owned()),
    ] {
        test.execute_unchecked(&format!(
            "INSERT INTO audit_log (id, org_id, workspace_id, actor_id, action, target_id, \
             created_at, actor_kind, detail, identity) VALUES ('{id}', '{}', {workspace}, \
             '{actor}', 'oidc_identity_bound', '{}', 1, 'person', \
             'https://issuer.example.test', 'someone@example.test')",
            test.org_id, test.org_id
        ))
        .await
        .expect("written past the store");
    }
    let paging = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };

    let of_workspace = test
        .audit_records(AuditPlace::Workspace(&test.workspace_id), paging)
        .await
        .expect("read");
    let of_org = test
        .audit_records(AuditPlace::Organization, paging)
        .await
        .expect("read");

    let told = |records: &[fvoci_model::AuditRecord]| -> Vec<(String, Option<String>)> {
        records
            .iter()
            .map(|record| (record.id.clone(), record.identity.clone()))
            .collect()
    };
    assert_eq!(told(&of_workspace), [(in_workspace, None)]);
    assert_eq!(
        told(&of_org),
        [(of_organization, Some("someone@example.test".to_owned()))]
    );
    test.teardown().await;
}
