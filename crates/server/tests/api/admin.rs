//! Integration tests for server infrastructure around the API: the local Unix
//! administration socket, TLS serving, and retired non-standalone routes.

use std::path::{Path, PathBuf};
use std::time::Duration;

use fvoci_model::new_id;
use fvoci_server::TlsFiles;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::{Client, PASSWORD, ROOT_WORKSPACE_ID, Setup, TestServer};

/// The administration socket. Unix sockets are what it is made of.
#[cfg(unix)]
pub(crate) mod socket {
    use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
    use std::path::{Path, PathBuf};

    use fvoci_model::{Problem, new_id};
    use reqwest::{Method, StatusCode};
    use serde_json::json;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    use tokio::net::UnixStream;

    use crate::common::{Client, PASSWORD, Setup, TestServer, USER1_USERNAME};

    const NEW_PASSWORD: &str = "N3w-Pa$$word";
    const ADMIN_PASSWORD_ROUTE: &str = "/api/v1/admin/users/user1/password";

    /// A path for a socket. The build directory is too long for a socket address, so the
    /// sockets of these tests live in the system's directory for temporary files.
    pub(crate) fn socket_path() -> PathBuf {
        std::env::temp_dir().join(format!("fvoci-{}.socket", new_id()))
    }

    /// A server with `user1` and `user2` and the administration socket at the returned path.
    async fn start_with_admin_socket() -> (TestServer, PathBuf) {
        let socket = socket_path();
        let th = Setup::native()
            .admin_socket(&socket)
            .start()
            .await
            .init_basic()
            .await;
        (th, socket)
    }

    /// What the server answered on the socket.
    #[derive(Debug)]
    pub(crate) struct SocketResponse {
        pub(crate) status: u16,
        /// The status line and the headers, as sent.
        head: String,
        pub(crate) body: String,
    }

    impl SocketResponse {
        /// Checks the answer of an operation without a result: status 204, no body, and
        /// no `Content-Type` for the body that is not there.
        #[track_caller]
        pub(crate) fn check_no_content(&self) {
            assert_eq!(self.status, 204, "the status; body: {}", self.body);
            assert_eq!(self.body, "", "a 204 has no body");
            assert!(
                !self.head.to_lowercase().contains("content-type:"),
                "a 204 names no content type: {}",
                self.head
            );
        }

        /// Checks an error answer: the status, the name of its type, the detail, and
        /// that it is a problem with the identifier of the request.
        #[track_caller]
        pub(crate) fn check_error(&self, status: u16, name: &str, message: &str) {
            assert_eq!(self.status, status, "the status; body: {}", self.body);
            assert!(
                self.head
                    .to_lowercase()
                    .contains("content-type: application/problem+json"),
                "an error is a problem: {}",
                self.head
            );
            let body: Problem = serde_json::from_str(&self.body).expect("an error body");
            assert_eq!(body.status, status, "{}", self.body);
            assert_eq!(
                body.problem_type,
                format!("/problems/{name}"),
                "{}",
                self.body
            );
            assert_eq!(body.detail, message, "{}", self.body);
            let request_id = body.request_id.expect("the request is identified");
            assert_eq!(
                body.instance,
                Some(format!("urn:uuid:{request_id}")),
                "{}",
                self.body
            );
        }
    }

    /// Sends one HTTP request through the Unix socket. No header but the ones the protocol
    /// needs: the administration API asks for no session and no request-forgery header.
    pub(crate) async fn socket_request(
        socket: &Path,
        method: &str,
        target: &str,
        body: &str,
    ) -> SocketResponse {
        let mut stream = UnixStream::connect(socket)
            .await
            .expect("the socket accepts connections");
        let request = format!(
            "{method} {target} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .await
            .expect("the request is sent");
        let mut answer = Vec::new();
        stream
            .read_to_end(&mut answer)
            .await
            .expect("the answer is read");
        let answer = String::from_utf8_lossy(&answer);
        let (head, body) = answer
            .split_once("\r\n\r\n")
            .expect("the answer has a head and a body");
        let status = head
            .split(' ')
            .nth(1)
            .and_then(|status| status.parse().ok())
            .expect("the answer starts with a status line");
        SocketResponse {
            status,
            head: head.to_owned(),
            body: body.to_owned(),
        }
    }

    async fn set_password(socket: &Path, username: &str, body: &str) -> SocketResponse {
        let target = format!("/api/v1/admin/users/{username}/password");
        socket_request(socket, "POST", &target, body).await
    }

    /// Whether `user1` can sign in with the password.
    async fn can_log_in(th: &TestServer, password: &str) -> bool {
        let mut client = th.anonymous();
        let response = client.login(USER1_USERNAME, password).await;
        match response.status {
            StatusCode::NO_CONTENT => true,
            StatusCode::UNAUTHORIZED => {
                assert_eq!(
                    response.error_message(),
                    "incorrect login",
                    "the refusal of a wrong password"
                );
                false
            }
            other => panic!("unexpected status {other} at login: {}", response.text()),
        }
    }

    /// Replacing a user's password via the admin socket requires no old password and
    /// returns an empty body.
    #[tokio::test]
    async fn admin_set_password_changes_the_password() {
        let (th, socket) = start_with_admin_socket().await;

        let body = json!({ "password": NEW_PASSWORD }).to_string();
        let response = set_password(&socket, USER1_USERNAME, &body).await;

        response.check_no_content();
        assert!(can_log_in(&th, NEW_PASSWORD).await, "the new password");
        assert!(!can_log_in(&th, PASSWORD).await, "the old password");
        assert!(
            th.login("user2", PASSWORD).await.token.len() > 1,
            "other users keep their passwords"
        );
        th.teardown().await;
    }

    /// The way back in when no administrator can sign in: whoever can open the socket
    /// makes a user an administrator, by name.
    #[tokio::test]
    async fn the_socket_makes_a_user_an_administrator() {
        let (th, socket) = start_with_admin_socket().await;
        assert!(
            !th.client2.me().await.is_admin,
            "user2 is none to begin with"
        );

        let response = socket_request(&socket, "POST", "/api/v1/admin/users/USER2/admin", "").await;
        response.check_no_content();
        assert!(
            th.client2.me().await.is_admin,
            "user2 is one now, found by name in any letter case"
        );
        assert!(th.client.me().await.is_admin, "and user1 still is");

        let org_id = crate::oidc::org_of(&th).await;
        let records: Vec<fvoci_model::AuditRecord> = th
            .client
            .get(&format!("/org/{org_id}/audit"))
            .await
            .ok_json();
        let promoted: Vec<_> = records
            .iter()
            .filter(|record| record.action == fvoci_model::AuditAction::OrgAdminGranted)
            .collect();
        assert_eq!(promoted.len(), 1);
        assert_eq!(
            promoted[0].actor,
            fvoci_model::AuditRecordActor::ServerOperator
        );
        assert_eq!(
            promoted[0].subject.as_ref().expect("subject").id,
            th.client2.me().await.id
        );

        let response =
            socket_request(&socket, "POST", "/api/v1/admin/users/nobody/admin", "").await;
        assert_eq!(response.status, 404, "{}", response.body);
        th.teardown().await;
    }

    /// Setting a user's password via the admin socket invalidates every existing session
    /// of that user, since the old password may have been compromised.
    #[tokio::test]
    async fn admin_set_password_ends_every_session_of_the_user() {
        let (th, socket) = start_with_admin_socket().await;
        let second_device = th.login(USER1_USERNAME, PASSWORD).await;
        for (client, device) in [(&th.client, "the first"), (&second_device, "a second")] {
            client.get("/users/me").await.check_ok();
            assert_ne!(client.token, "", "{device} device is signed in");
        }

        let body = json!({ "password": NEW_PASSWORD }).to_string();
        set_password(&socket, USER1_USERNAME, &body)
            .await
            .check_no_content();

        for (client, device) in [(&th.client, "the first"), (&second_device, "a second")] {
            let response = client.get("/users/me").await;
            response.check_unauthorized();
            assert_eq!(
                response.error_message(),
                "invalid session token",
                "the session of {device} device is gone, as after a logout"
            );
        }
        let signed_in_again = th.login(USER1_USERNAME, NEW_PASSWORD).await;
        assert_eq!(
            signed_in_again.me().await.username,
            USER1_USERNAME,
            "a session opened with the new password works"
        );
        th.teardown().await;
    }

    /// Only the sessions of the user whose password was set end.
    #[tokio::test]
    async fn admin_set_password_leaves_the_sessions_of_other_users() {
        let (th, socket) = start_with_admin_socket().await;

        let body = json!({ "password": NEW_PASSWORD }).to_string();
        set_password(&socket, USER1_USERNAME, &body)
            .await
            .check_no_content();

        assert_eq!(
            th.client2.me().await.username,
            "user2",
            "the session of another user"
        );
        th.teardown().await;
    }

    /// A request that sets no password ends no session.
    #[tokio::test]
    async fn a_refused_admin_set_password_keeps_the_sessions() {
        let (th, socket) = start_with_admin_socket().await;

        set_password(&socket, USER1_USERNAME, r#"{"password":""}"#)
            .await
            .check_error(400, "bad-request", "password is required");
        let body = json!({ "password": NEW_PASSWORD }).to_string();
        set_password(&socket, "nobody", &body).await.check_error(
            404,
            "not-found",
            "user not found",
        );

        assert_eq!(
            th.client.me().await.username,
            USER1_USERNAME,
            "the session is still there"
        );
        assert!(can_log_in(&th, PASSWORD).await, "and so is the password");
        th.teardown().await;
    }

    /// The user is named in the path, percent-encoded like any path.
    #[tokio::test]
    async fn admin_set_password_decodes_the_username() {
        let (th, socket) = start_with_admin_socket().await;

        let body = json!({ "password": NEW_PASSWORD }).to_string();
        set_password(&socket, "us%65r1", &body)
            .await
            .check_no_content();

        assert!(can_log_in(&th, NEW_PASSWORD).await, "the new password");
        th.teardown().await;
    }

    /// An empty or missing password is rejected as a bad request rather than hashed.
    #[tokio::test]
    async fn admin_set_password_refuses_an_empty_password() {
        let (th, socket) = start_with_admin_socket().await;

        for body in [r#"{"password":""}"#, "{}", r#"{"password":null}"#, "null"] {
            let response = set_password(&socket, USER1_USERNAME, body).await;
            response.check_error(400, "bad-request", "password is required");
        }

        assert!(can_log_in(&th, PASSWORD).await, "the password is unchanged");
        th.teardown().await;
    }

    /// Admin password resets enforce the same password rules as self-service resets: at
    /// least eight characters and at most 1024 bytes, hashed in full.
    #[tokio::test]
    async fn admin_set_password_applies_the_password_rules() {
        let (th, socket) = start_with_admin_socket().await;

        for (password, message) in [
            ("a".to_owned(), "password must be at least 8 characters"),
            ("éééé".to_owned(), "password must be at least 8 characters"),
            ("x".repeat(1025), "password must be at most 1024 bytes"),
        ] {
            let body = json!({ "password": password }).to_string();
            let response = set_password(&socket, USER1_USERNAME, &body).await;
            response.check_error(400, "bad-request", message);
            assert!(can_log_in(&th, PASSWORD).await, "the password is unchanged");
        }

        // What bcrypt could not hash, or hashed in part: 73 bytes, and the longest
        // password there is.
        for password in ["x".repeat(73), "x".repeat(1024)] {
            let body = json!({ "password": password }).to_string();
            set_password(&socket, USER1_USERNAME, &body)
                .await
                .check_no_content();
            assert!(can_log_in(&th, &password).await, "{} bytes", password.len());
            let mut differs_at_the_end = password.clone();
            differs_at_the_end.pop();
            differs_at_the_end.push('y');
            assert!(
                !can_log_in(&th, &differs_at_the_end).await,
                "every byte of a {}-byte password counts",
                password.len()
            );
        }
        th.teardown().await;
    }

    /// A username that no account has returns 404.
    #[tokio::test]
    async fn admin_set_password_of_an_unknown_user_is_not_found() {
        let (th, socket) = start_with_admin_socket().await;

        let body = json!({ "password": NEW_PASSWORD }).to_string();
        let response = set_password(&socket, "nobody", &body).await;

        response.check_error(404, "not-found", "user not found");
        th.teardown().await;
    }

    /// A body that is not a JSON object with a string password returns 400.
    #[tokio::test]
    async fn admin_set_password_with_a_malformed_body_is_a_bad_request() {
        let (th, socket) = start_with_admin_socket().await;

        for body in ["", r#"{"password":"#, r#"{"password":5}"#, "5"] {
            let response = set_password(&socket, USER1_USERNAME, body).await;
            assert_eq!(response.status, 400, "{body}: {}", response.body);
            assert!(
                response.body.contains(r#""type":"/problems/bad-request""#),
                "{body}"
            );
        }

        assert!(can_log_in(&th, PASSWORD).await, "the password is unchanged");
        th.teardown().await;
    }

    /// The comparison of the file storage with the records, through the socket: without
    /// a body it reports and changes nothing; asked to remove what nothing names, it
    /// leaves an object written just now; a body it does not know is refused.
    #[tokio::test]
    async fn the_socket_compares_the_files_with_the_records() {
        const ROUTE: &str = "/api/v1/admin/files/reconcile";
        let (th, socket) = start_with_admin_socket().await;
        let stray = format!("{}/{}/stray", new_id(), new_id());
        th.app
            .files()
            .write(&stray, &mut &b"nothing names it"[..])
            .await
            .expect("written");

        for body in [
            "",
            r#"{"removeUnrecorded": false}"#,
            r#"{"removeUnrecorded": true}"#,
        ] {
            let response = socket_request(&socket, "POST", ROUTE, body).await;
            assert_eq!(response.status, 200, "{body}: {}", response.body);
            let report: serde_json::Value =
                serde_json::from_str(&response.body).expect("a JSON report");
            assert_eq!(
                report,
                json!({
                    "objects": 1,
                    "unrecorded": {"count": 1, "paths": [stray]},
                    "foreign": {"count": 0, "paths": []},
                    "doomed": 0,
                    "records": 0,
                    "missing": {"count": 0, "paths": []},
                }),
                "{body}"
            );
        }
        let listed = th
            .app
            .housekeeping()
            .store()
            .loose_objects()
            .await
            .expect("listed");
        assert_eq!(listed, [], "nothing is doomed");
        assert!(
            th.app.files().exists(&stray).await.expect("read"),
            "the object is there"
        );
        socket_request(&socket, "POST", ROUTE, r#"{"remove": true}"#)
            .await
            .check_error(
                400,
                "bad-request",
                "unknown field `remove`, expected `removeUnrecorded` at line 1 column 9",
            );
        th.teardown().await;
    }

    /// The socket serves the one administration route and nothing else: not the API, not
    /// `hello`, not the web app.
    #[tokio::test]
    async fn admin_socket_serves_only_the_admin_route() {
        let (th, socket) = start_with_admin_socket().await;

        for method in ["GET", "PUT", "DELETE"] {
            let response = socket_request(&socket, method, ADMIN_PASSWORD_ROUTE, "").await;
            response.check_error(405, "method-not-allowed", "method not allowed");
            assert!(
                response.head.to_lowercase().contains("allow: post"),
                "{method}: {}",
                response.head
            );
        }
        for target in [
            "/",
            "/api/v1/hello",
            "/api/v1/ping",
            "/api/v1/users/me",
            "/api/v1/admin/users/user1/password/",
            "/api/v1/admin/boards",
        ] {
            let response = socket_request(&socket, "GET", target, "").await;
            response.check_error(404, "not-found", "route not found");
        }

        // The `type` of such an answer is the address of a page on this socket too,
        // whatever path the server's other listener is served under.
        let response = socket_request(&socket, "GET", "/problems/not-found", "").await;
        assert_eq!(response.status, 200, "{}", response.body);
        assert!(
            response
                .body
                .starts_with("Not found\n\ntype: /problems/not-found\nstatus: 404\n"),
            "{}",
            response.body
        );
        let response = socket_request(&socket, "GET", "/problems/%FF", "").await;
        response.check_error(400, "bad-request", "the request is malformed");
        th.teardown().await;
    }

    /// The TCP listener does not know the administration route: as for any path under
    /// the API prefix that it does not know, the answer is 404, and nothing happens.
    #[tokio::test]
    async fn admin_route_is_not_reachable_over_tcp() {
        let (th, _socket) = start_with_admin_socket().await;
        let body = json!({ "password": NEW_PASSWORD });

        let with_header = th
            .anonymous()
            .post("/admin/users/user1/password", &body)
            .await;
        let without_header = Client::send(
            th.anonymous()
                .raw_request(Method::POST, ADMIN_PASSWORD_ROUTE)
                .json(&body),
        )
        .await;
        let signed_in = th.client.post("/admin/users/user1/password", &body).await;

        for response in [with_header, without_header, signed_in] {
            response.check_error(StatusCode::NOT_FOUND, "route not found");
        }
        assert!(can_log_in(&th, PASSWORD).await, "the password is unchanged");
        assert!(!can_log_in(&th, NEW_PASSWORD).await, "nothing was set");
        th.teardown().await;
    }

    /// The admin Unix socket file is mode 0600 (server user only) and removed on shutdown.
    #[tokio::test]
    async fn admin_socket_is_private_and_removed_on_shutdown() {
        let (th, socket) = start_with_admin_socket().await;

        let metadata = std::fs::symlink_metadata(&socket).expect("the socket file exists");
        assert!(metadata.file_type().is_socket(), "a socket");
        assert_eq!(
            metadata.permissions().mode() & 0o777,
            0o600,
            "only the owner may connect"
        );

        th.teardown().await;
        assert!(!socket.exists(), "the socket file is removed");
    }

    /// A socket file left behind by a server that was killed does not keep the next one
    /// from starting.
    #[tokio::test]
    async fn admin_socket_replaces_the_socket_of_an_earlier_run() {
        let socket = socket_path();
        drop(std::os::unix::net::UnixListener::bind(&socket).expect("a socket is bound"));
        assert!(socket.exists(), "the stale socket file exists");

        let th = Setup::native()
            .admin_socket(&socket)
            .start()
            .await
            .init_basic()
            .await;

        let body = json!({ "password": NEW_PASSWORD }).to_string();
        set_password(&socket, USER1_USERNAME, &body)
            .await
            .check_no_content();
        th.teardown().await;
        assert!(!socket.exists(), "the socket file is removed");
    }

    /// `serve_admin_socket` is off unless asked for, and the socket has its documented place.
    #[tokio::test]
    async fn admin_socket_is_closed_by_default() {
        let th = Setup::native().start().await;

        let config = th.app.config();
        assert!(!config.serve_admin_socket, "off by default");
        assert_eq!(config.admin_socket_path, "/var/tmp/fvoci_local.socket");
        th.teardown().await;
    }
}

/// A certificate for `localhost` and its key, written where the server is told to look.
fn write_certificate(directory: &Path) -> (TlsFiles, reqwest::Certificate) {
    let rcgen::CertifiedKey { cert, signing_key } =
        rcgen::generate_simple_self_signed(vec!["localhost".to_owned()])
            .expect("a certificate is generated");
    std::fs::create_dir_all(directory).expect("the certificate directory is created");
    let tls_files = TlsFiles {
        certificate: directory.join("cert.pem"),
        key: directory.join("key.pem"),
    };
    std::fs::write(&tls_files.certificate, cert.pem()).expect("the certificate is written");
    std::fs::write(&tls_files.key, signing_key.serialize_pem()).expect("the key is written");
    let root = reqwest::Certificate::from_pem(cert.pem().as_bytes())
        .expect("the certificate is read back");
    (tls_files, root)
}

fn certificate_directory() -> PathBuf {
    PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("tls-tests")
        .join(new_id())
}

/// The port the test server listens on.
fn port_of(th: &TestServer) -> u16 {
    th.base_url
        .rsplit(':')
        .next()
        .and_then(|port| port.parse().ok())
        .expect("the address ends with a port")
}

/// With `serve_https` and valid `cert.pem`/`key.pem`, the server speaks TLS and rejects
/// plain HTTP on the TLS port.
#[tokio::test]
async fn serve_https_serves_over_tls() {
    let directory = certificate_directory();
    let (tls_files, root) = write_certificate(&directory);
    let th = Setup::native()
        .tls_files(tls_files)
        .configure(|config| config.serve_https = true)
        .start()
        .await;
    let port = port_of(&th);
    let https = reqwest::Client::builder()
        .tls_certs_only([root])
        .timeout(Duration::from_secs(30))
        .build()
        .expect("an HTTPS client can be built");

    let ping = https
        .get(format!("https://localhost:{port}/api/v1/ping"))
        .send()
        .await
        .expect("the server answers over TLS");
    assert_eq!(ping.status(), StatusCode::OK);
    let ping_body: Value =
        serde_json::from_str(&ping.text().await.expect("the body is read")).expect("ping JSON");
    assert_eq!(ping_body["version"], json!(env!("CARGO_PKG_VERSION")));

    // The API works as over plain HTTP, here with a body in each direction.
    let register = https
        .post(format!("https://localhost:{port}/api/v1/register"))
        .header("X-Requested-With", "XMLHttpRequest")
        .json(&json!({
            "username": "user1", "email": "user1@sample.com", "password": PASSWORD,
            "organizationName": "The Firm",
        }))
        .send()
        .await
        .expect("the server answers over TLS");
    assert_eq!(register.status(), StatusCode::CREATED);
    let registered: Value = serde_json::from_str(&register.text().await.expect("the body is read"))
        .expect("the body is JSON");
    assert_eq!(registered["username"], "user1", "the user that was made");

    assert!(
        reqwest::Client::new()
            .get(format!("http://localhost:{port}/api/v1/ping"))
            .timeout(Duration::from_secs(30))
            .send()
            .await
            .is_err(),
        "plain HTTP gets no answer on the TLS port"
    );

    // Shutdown is as graceful as without TLS: the server stops without an error.
    th.teardown().await;
    let _ = std::fs::remove_dir_all(&directory);
}

/// A client that does not know the certificate does not get through, and the server
/// keeps serving the ones that do.
#[tokio::test]
async fn serve_https_failed_handshakes_do_not_disturb_the_server() {
    let directory = certificate_directory();
    let (tls_files, root) = write_certificate(&directory);
    let th = Setup::native()
        .tls_files(tls_files)
        .configure(|config| config.serve_https = true)
        .start()
        .await;
    let port = port_of(&th);
    let url = format!("https://localhost:{port}/api/v1/ping");

    let stranger = reqwest::Client::builder()
        .tls_certs_only([])
        .build()
        .expect("an HTTPS client can be built");
    assert!(
        stranger.get(&url).send().await.is_err(),
        "an unknown certificate is not trusted"
    );
    // A connection that never starts the handshake does not hold up the others.
    let _silent = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("the port accepts connections");

    let https = reqwest::Client::builder()
        .tls_certs_only([root])
        .build()
        .expect("an HTTPS client can be built");
    let ping = https.get(&url).send().await.expect("the server answers");
    assert_eq!(ping.status(), StatusCode::OK);
    let ping_body: Value =
        serde_json::from_str(&ping.text().await.expect("the body is read")).expect("ping JSON");
    assert_eq!(ping_body["version"], json!(env!("CARGO_PKG_VERSION")));

    th.teardown().await;
    let _ = std::fs::remove_dir_all(&directory);
}

/// When `serve_https` is enabled and certificate or key files are missing, startup fails
/// rather than falling back to plain HTTP.
#[tokio::test]
async fn serve_https_without_certificate_files_refuses_to_serve() {
    let directory = certificate_directory();
    let tls_files = TlsFiles {
        certificate: directory.join("cert.pem"),
        key: directory.join("key.pem"),
    };
    let th = Setup::native()
        .tls_files(tls_files)
        .configure(|config| config.serve_https = true)
        .start()
        .await;
    let port = port_of(&th);

    let error = th.stop().await.expect_err("serving is refused");

    let message = format!("{error:#}");
    for expected in [
        "serve_https is set",
        "cert.pem",
        "key.pem",
        "turn serve_https off",
    ] {
        assert!(
            message.contains(expected),
            "{expected:?} is missing from the refusal: {message}"
        );
    }
    assert!(
        reqwest::Client::new()
            .get(format!("http://localhost:{port}/api/v1/ping"))
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .is_err(),
        "nothing is served, in particular not plain HTTP"
    );
}

/// One of the two files is not enough, and the refusal names the one that is missing.
#[tokio::test]
async fn serve_https_with_a_certificate_but_no_key_refuses_to_serve() {
    let directory = certificate_directory();
    let (tls_files, _root) = write_certificate(&directory);
    std::fs::remove_file(&tls_files.key).expect("the key file is removed");
    let th = Setup::native()
        .tls_files(tls_files)
        .configure(|config| config.serve_https = true)
        .start()
        .await;

    let error = th.stop().await.expect_err("serving is refused");

    let message = format!("{error:#}");
    assert!(message.contains("key.pem"), "the missing file: {message}");
    assert!(
        !message.contains("cert.pem"),
        "the file that is there is not named: {message}"
    );
    let _ = std::fs::remove_dir_all(&directory);
}

/// Without `serve_https` the certificate files are not looked at: plain HTTP, as asked.
#[tokio::test]
async fn without_serve_https_missing_certificate_files_do_not_matter() {
    let directory = certificate_directory();
    let tls_files = TlsFiles {
        certificate: directory.join("cert.pem"),
        key: directory.join("key.pem"),
    };
    let th = Setup::native().tls_files(tls_files).start().await;

    let response = Client::send(th.client.raw_request(Method::GET, "/api/v1/ping")).await;

    response.check_ok();
    assert_eq!(
        response.value()["version"],
        json!(env!("CARGO_PKG_VERSION")),
        "over plain HTTP"
    );
    th.teardown().await;
}

/// Retired host-integration paths (channels, statistics, compliance exports) do not exist
/// and return 404 with or without a session.
#[tokio::test]
async fn routes_of_the_host_application_do_not_exist() {
    let paths = [
        &format!("/teams/{ROOT_WORKSPACE_ID}/channels/some-channel"),
        "/statistics",
        "/admin/boards",
        "/admin/boards?workspace_id=nope&page=x",
        "/admin/boards_history?modified_since=1",
        "/admin/blocks_history?modified_since=1&board_id=nope",
        &format!("{ROOT_WS}/boards/some-board/metadata"),
    ];

    let th = Setup::native().start().await.init_basic().await;
    for path in paths {
        for client in [&th.client, &th.anonymous()] {
            let response = client.get(path).await;
            response.check_error(StatusCode::NOT_FOUND, "route not found");
            assert_eq!(
                response.header("content-type"),
                "application/problem+json",
                "{path}"
            );
        }
    }
    th.teardown().await;
}
