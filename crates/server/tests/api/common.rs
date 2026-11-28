//! The in-process test server and HTTP/WebSocket client harness for API tests.
#![allow(
    dead_code,
    reason = "a shared helper module; each test module uses a different part of it"
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use fvoci_model::PublicLinkChange;
use fvoci_model::{
    Container, ContainerMember, ContainerVisibility, LoginRequest, RegisterRequest, User,
    Workspace, new_id,
};
use fvoci_server::app::App;
use fvoci_server::collab::worker::WorkerCommand;
use fvoci_server::config::Configuration;
use fvoci_server::config::collab::CollabSettings;
use fvoci_server::realtime::Hub;
use fvoci_server::{Server, TlsFiles};
use fvoci_store::Store;
use fvoci_store::testing::{TableOwner, TestDatabase};
use reqwest::{Method, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

/// A websocket connection to a test server.
pub(crate) type WebSocket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Waits until the server has carried out what `socket` sent before, and returns the
/// frames that came meanwhile (the websocket's own Ping and Pong apart).
///
/// The Pong to a Ping says that the server read what was sent before the Ping, not
/// that it did it: a connection reads on while it carries a command out. The hub says
/// the rest ([`Hub::carried_out`]), for every connection: one that another seam of the
/// tests holds keeps this waiting.
///
/// # Panics
///
/// Panics if the connection ends first, or the server is not done in ten seconds.
pub(crate) async fn carried_out(hub: &Hub, socket: &mut WebSocket) -> Vec<Message> {
    use futures_util::{SinkExt as _, StreamExt as _};

    let carried_out = async {
        socket
            .send(Message::Ping(vec![7].into()))
            .await
            .expect("the Ping is sent");
        let mut came = Vec::new();
        loop {
            let frame = socket
                .next()
                .await
                .expect("the connection stays open")
                .expect("a websocket frame");
            match frame {
                Message::Pong(_) => break,
                Message::Ping(_) => {}
                frame => came.push(frame),
            }
        }
        hub.carried_out().await;
        came
    };
    tokio::time::timeout(Duration::from_secs(10), carried_out)
        .await
        .expect("the server carried out what the connection sent")
}

pub(crate) const USER1_USERNAME: &str = "user1";
pub(crate) const USER2_USERNAME: &str = "user2";
pub(crate) const PASSWORD: &str = "Pa$$word";
/// The name of the organization the first user of a test server starts.
pub(crate) const ORG_NAME: &str = "The Firm";
/// The root workspace of every test server: the oldest workspace, the one the server would have
/// made at its first start. The harness makes it before the server starts, so that
/// tests can name it.
pub(crate) const ROOT_WORKSPACE_ID: &str = "00000000-0000-7000-8000-000000000000";
/// A second workspace, the one most tests make their boards in.
pub(crate) const TEST_WORKSPACE_ID: &str = "00000000-0000-7000-8000-000000000001";
/// A third workspace of the organization, which nobody is let into: a workspace that is
/// there and hidden from every user of a test.
pub(crate) const HIDDEN_WORKSPACE_ID: &str = "00000000-0000-7000-8000-000000000002";
/// The path of the root workspace, `/w/{ROOT_WORKSPACE_ID}`: what is of it is under it.
pub(crate) const ROOT_WS: &str = "/w/00000000-0000-7000-8000-000000000000";
/// The path of the test workspace, `/w/{TEST_WORKSPACE_ID}`.
pub(crate) const TEST_WS: &str = "/w/00000000-0000-7000-8000-000000000001";
/// The identifier of nothing: well-formed, and never given to anything.
pub(crate) const UNKNOWN_ID: &str = "00000000-0000-7000-8000-00000000dead";

/// The path of a board or a wiki, under its own workspace.
pub(crate) fn container_path(container: &Container) -> String {
    format!("/w/{}/containers/{}", container.workspace_id, container.id)
}

/// A running server with a database and directories of its own.
pub(crate) struct TestServer {
    /// The application state, for arranging and inspecting what the API has no route for.
    pub(crate) app: App,
    pub(crate) base_url: String,
    /// Where the metrics are served, if the server keeps them.
    pub(crate) metrics_address: Option<std::net::SocketAddr>,
    /// The first client; `user1` after [`TestServer::init_basic`].
    pub(crate) client: Client,
    /// The second client; `user2` after [`TestServer::init_basic`].
    pub(crate) client2: Client,
    /// The identifier of the stranger, once there is one.
    stranger: tokio::sync::OnceCell<String>,
    shutdown: oneshot::Sender<()>,
    serving: tokio::task::JoinHandle<anyhow::Result<()>>,
    database: TestDatabase,
    directory: PathBuf,
    /// What the server was started with, for a start again on the same database (which
    /// a test may change before).
    /// Boxed: the futures of the tests hold the server.
    pub(crate) config: Box<Configuration>,
    base_path: String,
}

/// As whom a test server connects to PostgreSQL.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum DatabaseRole {
    /// The role of the tests, which the wall between organizations holds.
    #[default]
    HeldByTheWall,
    /// A superuser, whom no policy holds.
    Superuser,
    /// The role of the tests, using tables that another role owns and brought up to
    /// date before the server starts.
    NotTheOwner(TableOwner),
}

/// What is in the database of a test server when it starts.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Database {
    /// The organization of the tests with its two workspaces, under the identifiers the
    /// tests name them by.
    #[default]
    Seeded,
    /// Nothing: as a server finds it at its first start.
    Empty,
}

/// The database of a test server, as `role` connects to it.
async fn test_database(role: DatabaseRole, directory: &Path) -> TestDatabase {
    match role {
        DatabaseRole::HeldByTheWall => TestDatabase::create(directory).await,
        DatabaseRole::Superuser => TestDatabase::create_as_superuser(directory).await,
        DatabaseRole::NotTheOwner(owner) => {
            let database = TestDatabase::create_owned_apart(directory, owner).await;
            Store::connect(database.db_type(), database.owner_database_connection())
                .await
                .expect("the owner brings the schema up to date")
                .close()
                .await;
            database
        }
    }
}

/// How a test server differs from the default one.
#[derive(Debug, Default)]
pub(crate) struct Setup {
    /// Whether `user1` is registered and signed in on both clients at the start.
    one_user: bool,
    allow_public_links: bool,
    base_path: String,
    configure: Vec<fn(&mut Configuration)>,
    admin_socket: Option<PathBuf>,
    smtp: Option<fvoci_server::config::smtp::SmtpSettings>,
    mail: Option<fvoci_server::config::mail::MailSettings>,
    search: Option<fvoci_server::config::search::SearchSettings>,
    tls_files: Option<TlsFiles>,
    database_role: DatabaseRole,
    database: Database,
    /// Whether the built-in templates are imported at startup. Off by default so a
    /// server starts quickly.
    with_templates: bool,
    /// `metrics_address`.
    metrics_at: Option<String>,
    /// Certificates the client of the providers trusts besides the system's.
    oidc_roots: Vec<Vec<u8>>,
}

impl Setup {
    /// `SetupTestHelper`: native authentication, no users yet.
    pub(crate) fn native() -> Self {
        Self::default()
    }

    /// One user, `user1`, registered and signed in on both clients: for tests that are
    /// about what a user does and not about who may do it.
    pub(crate) fn one_user() -> Self {
        Self {
            one_user: true,
            ..Self::default()
        }
    }

    /// `SetupTestHelperLocalMode`: native authentication with public sharing turned on.
    pub(crate) fn local_mode() -> Self {
        Self {
            allow_public_links: true,
            ..Self::default()
        }
    }

    /// Connects to PostgreSQL as a superuser: for the tests of what the server makes
    /// of a role that the wall between organizations does not hold.
    pub(crate) fn connecting_as_superuser(mut self) -> Self {
        self.database_role = DatabaseRole::Superuser;
        self
    }

    /// Connects to PostgreSQL as the role of the tests, using tables that `owner` owns
    /// and migrated before the server starts: as a server installed apart from the
    /// administrator of its database.
    pub(crate) fn connecting_as_a_role_that_does_not_own_the_tables(
        mut self,
        owner: TableOwner,
    ) -> Self {
        self.database_role = DatabaseRole::NotTheOwner(owner);
        self
    }

    /// Starts on an empty database, as a server does the first time: the organization
    /// and its first workspace are the server's to make, and [`ROOT_WORKSPACE_ID`] and
    /// [`TEST_WORKSPACE_ID`] name nothing.
    pub(crate) fn on_an_empty_database(mut self) -> Self {
        self.database = Database::Empty;
        self
    }

    /// Imports the built-in templates at startup, for tests that need them.
    pub(crate) fn with_templates(mut self) -> Self {
        self.with_templates = true;
        self
    }

    /// Serves everything under a path prefix such as `/boards`.
    pub(crate) fn base_path(mut self, base_path: &str) -> Self {
        base_path.clone_into(&mut self.base_path);
        self
    }

    /// Keeps the metrics and serves them at `address` (`metrics_address`).
    pub(crate) fn metrics_at(mut self, address: &str) -> Self {
        self.metrics_at = Some(address.to_owned());
        self
    }

    /// Trusts the certificate (DER) of a provider of the test's own.
    pub(crate) fn trusting_oidc_provider(mut self, certificate_der: &[u8]) -> Self {
        self.oidc_roots.push(certificate_der.to_vec());
        self
    }

    /// Changes the configuration before the server starts.
    pub(crate) fn configure(mut self, change: fn(&mut Configuration)) -> Self {
        self.configure.push(change);
        self
    }

    /// Delivers through the test's SMTP sink.
    pub(crate) fn smtp(mut self, settings: fvoci_server::config::smtp::SmtpSettings) -> Self {
        self.smtp = Some(settings);
        self
    }

    /// Sends mail by the way of `settings` (a fake provider's), with the limits of
    /// `smtp`.
    pub(crate) fn mail(
        mut self,
        (smtp, mail): (
            fvoci_server::config::smtp::SmtpSettings,
            fvoci_server::config::mail::MailSettings,
        ),
    ) -> Self {
        self.smtp = Some(smtp);
        self.mail = Some(mail);
        self
    }

    /// Points the server at a search server.
    pub(crate) fn search(mut self, settings: fvoci_server::config::search::SearchSettings) -> Self {
        self.search = Some(settings);
        self
    }

    /// Opens the administration socket at `path` (`serve_admin_socket`).
    pub(crate) fn admin_socket(mut self, path: &Path) -> Self {
        self.admin_socket = Some(path.to_owned());
        self
    }

    /// Reads the certificate and the key for `serve_https` from these files. The server only
    /// serves TLS if the configuration turns `serve_https` on as well; the clients of
    /// [`TestServer`] do not speak it.
    pub(crate) fn tls_files(mut self, tls_files: TlsFiles) -> Self {
        self.tls_files = Some(tls_files);
        self
    }

    /// Starts the test server and waits until it is listening.
    pub(crate) async fn start(self) -> TestServer {
        self.try_start().await.expect("the test server starts")
    }

    /// Starts the server, or returns why it would not start.
    pub(crate) async fn try_start(self) -> anyhow::Result<TestServer> {
        let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("api-tests")
            .join(new_id());
        let web_app_path = directory.join("web");
        write_test_web_app(&web_app_path);

        let database = test_database(self.database_role, &directory).await;
        let mut config = Configuration {
            database_kind: database.db_type().as_str().to_owned(),
            database_connection: database.database_connection().to_owned(),
            web_app_path: web_app_path.display().to_string(),
            file_storage: "local".to_owned(),
            file_storage_path: directory.join("uploads").display().to_string(),
            allow_public_links: self.allow_public_links,
            // Pings are tested on their own; elsewhere they would only be noise.
            skip_websocket_pings: true,
            // Importing the templates is slow; only tests that ask for them pay it.
            skip_template_init: !self.with_templates,
            // Whom to tell is worked off where a test says so (`tend_notifications`):
            // a task that reads the database every second is in the way of the tests
            // that count its reads. It runs in the test that turns it on.
            notifications_by_hand: true,
            // Every client of a test comes from one address, and many sign in at once.
            // The limit by address is tested where a test turns it on.
            login_address_attempts_per_minute: 0,
            // Webhook integration tests bind sinks on 127.0.0.1; SSRF refusal is tested
            // where a test turns this off or uses `Webhook::new` directly.
            webhook_allow_private_addresses: true,
            // The executable of the tests is no server: the workers of editing together
            // are started from the worker of the tests, which is built with what the
            // tests ask of a worker whatever the server's executable was last built
            // with.
            collab: CollabSettings {
                worker_command: Some(WorkerCommand::of_executable(
                    env!("CARGO_BIN_EXE_fvoci-test-worker").into(),
                )),
                ..CollabSettings::default()
            },
            ..Configuration::default()
        };
        if let Some(path) = &self.admin_socket {
            config.serve_admin_socket = true;
            config.admin_socket_path = path.display().to_string();
        }
        if let Some(address) = &self.metrics_at {
            address.clone_into(&mut config.metrics_address);
        }
        config
            .oidc_extra_root_certificates
            .clone_from(&self.oidc_roots);
        if let Some(smtp) = self.smtp {
            config.smtp = smtp;
        }
        if let Some(mail) = self.mail {
            config.mail = mail;
        }
        if let Some(search) = self.search {
            config.search = search;
        }
        for change in &self.configure {
            change(&mut config);
        }

        let seeded = if self.database == Database::Seeded {
            Some(seed_workspaces(&config).await)
        } else {
            None
        };
        let mut server = match Server::new(config.clone(), &self.base_path).await {
            Ok(server) => server,
            Err(error) => {
                // The database of a server that did not start is of no further use.
                database.remove().await;
                return Err(error);
            }
        };
        if let Some(tls_files) = self.tls_files {
            server = server.with_tls_files(tls_files);
        }
        let app = match seeded {
            Some(org_id) => server
                .app()
                .scoped(fvoci_store::Tenant::org(&org_id).expect("the fixture organization")),
            None => server.app().clone(),
        };
        let metrics_address = server.metrics_address();
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a local port is free");
        let address = listener.local_addr().expect("the listener has an address");
        let (shutdown, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(server.serve(listener, async move {
            let _ = stopped.await;
        }));

        let base_url = format!("http://{address}{}", self.base_path);
        let mut server = TestServer {
            app,
            metrics_address,
            client: Client::new(&base_url, ""),
            client2: Client::new(&base_url, ""),
            base_url,
            stranger: tokio::sync::OnceCell::new(),
            shutdown,
            serving,
            database,
            directory,
            config: Box::new(config),
            base_path: self.base_path.clone(),
        };
        if self.one_user {
            server.initialize_one_user().await;
        }
        Ok(server)
    }
}

fn write_test_web_app(web_app_path: &std::path::Path) {
    std::fs::create_dir_all(web_app_path.join("static")).expect("the web directory is created");
    std::fs::write(
        web_app_path.join("index.html"),
        "<html><head><base href=\"{{.BaseURL}}/\"></head><body>test web app</body></html>",
    )
    .expect("the index page is written");
    std::fs::write(
        web_app_path.join("static").join("main.js"),
        "console.log('test');",
    )
    .expect("a static file is written");
}

/// Stores the organization of a test server and its two workspaces, the root workspace first.
async fn seed_workspaces(config: &Configuration) -> String {
    let db_type = config
        .database_kind
        .parse()
        .expect("the test database type is one the store knows");
    let store = Store::connect(db_type, &config.database_connection)
        .await
        .expect("the test database takes the schema");
    let workspace = |workspace_id: &str| Workspace {
        id: workspace_id.to_owned(),
        ..Workspace::default()
    };
    let org_id = store
        .create_organization("", &workspace(ROOT_WORKSPACE_ID))
        .await
        .expect("the organization of the test server is stored");
    let ours = store.for_org(&org_id).expect("an identifier");
    for workspace_id in [TEST_WORKSPACE_ID, HIDDEN_WORKSPACE_ID] {
        ours.create_workspace(&workspace(workspace_id))
            .await
            .expect("a further workspace of the test server is stored");
    }
    store.close().await;
    org_id
}

impl TestServer {
    /// Initializes the one-user fixture and scopes test-only direct store operations.
    async fn initialize_one_user(&mut self) {
        self.client = self
            .register_and_login(USER1_USERNAME, "user1@sample.com", PASSWORD)
            .await;
        self.client2 = self.client.clone();
        let workspaces = self
            .app
            .store()
            .workspaces_of_user(
                &self.client.me().await.id,
                fvoci_store::Making::with_password(),
            )
            .await
            .expect("the founder's workspace");
        self.app = self.app.scoped(
            fvoci_store::Tenant::org(&workspaces[0].org_id).expect("the fixture organization"),
        );
    }

    /// `InitBasic`: registers and signs in `user1`, who starts the organization, on the first
    /// client and `user2`, whom a link for members lets in, on the second.
    pub(crate) async fn init_basic(mut self) -> Self {
        self.client = self
            .register_and_login(USER1_USERNAME, "user1@sample.com", PASSWORD)
            .await;
        self.client2 = self
            .register_and_login(USER2_USERNAME, "user2@sample.com", PASSWORD)
            .await;
        self
    }

    /// The identifier of a user who exists and never signs in: somebody to make boards
    /// and blocks on behalf of where the test's own users must not be involved. The
    /// user is stored on first use, past the registration route.
    pub(crate) async fn stranger(&self) -> String {
        self.stranger
            .get_or_init(|| async {
                let mut user = User {
                    id: new_id(),
                    username: "stranger".to_owned(),
                    email: "stranger@sample.com".to_owned(),
                    ..User::default()
                };
                self.app
                    .store()
                    .create_user(&mut user)
                    .await
                    .expect("the stranger is stored");
                user.id
            })
            .await
            .clone()
    }

    /// The stranger ([`Self::stranger`]) as a member of the workspace `workspace_id`:
    /// somebody to make boards on behalf of who may be their admin, as only who works
    /// in a board's workspace is.
    pub(crate) async fn stranger_in(&self, workspace_id: &str) -> String {
        let user_id = self.stranger().await;
        self.app
            .store()
            .add_workspace_member(workspace_id, &user_id, fvoci_model::WorkspaceRole::Member)
            .await
            .expect("the stranger works in the workspace");
        user_id
    }

    /// The store as the system organization reaches it: where the built-in templates
    /// are, which no other organization changes.
    pub(crate) fn system_store(&self) -> Store {
        self.app
            .store()
            .for_org(fvoci_store::SYSTEM_ORG_ID)
            .expect("the system organization's identifier")
    }

    /// A client without a session.
    pub(crate) fn anonymous(&self) -> Client {
        Client::new(&self.base_url, "")
    }

    /// A client that carries the given session token.
    pub(crate) fn client_with_token(&self, token: &str) -> Client {
        Client::new(&self.base_url, token)
    }

    /// `RegisterAndLogin`: makes an account and returns a client signed in as it. The
    /// first of a server registers and starts the organization ([`ORG_NAME`]); everybody after
    /// accepts a link for members into the root workspace
    /// ([`TestServer::member_link`]), as a server that holds one organization has it.
    pub(crate) async fn register_and_login(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Client {
        let bootstrapped = self
            .app
            .store()
            .is_bootstrapped()
            .await
            .expect("the instance is read");
        if bootstrapped {
            let request = serde_json::json!({
                "token": self.member_link().await,
                "account": { "username": username, "email": email, "password": password },
            });
            self.anonymous()
                .post("/invitations/accept", &request)
                .await
                .check_ok();
        } else {
            let request = RegisterRequest {
                username: username.to_owned(),
                email: email.to_owned(),
                password: password.to_owned(),
                display_name: String::new(),
                organization_name: ORG_NAME.to_owned(),
                language: String::new(),
            };
            self.anonymous()
                .post("/register", &request)
                .await
                .check_created();
        }
        let client = self.login(username, password).await;
        self.let_into_the_second_workspace(&client.me().await.id)
            .await;
        client
    }

    /// The root workspace: the oldest workspace of the organization, made with it.
    pub(crate) async fn root_workspace(&self) -> fvoci_model::Workspace {
        self.app
            .store()
            .get_workspaces()
            .await
            .expect("the workspaces are read")
            .into_iter()
            .next()
            .expect("the organization has a workspace")
    }

    /// The token of a new link that makes members of the root workspace, made past the
    /// route on behalf of an administrator of that workspace: the oldest one it has.
    pub(crate) async fn member_link(&self) -> String {
        let workspace = self.root_workspace().await;
        let admins = self
            .app
            .store()
            .texts_unchecked(&format!(
                "SELECT CAST(user_id AS TEXT) FROM workspace_members WHERE role = 'admin' \
                 AND CAST(workspace_id AS TEXT) = '{}' ORDER BY created_at, user_id",
                workspace.id
            ))
            .await
            .expect("the administrators of the root workspace are read");
        let inviter = admins
            .first()
            .expect("the root workspace has an administrator to invite on behalf of");
        self.app
            .invite(&workspace.id, inviter, fvoci_server::app::Inviting::Members)
            .await
            .expect("a link for members is made")
            .token
            .expect("a new link comes with its token")
    }

    /// Lets a user into the second workspace of the test server, [`TEST_WORKSPACE_ID`], where
    /// most tests make their boards. Whoever registers is in the root workspace, the
    /// organization's default workspace; the users of a test work in both. A server
    /// started on an empty database has no second workspace to be let into.
    pub(crate) async fn let_into_the_second_workspace(&self, user_id: &str) {
        if self.app.store().tenant() == fvoci_store::Tenant::Unset {
            return;
        }
        match self
            .app
            .store()
            .add_workspace_member(
                TEST_WORKSPACE_ID,
                user_id,
                fvoci_model::WorkspaceRole::Member,
            )
            .await
        {
            Ok(()) => {}
            Err(error) if error.is_not_found() => {}
            Err(error) => panic!("the user is let into the second workspace: {error}"),
        }
    }

    /// The user of the client, made a member, and no administrator, of every workspace
    /// they are in: for a test about a member, of the first user, who administers the
    /// workspace they made.
    pub(crate) async fn me_as_no_admin(&self, client: &Client) -> User {
        let user = client.me().await;
        self.app
            .store()
            .execute_unchecked(&format!(
                "UPDATE workspace_members SET role = 'member' WHERE user_id = '{}' \
                 AND role = 'admin'",
                user.id
            ))
            .await
            .expect("the user administers no workspace");
        user
    }

    /// `Login`: returns a client signed in as an existing user.
    pub(crate) async fn login(&self, username: &str, password: &str) -> Client {
        let mut client = self.anonymous();
        client.login(username, password).await.check_no_content();
        client
    }

    /// Makes one more account, which a link for members lets in, and signs it in.
    pub(crate) async fn add_user(&self, username: &str) -> Client {
        self.register_and_login(username, &format!("{username}@sample.com"), PASSWORD)
            .await
    }

    /// Shares a board, past the route and on behalf of nobody, and returns the read
    /// token the server made for it.
    pub(crate) async fn share(&self, board_id: &str) -> String {
        self.app
            .set_public_link(board_id, PublicLinkChange::On, "")
            .await
            .expect("the board is shared")
            .token
    }

    /// `CreateBoard`: creates an empty board as the first client's user.
    pub(crate) async fn create_board(
        &self,
        workspace_id: &str,
        board_type: ContainerVisibility,
    ) -> Container {
        let board = Container {
            workspace_id: workspace_id.to_owned(),
            visibility: board_type,
            ..Container::default()
        };
        self.client
            .post(&format!("/w/{workspace_id}/boards"), &board)
            .await
            .created_json()
    }

    /// Stops the server, removes its database and files, and returns how serving ended.
    /// For servers that are expected to refuse to serve.
    pub(crate) async fn stop(self) -> anyhow::Result<()> {
        let _ = self.shutdown.send(());
        let served = self.serving.await.expect("the server task does not panic");
        self.database.remove().await;
        let _ = std::fs::remove_dir_all(&self.directory);
        served
    }

    /// Stops the server as a server that is killed stops, past what it does when it
    /// is asked to stop (`abandon`, which is given the application), and starts another
    /// on the same database and files, with the clients signed in as they were.
    pub(crate) async fn killed_and_started_again(self, abandon: impl AsyncFnOnce(&App)) -> Self {
        abandon(&self.app).await;
        let _ = self.shutdown.send(());
        self.serving
            .await
            .expect("the server task does not panic")
            .expect("the server stops");
        let server = Server::new((*self.config).clone(), &self.base_path)
            .await
            .expect("the server starts again");
        let app = server.app().scoped(self.app.store().tenant());
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a local port is free");
        let address = listener.local_addr().expect("the listener has an address");
        let (shutdown, stopped) = oneshot::channel::<()>();
        let serving = tokio::spawn(server.serve(listener, async move {
            let _ = stopped.await;
        }));
        let base_url = format!("http://{address}{}", self.base_path);
        let again = |client: &Client| Client {
            base_url: base_url.clone(),
            ..client.clone()
        };
        Self {
            app,
            metrics_address: None,
            client: again(&self.client),
            client2: again(&self.client2),
            base_url: base_url.clone(),
            stranger: self.stranger,
            shutdown,
            serving,
            database: self.database,
            directory: self.directory,
            config: self.config,
            base_path: self.base_path,
        }
    }

    /// Stops the server cleanly, and keeps its database and files for another server
    /// to start on.
    pub(crate) async fn stop_keeping_data(self) -> KeptData {
        let _ = self.shutdown.send(());
        self.serving
            .await
            .expect("the server task does not panic")
            .expect("the server stops cleanly");
        KeptData {
            database: self.database,
            directory: self.directory,
        }
    }

    /// Stops the server and removes its temporary database and files.
    pub(crate) async fn teardown(self) {
        let _ = self.shutdown.send(());
        self.serving
            .await
            .expect("the server task does not panic")
            .expect("the server stops cleanly");
        self.database.remove().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// The database and the files of a test server that stopped.
pub(crate) struct KeptData {
    database: TestDatabase,
    directory: PathBuf,
}

impl KeptData {
    /// Removes the database and the files.
    pub(crate) async fn remove(self) {
        self.database.remove().await;
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

/// An HTTP client for the API that carries one session token.
#[derive(Debug, Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    base_url: String,
    /// The session token; nothing is sent while it is empty.
    pub(crate) token: String,
    /// How the token is sent.
    carrier: Carrier,
}

/// How a client presents its session token.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Carrier {
    /// In the `Authorization` header, as a client that is no browser does.
    Bearer,
    /// In the session cookie, as a browser does.
    Cookie,
}

/// The name of the session cookie of a server reached over plain HTTP, as the test
/// servers are.
pub(crate) const SESSION_COOKIE: &str = "fvoci_session";

/// What the server answered.
#[derive(Debug)]
pub(crate) struct ApiResponse {
    /// The path the request was sent to, the server's base path first.
    pub(crate) path: String,
    pub(crate) status: StatusCode,
    pub(crate) headers: reqwest::header::HeaderMap,
    pub(crate) body: Vec<u8>,
}

impl Client {
    pub(crate) fn new(base_url: &str, token: &str) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("an HTTP client can be built"),
            base_url: base_url.to_owned(),
            token: token.to_owned(),
            carrier: Carrier::Bearer,
        }
    }

    /// This client, answered with a redirect instead of following it: a test server's
    /// `server_url` is the default, which leads to whatever listens on port 8000 of the
    /// machine, if anything does.
    pub(crate) fn not_following_redirects(&self) -> Self {
        Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(30))
                .build()
                .expect("an HTTP client can be built"),
            ..self.clone()
        }
    }

    /// This client's session as a browser has it: the token goes out in the session
    /// cookie instead of the `Authorization` header.
    pub(crate) fn as_browser(&self) -> Self {
        Self {
            carrier: Carrier::Cookie,
            ..self.clone()
        }
    }

    /// Adds the client's token to a request, the way this client presents it.
    fn authorized(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.token.is_empty() {
            return request;
        }
        match self.carrier {
            Carrier::Bearer => request.bearer_auth(&self.token),
            Carrier::Cookie => request.header("Cookie", format!("{SESSION_COOKIE}={}", self.token)),
        }
    }

    /// Opens a websocket connection as this client: the handshake carries the token as
    /// this client presents it, and an `Origin` header if it is a browser's.
    pub(crate) async fn connect_websocket(&self) -> WebSocket {
        let headers: Vec<(&str, String)> = match self.carrier {
            Carrier::Bearer => Vec::new(),
            // A browser names the page's origin; the test servers take localhost:8000
            // for their own address.
            Carrier::Cookie => vec![("Origin", "http://localhost:8000".to_owned())],
        };
        self.try_connect_websocket(&headers)
            .await
            .expect("the websocket endpoint accepts the connection")
    }

    /// Tries to open a websocket connection as this client, with further headers on the
    /// handshake. The error is the status the handshake was refused with.
    pub(crate) async fn try_connect_websocket(
        &self,
        headers: &[(&str, String)],
    ) -> Result<WebSocket, StatusCode> {
        use tokio_tungstenite::tungstenite::Error;
        use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
        use tokio_tungstenite::tungstenite::http::{HeaderName, HeaderValue};

        let mut request = self
            .websocket_url()
            .into_client_request()
            .expect("the websocket address is a URL");
        let mut add = |name: &str, value: &str| {
            request.headers_mut().insert(
                HeaderName::from_bytes(name.as_bytes()).expect("a header name"),
                HeaderValue::from_str(value).expect("a header value"),
            );
        };
        if !self.token.is_empty() {
            match self.carrier {
                Carrier::Bearer => add("Authorization", &format!("Bearer {}", self.token)),
                Carrier::Cookie => add("Cookie", &format!("{SESSION_COOKIE}={}", self.token)),
            }
        }
        for (name, value) in headers {
            add(name, value);
        }
        match tokio_tungstenite::connect_async(request).await {
            Ok((stream, _)) => Ok(stream),
            Err(Error::Http(response)) => {
                Err(StatusCode::from_u16(response.status().as_u16()).expect("a status code"))
            }
            Err(other) => panic!("the websocket handshake failed without an answer: {other}"),
        }
    }

    /// Opens a websocket connection as this client, with a bearer token, whose
    /// socket takes `bytes` of what is sent to it before it is read: for a
    /// connection that does not read, of which the system would hold megabytes
    /// otherwise.
    pub(crate) async fn connect_websocket_receiving(&self, bytes: u32) -> WebSocket {
        use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;
        use tokio_tungstenite::tungstenite::http::HeaderValue;

        let mut request = self
            .websocket_url()
            .into_client_request()
            .expect("the websocket address is a URL");
        let bearer = HeaderValue::from_str(&format!("Bearer {}", self.token)).expect("a token");
        request.headers_mut().insert("Authorization", bearer);
        let address = request
            .uri()
            .authority()
            .expect("the address names a host")
            .as_str()
            .parse()
            .expect("the test servers are reached by address");
        let socket = tokio::net::TcpSocket::new_v4().expect("a socket");
        socket
            .set_recv_buffer_size(bytes)
            .expect("the socket's buffer is set");
        let stream = socket.connect(address).await.expect("the server listens");
        let (stream, _) = tokio_tungstenite::client_async(request, MaybeTlsStream::Plain(stream))
            .await
            .expect("the websocket endpoint accepts the connection");
        stream
    }

    /// The address of a path as it is written, with nothing put before it but the
    /// server's own address: for the paths outside the API, and for those that are
    /// written out in full.
    pub(crate) fn root_url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// The websocket address of the server.
    pub(crate) fn websocket_url(&self) -> String {
        format!("{}/ws", self.base_url.replacen("http", "ws", 1))
    }

    /// A request to an API path such as `/boards`, with the headers every API request
    /// needs. For requests the shorthand methods do not cover.
    pub(crate) fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .request(method, format!("{}/api/v1{path}", self.base_url))
            .header("X-Requested-With", "XMLHttpRequest");
        request = self.authorized(request);
        request
    }

    /// A request to an API path with the client's token and nothing else: without the
    /// header of the request-forgery check, as a link or an image tag asks.
    pub(crate) fn bare_request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.authorized(
            self.http
                .request(method, format!("{}/api/v1{path}", self.base_url)),
        )
    }

    /// A request to any path of the server, with no headers added.
    pub(crate) fn raw_request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http.request(method, self.root_url(path))
    }

    /// Sends a request and reads the whole answer.
    pub(crate) async fn send(request: reqwest::RequestBuilder) -> ApiResponse {
        let response = request.send().await.expect("the server answers");
        let path = response.url().path().to_owned();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response
            .bytes()
            .await
            .expect("the answer has a body")
            .to_vec();
        ApiResponse {
            path,
            status,
            headers,
            body,
        }
    }

    pub(crate) async fn get(&self, path: &str) -> ApiResponse {
        Self::send(self.request(Method::GET, path)).await
    }

    pub(crate) async fn delete(&self, path: &str) -> ApiResponse {
        Self::send(self.request(Method::DELETE, path)).await
    }

    pub(crate) async fn delete_json(&self, path: &str, body: &impl Serialize) -> ApiResponse {
        Self::send(self.request(Method::DELETE, path).json(body)).await
    }

    pub(crate) async fn post(&self, path: &str, body: &impl Serialize) -> ApiResponse {
        Self::send(self.request(Method::POST, path).json(body)).await
    }

    /// A `POST` without a body.
    pub(crate) async fn post_empty(&self, path: &str) -> ApiResponse {
        Self::send(self.request(Method::POST, path)).await
    }

    pub(crate) async fn put(&self, path: &str, body: &impl Serialize) -> ApiResponse {
        Self::send(self.request(Method::PUT, path).json(body)).await
    }

    pub(crate) async fn patch(&self, path: &str, body: &impl Serialize) -> ApiResponse {
        Self::send(self.request(Method::PATCH, path).json(body)).await
    }

    /// A request whose body is sent as given, for malformed input.
    pub(crate) async fn send_raw_body(
        &self,
        method: Method,
        path: &str,
        body: &str,
    ) -> ApiResponse {
        Self::send(self.request(method, path).body(body.to_owned())).await
    }

    /// `Login`: signs in and keeps the session token.
    pub(crate) async fn login(&mut self, username: &str, password: &str) -> ApiResponse {
        let request = LoginRequest {
            username: username.to_owned(),
            email: String::new(),
            password: password.to_owned(),
        };
        let response = self.post("/login", &request).await;
        if response.status == StatusCode::NO_CONTENT {
            self.token = response
                .session_cookie()
                .expect("a login answers with the session cookie");
        }
        response
    }

    /// `GetMe`.
    pub(crate) async fn me(&self) -> User {
        self.get("/users/me").await.ok_json()
    }

    /// Creates a board with the given fields as this client's user.
    pub(crate) async fn create_board(&self, board: &Container) -> ApiResponse {
        self.post(&format!("/w/{}/boards", board.workspace_id), board)
            .await
    }

    /// `AddMemberToBoard` on a board of the test workspace (`TEST_WORKSPACE_ID`).
    pub(crate) async fn add_member(&self, member: &ContainerMember) -> ApiResponse {
        self.add_member_in(TEST_WORKSPACE_ID, member).await
    }

    /// `AddMemberToBoard` on a board of the workspace.
    pub(crate) async fn add_member_in(
        &self,
        workspace_id: &str,
        member: &ContainerMember,
    ) -> ApiResponse {
        self.post(
            &format!(
                "/w/{workspace_id}/containers/{}/members",
                member.container_id
            ),
            member,
        )
        .await
    }
}

impl ApiResponse {
    /// The values of the `Set-Cookie` headers of the answer.
    pub(crate) fn set_cookies(&self) -> Vec<&str> {
        self.headers
            .get_all(reqwest::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect()
    }

    /// The session token the answer sets in the session cookie, under either of its
    /// names; `None` if the answer sets none. The token is empty where the cookie is
    /// cleared.
    pub(crate) fn session_cookie(&self) -> Option<String> {
        self.set_cookies().into_iter().find_map(|cookie| {
            let (pair, _attributes) = cookie.split_once(';').unwrap_or((cookie, ""));
            let (name, value) = pair.split_once('=')?;
            matches!(name, "fvoci_session" | "__Host-fvoci_session").then(|| value.to_owned())
        })
    }

    /// The body as text.
    pub(crate) fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The body decoded from JSON.
    #[track_caller]
    pub(crate) fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| {
            panic!(
                "the body is not the expected JSON ({error}): {}",
                self.text()
            )
        })
    }

    /// The body as loosely typed JSON, for checking the exact wire format.
    #[track_caller]
    pub(crate) fn value(&self) -> Value {
        self.json()
    }

    /// The answer without the two members that name the request (`instance`,
    /// `requestId`): the status and what is left of the body, `null` for a body that
    /// is no JSON. Two answers that are equal by it tell their asker the same.
    pub(crate) fn problem(&self) -> (StatusCode, Value) {
        let mut body: Value = serde_json::from_slice(&self.body).unwrap_or(Value::Null);
        if let Some(object) = body.as_object_mut() {
            object.remove("instance");
            object.remove("requestId");
        }
        (self.status, body)
    }

    /// The detail of an error answer: English text for developers.
    #[track_caller]
    pub(crate) fn error_message(&self) -> String {
        self.error_member("detail")
    }

    /// The type of an error answer, such as `/problems/not-found`.
    #[track_caller]
    pub(crate) fn problem_type(&self) -> String {
        self.error_member("type")
    }

    /// Where in the request the one thing is that an error answer refused: the JSON
    /// Pointer of a member of the body as it is, `parameter <name>` or `header <name>`.
    /// `None` if the answer has no `errors`.
    ///
    /// # Panics
    ///
    /// Panics if `errors` has more than one entry, or the entry not exactly one place.
    #[track_caller]
    pub(crate) fn refused_place(&self) -> Option<String> {
        let mut places = self.refused_places();
        assert!(places.len() <= 1, "one thing is refused: {}", self.text());
        places.pop()
    }

    /// As [`ApiResponse::refused_place`], for an answer that refuses something.
    #[track_caller]
    pub(crate) fn refused_at(&self) -> String {
        self.refused_place()
            .unwrap_or_else(|| panic!("the answer names nothing it refused: {}", self.text()))
    }

    /// Where each thing is that an error answer refused, in the order of `errors`, as
    /// [`ApiResponse::refused_place`] writes it.
    #[track_caller]
    pub(crate) fn refused_places(&self) -> Vec<String> {
        let body = self.value();
        let Some(errors) = body.get("errors") else {
            return Vec::new();
        };
        let errors = errors.as_array().expect("errors is a list");
        assert!(!errors.is_empty(), "errors is left out when it is empty");
        errors
            .iter()
            .map(|entry| {
                for member in ["detail", "reason"] {
                    assert!(
                        entry[member].as_str().is_some_and(|text| !text.is_empty()),
                        "an entry has its {member}: {entry}"
                    );
                }
                let places: Vec<String> = [
                    ("pointer", ""),
                    ("parameter", "parameter "),
                    ("header", "header "),
                ]
                .into_iter()
                .filter_map(|(member, prefix)| {
                    entry.get(member).map(|place| {
                        format!("{prefix}{}", place.as_str().expect("a place is text"))
                    })
                })
                .collect();
                assert_eq!(places.len(), 1, "an entry is in one place: {entry}");
                places.into_iter().next().unwrap_or_default()
            })
            .collect()
    }

    /// The stable reason of an error answer, such as `last_admin`; empty if none.
    #[track_caller]
    pub(crate) fn error_reason(&self) -> String {
        self.error_member("reason")
    }

    #[track_caller]
    fn error_member(&self, member: &str) -> String {
        self.value()[member].as_str().unwrap_or_default().to_owned()
    }

    /// Checks that the answer is the error with this status and detail, in the one
    /// format of the API's errors ([`ApiResponse::check_problem`]).
    #[track_caller]
    pub(crate) fn check_error(&self, status: StatusCode, message: &str) {
        self.check_status(status);
        let body = self.check_problem();
        assert_eq!(body["detail"], message, "the detail: {body}");
    }

    /// Checks that the answer is a problem of RFC 9457 as the API sends it, and returns
    /// it: `application/problem+json`, the status repeated in the body, the `type` of
    /// the `reason` or else of the status, its title, and the identifier of the
    /// request, which `instance` and the response header repeat.
    #[track_caller]
    pub(crate) fn check_problem(&self) -> Value {
        assert_eq!(
            self.header("content-type"),
            "application/problem+json",
            "the content type of an error: {}",
            self.text()
        );
        let body = self.value();
        assert_eq!(body["status"], self.status.as_u16(), "the status: {body}");
        let of_status = match self.status.as_u16() {
            400 => "bad-request",
            401 => "unauthorized",
            403 => "forbidden",
            404 => "not-found",
            405 => "method-not-allowed",
            409 => "conflict",
            412 => "precondition-failed",
            413 => "payload-too-large",
            416 => "range-not-satisfiable",
            429 => "rate-limited",
            500 => "internal",
            503 => "service-unavailable",
            other => panic!("status {other} is no error status of the API"),
        };
        let name = match body.get("reason") {
            Some(reason) => reason.as_str().expect("a reason is text").replace('_', "-"),
            None => of_status.to_owned(),
        };
        // Under the path the server is served at, which a test may have set: what
        // comes before the API's prefix, or the pages of the problems, in the path asked.
        let base_path = ["/api/v1", "/problems"]
            .iter()
            .find_map(|marker| self.path.find(marker))
            .map_or("", |at| &self.path[..at]);
        assert_eq!(
            body["type"],
            format!("{base_path}/problems/{name}"),
            "the type is of the reason, or else of the status: {body}"
        );
        let kind = fvoci_server::problems::find(&name);
        assert!(kind.is_some(), "a kind of problem the server lists: {body}");
        assert_eq!(
            body["title"],
            kind.map_or_default(|kind| kind.title),
            "the title of the type: {body}"
        );
        assert!(body["detail"].is_string(), "the detail: {body}");
        let request_id = body["requestId"].as_str().unwrap_or_default();
        assert!(!request_id.is_empty(), "the request identifier: {body}");
        assert_eq!(
            body["instance"],
            format!("urn:uuid:{request_id}"),
            "the instance is the request: {body}"
        );
        assert_eq!(
            self.header("x-request-id"),
            request_id,
            "the header repeats the identifier"
        );
        for old in ["error", "message", "field", "request_id", "retry_after"] {
            assert!(body.get(old).is_none(), "no member {old}: {body}");
        }
        self.refused_places();
        body
    }

    /// Checks that the answer is the server error, which never says what went wrong.
    #[track_caller]
    pub(crate) fn check_internal_error(&self) {
        self.check_error(StatusCode::INTERNAL_SERVER_ERROR, "internal server error");
    }

    /// The value of a response header, or the empty string.
    pub(crate) fn header(&self, name: &str) -> String {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    #[track_caller]
    pub(crate) fn check_status(&self, expected: StatusCode) {
        assert_eq!(
            self.status,
            expected,
            "unexpected status; body: {}",
            self.text()
        );
        // Every error the API answers with is a problem, whatever the test is about.
        if expected.is_client_error() || expected.is_server_error() {
            self.check_problem();
        }
    }

    /// `CheckOK`.
    #[track_caller]
    pub(crate) fn check_ok(&self) {
        self.check_status(StatusCode::OK);
    }

    /// Checks for status 201, the answer of an operation that made something new.
    #[track_caller]
    pub(crate) fn check_created(&self) {
        self.check_status(StatusCode::CREATED);
    }

    /// Checks for status 204, the answer of an operation without a result, and for what
    /// belongs to it: no body, and no `Content-Type` for the body that is not there.
    #[track_caller]
    pub(crate) fn check_no_content(&self) {
        self.check_status(StatusCode::NO_CONTENT);
        assert!(self.body.is_empty(), "a 204 has no body: {:?}", self.text());
        assert!(
            !self.headers.contains_key(reqwest::header::CONTENT_TYPE),
            "a 204 names no content type: {:?}",
            self.header("content-type")
        );
    }

    /// `CheckBadRequest`.
    #[track_caller]
    pub(crate) fn check_bad_request(&self) {
        self.check_status(StatusCode::BAD_REQUEST);
    }

    /// `CheckUnauthorized`.
    #[track_caller]
    pub(crate) fn check_unauthorized(&self) {
        self.check_status(StatusCode::UNAUTHORIZED);
    }

    /// `CheckForbidden`.
    #[track_caller]
    pub(crate) fn check_forbidden(&self) {
        self.check_status(StatusCode::FORBIDDEN);
    }

    /// `CheckNotFound`.
    #[track_caller]
    pub(crate) fn check_not_found(&self) {
        self.check_status(StatusCode::NOT_FOUND);
    }

    /// `CheckRequestEntityTooLarge`.
    #[track_caller]
    pub(crate) fn check_request_entity_too_large(&self) {
        self.check_status(StatusCode::PAYLOAD_TOO_LARGE);
    }

    /// `CheckNotImplemented`.
    #[track_caller]
    pub(crate) fn check_not_implemented(&self) {
        self.check_status(StatusCode::NOT_IMPLEMENTED);
    }

    /// Checks for status 200 and decodes the body.
    #[track_caller]
    pub(crate) fn ok_json<T: DeserializeOwned>(&self) -> T {
        self.check_ok();
        self.json()
    }

    /// Checks for status 201 and decodes the body: what the operation made.
    #[track_caller]
    pub(crate) fn created_json<T: DeserializeOwned>(&self) -> T {
        self.check_created();
        self.json()
    }
}
