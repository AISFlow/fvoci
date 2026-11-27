//! Independent regression for credentials in encoded invitation-page addresses.

#[path = "api/common.rs"]
mod common;

use std::io;
use std::sync::{Arc, Mutex};

use fvoci_model::Invitation;
use reqwest::Method;
use serde_json::json;

use common::{Client, ROOT_WORKSPACE_ID, Setup};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("the capture lock is not poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn encoded_invitation_page_paths_do_not_log_the_live_token() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);
    let th = Setup::native()
        .base_path("/app")
        .start()
        .await
        .init_basic()
        .await;
    let link: Invitation = th
        .client
        .post(
            &format!("/w/{ROOT_WORKSPACE_ID}/invitations"),
            &json!({"role": "member"}),
        )
        .await
        .created_json();
    let token = link.token.expect("the link has its token once");

    // parseRoute decodes each path segment before matching "invite". These are
    // invitation pages just like the canonical spelling, including under a base path.
    for segment in ["invite", "%69nvite", "inv%69te"] {
        let response = Client::send(
            th.anonymous()
                .raw_request(Method::GET, &format!("/{segment}/{token}")),
        )
        .await;
        response.check_ok();
        assert!(
            String::from_utf8_lossy(&response.body).contains("test web app"),
            "the invitation page serves the application"
        );
    }
    th.teardown().await;

    let log =
        String::from_utf8_lossy(&captured.0.lock().expect("the capture lock is not poisoned"))
            .into_owned();
    assert!(
        log.contains("/app/invite/<token>"),
        "requests were captured"
    );
    assert!(
        !log.contains(&token),
        "a live invitation token was written to the request log for an encoded page path"
    );
}
