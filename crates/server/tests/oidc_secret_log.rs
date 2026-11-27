//! The client secret of a provider, and the server's key, are in no line of the log:
//! not when the provider is set, read, signed in through, or refused.
//!
//! A binary of its own: the log is caught by a subscriber of this thread, and the
//! interest of `tracing` in each place that logs is shared by the whole process, so
//! that tests on other threads, without a subscriber, could leave it out.

#[path = "api/fake_oidc.rs"]
mod fake_oidc;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use fvoci_model::{UserWorkspace, new_id};
use fvoci_server::Server;
use fvoci_server::auth::sealed::SecretKey;
use fvoci_server::config::Configuration;
use fvoci_store::testing::TestDatabase;
use serde_json::json;

use fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, Forgery};

/// What a subscriber of the log writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().expect("lock poisoned")).into_owned()
    }
}

impl std::io::Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("lock poisoned")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The value of the `name=` cookie a response sets.
fn cookie(response: &reqwest::Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|cookie| cookie.split(';').next()?.strip_prefix(&format!("{name}=")))
        .map(str::to_owned)
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("a Location header")
        .to_owned()
}

/// Signs in through the provider, and returns where the server sent the browser.
async fn sign_in(http: &reqwest::Client, base: &str, root: &str, org_id: &str) -> String {
    let started = http
        .post(format!("{base}/api/v1/auth/oidc/start?org={org_id}"))
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await
        .expect("an answer");
    let flow = cookie(&started, "fvoci_oidc").expect("the cookie of the sign-in");
    let started: fvoci_model::OidcStart = started.json().await.expect("where the browser goes");
    let authorized = http
        .get(&started.authorization_url)
        .send()
        .await
        .expect("the provider answers");
    let back = location(&authorized);
    let path = back.strip_prefix(root).expect("back to the server");
    let returned = http
        .get(format!("{base}{path}"))
        .header("Cookie", format!("fvoci_oidc={flow}"))
        .send()
        .await
        .expect("an answer");
    location(&returned)
}

/// A running server on `database` that reaches the fake provider and seals with the key
/// of `key_text`: its address as configured, where it listens, and how to stop it.
async fn start(
    database: &TestDatabase,
    directory: &std::path::Path,
    fake: &FakeProvider,
    key_text: &str,
) -> (
    String,
    String,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let config = Configuration {
        database_kind: database.db_type().as_str().to_owned(),
        database_connection: database.database_connection().to_owned(),
        file_storage: "local".to_owned(),
        file_storage_path: directory.join("uploads").display().to_string(),
        skip_template_init: true,
        login_address_attempts_per_minute: 0,
        secret_key: Some(SecretKey::from_base64(key_text).expect("a key")),
        oidc_allow_private_addresses: true,
        oidc_extra_root_certificates: vec![fake.certificate_der.clone()],
        ..Configuration::default()
    };
    let root = config.server_url.clone();
    let server = Server::new(config, "").await.expect("the server starts");
    tracing::debug!(config = ?server.app().config(), "the configuration");
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .expect("a local port is free");
    let base = format!("http://{}", listener.local_addr().expect("an address"));
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let serving = tokio::spawn(server.serve(listener, async move {
        let _ = stopped.await;
    }));

    (root, base, stop, serving)
}

#[tokio::test]
async fn the_client_secret_and_the_key_are_not_logged() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let fake = FakeProvider::start().await;
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("oidc-secret-log")
        .join(new_id());
    std::fs::create_dir_all(&directory).expect("the directory is made");
    let database = TestDatabase::create(&directory).await;
    let key_text = base64::engine::general_purpose::STANDARD.encode([7_u8; 32]);
    let (root, base, stop, serving) = start(&database, &directory, &fake, &key_text).await;

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
        .tls_certs_merge([
            reqwest::Certificate::from_der(&fake.certificate_der).expect("a certificate")
        ])
        .build()
        .expect("an HTTP client");
    let api = |path: &str| format!("{base}/api/v1{path}");
    http.post(api("/register"))
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&json!({
            "username": "admin", "email": "admin@example.com", "password": "Pa$$word1",
            "organizationName": "The Firm",
        }))
        .send()
        .await
        .expect("registered");
    let login = http
        .post(api("/login"))
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&json!({ "username": "admin", "password": "Pa$$word1" }))
        .send()
        .await
        .expect("signed in");
    let token = cookie(&login, "fvoci_session").expect("a session");
    let workspaces: Vec<UserWorkspace> = http
        .get(api("/users/me/workspaces"))
        .bearer_auth(&token)
        .send()
        .await
        .expect("an answer")
        .json()
        .await
        .expect("the workspaces");
    let org_id = workspaces.first().expect("a workspace").org_id.clone();
    let provider = api(&format!("/org/{org_id}/oidc"));
    let put = http
        .put(&provider)
        .bearer_auth(&token)
        .json(
            &json!({ "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET }),
        )
        .send()
        .await
        .expect("an answer");
    assert_eq!(put.status(), reqwest::StatusCode::OK);
    http.get(&provider)
        .bearer_auth(&token)
        .send()
        .await
        .expect("an answer");

    assert_eq!(
        sign_in(&http, &base, &root, &org_id).await,
        format!("{root}/")
    );
    fake.forge(Forgery::OtherKey);
    let refused = sign_in(&http, &base, &root, &org_id).await;
    assert!(
        refused.contains("oidc_error=oidc-token-invalid"),
        "{refused}"
    );

    let _ = stop.send(());
    serving
        .await
        .expect("the server's task ends")
        .expect("the server stops");
    database.remove().await;
    let _ = std::fs::remove_dir_all(&directory);
    fake.stop();

    let log = captured.text();
    assert!(
        log.contains("the provider of an organization was set")
            && log.contains("signature does not verify")
            && log.contains("SecretKey(..)"),
        "the log was caught: {log}"
    );
    for secret in [CLIENT_SECRET, &key_text] {
        assert!(!log.contains(secret), "{secret} is not logged: {log}");
    }
}
