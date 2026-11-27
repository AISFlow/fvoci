//! The token of an invitation is in no line of the log and in no row of the database:
//! not when its link is made, listed, looked at, opened as a page, accepted, refused or
//! revoked.
//!
//! A binary of its own: the log is caught by a subscriber of this thread, and the
//! interest of `tracing` in each place that logs is shared by the whole process, so
//! that tests on other threads, without a subscriber, could leave it out.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fvoci_model::{Container, Invitation, UserWorkspace, new_id};
use fvoci_server::Server;
use fvoci_server::config::Configuration;
use fvoci_store::testing::TestDatabase;
use serde_json::{Value, json};

const PASSWORD: &str = "Pa$$word1";

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

/// A client of one server, with or without a session.
struct Api {
    http: reqwest::Client,
    base: String,
    session: Option<String>,
}

impl Api {
    fn signed_out(&self) -> Self {
        Self {
            http: self.http.clone(),
            base: self.base.clone(),
            session: None,
        }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<&Value>,
    ) -> (u16, String) {
        let mut request = self
            .http
            .request(method, format!("{}{path}", self.base))
            .header("X-Requested-With", "XMLHttpRequest");
        if let Some(session) = &self.session {
            request = request.bearer_auth(session);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let answer = request.send().await.expect("the server answers");
        let status = answer.status().as_u16();
        (status, answer.text().await.expect("the body is read"))
    }

    async fn post(&self, path: &str, body: &Value) -> (u16, String) {
        self.send(reqwest::Method::POST, path, Some(body)).await
    }

    async fn get(&self, path: &str) -> (u16, String) {
        self.send(reqwest::Method::GET, path, None).await
    }

    /// Signs in and returns the client with the session.
    async fn signed_in(&self, username: &str) -> Self {
        let answer = self
            .http
            .post(format!("{}/api/v1/login", self.base))
            .header("X-Requested-With", "XMLHttpRequest")
            .json(&json!({"username": username, "password": PASSWORD}))
            .send()
            .await
            .expect("the server answers");
        let session = answer
            .headers()
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|cookie| cookie.split(';').next()?.strip_prefix("fvoci_session="))
            .expect("a session")
            .to_owned();
        Self {
            http: self.http.clone(),
            base: self.base.clone(),
            session: Some(session),
        }
    }
}

fn account(username: &str) -> Value {
    json!({
        "username": username,
        "email": format!("{username}@example.com"),
        "password": PASSWORD,
    })
}

fn parsed<T: serde::de::DeserializeOwned>((status, body): (u16, String)) -> T {
    assert!((200..300).contains(&status), "{status}: {body}");
    serde_json::from_str(&body).expect("the answer is what the contract says")
}

/// Everything that is done with the links of a workspace, as the routes do it; returns
/// the tokens that were handed out and the bodies that were answered after the making.
async fn use_the_links(api: &Api) -> (Vec<String>, Vec<String>) {
    let (status, body) = api
        .post(
            "/api/v1/register",
            &json!({
                "username": "admin", "email": "admin@example.com", "password": PASSWORD,
                "organizationName": "The Firm",
            }),
        )
        .await;
    assert_eq!(status, 201, "{body}");
    let admin = api.signed_in("admin").await;
    let workspaces: Vec<UserWorkspace> = parsed(admin.get("/api/v1/users/me/workspaces").await);
    let workspace = &workspaces[0].id;
    let board: Container = parsed(
        admin
            .post(
                &format!("/api/v1/w/{workspace}/boards"),
                &json!({"workspaceId": workspace, "visibility": "private", "title": "Plans"}),
            )
            .await,
    );
    let invitations = format!("/api/v1/w/{workspace}/invitations");
    let make = async |body: Value| -> Invitation { parsed(admin.post(&invitations, &body).await) };

    let for_members = make(json!({"role": "member"})).await;
    let for_a_guest = make(json!({"role": "guest", "containerId": board.id})).await;
    let revoked = make(json!({"role": "guest", "containerId": board.id})).await;
    let tokens: Vec<String> = [&for_members, &for_a_guest, &revoked]
        .into_iter()
        .map(|link| link.token.clone().expect("a new link comes with its token"))
        .collect();

    // What is answered once the links are made: none of it has a token.
    let mut answers = Vec::new();
    let nobody = api.signed_out();
    answers.push(admin.get(&invitations).await.1);
    for token in &tokens {
        let look = json!({"token": token});
        answers.push(nobody.post("/api/v1/invitations/preview", &look).await.1);
        // The page of the link, as a browser asks for it.
        answers.push(nobody.get(&format!("/invite/{token}")).await.1);
    }
    let (status, body) = admin
        .send(
            reqwest::Method::DELETE,
            &format!("{invitations}/{}", revoked.id),
            None,
        )
        .await;
    assert_eq!(status, 204, "{body}");
    for (username, token) in [("member", &tokens[0]), ("guest", &tokens[1])] {
        let accept = json!({"token": token, "account": account(username)});
        let (status, body) = nobody.post("/api/v1/invitations/accept", &accept).await;
        assert_eq!(status, 200, "{body}");
        answers.push(body);
    }
    // What is refused: a link that is used up, one that was revoked, a name that is
    // taken, an account that is refused for what it says.
    for (token, username) in [
        (&tokens[1], "late"),
        (&tokens[2], "later"),
        (&tokens[0], "member"),
        (&tokens[0], ""),
    ] {
        let accept = json!({"token": token, "account": account(username)});
        let (status, body) = nobody.post("/api/v1/invitations/accept", &accept).await;
        assert!((400..500).contains(&status), "{status}: {body}");
        answers.push(body);
    }
    answers.push(admin.get(&invitations).await.1);
    (tokens, answers)
}

#[tokio::test]
async fn the_token_of_an_invitation_is_not_logged_stored_or_answered_again() {
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _logging = tracing::subscriber::set_default(subscriber);

    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("invitation-token-log")
        .join(new_id());
    let web_app_path = directory.join("web");
    std::fs::create_dir_all(&web_app_path).expect("the directory is made");
    std::fs::write(
        web_app_path.join("index.html"),
        "<html><body>the app</body></html>",
    )
    .expect("the index page is written");
    let database = TestDatabase::create(&directory).await;
    let config = Configuration {
        database_kind: database.db_type().as_str().to_owned(),
        database_connection: database.database_connection().to_owned(),
        web_app_path: web_app_path.display().to_string(),
        file_storage: "local".to_owned(),
        file_storage_path: directory.join("uploads").display().to_string(),
        skip_template_init: true,
        login_address_attempts_per_minute: 0,
        ..Configuration::default()
    };
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
    let api = Api {
        http: reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("an HTTP client"),
        base,
        session: None,
    };

    let (tokens, answers) = use_the_links(&api).await;

    let stored = app
        .store()
        .system()
        .texts_unchecked(
            "SELECT CAST(id AS TEXT) || ' ' || token_hash || ' ' || CAST(created_by AS TEXT) \
             FROM invitations",
        )
        .await
        .expect("the links are read")
        .join("\n");
    let _ = stop.send(());
    serving
        .await
        .expect("the server task does not panic")
        .expect("the server stops cleanly");

    let log = captured.text();
    assert!(
        log.contains("/invite/<token>") && log.contains("/api/v1/invitations/accept"),
        "the requests are in the log that was caught"
    );
    assert_eq!(stored.lines().count(), 3, "the three links: {stored}");
    for token in &tokens {
        assert_eq!(token.len(), 43, "256 random bits");
        assert!(!log.contains(token.as_str()), "a token is in the log");
        assert!(
            !stored.contains(token.as_str()),
            "a token is in the database"
        );
        for answer in &answers {
            assert!(
                !answer.contains(token.as_str()),
                "a token is answered again: {answer}"
            );
        }
    }

    database.remove().await;
    let _ = std::fs::remove_dir_all(&directory);
}
