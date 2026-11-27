//! What a stop leaves of the mail: the queues are in memory, so the messages that still
//! hold a place once the shutdown's wait is over are counted, and never sent.

use std::time::Duration;

use fvoci_model::Invitation;
use fvoci_server::app::UnsentMail;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup};
use crate::fake_smtp::FakeSmtp;

#[tokio::test]
async fn mail_still_queued_when_the_wait_ends_is_counted_as_unsent() {
    let smtp = FakeSmtp::start_on(0).await;
    // Every message is held at the relay until it is released.
    smtp.hold("@sample.com");
    let th = Setup::native()
        .smtp(smtp.settings())
        .configure(|config| config.smtp.mail_reset_per_account_hour = 5)
        .start()
        .await
        .init_basic()
        .await;
    assert!(th.app.unsent_mail().is_empty(), "nothing is queued yet");
    for _ in 0..3 {
        th.app.spawn_background(
            th.app
                .prepare_password_reset("user1@sample.com", "en", None)
                .expect("a reset is asked for"),
        );
    }
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":link.token,"recipients":["new@sample.com"]}),
        )
        .await
        .check_no_content();
    // A reset takes its place once its account was looked up.
    let queued = UnsentMail {
        recovery: 3,
        invitations: 1,
    };
    tokio::time::timeout(Duration::from_secs(10), async {
        while th.app.unsent_mail() != queued {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("all four are queued: {:?}", th.app.unsent_mail()));
    smtp.attempted(1).await;

    assert!(
        !th.app.finish_background(Duration::from_millis(200)).await,
        "a held message keeps the wait from ending"
    );
    assert_eq!(
        th.app.unsent_mail(),
        queued,
        "the one being sent and those waiting for their turn"
    );

    smtp.release();
    assert!(th.app.finish_background(Duration::from_secs(15)).await);
    assert!(th.app.unsent_mail().is_empty(), "every place is given back");
    assert_eq!(smtp.messages().await.len(), 4);
    th.teardown().await;
}
