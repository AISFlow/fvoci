//! An organization's taking no password is recorded when it is switched on, switched
//! off, and released by who runs the server: a line of the log under the target
//! `audit`, with the organization, the provider, who did it and what was done.
//!
//! A binary of its own, as `oidc_secret_log.rs` is and for its reason: the log is
//! caught by a subscriber of this thread.

#[path = "api/fake_oidc.rs"]
mod fake_oidc;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine as _;
use fvoci_model::{OidcProvider, OidcStart, User, UserWorkspace, new_id};
use fvoci_server::Server;
use fvoci_server::auth::sealed::SecretKey;
use fvoci_server::config::Configuration;
use fvoci_store::Identity;
use fvoci_store::testing::TestDatabase;
use serde_json::json;

use fake_oidc::{CLIENT_ID, CLIENT_SECRET, FakeProvider, Person};

/// What a subscriber of the log writes.
#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    /// The lines of the record, oldest first.
    fn record(&self) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().expect("lock poisoned"))
            .lines()
            .filter(|line| line.contains(" audit: "))
            .map(str::to_owned)
            .collect()
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

/// Signs in through the provider, and returns the token of the session.
async fn sign_in(http: &reqwest::Client, base: &str, root: &str, org_id: &str) -> String {
    let started = http
        .post(format!("{base}/api/v1/auth/oidc/start?org={org_id}"))
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await
        .expect("an answer");
    let flow = cookie(&started, "fvoci_oidc").expect("the cookie of the sign-in");
    let started: OidcStart = started.json().await.expect("where the browser goes");
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
    cookie(&returned, "fvoci_session").expect("signed in through the provider")
}

/// A running server that reaches the fake provider, and what stops it.
struct Running {
    app: fvoci_server::app::App,
    root: String,
    base: String,
    http: reqwest::Client,
    stop: tokio::sync::oneshot::Sender<()>,
    serving: tokio::task::JoinHandle<anyhow::Result<()>>,
    database: TestDatabase,
    directory: PathBuf,
}

impl Running {
    async fn start(fake: &FakeProvider) -> Self {
        let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("sso-record")
            .join(new_id());
        std::fs::create_dir_all(&directory).expect("the directory is made");
        let database = TestDatabase::create(&directory).await;
        let key = base64::engine::general_purpose::STANDARD.encode([9_u8; 32]);
        let config = Configuration {
            serve_admin_socket: cfg!(unix),
            admin_socket_path: std::env::temp_dir()
                .join(format!("fvoci-operator-{}.sock", new_id()))
                .display()
                .to_string(),
            database_kind: database.db_type().as_str().to_owned(),
            database_connection: database.database_connection().to_owned(),
            file_storage: "local".to_owned(),
            file_storage_path: directory.join("uploads").display().to_string(),
            skip_template_init: true,
            login_address_attempts_per_minute: 0,
            password_hash_concurrency: 1,
            password_hash_queue: 0,
            secret_key: Some(SecretKey::from_base64(&key).expect("a key")),
            oidc_allow_private_addresses: true,
            oidc_extra_root_certificates: vec![fake.certificate_der.clone()],
            ..Configuration::default()
        };
        let root = config.server_url.clone();
        let server = Server::new(config, "").await.expect("the server starts");
        let app = server.app().clone();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a local port is free");
        let base = format!("http://{}", listener.local_addr().expect("an address"));
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let serving = tokio::spawn(server.serve(listener, async move {
            let _ = stopped.await;
        }));
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .tls_certs_merge([
                reqwest::Certificate::from_der(&fake.certificate_der).expect("a certificate")
            ])
            .build()
            .expect("an HTTP client");
        Self {
            app,
            root,
            base,
            http,
            stop,
            serving,
            database,
            directory,
        }
    }

    fn api(&self, path: &str) -> String {
        format!("{}/api/v1{path}", self.base)
    }

    /// Reads what is at `path` as the session of `token`.
    async fn read<T: serde::de::DeserializeOwned>(&self, path: &str, token: &str) -> T {
        self.http
            .get(self.api(path))
            .bearer_auth(token)
            .send()
            .await
            .expect("an answer")
            .json()
            .await
            .expect("what was asked for")
    }

    /// Registers the first user, who administers the organization, and signs in with
    /// the password: the token of that session.
    async fn admin(&self) -> String {
        let login = json!({ "username": "admin", "password": "Pa$$word1" });
        let register = json!({
            "username": "admin", "email": "admin@example.com", "password": "Pa$$word1",
            "organizationName": "The organization",
        });
        let mut token = None;
        for (path, body) in [("/register", &register), ("/login", &login)] {
            let answer = self
                .http
                .post(self.api(path))
                .header("X-Requested-With", "XMLHttpRequest")
                .json(body)
                .send()
                .await
                .expect("an answer");
            assert!(answer.status().is_success(), "{path}: {}", answer.status());
            token = cookie(&answer, "fvoci_session");
        }
        token.expect("signed in with the password")
    }

    /// Sets the organization's provider, the fake one, with `ssoRequired` if given.
    async fn set_provider(
        &self,
        org_id: &str,
        token: &str,
        fake: &FakeProvider,
        sso_required: Option<bool>,
    ) -> OidcProvider {
        let mut settings = json!({
            "issuer": fake.issuer, "clientId": CLIENT_ID, "clientSecret": CLIENT_SECRET,
        });
        if let Some(on) = sso_required {
            settings["ssoRequired"] = json!(on);
        }
        let answer = self
            .http
            .put(self.api(&format!("/org/{org_id}/oidc")))
            .bearer_auth(token)
            .json(&settings)
            .send()
            .await
            .expect("an answer");
        assert_eq!(
            answer.status(),
            reqwest::StatusCode::OK,
            "the provider is set"
        );
        answer.json().await.expect("the provider")
    }

    async fn stop(self) {
        let _ = self.stop.send(());
        self.serving
            .await
            .expect("the server's task ends")
            .expect("the server stops");
        self.database.remove().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

#[tokio::test]
async fn switching_on_switching_off_and_the_operators_release_are_recorded() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let fake = FakeProvider::start().await;
    let server = Running::start(&fake).await;
    let token = server.admin().await;
    let me: User = server.read("/users/me", &token).await;
    let workspaces: Vec<UserWorkspace> = server.read("/users/me/workspaces", &token).await;
    let org_id = workspaces.first().expect("a workspace").org_id.clone();
    let issuer = server
        .set_provider(&org_id, &token, &fake, None)
        .await
        .issuer;
    assert!(captured.record().is_empty(), "nothing to record yet");

    // The administrator, as the provider knows them, signed in through it.
    server
        .app
        .store()
        .add_identity(
            &me.id,
            Identity {
                issuer: &fake.issuer,
                subject: "subject-of-the-admin",
            },
            "admin@example.com",
        )
        .await
        .expect("tied");
    fake.sign_in_as(Person::new("subject-of-the-admin", "admin@example.com"));
    let through = sign_in(&server.http, &server.base, &server.root, &org_id).await;
    for on in [true, false, false, true] {
        let stored = server
            .set_provider(&org_id, &through, &fake, Some(on))
            .await;
        assert_eq!(stored.sso_required, on);
    }
    // Twice: released already, there is nothing more to record.
    for _ in 0..2 {
        server
            .app
            .release_sso_as_operator(&org_id)
            .await
            .expect("released");
    }

    let record = captured.record();
    let actions: Vec<&str> = record
        .iter()
        .filter_map(|line| line.split("action=\"").nth(1)?.split('"').next())
        .collect();
    assert_eq!(
        actions,
        [
            "sso_required",
            "sso_released",
            "sso_required",
            "sso_released_by_operator"
        ],
        "{record:#?}"
    );
    for line in &record {
        assert!(
            line.contains(&format!("org_id={org_id}")) && line.contains(&issuer),
            "the organization and its provider: {line}"
        );
    }
    for by_the_admin in &record[..3] {
        assert!(
            by_the_admin.contains(&format!("user_id=\"{}\"", me.id)),
            "who did it: {by_the_admin}"
        );
    }
    assert!(
        record[3].contains("user_id=\"\""),
        "nobody's account: {}",
        record[3]
    );
    #[cfg(unix)]
    global_commands_stay_out_of_the_organizations_log(
        &server,
        &captured,
        &org_id,
        &me,
        &[&token, &through, CLIENT_SECRET],
    )
    .await;
    server.stop().await;
    fake.stop();
}

#[cfg(unix)]
async fn global_commands_stay_out_of_the_organizations_log(
    server: &Running,
    captured: &Captured,
    org_id: &str,
    me: &User,
    secrets: &[&str],
) {
    let store = server.app.store().for_org(org_id).expect("organization");
    let paging = fvoci_store::Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };
    let before = store
        .audit_records(fvoci_store::AuditPlace::Organization, paging)
        .await
        .expect("records");
    let http = reqwest::Client::builder()
        .unix_socket(server.app.config().admin_socket_path.as_str())
        .timeout(Duration::from_secs(10))
        .build()
        .expect("socket client");
    let other = if fvoci_store::testing::db_type_of_the_tests() == fvoci_store::DbType::Postgres {
        let workspace = fvoci_model::Workspace {
            id: new_id(),
            ..fvoci_model::Workspace::default()
        };
        let org = store
            .system()
            .create_organization("Other", &workspace)
            .await
            .expect("another organization");
        Some(store.for_org(&org).expect("organization"))
    } else {
        None
    };
    let password = "operator-private-replacement-password";
    for (route, body, status) in [
        (
            format!("users/{}/password", me.username),
            json!({"password":password}),
            204,
        ),
        (format!("users/{}/admin", me.username), json!({}), 204),
        (
            "files/reconcile".to_owned(),
            json!({"removeUnrecorded":false}),
            200,
        ),
        (
            "users/absent-operator-target/password".to_owned(),
            json!({"password":password}),
            404,
        ),
    ] {
        assert_eq!(operator_post(&http, &route, &body).await, status, "{route}");
    }
    only_the_password_set_is_recorded(&store, &before, org_id, me).await;
    for (route, body) in [
        (
            format!("users/{}/password", me.username),
            json!({"password":"x"}),
        ),
        (
            "files/reconcile".to_owned(),
            json!({"unknownSecret":"never-log-this-link"}),
        ),
    ] {
        assert_eq!(operator_post(&http, &route, &body).await, 400, "{route}");
    }
    let attempts = (0..24).map(|_| {
        http.post(format!(
            "http://localhost/api/v1/admin/users/{}/password",
            me.username
        ))
        .json(&json!({"password":password}))
        .send()
    });
    let answers = futures_util::future::join_all(attempts).await;
    let refused = answers
        .iter()
        .filter(|answer| answer.as_ref().expect("answer").status().as_u16() == 429)
        .count();
    assert!(refused > 0, "a queue of zero refuses concurrent hashing");
    if let Some(other) = other {
        assert_eq!(
            other
                .audit_records(fvoci_store::AuditPlace::Organization, paging)
                .await
                .expect("other records"),
            [],
            "nothing to an organization the person is no member of"
        );
    }
    operator_lines_name_no_secret(captured, me, secrets, password, refused);
    the_release_has_its_line(&http, captured, org_id).await;
}

/// The password the operator set is the one record the global commands add: the
/// person's organization's, the operator's, naming nothing of the password.
#[cfg(unix)]
async fn only_the_password_set_is_recorded(
    store: &fvoci_store::Store,
    before: &[fvoci_model::AuditRecord],
    org_id: &str,
    me: &User,
) {
    let paging = fvoci_store::Paging {
        page: 0,
        per_page: std::num::NonZeroU32::new(100).expect("a page size"),
    };
    let after = store
        .audit_records(fvoci_store::AuditPlace::Organization, paging)
        .await
        .expect("records");
    let (set, kept) = after.split_first().expect("a record of the password set");
    assert_eq!(kept, before, "the other global commands add nothing");
    let subject = set.subject.as_ref().map(|subject| subject.id.as_str());
    assert_eq!(
        (set.action, &set.actor),
        (
            fvoci_model::AuditAction::PasswordSetByOperator,
            &fvoci_model::AuditRecordActor::ServerOperator
        ),
        "the operator's password set"
    );
    assert_eq!(
        (set.target_id.as_str(), subject),
        (org_id, Some(me.id.as_str())),
        "the organization's record of the person"
    );
}

/// The socket's release of an organization is a line of the log as the global commands
/// are, naming the organization; its record is the organization's.
#[cfg(unix)]
async fn the_release_has_its_line(http: &reqwest::Client, captured: &Captured, org_id: &str) {
    let release = format!("orgs/{org_id}/sso/release");
    assert_eq!(
        operator_post(http, &release, &json!(null)).await,
        204,
        "released, or released already"
    );
    let record = captured.record();
    let released = record
        .iter()
        .rfind(|line| line.contains("command=\"release_sso\""))
        .expect("the release has its line");
    assert!(
        released.contains(org_id) && released.contains("result=\"succeeded\""),
        "{released}"
    );
}

/// Sends one command to the administration socket and answers its status.
#[cfg(unix)]
async fn operator_post(http: &reqwest::Client, route: &str, body: &serde_json::Value) -> u16 {
    http.post(format!("http://localhost/api/v1/admin/{route}"))
        .json(body)
        .send()
        .await
        .expect("socket answers")
        .status()
        .as_u16()
}

/// The structured lines of the global commands: one per command, with its outcome and
/// target, and none of what was sent with it.
#[cfg(unix)]
fn operator_lines_name_no_secret(
    captured: &Captured,
    me: &User,
    secrets: &[&str],
    password: &str,
    refused: usize,
) {
    let record = captured.record();
    let operator: Vec<_> = record
        .iter()
        .filter(|line| line.contains("actor=\"operator\""))
        .collect();
    assert_eq!(operator.len(), 30, "{record:#?}");
    for (line, command) in operator[..4].iter().zip([
        "reset_password",
        "make_admin",
        "reconcile_files",
        "reset_password",
    ]) {
        assert!(line.contains(&format!("command=\"{command}\"")), "{line}");
        assert!(!line.contains(password), "no password in the record");
        for secret in secrets {
            assert!(
                !line.contains(secret),
                "no token or client secret in the record"
            );
        }
    }
    for line in &operator[..2] {
        assert!(
            line.contains(&me.id) && line.contains("result=\"succeeded\""),
            "{line}"
        );
    }
    assert!(operator[3].contains("result=\"failed\""), "{}", operator[3]);
    assert_eq!(
        operator
            .iter()
            .filter(|line| line.contains("status=429"))
            .count(),
        refused,
        "every refused hashing is a line"
    );
    for line in &operator {
        assert!(
            !line.contains(password) && !line.contains("never-log-this-link"),
            "no secrets"
        );
    }
    for line in &operator[4..6] {
        assert!(
            line.contains("status=400") && line.contains("result=\"failed\""),
            "{line}"
        );
    }
}
