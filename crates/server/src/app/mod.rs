//! The application layer: what the server does, independent of HTTP.
//!
//! [`App`] holds the server's shared state. Its operations are spread over the modules of
//! this directory by area.

mod accept_language;
mod access;
mod account_deletion;
mod account_erasure;
mod api_tokens;
mod audience;
mod auth;
mod blocks;
mod board_ops;
mod board_parts;
mod body_markdown;
mod body_versions;
mod caller;
mod card_filters;
mod card_writes;
mod cards;
mod category;
mod category_containers;
mod changes;
mod comments;
mod container_copies;
mod container_members;
mod containers;
mod containers_and_blocks;
mod course_events;
mod course_schedules;
mod courses;
mod cross_workspace_copy;
mod diaries;
mod doomed;
mod download;
mod email_change;
mod export;
mod export_markdown;
mod favorites;
mod file_leases;
mod file_name;
mod file_versions;
mod files;
mod housekeeping;
mod import;
mod invitations;
mod language;
mod limits;
mod list_page;
mod loose_objects;
mod mail;
mod notification_triage;
mod notifications;
mod oidc;
mod oidc_account;
mod oidc_carry;
mod oidc_link;
mod oidc_pending;
mod oidc_purpose;
mod oidc_settings;
mod onboarding;
mod org_admin;
mod organizations;
mod overview;
mod pages;
mod password_reset;
mod presence;
mod reached;
mod reactions;
mod readiness;
mod reconcile;
mod references;
mod search;
mod server_lock;
mod slots;
mod snapshots;

pub use board_ops::BoardStart;
pub use board_parts::{
    ATTACH_DENIED, ATTACHMENTS_DENIED, CREATE_VIEW_DENIED, DUPLICATE_VIEW_DENIED, MOVE_VIEW_DENIED,
    VIEWS_DENIED,
};
pub use card_writes::{CREATE_CARD_DENIED, NewCardRequest, PATCH_CARD_DENIED};
pub(crate) use changes::Changes;
pub use comments::is_comment_on;
pub(crate) use download::download;
pub use mail::UnsentMail;
pub(crate) use notifications::start_upkeep as start_notification_upkeep;
pub use notifications::tend as tend_notifications;
pub(crate) use presence::PresenceAccess;
pub(crate) use server_lock::HeldLock;
pub(crate) use server_lock::take_server_lock;
use slots::Busy;
pub use slots::Slot;
pub use startup::{check_database, migrate_database};
pub(crate) use startup::{open_store, prepare_store};
pub(crate) use template_stock::start_restock as start_template_restock;
mod public_links;
mod shared_reads;
mod signups;
mod sso;
mod startup;
mod store_metrics;
mod subscriptions;
mod teams;
mod template_stock;
mod templates;
mod time_entries;
mod today;
mod trash;
mod tree;
mod unit_progress;
mod unit_reviews;
mod uploads;
mod users;
mod webhooks;
mod workspace_members;
mod workspaces;

pub use access::{ContainerAccess, DeletedContainerAccess, as_managed};
pub use audience::{Audience, Former, UnsaidByLink};
pub use blocks::ChangeNotification;
pub(crate) use body_markdown::comment_text_of_markdown;
pub use body_markdown::{BodyInMarkdown, MarkdownReader, body_of_markdown};
pub use caller::{Caller, CallerAuth};
pub use card_filters::{CardsFilter, CardsOrder, CardsUnder};
pub use cards::CardPreviews;
pub use category_containers::DEFAULT_CATEGORY_CONTAINERS;
pub use course_events::MEETING_REMINDER_WINDOW_MILLIS;
pub use course_schedules::CourseEnrollment;
pub use courses::{
    CREATE_UNIT_DENIED, DELETE_UNIT_DENIED, MOVE_UNIT_DENIED, PATCH_UNIT_DENIED,
    UNDELETE_UNIT_DENIED, UNIT_TRASH_DENIED,
};
pub use diaries::{
    CREATE_ENTRY_DENIED, DELETE_ENTRY_DENIED, ENTRY_TRASH_DENIED, EntryAnswer, PATCH_ENTRY_DENIED,
    UNDELETE_ENTRY_DENIED,
};
pub use export::ExportedArchive;
pub(crate) use export::utc_day;
pub(crate) use file_name::check_file_name;
pub(crate) use files::FILE;
pub use housekeeping::TrashPurged;
pub use import::{
    BundleLimits, ImportCounts, ImportFormat, ImportSummary, ImportWarning, ImportWarningCode,
    Imported, MAX_REPORTED_WARNINGS, ParseImportFormatError,
};
pub use invitations::{Accepting, Admission, Inviting};
pub use limits::OrgLimits;
pub use list_page::{DEFAULT_PER_PAGE, ListPage, MAX_PER_PAGE};
pub use loose_objects::{RemovedObjects, WRITING_GRACE_MILLIS};
pub use oidc::{
    BackTo, CALLBACK_PATH, Callback, FinishedLogin, LOGIN_FLOW_LIFETIME_MILLIS, LoginFailure,
    Returned, StartedLogin,
};
pub use oidc_carry::HELD_SIGN_IN_LIFETIME_MILLIS;
pub use oidc_settings::secret_context;
pub use overview::OverviewReader;
pub use pages::{
    CREATE_PAGE_DENIED, DELETE_PAGE_DENIED, DUPLICATE_PAGE_DENIED, MOVE_PAGE_DENIED,
    PAGE_TRASH_DENIED, PATCH_PAGE_DENIED, UNDELETE_PAGE_DENIED,
};
pub use reached::{BlockAction, Reached};
pub(crate) use reached::{refusal, require_granted};
pub use readiness::{Readiness, ReadinessCheck};
pub use reconcile::{Found, Reconciliation, UNRECORDED_MIN_AGE_MILLIS, Unrecorded};
pub use references::MAX_REFERENCE_IDS;
pub use shared_reads::{MAX_SHARED_READ_FAILURES, SHARED_READ_WINDOW, SharedReads};
pub use signups::{ConfirmedSignup, Registrant, SignupMarker};
pub use sso::{Asker, SessionKept, SessionMade};
pub use time_entries::TimeEntryQuery;
pub use unit_progress::{PROGRESS_DENIED, require_progress_recorder};
pub(crate) use uploads::PART_IDLE_TIMEOUT;
pub use uploads::{
    CLOSED_UPLOAD_GRACE_MILLIS, CleanedUp, FileToUpload, MAX_OPEN_UPLOADS, MAX_UPLOAD_NAME_CHARS,
    PARTS_IN_MEMORY, PartDigest, UPLOAD_LIFETIME_MILLIS,
};
pub use users::sanitize;
pub use workspaces::Entered;

use std::sync::Arc;

use anyhow::Context as _;

use fvoci_model::{ClientConfig, ContainerKinds, OrgKind};
use fvoci_store::{Store, Tenant};
use serde::Serialize;
use tokio_util::task::TaskTracker;

use crate::auth::Authenticator;
use crate::auth::hash_queue::HashQueue;
use crate::auth::limiter::{Limits, LoginLimiter};
use crate::auth::oidc::fetch::{Addresses, Fetcher};
use crate::collab::Collab;
use crate::config::{Configuration, Settings};
use crate::error::{AppError, Reason};
use crate::metrics::{Metrics, OpenUploads, Pool};
use crate::realtime::Hub;
use crate::search::Search;
use crate::storage::FileBackend;
use crate::webhook::Webhook;

/// The shared state of the server. Cloning is cheap; every clone is the same server.
///
/// A handle reaches the rows of one organization, the one its store names
/// ([`Tenant`]). [`App::scoped`] gives a handle to the same server that reaches other
/// rows; work that goes on after a request takes its handle along, and with it the
/// organization.
#[derive(Debug, Clone)]
pub struct App {
    inner: Arc<Inner>,
    store: Store,
    ws: Hub,
    /// What of its organization this handle reaches, as the request it serves entered.
    reach: Reach,
    /// The immutable kind of this handle's organization, populated on entry or read at
    /// most once per handle (`App::tell_webhooks`). Reset whenever [`App::scoped`]
    /// changes the tenant.
    org_kind: Arc<tokio::sync::OnceCell<OrgKind>>,
}

/// What of its organization a handle reaches, and as whom: narrowed as a request enters
/// ([`App::in_workspace`], [`App::reaching`], [`App::capped_to`]), never widened.
#[derive(Debug, Clone)]
struct Reach {
    /// The workspace a request names in its path ([`App::in_workspace`]): what is of
    /// another workspace is not there for this handle. `None` reaches every workspace
    /// of the organization.
    workspace: Option<Arc<str>>,
    /// Verified `(user_id, workspace_id, actor)` from [`App::in_workspace_with_actor`],
    /// avoiding a duplicate `resolve_workspace` query inside [`App::actor`] (`1.2.1`).
    known_actor: Option<Arc<(String, String, crate::permissions::Actor)>>,
    /// The kinds of board this handle reaches ([`App::reaching`]): a board of another
    /// kind is not there for it, nor what is on one.
    kinds: ContainerKinds,
    /// The most [`App::kinds`] may be, whatever an operation asks to reach
    /// ([`App::capped_to`]): every kind for a person, and none of a diary for an agent
    /// by a token. It goes with the handle wherever it is scoped.
    allowed_kinds: ContainerKinds,
    /// The token an agent writes blocks through with this handle ([`App::writing_for`]);
    /// `None` for a person. Only the writes of blocks are told of as the agent's
    /// (`App::store_telling`): what the rooms of bodies write later is their editors'.
    agent: Option<Arc<crate::auth::api_token::Delegation>>,
}

impl Reach {
    /// All of the organization, for nobody in particular: a handle no request entered.
    fn everything() -> Self {
        Self {
            workspace: None,
            known_actor: None,
            kinds: ContainerKinds::EVERY,
            allowed_kinds: ContainerKinds::EVERY,
            agent: None,
        }
    }

    /// The same, in another organization: no workspace of this one, and nobody found in
    /// one; the kinds it may reach and the agent that writes go along.
    fn elsewhere(&self) -> Self {
        Self {
            workspace: None,
            known_actor: None,
            ..self.clone()
        }
    }

    /// The same, in `workspace_id` alone, where `known_actor` is who was found there.
    fn in_workspace(
        &self,
        workspace_id: &str,
        known_actor: Option<(String, String, crate::permissions::Actor)>,
    ) -> Self {
        Self {
            workspace: Some(Arc::from(workspace_id)),
            known_actor: known_actor.map(Arc::new),
            ..self.clone()
        }
    }

    /// Reaching containers of `kinds` alone, of those it may.
    fn reaching(&mut self, kinds: ContainerKinds) {
        self.kinds = kinds.intersection(self.allowed_kinds);
    }

    /// Never reaching more than `kinds`, whatever it is narrowed to after.
    fn capped_to(&mut self, kinds: ContainerKinds) {
        self.allowed_kinds = self.allowed_kinds.intersection(kinds);
        self.kinds = self.kinds.intersection(kinds);
    }
}

#[derive(Debug)]
struct Inner {
    settings: Settings,
    auth: Authenticator,
    login_limiter: LoginLimiter,
    reset_limiter: LoginLimiter,
    mail: mail::Mailer,
    password_work: HashQueue,
    files: FileBackend,
    webhook: Webhook,
    webhook_queue: webhooks::WebhookQueue,
    /// The client of the organizations' providers.
    oidc: Fetcher,
    /// The Prometheus metrics, kept if `metrics_address` is set.
    metrics: Metrics,
    /// Work that continues after a response was sent: websocket broadcasts and webhooks.
    background: TaskTracker,
    /// Held while a user's default category is looked for and created, so that two
    /// requests of a new user do not each create one.
    default_category: tokio::sync::Mutex<()>,
    /// Held while the limits on open uploads are checked and a session is stored, so
    /// that sessions opened at the same moment do not pass the limits together.
    opening_upload: tokio::sync::Mutex<()>,
    /// Held while the list of whom to tell is worked off (`notifications::tend`): one
    /// run at a time.
    telling: tokio::sync::Mutex<()>,
    /// The turns of telling the reactions to a block (`reactions::TellingTurns`): what
    /// is read last of a block is told last.
    telling_reactions: reactions::TellingTurns,
    /// The uploads' turns (`uploads::UploadTurns`).
    upload_turns: uploads::UploadTurns,
    /// Active `(upload_id, part_number)` receivers and per-user concurrency caps.
    receiving_parts: uploads::ReceivingParts,
    /// The memory the parts of uploads to the object store may take while they are
    /// received, in KiB (`uploads::PARTS_IN_MEMORY`).
    part_memory: Arc<tokio::sync::Semaphore>,
    /// The last answer of the readiness checks.
    readiness: readiness::Kept,
    /// Whether the built-in templates are owed.
    template_stock: template_stock::TemplateStock,
    /// The exports that run.
    exports: slots::Slots,
    /// The imports that run.
    imports: slots::Slots,
    /// The documents of the bodies being edited together.
    collab: Collab,
    /// Whether this process holds the lock of the server (`server_lock`).
    server_lock: server_lock::Holder,
    /// The limit on reads of shared boards whose token opens nothing.
    shared_reads: SharedReads,
    /// Who has which file open in the editor (`file_leases`).
    file_leases: file_leases::FileLeases,
    /// The search of bodies, on the search server if one is configured.
    search: Search,
    /// Optional fixed Unix timestamp in milliseconds for deterministic schedule timezone tests.
    #[cfg(feature = "test-util")]
    schedule_now_millis: std::sync::Mutex<Option<i64>>,
}

/// What `ping` reports about the server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServerMetadata {
    /// The version of the server, the one the contract's `info.version` gives too.
    pub version: String,
    /// The commit the server was built from; empty unless set at build time.
    pub commit: String,
    /// The build date; empty unless set at build time.
    pub build_date: String,
    /// The database kind: `sqlite` or `postgres`.
    pub db_type: String,
    /// The database's version string.
    pub db_version: String,
}

impl App {
    /// Assembles the application state of a configuration that was checked.
    ///
    /// # Errors
    ///
    /// Returns an error if webhooks are configured and their HTTP client cannot be set
    /// up, or if `metrics_address` is no address.
    pub fn new(settings: Settings, store: Store, files: FileBackend) -> anyhow::Result<Self> {
        let config = settings.config();
        let metrics = store_metrics::set_up(config, &store)?;
        store.set_comment_versions_kept(config.max_comment_versions);
        store.set_body_versions_kept(config.collab.collab_body_versions);
        store.set_body_versions_max_bytes(config.collab.body_versions_max_bytes);
        store.set_file_versions_kept(config.max_file_versions);
        // The authenticator reads what is no organization's (sessions), and asks the
        // directory whose a shared board is: it is given no organization to reach.
        let auth = Authenticator::new(store.scoped(Tenant::Unset), &settings);
        let login_limiter =
            LoginLimiter::new(Limits::from_config(config)).with_metrics(metrics.clone());
        let password_work = HashQueue::new(
            usize::try_from(config.password_hash_concurrency).unwrap_or(usize::MAX),
            usize::try_from(config.password_hash_queue).unwrap_or(usize::MAX),
            std::time::Duration::from_secs(config.password_hash_queue_seconds),
        );
        let ws = Hub::new(
            Audience::new(store.clone()),
            auth.clone(),
            settings.websocket_ping(),
            metrics.clone(),
        );
        let files = files.measured(metrics.clone());
        let (webhook, oidc) = Self::outbound_clients(config)?;
        let part_memory = uploads::part_memory_kib(config.upload_part_bytes);
        let slots =
            |capacity: u32| slots::Slots::new(usize::try_from(capacity).unwrap_or(usize::MAX));
        let exports = slots(config.max_concurrent_exports);
        let imports = slots(config.max_concurrent_imports);
        let collab = Collab::new(&config.collab)
            .context("unable to find the executable to start the collab workers from")?;
        let search = Search::from_config(&config.search).context("unable to set up the search")?;
        // From before the first request: what changes is noted for the search server.
        if search.configured() {
            store.keep_search_outbox();
        }
        Ok(Self {
            store,
            ws,
            reach: Reach::everything(),
            org_kind: Arc::new(tokio::sync::OnceCell::new()),
            inner: Arc::new(Inner {
                reset_limiter: LoginLimiter::new(Limits::from_config(config)),
                mail: mail::Mailer::new(
                    &config.smtp,
                    &config.mail,
                    settings.server_url().as_str(),
                    metrics.clone(),
                )?,
                settings,
                auth,
                login_limiter,
                password_work,
                files,
                webhook,
                webhook_queue: webhooks::WebhookQueue::default(),
                oidc,
                metrics,
                background: TaskTracker::new(),
                default_category: tokio::sync::Mutex::new(()),
                opening_upload: tokio::sync::Mutex::new(()),
                telling: tokio::sync::Mutex::new(()),
                telling_reactions: reactions::TellingTurns::default(),
                upload_turns: uploads::UploadTurns::default(),
                receiving_parts: uploads::ReceivingParts::default(),
                part_memory: Arc::new(tokio::sync::Semaphore::new(part_memory)),
                readiness: readiness::Kept::default(),
                template_stock: template_stock::TemplateStock::default(),
                exports,
                imports,
                collab,
                server_lock: server_lock::Holder::default(),
                shared_reads: SharedReads::default(),
                file_leases: file_leases::FileLeases::default(),
                search,
                #[cfg(feature = "test-util")]
                schedule_now_millis: std::sync::Mutex::default(),
            }),
        })
    }

    fn outbound_clients(config: &Configuration) -> anyhow::Result<(Webhook, Fetcher)> {
        let webhook = Webhook::with_addresses(
            config.block_change_webhooks.clone(),
            if config.webhook_allow_private_addresses {
                Addresses::Any
            } else {
                Addresses::PublicOnly
            },
        )
        .context("unable to set up the webhook client")?;
        let oidc = Fetcher::new(
            if config.oidc_allow_private_addresses {
                Addresses::Any
            } else {
                Addresses::PublicOnly
            },
            &config.oidc_trusted_roots()?,
        )
        .context("unable to set up the client of the providers")?;
        Ok((webhook, oidc))
    }

    /// The Prometheus metrics; they do nothing unless `metrics_address` is set.
    pub fn metrics(&self) -> &Metrics {
        &self.inner.metrics
    }

    /// The documents of the bodies being edited together.
    pub fn collab(&self) -> &Collab {
        &self.inner.collab
    }

    /// The server configuration.
    pub fn config(&self) -> &Configuration {
        self.inner.settings.config()
    }

    /// What the check of the server configuration read from it.
    pub fn settings(&self) -> &Settings {
        &self.inner.settings
    }

    /// Current Unix timestamp in milliseconds used by course schedule and review "today"
    /// lookups, backed by the store's monotonic clock unless overridden in tests.
    pub(crate) fn schedule_now_millis(&self) -> i64 {
        #[cfg(feature = "test-util")]
        if let Some(fixed) = *self
            .inner
            .schedule_now_millis
            .lock()
            .expect("schedule clock lock poisoned")
        {
            return fixed;
        }
        self.store.clock().now_millis()
    }

    /// Overrides the clock used by course schedule "today" calculations in tests.
    /// Pass `None` to restore the real system clock.
    ///
    /// # Panics
    ///
    /// Panics if the internal test clock mutex is poisoned.
    #[cfg(feature = "test-util")]
    pub fn set_schedule_now_millis(&self, now: Option<i64>) {
        *self
            .inner
            .schedule_now_millis
            .lock()
            .expect("schedule clock lock poisoned") = now;
    }

    /// The same server, reaching the rows of `tenant`: of the organization a request
    /// names, of the system organization that holds the built-in templates, or of all
    /// ([`Tenant::System`], for the work the server does on nobody's behalf).
    #[must_use]
    pub fn scoped(&self, tenant: Tenant) -> Self {
        let store = self.store.scoped(tenant);
        // The hub is the same, with the same connections; what it asks about the
        // boards, to know whom a change is told to, it asks as this handle reaches them.
        let mut ws = self.ws.clone();
        ws.audience = self.ws.audience.with_store(store.clone());
        Self {
            inner: Arc::clone(&self.inner),
            ws,
            store,
            reach: self.reach.elsewhere(),
            org_kind: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// The same handle, whose writes of blocks are those of the agent of `delegation`:
    /// whom they name is told of them as the agent's, the token's owner as well where
    /// the token asks for that.
    #[must_use]
    pub(crate) fn writing_for(mut self, delegation: &crate::auth::api_token::Delegation) -> Self {
        self.reach.agent = Some(Arc::new(delegation.clone()));
        self
    }

    /// Records the immutable kind of this handle's organization so later checks
    /// ([`App::tell_webhooks`]) do not query it again.
    #[must_use]
    pub(super) fn with_org_kind(mut self, kind: OrgKind) -> Self {
        self.org_kind = Arc::new(tokio::sync::OnceCell::new_with(Some(kind)));
        self
    }

    /// The same handle, reaching boards of `kinds` only: for an operation that is about
    /// what a board holds (cards, views) or about what a wiki holds (pages), to which a
    /// container of the other kind is one that is not there. A handle reaches both kinds
    /// until it is narrowed: what is the same for a board and a wiki (who is on it, its
    /// files, the rooms of its bodies) is asked of both.
    ///
    /// What it reaches stays within what it may ([`App::capped_to`]): asked for more, it
    /// reaches that much.
    #[must_use]
    pub fn reaching(mut self, kinds: ContainerKinds) -> Self {
        self.reach.reaching(kinds);
        self
    }

    /// The kinds of board this handle reaches ([`App::reaching`]).
    #[must_use]
    pub fn kinds(&self) -> ContainerKinds {
        self.reach.kinds
    }

    /// The same handle, never reaching more than `kinds`, however it is narrowed or
    /// scoped after: for an agent, which reaches no diary.
    #[must_use]
    pub fn capped_to(mut self, kinds: ContainerKinds) -> Self {
        self.reach.capped_to(kinds);
        self
    }

    /// The most this handle may reach ([`App::capped_to`]): what an operation that is
    /// about every kind of container reaches.
    #[must_use]
    pub fn allowed_kinds(&self) -> ContainerKinds {
        self.reach.allowed_kinds
    }

    /// The same handle, for a request that names the workspace in its path: a board of
    /// another workspace, and what is on it, is not there for it ([`App::container_access`]),
    /// but for the built-in templates, which are every workspace's.
    #[must_use]
    pub fn in_workspace(&self, workspace_id: &str) -> Self {
        Self {
            reach: self.reach.in_workspace(workspace_id, None),
            ..self.clone()
        }
    }

    /// [`App::in_workspace`], remembering the caller's verified [`crate::permissions::Actor`]
    /// in `workspace_id` so subsequent checks in the same request do not re-query
    /// `resolve_workspace` (`1.2.1`).
    #[must_use]
    pub(crate) fn in_workspace_with_actor(
        &self,
        workspace_id: &str,
        user_id: &str,
        actor: crate::permissions::Actor,
    ) -> Self {
        let known = (user_id.to_owned(), workspace_id.to_owned(), actor);
        Self {
            reach: self.reach.in_workspace(workspace_id, Some(known)),
            ..self.clone()
        }
    }

    /// Whether what is of `workspace_id` is there for this handle: always, unless the
    /// handle is for one workspace ([`App::in_workspace`]) and that is another.
    #[must_use]
    pub fn reaches_workspace(&self, workspace_id: &str) -> bool {
        self.reach
            .workspace
            .as_deref()
            .is_none_or(|ours| ours == workspace_id)
    }

    /// Whether the board is there for this handle: one of a kind it reaches
    /// ([`App::reaching`]) that is a built-in template or of a workspace it reaches
    /// ([`App::reaches_workspace`]). The one place that says so: every way to a board, to
    /// what is on it and to what is stored for it asks here, or reads a list of the
    /// handle's kinds.
    #[must_use]
    pub fn reaches_container(&self, container: &fvoci_model::Container) -> bool {
        self.reach.kinds.holds(container.kind)
            && (container.is_builtin || self.reaches_workspace(&container.workspace_id))
    }

    /// The database, as this handle reaches it.
    pub fn store(&self) -> &Store {
        &self.store
    }

    /// The client of the organizations' providers.
    pub(crate) fn oidc_fetcher(&self) -> &Fetcher {
        &self.inner.oidc
    }

    /// The session and read-token checks.
    pub fn authenticator(&self) -> &Authenticator {
        &self.inner.auth
    }

    /// The search of bodies.
    pub fn search(&self) -> &Search {
        &self.inner.search
    }

    /// The limit on reads of shared boards whose token opens nothing.
    pub(crate) fn shared_reads(&self) -> &SharedReads {
        &self.inner.shared_reads
    }

    /// Who has which file open in the editor.
    fn file_leases(&self) -> &file_leases::FileLeases {
        &self.inner.file_leases
    }

    /// The limits on login attempts.
    pub(crate) fn login_limiter(&self) -> &LoginLimiter {
        &self.inner.login_limiter
    }

    /// The queue passwords are hashed and checked in.
    pub(crate) fn inner_password_work(&self) -> &HashQueue {
        &self.inner.password_work
    }

    /// The websocket hub.
    pub fn ws(&self) -> &Hub {
        &self.ws
    }

    /// The file storage.
    pub fn files(&self) -> &FileBackend {
        &self.inner.files
    }

    /// Waits for the turn to create a user's default category.
    pub(crate) async fn default_category_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.default_category.lock().await
    }

    /// Waits for the turn to check the limits on open uploads and store a session.
    pub(crate) async fn opening_upload_turn(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.opening_upload.lock().await
    }

    /// The turns of the uploads.
    pub(crate) fn upload_turns(&self) -> &uploads::UploadTurns {
        &self.inner.upload_turns
    }

    /// Active part-upload guards and per-user concurrency semaphores.
    pub(crate) fn receiving_parts(&self) -> &uploads::ReceivingParts {
        &self.inner.receiving_parts
    }

    /// The memory parts of uploads to the object store may take, in KiB.
    pub(crate) fn part_memory(&self) -> &Arc<tokio::sync::Semaphore> {
        &self.inner.part_memory
    }

    /// Runs `work` in the background, after the caller has returned. Used for
    /// notifications that must not delay or fail a request.
    pub fn spawn_background(&self, work: impl Future<Output = ()> + Send + 'static) {
        self.inner.background.spawn(work);
    }

    /// Runs `work` as a task of its own and returns its handle. Used to let a request
    /// handler finish what it started when its client goes away; shutdown waits for these
    /// tasks as it does for background work.
    pub(crate) fn spawn_tracked<T: Send + 'static>(
        &self,
        work: impl Future<Output = T> + Send + 'static,
    ) -> tokio::task::JoinHandle<T> {
        self.inner.background.spawn(work)
    }

    /// The mail queued and not yet delivered or given up: for the shutdown to say what
    /// it leaves unsent.
    pub fn unsent_mail(&self) -> UnsentMail {
        self.inner.mail.unsent()
    }

    /// Logs how much mail still holds a place in its queue, if any: once the shutdown
    /// stops waiting for it, it is not sent.
    pub(crate) fn warn_of_unsent_mail(&self) {
        let unsent = self.unsent_mail();
        if !unsent.is_empty() {
            tracing::warn!(
                recovery = unsent.recovery,
                invitations = unsent.invitations,
                "mail still queued at shutdown: it is kept in memory and will not be sent"
            );
        }
    }

    /// Waits until all background work has finished, or `timeout` has passed. Returns
    /// `false` on timeout.
    pub async fn finish_background(&self, timeout: std::time::Duration) -> bool {
        self.inner.background.close();
        let finished = tokio::time::timeout(timeout, self.inner.background.wait())
            .await
            .is_ok();
        self.inner.background.reopen();
        finished
    }

    /// Takes the place of an export of the user's, given back when the [`Slot`] is
    /// dropped.
    ///
    /// # Errors
    ///
    /// Returns a 429 if the user has an export running (reason `export_running`) or the
    /// server runs as many as `max_concurrent_exports` (reason `server_exports_busy`).
    pub fn start_export(&self, user_id: &str) -> Result<Slot, AppError> {
        self.inner.exports.take(user_id).map_err(|busy| match busy {
            Busy::User => AppError::limited(
                "an export of yours is running; try again when it is done",
                Reason::ExportRunning,
            ),
            Busy::Server => AppError::limited(
                "the server runs as many exports as it may; try again later",
                Reason::ServerExportsBusy,
            ),
        })
    }

    /// How many exports and how many imports hold their place, as their limits count
    /// them: for the metrics.
    fn archives_running(&self) -> (usize, usize) {
        (self.inner.exports.held(), self.inner.imports.held())
    }

    /// Reads into the metrics what is measured by asking, for a scrape: the archives
    /// that hold their place, the connection pool, the open uploads and the live
    /// sessions. The last two stay as they were if the database cannot be read in time,
    /// which is counted, or another scrape is reading them.
    pub(crate) async fn read_storage_metrics(&self) {
        let metrics = self.metrics();
        if !metrics.is_kept() {
            return;
        }
        let (exports, imports) = self.archives_running();
        metrics.set_archives_running(exports, imports);
        let pool = self.store().pool_status();
        metrics.set_pool(Pool {
            connections: pool.connections,
            idle: pool.idle,
            max_connections: pool.max_connections,
        });
        let now = crate::auth::now_millis();
        let store = self.store().clone();
        let open_uploads = async move {
            let open = store.open_uploads(now).await?;
            Ok::<_, fvoci_store::StoreError>(OpenUploads {
                sessions: open.sessions,
                bytes: open.bytes,
            })
        };
        let idle_since = now.saturating_sub(self.config().session_idle_millis());
        let store = self.store().clone();
        let live_sessions = async move { store.count_live_sessions(now, idle_since).await };
        tokio::join!(
            metrics.read_open_uploads(open_uploads),
            metrics.read_live_sessions(live_sessions),
        );
    }

    /// Takes the place of an import of the user's, as [`Self::start_export`] does.
    ///
    /// # Errors
    ///
    /// Returns a 429 if the user has an import running (reason `import_running`) or the
    /// server runs as many as `max_concurrent_imports` (reason `server_imports_busy`).
    pub fn start_import(&self, user_id: &str) -> Result<Slot, AppError> {
        self.inner.imports.take(user_id).map_err(|busy| match busy {
            Busy::User => AppError::limited(
                "an import of yours is running; try again when it is done",
                Reason::ImportRunning,
            ),
            Busy::Server => AppError::limited(
                "the server runs as many imports as it may; try again later",
                Reason::ServerImportsBusy,
            ),
        })
    }

    /// The part of the configuration the web app needs.
    pub fn client_config(&self) -> ClientConfig {
        let config = self.config();
        let limits = self.limits();
        ClientConfig {
            allow_public_links: config.allow_public_links,
            name_display: config.name_display.clone(),
            feature_flags: config.feature_flags.clone(),
            max_file_bytes: i64::try_from(limits.largest_file.get()).unwrap_or(i64::MAX),
            max_import_bytes: i64::try_from(limits.largest_import.get()).unwrap_or(i64::MAX),
            trash_retention_days: config.trash_retention_days,
            // Every server searches bodies: on its search server, or on its database
            // where none is configured. One that cannot hold the index answers 503.
            search_enabled: true,
        }
    }

    /// What `ping` reports.
    pub async fn server_metadata(&self) -> ServerMetadata {
        let db_version = match self.store().db_version().await {
            Ok(version) => version,
            Err(error) => {
                tracing::error!(%error, "error checking database version");
                String::new()
            }
        };
        ServerMetadata {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            commit: option_env!("FVOCI_BUILD_HASH")
                .unwrap_or_default()
                .to_owned(),
            build_date: option_env!("FVOCI_BUILD_DATE")
                .unwrap_or_default()
                .to_owned(),
            db_type: self.store().db_type().as_str().to_owned(),
            db_version,
        }
    }
}
