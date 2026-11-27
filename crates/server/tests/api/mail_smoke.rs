//! One message through a real way of sending, for whoever holds its keys: ignored
//! unless asked for, and then sent only when the environment names the way.
//!
//! ```sh
//! FVOCI_MAIL_SMOKE_MAIL_TRANSPORT=ses FVOCI_MAIL_SMOKE_SES_REGION=… \
//! FVOCI_MAIL_SMOKE_SES_ACCESS_KEY_ID=… FVOCI_MAIL_SMOKE_SES_SECRET_ACCESS_KEY=… \
//! FVOCI_MAIL_SMOKE_MAIL_FROM=sender@your.domain FVOCI_MAIL_SMOKE_TO=you@your.domain \
//! cargo test -p fvoci-server --test api -- --ignored mail_smoke --nocapture
//! ```
//!
//! Each key of the configuration is read as `FVOCI_MAIL_SMOKE_<KEY>` instead of
//! `FVOCI_<KEY>` (`MAIL_TRANSPORT`, `MAIL_FROM`, the provider's keys, `SMTP_*`);
//! `FVOCI_MAIL_SMOKE_TO` is the one recipient. An invitation to the test's workspace is
//! mailed there, and the test passes when the way took it: nothing given up, the way
//! not closed.

use std::time::Duration;

use fvoci_model::Invitation;
use fvoci_server::config::Configuration;
use serde_json::json;

use crate::common::{ROOT_WORKSPACE_ID, Setup};
use crate::metrics::{sample, scrape};

/// The prefix of the environment variables of this test.
const PREFIX: &str = "FVOCI_MAIL_SMOKE_";

#[tokio::test]
#[ignore = "sends real mail: needs a way's keys in FVOCI_MAIL_SMOKE_*"]
async fn mail_smoke_one_invitation_leaves_through_the_way_of_the_environment() {
    let smoke = |name: &str| std::env::var(format!("{PREFIX}{name}")).ok();
    let (Some(transport), Some(to)) = (smoke("MAIL_TRANSPORT"), smoke("TO")) else {
        eprintln!("{PREFIX}MAIL_TRANSPORT and {PREFIX}TO are not set: nothing is sent");
        return;
    };
    let read = Configuration::parse("{}", |name| {
        name.strip_prefix("FVOCI_")
            .and_then(|key| std::env::var(format!("{PREFIX}{key}")).ok())
    })
    .expect("the keys of the environment are read");
    let mut smtp = read.smtp.clone();
    // One message: no pace to keep but the provider's.
    smtp.mail_per_second = 1;
    let th = Setup::native()
        .mail((smtp, read.mail.clone()))
        .metrics_at("127.0.0.1:0")
        .start()
        .await
        .init_basic()
        .await;
    let path = format!("/w/{ROOT_WORKSPACE_ID}/invitations");
    let link: Invitation = th
        .client
        .post(&path, &json!({"role":"member"}))
        .await
        .created_json();
    th.client
        .post(
            &format!("{path}/mail"),
            &json!({"token":link.token,"recipients":[to]}),
        )
        .await
        .check_no_content();
    let finished = th.app.finish_background(Duration::from_secs(60)).await;
    let body = scrape(&th).await;
    assert_eq!(
        sample(&body, "fvoci_mail_path_down", &[]),
        Some(0.0),
        "{transport}: the way refuses the keys or cannot be reached (the log says the code)"
    );
    assert!(
        finished,
        "{transport}: the message is neither sent nor given up within a minute"
    );
    for class in ["permanent", "temporary"] {
        assert_eq!(
            sample(
                &body,
                "fvoci_mail_failed_total",
                &[("class", class), ("kind", "invitation")]
            ),
            Some(0.0),
            "{transport}: nothing was given up ({class}); the log says the code"
        );
    }
    eprintln!("{transport}: the invitation left for the recipient");
    th.teardown().await;
}
