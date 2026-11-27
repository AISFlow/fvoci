//! The provider an organization moved away from, in the store: kept for its time when
//! the issuer changes, read as none past it, ended early by an administrator, removed
//! by the housekeeping when its time ran out, and gone with another move or with the
//! provider; each recorded in the organization's audit log.

use fvoci_model::{AuditAction, AuditRecordActor, OrgRole, Session, User, new_id};
use std::time::Duration;

use fvoci_store::{
    AuditPlace, DbType, FormerEnd, FormerProvider, Identity, Paging, PendingSettings, Removal,
    Store, StoredProvider, Tenant, Trial, Tried,
};

use crate::common;

const ONE: &str = "https://one.example.test";
const TWO: &str = "https://two.example.test";
const THREE: &str = "https://three.example.test";

/// Thirty days.
const KEPT: Duration = Duration::from_hours(30 * 24);

/// [`KEPT`] in milliseconds.
const KEPT_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;

/// Now, in milliseconds since the Unix epoch.
fn now_millis() -> i64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is past the epoch");
    i64::try_from(since.as_millis()).expect("the time fits")
}

fn provider(issuer: &str, client_id: &str) -> StoredProvider {
    StoredProvider {
        id: String::new(),
        org_id: String::new(),
        issuer: issuer.to_owned(),
        client_id: client_id.to_owned(),
        sealed_client_secret: format!("the sealed secret of {client_id} at {issuer}"),
        auto_join: true,
        enabled: true,
        sso_required: false,
        revision: 0,
        created_at: 0,
        updated_at: 0,
    }
}

/// Sets the provider in place of `read`, as `actor`, keeping the one before for `kept`.
async fn put(
    store: &Store,
    next: &StoredProvider,
    read: Option<&StoredProvider>,
    actor: &str,
    kept: Duration,
) -> StoredProvider {
    store
        .put_oidc_provider_as_read(next, read, actor, kept)
        .await
        .expect("asked")
        .expect("stored")
        .provider
}

async fn former(store: &Store, now: i64) -> Option<FormerProvider> {
    store.oidc_former_provider(now).await.expect("read")
}

/// The organization's records of the former provider: what was done, by whom, to
/// which issuer.
async fn records(store: &Store) -> Vec<(AuditAction, AuditRecordActor, String)> {
    let paging = Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };
    store
        .audit_records(AuditPlace::Organization, paging)
        .await
        .expect("records")
        .into_iter()
        .filter(|record| record.action.as_str().starts_with("oidc_former_provider_"))
        .map(|record| {
            (
                record.action,
                record.actor,
                record.detail.unwrap_or_default(),
            )
        })
        .collect()
}

/// Whether the records have one of `action` for `issuer`, and who by.
fn recorded(
    records: &[(AuditAction, AuditRecordActor, String)],
    action: AuditAction,
    issuer: &str,
) -> Vec<AuditRecordActor> {
    records
        .iter()
        .filter(|(done, _, detail)| *done == action && detail == issuer)
        .map(|(_, actor, _)| actor.clone())
        .collect()
}

fn is_person(actor: &AuditRecordActor, user_id: &str) -> bool {
    matches!(actor, AuditRecordActor::Person(person) if person.id == user_id)
}

/// Another issuer keeps the provider before, with its client and sealed secret, for
/// the time given, and it is read as none from the moment that time ran out; another
/// client of the same issuer keeps none, and leaves the one kept.
#[tokio::test]
async fn another_issuer_keeps_the_provider_before_for_its_time() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let first = put(&test, &provider(ONE, "client"), None, &actor, KEPT).await;
    let another_client = put(&test, &provider(ONE, "next"), Some(&first), &actor, KEPT).await;
    let none_yet = former(&test, now_millis()).await;

    let moved = put(
        &test,
        &provider(TWO, "client"),
        Some(&another_client),
        &actor,
        KEPT,
    )
    .await;
    let kept = former(&test, now_millis()).await.expect("kept");
    put(&test, &provider(TWO, "next"), Some(&moved), &actor, KEPT).await;
    let still = former(&test, now_millis()).await;

    assert_eq!(none_yet, None, "the issuer did not change");
    assert_eq!(
        (
            kept.issuer.as_str(),
            kept.client_id.as_str(),
            kept.sealed_client_secret.as_str()
        ),
        (ONE, "next", another_client.sealed_client_secret.as_str()),
        "the provider as it was"
    );
    assert_eq!(kept.org_id, moved.org_id);
    assert_eq!(kept.expires_at - kept.created_at, KEPT_MILLIS);
    assert!(former(&test, kept.expires_at - 1).await.is_some());
    assert_eq!(former(&test, kept.expires_at).await, None, "past its time");
    assert_eq!(still, Some(kept), "another client of the issuer leaves it");
    let records = records(&test).await;
    let by = recorded(&records, AuditAction::OidcFormerProviderKept, ONE);
    assert!(by.len() == 1 && is_person(&by[0], &actor), "{records:?}");
    assert_eq!(records.len(), 1, "{records:?}");
    test.teardown().await;
}

/// A time of zero keeps none.
#[tokio::test]
async fn no_time_keeps_none() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let first = put(
        &test,
        &provider(ONE, "client"),
        None,
        &actor,
        Duration::ZERO,
    )
    .await;

    put(
        &test,
        &provider(TWO, "client"),
        Some(&first),
        &actor,
        Duration::ZERO,
    )
    .await;

    assert_eq!(former(&test, now_millis()).await, None);
    assert_eq!(records(&test).await, []);
    test.teardown().await;
}

/// An administrator ends the keeping before its time, once, of the one they read and
/// no other: it is recorded as theirs.
#[tokio::test]
async fn it_is_ended_before_its_time_once_as_it_was_read() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let first = put(&test, &provider(ONE, "client"), None, &actor, KEPT).await;
    put(&test, &provider(TWO, "client"), Some(&first), &actor, KEPT).await;
    let kept = former(&test, now_millis()).await.expect("kept");
    let end = async |kept_since: i64| {
        test.end_oidc_former_provider(&actor, kept_since, now_millis())
            .await
            .expect("asked")
    };

    let another = end(kept.created_at - 1).await;
    let still = former(&test, now_millis()).await;
    let ended = end(kept.created_at).await;
    let again = end(kept.created_at).await;

    assert_eq!(another, FormerEnd::Another, "not the one that was read");
    assert_eq!(still.as_ref(), Some(&kept));
    assert_eq!(
        (ended, again),
        (FormerEnd::Ended, FormerEnd::NoneKept),
        "ended once"
    );
    assert_eq!(former(&test, now_millis()).await, None);
    let records = records(&test).await;
    let by = recorded(&records, AuditAction::OidcFormerProviderEnded, ONE);
    assert!(by.len() == 1 && is_person(&by[0], &actor), "{records:?}");
    test.teardown().await;
}

/// One whose time ran out is not an administrator's to end: the housekeeping removes
/// it, once, with the system's reach and no other, and records it as the server's own
/// doing; one still kept stays.
#[tokio::test]
async fn one_whose_time_ran_out_is_removed_by_the_housekeeping() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let first = put(&test, &provider(ONE, "client"), None, &actor, KEPT).await;
    put(&test, &provider(TWO, "client"), Some(&first), &actor, KEPT).await;
    let kept = former(&test, now_millis()).await.expect("kept");
    let system = test.system();

    let too_early = system
        .end_lapsed_oidc_former_providers(kept.expires_at - 1)
        .await
        .expect("asked");
    let by_an_administrator = test
        .end_oidc_former_provider(&actor, kept.created_at, kept.expires_at)
        .await
        .expect("asked");
    // SQLite holds one organization: there is no other to ask as.
    let by_another_organization = if common::db_type() == DbType::Postgres {
        let (other_org, _) = common::organization(&test.scoped(Tenant::Unset), "the other").await;
        test.for_org(&other_org)
            .expect("an identifier")
            .end_lapsed_oidc_former_providers(kept.expires_at)
            .await
            .expect("asked")
    } else {
        0
    };
    let by_no_organization = test
        .scoped(Tenant::Unset)
        .end_lapsed_oidc_former_providers(kept.expires_at)
        .await
        .expect("asked");
    let removed = system
        .end_lapsed_oidc_former_providers(kept.expires_at)
        .await
        .expect("asked");
    let again = system
        .end_lapsed_oidc_former_providers(kept.expires_at)
        .await
        .expect("asked");

    assert_eq!((too_early, removed, again), (0, 1, 0));
    assert_eq!(
        (by_another_organization, by_no_organization),
        (0, 0),
        "a handle of another organization, or of none, ends none of ours"
    );
    assert_eq!(
        by_an_administrator,
        FormerEnd::NoneKept,
        "its time ran out: not theirs to end"
    );
    let rows = test.count_rows("oidc_former_providers").await;
    assert_eq!(rows.expect("counted"), 0);
    let records = records(&test).await;
    assert_eq!(
        recorded(&records, AuditAction::OidcFormerProviderLapsed, ONE),
        [AuditRecordActor::Server],
        "{records:?}"
    );
    assert_eq!(
        recorded(&records, AuditAction::OidcFormerProviderEnded, ONE),
        []
    );
    test.teardown().await;
}

/// Another move puts the provider before in the place of the one kept, which is
/// recorded as ended by who moved; and the removal of the provider ends what is kept.
#[tokio::test]
async fn another_move_and_the_removal_of_the_provider_end_it() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let mover = new_id();
    let first = put(&test, &provider(ONE, "client"), None, &actor, KEPT).await;
    let second = put(&test, &provider(TWO, "client"), Some(&first), &actor, KEPT).await;

    put(
        &test,
        &provider(THREE, "client"),
        Some(&second),
        &mover,
        KEPT,
    )
    .await;
    let after_the_move = former(&test, now_millis()).await.expect("kept");
    let removal = test.delete_oidc_provider(&actor).await.expect("asked");
    let after_the_removal = former(&test, now_millis()).await;

    assert_eq!(after_the_move.issuer, TWO);
    assert!(matches!(removal, Removal::Removed { .. }));
    assert_eq!(after_the_removal, None);
    let records = records(&test).await;
    let ended_one = recorded(&records, AuditAction::OidcFormerProviderEnded, ONE);
    let kept_two = recorded(&records, AuditAction::OidcFormerProviderKept, TWO);
    let ended_two = recorded(&records, AuditAction::OidcFormerProviderEnded, TWO);
    assert!(
        ended_one.len() == 1 && is_person(&ended_one[0], &mover),
        "{records:?}"
    );
    assert!(
        kept_two.len() == 1 && is_person(&kept_two[0], &mover),
        "{records:?}"
    );
    assert!(
        ended_two.len() == 1 && is_person(&ended_two[0], &actor),
        "{records:?}"
    );
    test.teardown().await;
}

/// A move that meets a former provider whose time ran out, not removed yet, records
/// that one as lapsed, the server's doing, and not as ended by who moves.
#[tokio::test]
async fn a_move_records_one_whose_time_ran_out_as_lapsed() {
    let test = common::setup().await;
    let actor = common::user(&test, "administrator").await;
    let first = put(&test, &provider(ONE, "client"), None, &actor, KEPT).await;
    let second = put(&test, &provider(TWO, "client"), Some(&first), &actor, KEPT).await;
    test.execute_unchecked("UPDATE oidc_former_providers SET expires_at = 1")
        .await
        .expect("its time ran out");

    put(
        &test,
        &provider(THREE, "client"),
        Some(&second),
        &actor,
        KEPT,
    )
    .await;

    let kept = former(&test, now_millis()).await.expect("kept");
    assert_eq!(kept.issuer, TWO);
    let records = records(&test).await;
    assert_eq!(
        recorded(&records, AuditAction::OidcFormerProviderLapsed, ONE),
        [AuditRecordActor::Server],
        "{records:?}"
    );
    assert_eq!(
        recorded(&records, AuditAction::OidcFormerProviderEnded, ONE),
        [],
        "{records:?}"
    );
    test.teardown().await;
}

/// A trial of pending settings of another issuer keeps the provider before as a save
/// of them does, recorded as the trier's; with no time it keeps none.
#[tokio::test]
async fn a_trial_that_moves_keeps_the_provider_before() {
    for kept_for in [KEPT, Duration::ZERO] {
        let test = common::setup().await;
        let mut trier = User {
            id: new_id(),
            username: "trier".to_owned(),
            email: "trier@example.com".to_owned(),
            password_hash: "a password hash".to_owned(),
            ..User::default()
        };
        test.create_user(&mut trier).await.expect("stored");
        test.execute_unchecked(&format!(
            "UPDATE organization_members SET role = '{}' WHERE user_id = '{}'",
            OrgRole::Admin.as_str(),
            trier.id
        ))
        .await
        .expect("an administrator of the organization");
        let identity = Identity {
            issuer: TWO,
            subject: "the trier at two",
        };
        test.add_identity(&trier.id, identity, "")
            .await
            .expect("tied");
        let enforced = StoredProvider {
            sso_required: true,
            ..provider(ONE, "client")
        };
        let enforced = put(&test, &enforced, None, &trier.id, KEPT).await;
        let aside = PendingSettings {
            issuer: TWO.to_owned(),
            client_id: "client".to_owned(),
            sealed_client_secret: "the sealed secret at two".to_owned(),
        };
        let (_, pending) = test
            .put_oidc_pending_provider(&aside, &enforced, &enforced, &trier.id)
            .await
            .expect("asked")
            .expect("saved aside");
        let mut session = Session {
            id: new_id(),
            created_at: 1,
            expires_at: i64::MAX,
            ..Session::default()
        };

        let applied = test
            .finish_oidc_trial(
                Trial {
                    provider_id: &enforced.id,
                    pending_at: pending.created_at,
                    trier_id: &trier.id,
                    keep_former: kept_for,
                },
                Tried::Known {
                    user_id: &trier.id,
                    password_hash: &trier.password_hash,
                    identity,
                },
                &mut session,
                "the digest of the trial's session",
            )
            .await
            .expect("asked")
            .expect("applied");

        assert_eq!(applied.provider.issuer, TWO);
        let kept = former(&test, now_millis()).await;
        let records = records(&test).await;
        let by = recorded(&records, AuditAction::OidcFormerProviderKept, ONE);
        if kept_for.is_zero() {
            assert_eq!(kept, None, "no time keeps none");
            assert_eq!(by, [], "{records:?}");
        } else {
            let kept = kept.expect("the provider before is kept");
            assert_eq!(
                (kept.issuer.as_str(), kept.sealed_client_secret.as_str()),
                (ONE, enforced.sealed_client_secret.as_str())
            );
            assert_eq!(kept.expires_at - kept.created_at, KEPT_MILLIS);
            assert!(by.len() == 1 && is_person(&by[0], &trier.id), "{records:?}");
        }
        test.teardown().await;
    }
}
