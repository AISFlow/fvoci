//! The server configuration: a JSON file, `FVOCI_*` environment variables, and a few
//! command-line overrides.
//!
//! A key of the file matches a setting only when it is written exactly as the setting
//! is named. Keys that no setting reads are named, without their values, in a warning
//! when the server starts ([`Configuration::check`]).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{Context as _, bail};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::{Map, Value};

/// The prefix of the environment variables that override configuration keys.
pub const ENV_PREFIX: &str = "FVOCI_";

/// The default of `max_request_bytes`: 12 MiB. The largest block the model accepts, a
/// title of 16,383 characters and fields of 800,000, takes up to about 3.3 MB as UTF-8,
/// and a card's body up to 8 MiB with it, so every single valid block fits. Requests of
/// the web app that write no body measured below 1 kB.
pub const DEFAULT_MAX_REQUEST_BYTES: u64 = 12 * 1024 * 1024;

pub mod collab;
mod environment;
mod keys;
pub mod mail;
mod oidc_trust;
mod scrapes;
pub mod search;
mod secret_key;
mod security;
mod settings;
pub mod smtp;
mod trusted_proxies;
mod upload_limits;
pub mod wiki;

use environment::{ENV_KEYS, EnvKind, env_value};
pub use scrapes::{MetricsAddress, MetricsHost, MetricsToken};
pub use secret_key::{ENV_ONLY, SECRET_KEY_ENV};
pub use security::SecurityContact;
pub use settings::{OrgCreation, ServerUrl, Settings};
pub use trusted_proxies::TrustedProxies;

use crate::auth::sealed::SecretKey;

/// The keys of an object of the configuration file that no setting reads, by name.
/// Their values are not kept: one may be a secret.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UnreadKeys(Vec<String>);

impl<'de> Deserialize<'de> for UnreadKeys {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(UnreadKeysVisitor)
    }
}

/// Reads the keys of a map in order, and passes over their values.
struct UnreadKeysVisitor;

impl<'de> serde::de::Visitor<'de> for UnreadKeysVisitor {
    type Value = UnreadKeys;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the keys of an object")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<UnreadKeys, A::Error> {
        let mut keys = Vec::new();
        while let Some((key, IgnoredAny)) = map.next_entry::<String, IgnoredAny>()? {
            keys.push(key);
        }
        keys.sort_unstable();
        Ok(UnreadKeys(keys))
    }
}

/// The settings of the S3 file backend, the object `file_storage_s3`.
#[derive(Clone, Default, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
pub struct S3Config {
    /// The access key; empty to use the environment's credentials.
    pub access_key_id: String,
    /// The secret of the access key; empty with an empty access key.
    pub secret_access_key: String,
    /// The bucket files are stored in.
    pub bucket: String,
    /// A prefix put before every object key.
    pub key_prefix: String,
    /// The region of the bucket.
    pub region: String,
    /// The endpoint host, with port if needed; empty for AWS.
    pub endpoint: String,
    /// Whether to talk to the endpoint over TLS.
    pub https: bool,
    /// Whether to ask for server-side encryption of stored objects.
    pub server_side_encryption: bool,
    /// The request timeout in milliseconds; zero for the default.
    pub timeout_ms: i64,
    /// The keys of the object that no setting reads. Last: it takes what the others
    /// leave.
    #[serde(flatten)]
    #[cfg_attr(test, serde(skip_serializing))]
    pub unread: UnreadKeys,
}

impl std::fmt::Debug for S3Config {
    /// Leaves out the secret key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("S3Config")
            .field("access_key_id", &self.access_key_id)
            .field("bucket", &self.bucket)
            .field("key_prefix", &self.key_prefix)
            .field("region", &self.region)
            .field("endpoint", &self.endpoint)
            .field("https", &self.https)
            .field("server_side_encryption", &self.server_side_encryption)
            .field("timeout_ms", &self.timeout_ms)
            .finish_non_exhaustive()
    }
}

/// The server configuration.
///
/// The name of each setting is its key in the configuration file. `docs/configuration.md`
/// documents every key, and a test holds the two together.
#[derive(Clone, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "each flag is a separate key of the configuration file"
)]
pub struct Configuration {
    /// The public URL of the server. Its path is the base path the web app is served under.
    pub server_url: String,
    /// The port to listen on.
    pub port: u16,
    /// The database kind: `sqlite` or `postgres`.
    pub database_kind: String,
    /// The database connection: the path of an SQLite file, or a PostgreSQL URL.
    pub database_connection: String,
    /// Maximum connections in the server's database pool: 20 unless set. At least 1.
    pub database_max_connections: u32,
    /// How many times to try to reach the database at startup.
    pub database_connect_attempts: u32,
    /// How long a server that starts waits for the lock of the server of its database,
    /// which another process holds, before it refuses to start, in seconds.
    pub server_lock_wait_seconds: u64,
    /// Whether to serve HTTPS, with `./cert/cert.pem` and `./cert/key.pem`.
    pub serve_https: bool,
    /// The directory holding the web app: `index.html` and `static/`.
    pub web_app_path: String,
    /// The file backend: `local` or `s3`.
    pub file_storage: String,
    /// The settings of the `s3` file backend.
    pub file_storage_s3: S3Config,
    /// The directory of the `local` file backend.
    pub file_storage_path: String,
    /// The largest accepted upload in bytes: 100 MiB unless set. At least 1, and at most
    /// what an upload in parts carries (`upload_part_bytes` × 10,000).
    pub max_file_bytes: i64,
    /// Cumulative stored file and retained file-version byte ceiling for one organization:
    /// 50 GiB unless set. At least `max_file_bytes`.
    pub max_org_storage_bytes: u64,
    /// Where the Prometheus metrics are served, as `host:port`; empty for none. A
    /// listener of their own, apart from the API's ([`Configuration::metrics_listen_address`]).
    pub metrics_address: String,
    /// The bearer token a scrape of the metrics must send; empty for none. Never
    /// written by `Debug` ([`Configuration::metrics_scrape_token`]).
    pub metrics_token: String,
    /// URLs that receive a POST with every changed block.
    pub block_change_webhooks: Vec<String>,
    /// Further origins whose pages may send requests that change something and open
    /// websocket connections, next to the origin of `server_url`. For development,
    /// where the web app is served by a tool of its own on another port.
    pub allowed_origins: Vec<String>,
    /// How many days a session may go unused before it ends.
    pub session_idle_days: u32,
    /// How many days after signing in a session ends, however much it is used.
    pub session_max_days: u32,
    /// Whether to listen on the loopback interface only.
    pub listen_loopback_only: bool,
    /// Whether to serve the administration API on a Unix socket.
    pub serve_admin_socket: bool,
    /// The path of the administration socket.
    pub admin_socket_path: String,
    /// Whether boards and wikis may be shared publicly through a read token.
    pub allow_public_links: bool,
    /// Feature flags handed to the web app.
    pub feature_flags: BTreeMap<String, String>,
    /// How the web app displays other users' names.
    pub name_display: String,
    /// Whether user lists include email addresses.
    pub show_email_addresses: bool,
    /// Whether the providers of the organizations may be reached at addresses that are
    /// not public (loopback, private networks): for an installation whose provider is
    /// on its own network. Off, such a provider is neither stored nor signed in
    /// through.
    pub oidc_allow_private_addresses: bool,
    /// A PEM file of additional certificate authorities trusted by the OIDC client.
    /// Empty to use only the system roots. Loaded when the client starts.
    pub oidc_extra_ca_file: String,
    /// How many days the provider an organization moves away from (another issuer) is
    /// kept, for its people to carry their accounts over to the new one. Zero keeps
    /// none. An organization's limit: read through `App::limits`.
    pub oidc_former_provider_days: u32,
    /// The key the server seals what it keeps secret in the database with: the client
    /// secrets of the providers. Read from `FVOCI_SECRET_KEY` only, never from the
    /// file, and never written by `Debug`. Without it no provider can be stored.
    #[serde(skip)]
    pub secret_key: Option<SecretKey>,
    /// Certificates (DER) that the client of the providers trusts besides the
    /// system's. Not a configuration file key: the tests set it for a provider of
    /// their own, as they set `skip_template_init`.
    #[serde(skip)]
    pub oidc_extra_root_certificates: Vec<Vec<u8>>,
    /// Whether the server holds one organization (`first_user_only`) or takes in
    /// several (`open`). `open` needs PostgreSQL, and a database role that the wall
    /// between organizations holds.
    pub org_creation: String,
    /// How often the server pings each websocket, in seconds: 1 to
    /// [`MAX_WEBSOCKET_PING_SECONDS`]. Keeps proxies and CDNs from closing idle
    /// connections, and a page whose timers are stopped within the quiet limit of a
    /// connection.
    pub websocket_ping_seconds: u64,
    /// How many connections may read one shared board by its public link at once:
    /// following its blocks by the link's read token, or visiting it with that token.
    /// One more is not taken, and is not counted among the board's anonymous readers.
    pub public_link_connections_per_container: u32,
    /// How many of a board's connections by its public link may come from one address
    /// (an IPv6 address counted by its /64), within `public_link_connections_per_container`.
    pub public_link_connections_per_address: u32,
    /// Whether the JSON answers of the API are compressed (gzip, brotli or zstd, as the
    /// client's `Accept-Encoding` asks). On unless set: a proxy in front that compresses
    /// itself can have it off.
    pub compress_responses: bool,
    /// The contact URI published in `/.well-known/security.txt` (`mailto:`, `https://`,
    /// or `tel:` per RFC 9116); empty disables the endpoint (404).
    pub security_contact: String,
    /// The largest body, in bytes, of a request to an API route that takes JSON. A
    /// larger one is refused with 413. It does not apply to file uploads, which are
    /// limited to one part (`upload_part_bytes`) as they are read, nor to the parts of
    /// uploads, limited to their size, nor to archive imports, which have no limit on
    /// the request yet.
    pub max_request_bytes: u64,
    /// How many days a deleted board or block stays in the trash, where it can be
    /// restored from, before it is removed for good.
    pub trash_retention_days: u32,
    /// The size in bytes of every part of an upload in parts but the last. At least 5 MiB,
    /// the smallest part an S3 multipart upload takes.
    pub upload_part_bytes: u64,
    /// The directory of the server's temporary files: the parts of uploads that are not
    /// complete yet. Empty for `.tmp` under `file_storage_path`. Not the system's temporary
    /// directory, which may be held in memory.
    pub tmp_path: String,
    /// The most bytes all open uploads in parts of the server may hold back at once:
    /// the sum of their declared sizes.
    pub max_pending_upload_bytes: u64,
    /// The largest archive an import takes, in bytes: 256 MiB unless set. At least 1,
    /// and at most what an upload in parts carries (`upload_part_bytes` × 10,000).
    pub max_import_bytes: u64,
    /// How many earlier versions of a comment's body the server keeps: the first one,
    /// with which the comment was posted, and the newest. At least 1.
    pub max_comment_versions: u32,
    /// How many versions of a file the server keeps, the newest: the bytes the file has
    /// and those it had before it was written anew. At least 1.
    pub max_file_versions: u32,
    /// How many exports (`exportContainerArchive`, `exportWorkspaceArchive`) the server runs at
    /// once; a user runs one at a time. An export runs until its archive is sent. At
    /// least 1.
    pub max_concurrent_exports: u32,
    /// How long, in seconds, an export's download may send nothing before it is given
    /// up: its place among the exports and its temporary file are given back, and the
    /// connection is broken. At least 1.
    pub export_stall_seconds: u64,
    /// How many imports (`importArchive`) the server runs at once; a user runs one at a
    /// time. At least 1.
    pub max_concurrent_imports: u32,
    /// The addresses, and address ranges such as `10.0.0.0/8`, of the reverse proxies
    /// in front of the server. A request from one of them is taken to come from the
    /// address the proxy names in `X-Forwarded-For`; that header is believed from
    /// nobody else. Empty if clients reach the server directly.
    pub trusted_proxies: Vec<String>,
    /// How many failed logins in a row an address has for free at one account before it
    /// is made to wait.
    pub login_pair_free_failures: u32,
    /// The wait, in seconds, after the first failed login that is not free. It doubles
    /// with each further failure.
    pub login_pair_base_delay_seconds: u64,
    /// The longest wait of an address at one account, in seconds.
    pub login_pair_max_delay_seconds: u64,
    /// How many failed logins in a row, from anywhere, lock an account.
    pub login_account_max_failures: u32,
    /// How long an account stays locked, in seconds.
    pub login_account_lock_seconds: u64,
    /// How many logins and registrations one address may attempt per minute, one at a
    /// time. Zero for no limit by address, as for a server whose users all come from
    /// one address and no trusted proxy tells them apart.
    pub login_address_attempts_per_minute: u32,
    /// How many passwords are hashed or checked at a time. One hash takes 19 MiB and
    /// some milliseconds of a processor.
    pub password_hash_concurrency: u32,
    /// How many requests may wait for their turn to have a password hashed.
    pub password_hash_queue: u32,
    /// How long a request waits for that turn before it is told to try again, in
    /// seconds.
    pub password_hash_queue_seconds: u64,
    /// The keys of editing a body together and its version limits.
    #[serde(flatten)]
    pub collab: collab::CollabSettings,
    /// SMTP transport settings and the limits of all mail; an empty host disables
    /// outgoing mail unless `mail_transport` names a provider.
    #[serde(flatten)]
    pub smtp: smtp::SmtpSettings,
    /// The way mail leaves (`mail_transport`) and the keys of the providers' APIs.
    #[serde(flatten)]
    pub mail: mail::MailSettings,
    /// The keys `search_*` (the search server of the bodies).
    #[serde(flatten)]
    pub search: search::SearchSettings,
    /// The keys `wiki_*` (the limits of a wiki's tree).
    #[serde(flatten)]
    pub wiki: wiki::WikiSettings,
    /// Whether to skip importing the built-in templates at startup. Not a configuration
    /// file key; the test harness sets it so a server that does not need templates
    /// starts quickly.
    #[serde(skip)]
    pub skip_template_init: bool,
    /// Whether the websockets go unpinged, whatever `websocket_ping_seconds` says. Not a
    /// configuration file key; the test harness sets it where pings would only be noise,
    /// as it sets `skip_template_init`.
    #[serde(skip)]
    pub skip_websocket_pings: bool,
    /// Whether whom to tell of a change is worked off only when it is called
    /// (`app::tend_notifications`), and no task does it every second. Not a
    /// configuration file key: the tests set it, to say when people are told, as they
    /// set `upkeep_by_hand` of the search.
    #[serde(skip)]
    pub notifications_by_hand: bool,
    /// Whether `block_change_webhooks` may reach loopback/private addresses over plain
    /// HTTP. Not a configuration file key: integration tests set it for local mock
    /// webhook receivers.
    #[serde(skip)]
    pub webhook_allow_private_addresses: bool,
    /// The keys of the file that no setting reads. Last: it takes what the others,
    /// the flattened ones among them, leave.
    #[serde(flatten)]
    #[cfg_attr(test, serde(skip_serializing))]
    pub unread: UnreadKeys,
}

impl Default for Configuration {
    fn default() -> Self {
        Self {
            server_url: "http://localhost:8000".to_owned(),
            port: 8000,
            database_kind: "sqlite".to_owned(),
            database_connection: "./fvoci.db".to_owned(),
            database_max_connections: fvoci_store::DEFAULT_MAX_CONNECTIONS,
            database_connect_attempts: 5,
            server_lock_wait_seconds: 30,
            serve_https: false,
            web_app_path: "./pack".to_owned(),
            file_storage: "local".to_owned(),
            file_storage_s3: S3Config::default(),
            file_storage_path: "./files".to_owned(),
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_org_storage_bytes: DEFAULT_MAX_ORG_STORAGE_BYTES,
            metrics_address: String::new(),
            metrics_token: String::new(),
            block_change_webhooks: Vec::new(),
            allowed_origins: Vec::new(),
            session_idle_days: 14,
            session_max_days: 90,
            listen_loopback_only: false,
            serve_admin_socket: false,
            admin_socket_path: "/var/tmp/fvoci_local.socket".to_owned(),
            allow_public_links: false,
            feature_flags: BTreeMap::new(),
            name_display: "username".to_owned(),
            show_email_addresses: false,
            oidc_allow_private_addresses: false,
            oidc_extra_ca_file: String::new(),
            oidc_former_provider_days: 30,
            secret_key: None,
            oidc_extra_root_certificates: Vec::new(),
            org_creation: ORG_CREATION_FIRST_USER_ONLY.to_owned(),
            websocket_ping_seconds: 30,
            public_link_connections_per_container: 200,
            public_link_connections_per_address: 10,
            compress_responses: true,
            security_contact: String::new(),
            max_request_bytes: DEFAULT_MAX_REQUEST_BYTES,
            trash_retention_days: 30,
            upload_part_bytes: DEFAULT_UPLOAD_PART_BYTES,
            tmp_path: String::new(),
            max_pending_upload_bytes: DEFAULT_MAX_PENDING_UPLOAD_BYTES,
            max_import_bytes: DEFAULT_MAX_IMPORT_BYTES,
            max_comment_versions: DEFAULT_MAX_COMMENT_VERSIONS,
            max_file_versions: fvoci_store::DEFAULT_FILE_VERSIONS_KEPT,
            max_concurrent_exports: DEFAULT_MAX_CONCURRENT_ARCHIVES,
            export_stall_seconds: 60,
            max_concurrent_imports: DEFAULT_MAX_CONCURRENT_ARCHIVES,
            trusted_proxies: Vec::new(),
            login_pair_free_failures: 5,
            login_pair_base_delay_seconds: 30,
            login_pair_max_delay_seconds: 60 * 60,
            login_account_max_failures: 100,
            login_account_lock_seconds: 60 * 60,
            login_address_attempts_per_minute: 20,
            password_hash_concurrency: 4,
            password_hash_queue: 16,
            password_hash_queue_seconds: 5,
            collab: collab::CollabSettings::default(),
            smtp: smtp::SmtpSettings::default(),
            mail: mail::MailSettings::default(),
            search: search::SearchSettings::default(),
            wiki: wiki::WikiSettings::default(),
            skip_template_init: false,
            skip_websocket_pings: false,
            notifications_by_hand: false,
            webhook_allow_private_addresses: false,
            unread: UnreadKeys::default(),
        }
    }
}

impl std::fmt::Debug for Configuration {
    /// Leaves out the database connection, which may hold a password.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Configuration")
            .field("smtp", &self.smtp)
            .field("mail", &self.mail)
            .field("search", &self.search)
            .field("wiki", &self.wiki)
            .field("server_url", &self.server_url)
            .field("port", &self.port)
            .field("database_kind", &self.database_kind)
            .field("database_max_connections", &self.database_max_connections)
            .field("serve_https", &self.serve_https)
            .field("web_app_path", &self.web_app_path)
            .field("file_storage", &self.file_storage)
            .field("file_storage_s3", &self.file_storage_s3)
            .field("file_storage_path", &self.file_storage_path)
            .field("max_file_bytes", &self.max_file_bytes)
            .field("max_org_storage_bytes", &self.max_org_storage_bytes)
            .field("block_change_webhooks", &self.block_change_webhooks)
            .field("allowed_origins", &self.allowed_origins)
            .field("session_idle_days", &self.session_idle_days)
            .field("session_max_days", &self.session_max_days)
            .field("listen_loopback_only", &self.listen_loopback_only)
            .field("serve_admin_socket", &self.serve_admin_socket)
            .field("admin_socket_path", &self.admin_socket_path)
            .field("allow_public_links", &self.allow_public_links)
            .field("feature_flags", &self.feature_flags)
            .field("name_display", &self.name_display)
            .field("show_email_addresses", &self.show_email_addresses)
            .field(
                "oidc_allow_private_addresses",
                &self.oidc_allow_private_addresses,
            )
            .field("oidc_extra_ca_file", &self.oidc_extra_ca_file)
            .field("oidc_former_provider_days", &self.oidc_former_provider_days)
            .field("secret_key", &self.secret_key)
            .field("org_creation", &self.org_creation)
            .field("server_lock_wait_seconds", &self.server_lock_wait_seconds)
            .field("websocket_ping_seconds", &self.websocket_ping_seconds)
            .field(
                "public_link_connections_per_container",
                &self.public_link_connections_per_container,
            )
            .field(
                "public_link_connections_per_address",
                &self.public_link_connections_per_address,
            )
            .field("compress_responses", &self.compress_responses)
            .field("security_contact", &self.security_contact)
            .field("max_request_bytes", &self.max_request_bytes)
            .field("trash_retention_days", &self.trash_retention_days)
            .field("upload_part_bytes", &self.upload_part_bytes)
            .field("tmp_path", &self.tmp_path)
            .field("max_pending_upload_bytes", &self.max_pending_upload_bytes)
            .field("max_import_bytes", &self.max_import_bytes)
            .field("max_comment_versions", &self.max_comment_versions)
            .field("max_file_versions", &self.max_file_versions)
            .field("max_concurrent_exports", &self.max_concurrent_exports)
            .field("export_stall_seconds", &self.export_stall_seconds)
            .field("max_concurrent_imports", &self.max_concurrent_imports)
            .field("trusted_proxies", &self.trusted_proxies)
            .field("login_pair_free_failures", &self.login_pair_free_failures)
            .field(
                "login_pair_base_delay_seconds",
                &self.login_pair_base_delay_seconds,
            )
            .field(
                "login_pair_max_delay_seconds",
                &self.login_pair_max_delay_seconds,
            )
            .field(
                "login_account_max_failures",
                &self.login_account_max_failures,
            )
            .field(
                "login_account_lock_seconds",
                &self.login_account_lock_seconds,
            )
            .field(
                "login_address_attempts_per_minute",
                &self.login_address_attempts_per_minute,
            )
            .field("password_hash_concurrency", &self.password_hash_concurrency)
            .field("password_hash_queue", &self.password_hash_queue)
            .field(
                "password_hash_queue_seconds",
                &self.password_hash_queue_seconds,
            )
            .field("collab", &self.collab)
            .field("unread", &self.unread)
            .finish_non_exhaustive()
    }
}

const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1000;

/// The largest file, unless the configuration says otherwise: 100 MiB.
const DEFAULT_MAX_FILE_BYTES: i64 = 100 * 1024 * 1024;

/// Cumulative stored file byte ceiling for one organization unless configured: 50 GiB.
const DEFAULT_MAX_ORG_STORAGE_BYTES: u64 = 50 * 1024 * 1024 * 1024;

/// What all open uploads may hold back, unless the configuration says otherwise: 2 GiB.
const DEFAULT_MAX_PENDING_UPLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The largest archive an import takes unless configured.
const DEFAULT_MAX_IMPORT_BYTES: u64 = 256 * 1024 * 1024;

/// How many earlier versions of a comment the server keeps unless configured.
const DEFAULT_MAX_COMMENT_VERSIONS: u32 = fvoci_store::DEFAULT_COMMENT_VERSIONS_KEPT;

/// How many exports, and how many imports, the server runs at once unless configured.
const DEFAULT_MAX_CONCURRENT_ARCHIVES: u32 = 2;

/// The size of the parts of an upload, unless the configuration says otherwise: 8 MiB. A
/// file of 100 MiB comes in 13 parts.
const DEFAULT_UPLOAD_PART_BYTES: u64 = 8 * 1024 * 1024;

/// The smallest part an S3 multipart upload takes, and so the smallest part size.
pub const MIN_UPLOAD_PART_BYTES: u64 = 5 * 1024 * 1024;

/// The largest part an S3 multipart upload takes, and so the largest part size.
pub const MAX_UPLOAD_PART_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// The longest period of the websockets' pings, in seconds: a Ping must come back
/// within the quiet limit of a connection (90 seconds, `contracts/websocket.md` 3) with
/// a margin for a slow network (30 seconds).
pub const MAX_WEBSOCKET_PING_SECONDS: u64 = 59;

/// The object of the file whose keys are settings too, each named `<object>.<key>`.
const NESTED: &str = "file_storage_s3";

/// The environment variable of a key: [`ENV_PREFIX`] and the key in capitals, a dot as
/// `_`.
fn env_name(key: &str) -> String {
    format!("{ENV_PREFIX}{}", key.to_uppercase().replace('.', "_"))
}

/// Sets a key in the file's keys, a key of `file_storage_s3` named
/// `file_storage_s3.<key>` (which [`Configuration::parse`] has made an object).
fn set_value(keys: &mut Map<String, Value>, key: &str, value: Value) {
    match key.split_once('.') {
        Some((outer, inner)) => {
            if let Some(Value::Object(outer)) = keys.get_mut(outer) {
                outer.insert(inner.to_owned(), value);
            }
        }
        None => {
            keys.insert(key.to_owned(), value);
        }
    }
}

/// Whether a configuration value says nothing: what an empty template leaves.
fn is_unset(value: &Value) -> bool {
    match value {
        Value::Null | Value::Bool(false) => true,
        Value::String(text) => text.is_empty(),
        Value::Number(number) => number.as_i64() == Some(0),
        Value::Array(items) => items.is_empty(),
        Value::Bool(true) | Value::Object(_) => false,
    }
}

/// `org_creation`: the server has one organization.
pub const ORG_CREATION_FIRST_USER_ONLY: &str = "first_user_only";
/// `org_creation`: the server takes in several organizations.
pub const ORG_CREATION_OPEN: &str = "open";

impl Configuration {
    /// Whether the server sends mail: `mail_transport` names a way, or `smtp_host` is
    /// set.
    pub fn mail_enabled(&self) -> bool {
        self.mail.enabled(&self.smtp)
    }

    /// Reads the configuration file at `path` and applies environment overrides from the
    /// process environment.
    ///
    /// # Errors
    ///
    /// Returns an error if the file cannot be read, is not a JSON object, holds a value
    /// of the wrong type, or an environment override cannot be parsed.
    pub fn read(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the configuration file {}", path.display()))?;
        Self::parse(&text, |name| std::env::var(name).ok())
            .with_context(|| format!("in the configuration file {}", path.display()))
    }

    /// Decodes configuration JSON, with environment variables looked up through `env`.
    ///
    /// # Errors
    ///
    /// Returns an error if `text` is not a JSON object, gives a key twice, holds a value
    /// of the wrong type, or an environment override cannot be parsed.
    pub fn parse(text: &str, env: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let Value::Object(mut keys) =
            serde_json::from_str(text).context("the file is not valid JSON")?
        else {
            bail!("the configuration must be a JSON object");
        };
        // Which of two values of a key would win is the order of the text: a key given
        // twice is refused, by its name alone (its value may be a secret, such as the
        // token of the metrics).
        keys::check_each_once(text)?;
        match keys.get(NESTED) {
            Some(Value::Object(_)) => {}
            None | Some(Value::Null) => {
                keys.insert(NESTED.to_owned(), Value::Object(Map::new()));
            }
            Some(_) => bail!("{NESTED} must be a JSON object"),
        }

        for (key, kind) in ENV_KEYS
            .into_iter()
            .chain(collab::ENV_KEYS)
            .chain(smtp::ENV_KEYS)
            .chain(mail::ENV_KEYS)
            .chain(search::ENV_KEYS)
            .chain(wiki::ENV_KEYS)
        {
            let name = env_name(key);
            if let Some(text) = env(&name)
                && !scrapes::is_unset_token(key, &text)
            {
                set_value(&mut keys, key, env_value(&name, &text, kind)?);
            }
        }

        scrapes::check_token_is_text(&keys)?;
        smtp::check_password_is_text(&keys)?;
        mail::check_secrets_are_text(&keys)?;
        search::check_secrets_are_text(&keys)?;
        let secret_key = secret_key::read(&keys, &env, is_unset)?;
        let mut config: Self = serde_json::from_value(Value::Object(keys))
            .context("a key has a value of the wrong type")?;
        config.secret_key = secret_key;
        Ok(config)
    }

    /// The warning of the keys of the file that no setting reads, if it has any: their
    /// names, each quoted as a string is (a key may hold a line break), never their
    /// values, which may be secrets.
    #[must_use]
    pub fn unread_keys_warning(&self) -> Option<String> {
        let unread = self.unread_keys();
        (!unread.is_empty()).then(|| {
            let names: Vec<String> = unread.iter().map(|key| format!("{key:?}")).collect();
            format!(
                "the configuration file has keys that no setting reads: {}",
                names.join(", ")
            )
        })
    }

    /// The keys of the file that no setting reads, by name: a key of `file_storage_s3`
    /// as `file_storage_s3.<key>`.
    #[must_use]
    pub fn unread_keys(&self) -> Vec<String> {
        let nested = self
            .file_storage_s3
            .unread
            .0
            .iter()
            .map(|key| format!("{NESTED}.{key}"));
        self.unread.0.iter().cloned().chain(nested).collect()
    }

    /// How long a session may go unused before it ends, in milliseconds.
    pub fn session_idle_millis(&self) -> i64 {
        i64::from(self.session_idle_days).saturating_mul(MILLIS_PER_DAY)
    }

    /// How long after signing in a session ends, in milliseconds.
    pub fn session_max_millis(&self) -> i64 {
        i64::from(self.session_max_days).saturating_mul(MILLIS_PER_DAY)
    }

    /// How long the provider an organization moves away from is kept.
    pub fn oidc_former_provider_kept(&self) -> std::time::Duration {
        std::time::Duration::from_hours(u64::from(self.oidc_former_provider_days) * 24)
    }

    /// How long a deleted board or block stays in the trash, in milliseconds.
    pub fn trash_retention_millis(&self) -> i64 {
        i64::from(self.trash_retention_days).saturating_mul(MILLIS_PER_DAY)
    }

    /// The directory of the server's temporary files: `tmp_path`, or `.tmp` under
    /// `file_storage_path`.
    pub fn tmp_dir(&self) -> std::path::PathBuf {
        if self.tmp_path.is_empty() {
            std::path::Path::new(&self.file_storage_path).join(".tmp")
        } else {
            std::path::PathBuf::from(&self.tmp_path)
        }
    }

    /// How long an export's download may send nothing before it is given up
    /// (`export_stall_seconds`).
    pub fn export_stall(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.export_stall_seconds)
    }

    /// How long a start waits for the lock of the server (`server_lock_wait_seconds`).
    pub fn server_lock_wait(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.server_lock_wait_seconds)
    }

    /// The contact URI for `/.well-known/security.txt`, or `None` if unset.
    ///
    /// # Errors
    ///
    /// Returns an error if `security_contact` is non-empty and not a valid `mailto:`,
    /// `https://`, or `tel:` URI (RFC 9116).
    pub fn security_contact(&self) -> anyhow::Result<Option<SecurityContact>> {
        SecurityContact::parse(&self.security_contact)
    }

    /// Checks that the configuration only asks for what this server supports, and reads
    /// the settings that are text into what they name.
    ///
    /// Returns the [`Settings`] and the warnings: keys of the file that no setting reads,
    /// and settings that are accepted but work against each other.
    ///
    /// # Errors
    ///
    /// Returns an error naming the first setting this server cannot honor: an unknown
    /// `org_creation` or `database_kind`, a metrics address that is no `host:port`, a
    /// `server_url` that is no URL, port 0, a limit of zero, a ping period out of its
    /// range, or an unknown file backend.
    pub fn check(self) -> anyhow::Result<(Settings, Vec<String>)> {
        let org_creation: OrgCreation = self.org_creation.parse()?;
        let database_kind: fvoci_store::DbType = self.database_kind.parse()?;
        // SQLite holds one organization, and nothing would keep several apart.
        if org_creation == OrgCreation::Open && database_kind == fvoci_store::DbType::Sqlite {
            bail!(
                "org_creation is {ORG_CREATION_OPEN:?}, which needs database_kind \"postgres\": \
                 an SQLite database holds one organization"
            );
        }
        // Zero would listen on any free port, not on one the operator can name.
        if self.port == 0 {
            bail!("port must be 1 to 65535, not 0");
        }
        if self.database_max_connections == 0 {
            bail!("database_max_connections must be at least 1");
        }
        // Zero does not mean "no limit": a server that takes bodies of any size is not
        // something to end up with by leaving a number out.
        if self.max_request_bytes == 0 {
            bail!("max_request_bytes must be a positive number of bytes, not 0");
        }
        if !(1..=MAX_WEBSOCKET_PING_SECONDS).contains(&self.websocket_ping_seconds) {
            bail!(
                "websocket_ping_seconds must be 1 to {MAX_WEBSOCKET_PING_SECONDS}, not {}",
                self.websocket_ping_seconds
            );
        }
        // The session cookie and the check of where a request comes from both follow
        // from this address.
        let origins = crate::auth::token::Origins::new(&self.server_url, &self.allowed_origins)
            .map_err(anyhow::Error::msg)?;
        let server_url: ServerUrl = self.server_url.parse()?;
        let trusted_proxies =
            TrustedProxies::parse(&self.trusted_proxies).map_err(anyhow::Error::msg)?;
        self.check_login_limits()?;
        // Zero would end every session as it starts.
        if self.session_idle_days == 0 || self.session_max_days == 0 {
            bail!("session_idle_days and session_max_days must be at least 1");
        }
        // Zero would empty the trash as it fills: deleting could not be undone.
        if self.trash_retention_days == 0 {
            bail!("trash_retention_days must be at least 1");
        }
        let largest = self.check_upload_limits()?;
        self.smtp.check()?;
        self.mail.check(&self.smtp)?;
        self.search.server()?;
        self.wiki.check()?;
        self.metrics_listen_address()?;
        self.check_metrics_token()?;
        self.security_contact()?;
        if !["local", "s3"].contains(&self.file_storage.as_str()) {
            bail!(
                "file_storage {:?} is not supported; use \"local\" or \"s3\"",
                self.file_storage
            );
        }

        let warnings = self.warnings();
        let read = settings::Read {
            org_creation,
            database_kind,
            server_url,
            origins,
            trusted_proxies,
            largest_file: largest.file,
            largest_import: largest.import,
        };
        Ok((Settings::new(self, read), warnings))
    }

    /// The warnings of a configuration that is accepted: keys of the file that no
    /// setting reads, and settings that work against each other.
    fn warnings(&self) -> Vec<String> {
        let mut warnings: Vec<String> = self.unread_keys_warning().into_iter().collect();
        // Reached over HTTPS without ending TLS itself: something in front of the server
        // does, and every request arrives from its address. Unless the server is told
        // which address that is, all clients are one client to the login limits (one
        // that guesses holds everybody up) and to the readers of a public link (one
        // address's share of a board is all a board takes).
        if self.server_url.starts_with("https://")
            && !self.serve_https
            && self.trusted_proxies.is_empty()
        {
            warnings.push(
                "server_url is https but the server does not end TLS itself (serve_https is \
                 off) and trusted_proxies is empty: behind a reverse proxy every client is \
                 seen at the proxy's address, and the login limits and the readers of a \
                 public link (public_link_connections_per_address) then count all clients \
                 as one; set trusted_proxies to the proxy's addresses"
                    .to_owned(),
            );
        }
        if !self.metrics_token.is_empty() && self.metrics_address.is_empty() {
            warnings.push(
                "metrics_token is set but metrics_address is empty: no metrics are served"
                    .to_owned(),
            );
        }
        if self.metrics_token.is_empty()
            && let Ok(Some(address)) = self.metrics_listen_address()
            && !address.is_loopback()
        {
            warnings.push(
                "metrics_address is not a loopback address and metrics_token is empty: \
                 anyone who reaches the metrics listener can read server metrics without \
                 authentication; set metrics_token or bind metrics_address to 127.0.0.1 / [::1]"
                    .to_owned(),
            );
        }
        let is_plain_http = self
            .server_url
            .trim()
            .get(..7)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"));
        if is_plain_http
            && self.allowed_origins.iter().any(|origin| {
                origin
                    .trim()
                    .get(..8)
                    .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
            })
        {
            warnings.push(
                "server_url is http:// while allowed_origins includes an https:// origin: \
                 session cookies will be issued without the Secure attribute or __Host- prefix; \
                 set server_url to https:// when browsers reach the server over HTTPS"
                    .to_owned(),
            );
        }
        warnings
    }

    /// Checks the settings of the login limits and of the queue of password work.
    fn check_login_limits(&self) -> anyhow::Result<()> {
        // A wait of nothing is no wait, an account that locks at no failure is always
        // locked, and nobody's password is checked by a queue without a place.
        for (name, value) in [
            (
                "login_pair_base_delay_seconds",
                self.login_pair_base_delay_seconds,
            ),
            (
                "login_pair_max_delay_seconds",
                self.login_pair_max_delay_seconds,
            ),
            (
                "login_account_lock_seconds",
                self.login_account_lock_seconds,
            ),
            (
                "login_account_max_failures",
                u64::from(self.login_account_max_failures),
            ),
            (
                "password_hash_concurrency",
                u64::from(self.password_hash_concurrency),
            ),
            (
                "password_hash_queue_seconds",
                self.password_hash_queue_seconds,
            ),
        ] {
            if value == 0 {
                bail!("{name} must be at least 1");
            }
        }
        // A wait is a time on the clock: one that is longer than this means "for ever",
        // and that is not what a number of seconds should be made to say.
        let longest = crate::auth::limiter::LONGEST_WAIT.as_secs();
        for (name, seconds) in [
            (
                "login_pair_base_delay_seconds",
                self.login_pair_base_delay_seconds,
            ),
            (
                "login_pair_max_delay_seconds",
                self.login_pair_max_delay_seconds,
            ),
            (
                "login_account_lock_seconds",
                self.login_account_lock_seconds,
            ),
        ] {
            if seconds > longest {
                bail!("{name} must not be more than {longest} (ten years)");
            }
        }
        if self.login_pair_max_delay_seconds < self.login_pair_base_delay_seconds {
            bail!(
                "login_pair_max_delay_seconds must not be less than login_pair_base_delay_seconds"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
