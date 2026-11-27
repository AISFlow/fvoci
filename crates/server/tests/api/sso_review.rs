//! SSO enforcement must not acknowledge success while an old room can still write.

use std::time::Duration;

use fvoci_model::OidcProviderSettings;
use fvoci_store::testing::db_type_of_the_tests;
use fvoci_store::{DbType, Store};

use crate::common::TEST_WORKSPACE_ID;
use crate::fake_oidc::{CLIENT_ID, FakeProvider};
use crate::oidc::{org_of, server_for, set_provider};
use crate::rooms::{Peer, paragraphs, table};
use crate::sso::admin_through_provider;

#[tokio::test]
async fn a_slow_membership_recheck_cannot_leave_a_password_room_writable_after_enforcement() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let fake = FakeProvider::start().await;
    let ts = server_for(&fake).await;
    let org_id = org_of(&ts).await;
    set_provider(&ts, &org_id, &fake).await;
    let admin = admin_through_provider(&ts, &fake, &org_id).await;
    let session = ts
        .app
        .authenticator()
        .find_session(&admin.token)
        .await
        .expect("a session");
    // Complete the HTTP extractor's authorization before the membership table is busy.
    let org = ts
        .app
        .enter_org_as_admin(&session, &org_id)
        .await
        .expect("the organization is checked")
        .expect("its administrator");
    let card = table(&ts, &paragraphs(&["before enforcement"])).await;
    let mut peer = Peer::connect(&ts.client, 191).await;
    // joinDoc alone: this connection has no subscribeWorkspace subscription.
    peer.joined(TEST_WORKSPACE_ID, &card.card_id).await;
    let before = ts
        .app
        .store()
        .get_body_doc(&card.card_id)
        .await
        .expect("the document is read")
        .expect("the document exists")
        .log_end;

    let held = hold_memberships(ts.app.store()).await;

    // The handler calls this after its extractor. Only the membership recheck is
    // blocked; provider writes, document writes and session reads remain available.
    let changed = org
        .put_oidc_provider(
            &org_id,
            &session,
            &OidcProviderSettings {
                issuer: fake.issuer.clone(),
                client_id: CLIENT_ID.to_owned(),
                sso_required: Some(true),
                ..OidcProviderSettings::default()
            },
        )
        .await
        .expect("enforcement is acknowledged");
    assert!(changed.sso_required, "the organization now requires SSO");
    peer.write("written with a password after enforcement returned")
        .await;
    let message = peer.incoming(Duration::from_secs(2)).await;
    let after = ts
        .app
        .store()
        .get_body_doc(&card.card_id)
        .await
        .expect("the document is read")
        .expect("the document exists")
        .log_end;

    held.await.expect("the test's lock task ends");
    drop(peer);
    ts.teardown().await;
    fake.stop();
    assert_eq!(
        after, before,
        "the old password session must write nothing: {message:?}"
    );
}

/// Holds only this test database's membership table, like slow maintenance.
async fn hold_memberships(store: &Store) -> tokio::task::JoinHandle<()> {
    let owned = store.system();
    let held = tokio::spawn(async move {
        owned
            .execute_unchecked(
                "DO $review$ BEGIN \
             LOCK TABLE workspace_members IN ACCESS EXCLUSIVE MODE; \
             PERFORM pg_sleep(15); END $review$",
            )
            .await
            .expect("the temporary membership lock is released");
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let locks = store
                .count_rows(
                    "pg_locks WHERE relation = 'workspace_members'::regclass \
                 AND mode = 'AccessExclusiveLock' AND granted",
                )
                .await
                .expect("the membership lock is observed");
            if locks > 0 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the recheck is delayed before enforcing SSO");

    held
}
