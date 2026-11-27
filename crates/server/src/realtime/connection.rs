//! A connection as the hub counts it, from the side of the transport that serves it
//! (`crate::ws`): opened with whose it is and where its messages go, it is looked up
//! by its session, carries out the commands it is handed and is told of the visits of
//! its board, until it is dropped.

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::quiet::QuietLimit;
use super::{Command, Hub, Identity, Listener, ListenerId, SessionToken, presence, subscribe};
use crate::app::{App, SessionKept};
use crate::auth::limiter::AddressKey;
use crate::collab::{Outbox, Seat};
use crate::error::{AppError, ErrorKind};
use crate::metrics::{Disconnect, Metrics};

/// How long after a session was not found under a connection's token it is looked up
/// again, under the token the connection has then: longer than a new token takes from
/// the store to the connection, and no wait anybody notices before a connection of an
/// ended session closes (those are closed by whatever ends the session; this lookup is
/// for the ones that missed).
const TOKEN_HANDOVER: Duration = Duration::from_millis(200);

/// A connection counted among the hub's. Dropping it takes it out of the hub, also
/// when the future that serves it is cancelled or unwinds.
pub(crate) struct Connection {
    hub: Hub,
    id: ListenerId,
    /// The token its session is looked up by; `None` for a connection without one.
    token: Option<Arc<SessionToken>>,
    /// Cancelled when the hub gives the connection up.
    closed: CancellationToken,
    /// Why the hub gave it up, set before `closed` is cancelled.
    closed_because: Arc<OnceLock<Disconnect>>,
}

/// A connection just opened, with its seat among the rooms of the bodies edited
/// together and what those rooms send it.
pub(crate) struct Opened {
    pub(crate) connection: Connection,
    pub(crate) seat: Seat,
    pub(crate) rooms: Outbox,
}

impl Hub {
    /// Counts a connection of `identity` (without one, it follows shared blocks and
    /// announces anonymous visits by read token) from `address` (none through the
    /// administration socket), whose messages go to `outbox`, and seats it among the
    /// rooms of `app`'s bodies.
    pub(crate) fn open(
        &self,
        app: &App,
        identity: Option<Identity>,
        address: Option<IpAddr>,
        outbox: mpsc::Sender<Bytes>,
    ) -> Opened {
        let closed = CancellationToken::new();
        let closed_because = Arc::new(OnceLock::new());
        let mut who = subscribe::Who::of(identity);
        let (seat, rooms) = app.collab().seat(
            app,
            &who.user_id,
            who.making
                .clone()
                .unwrap_or_else(SessionKept::with_password),
        );
        let token = who.token.take().map(SessionToken::shared);
        self.watch_access_changes(app);
        let id = self.add_listener(Listener {
            address: address.map(AddressKey::from),
            closed: closed.clone(),
            closed_because: Arc::clone(&closed_because),
            token: token.clone(),
            ..Listener::new(who, outbox)
        });
        Opened {
            connection: Connection {
                hub: self.clone(),
                id,
                token,
                closed,
                closed_because,
            },
            seat,
            rooms,
        }
    }

    /// How often each connection is pinged, if at all.
    pub(crate) fn ping_interval(&self) -> Option<Duration> {
        self.inner.ping_interval
    }

    /// How long a connection may send nothing; the tests may shorten it meanwhile.
    pub(crate) fn quiet_limit(&self) -> QuietLimit {
        self.inner.quiet_limit.clone()
    }

    /// Where the connections and why they end are counted.
    pub(crate) fn metrics(&self) -> &Metrics {
        &self.inner.metrics
    }

    /// Counts a connection among the hub's, under a number of its own.
    pub(super) fn add_listener(&self, listener: Listener) -> ListenerId {
        let mut listeners = self.listeners();
        listeners.next_id += 1;
        let id = listeners.next_id;
        if !listener.user_id.is_empty() {
            listeners
                .by_user
                .entry(listener.user_id.clone())
                .or_default()
                .push(id);
        }
        listeners.by_id.insert(id, listener);
        id
    }

    /// Looks the session of `token` up under the token it has now and, not found, once
    /// more a moment later under the one it has then. A session that gives, sets or
    /// changes its account's password goes on under a new token, which is stored a
    /// moment before its connections are handed it (`SessionToken`): looked up in
    /// between, or under the token read just before, that session has not ended.
    ///
    /// # Errors
    ///
    /// Returns what the last lookup failed with: unauthorized for a session that ended.
    async fn find_kept_session(&self, token: &SessionToken) -> Result<(), AppError> {
        match self.inner.auth.find_session(&token.current()).await {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() != ErrorKind::Unauthorized => return Err(error),
            Err(_) => {}
        }
        tokio::time::sleep(TOKEN_HANDOVER).await;
        self.inner
            .auth
            .find_session(&token.current())
            .await
            .map(|_| ())
    }
}

impl Connection {
    /// Cancelled when the hub gives the connection up: it does not keep up, its session
    /// ended, or what it may be told could not be read.
    pub(crate) fn closed(&self) -> &CancellationToken {
        &self.closed
    }

    /// Why the hub gave the connection up, set before [`Self::closed`] is cancelled.
    pub(crate) fn closed_because(&self) -> &Arc<OnceLock<Disconnect>> {
        &self.closed_because
    }

    /// Whether the session of the connection is still there, looked up once more.
    /// The sessions of a user can be ended between the handshake and the opening that
    /// made this a connection of that user; the closing of the user's connections
    /// would then have passed it by. A connection without a session has nothing to
    /// look up.
    ///
    /// # Errors
    ///
    /// Returns unauthorized for a session that ended, and what the store failed with
    /// if it could not be read.
    pub(crate) async fn session_kept(&self) -> Result<(), AppError> {
        match &self.token {
            Some(token) => self.hub.find_kept_session(token).await,
            None => Ok(()),
        }
    }

    /// Returns once the session of the connection has ended, looked up every `period`
    /// under the token it has then; never for a connection without a session. A store
    /// that cannot be read ends nothing.
    pub(crate) async fn session_ends(&self, period: Duration) {
        let Some(token) = &self.token else {
            return std::future::pending().await;
        };
        let mut checks = tokio::time::interval(period);
        checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick is at once; the session was looked up just before.
        checks.tick().await;
        loop {
            checks.tick().await;
            match self.hub.find_kept_session(token).await {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::Unauthorized => return,
                Err(error) => {
                    tracing::warn!(%error, "a websocket connection's session could not be read");
                }
            }
        }
    }

    /// Sends the connection its board's roster as it changes, until it ends (it never
    /// returns).
    pub(crate) async fn tell_presence(&self, app: &App) -> Infallible {
        self.hub.presence_loop(app, self.id).await
    }

    /// Carries out the command `text`; whether the connection stays open. A text that
    /// is no command is passed over.
    pub(crate) async fn carry_out(&self, app: &App, text: &str) -> bool {
        let Ok(command) = serde_json::from_str::<Command>(text) else {
            tracing::debug!("websocket: a command could not be parsed");
            return true;
        };
        // No command closes a connection now; were one to, the client asked for it.
        // Boxed: what a command holds is large, and is no part of what reading holds.
        Box::pin(self.hub.handle(app, self.id, command)).await
    }

    /// Counts the connection among those that read a command they have not carried
    /// out yet, until what is returned is dropped.
    #[cfg(feature = "test-util")]
    pub(crate) fn begin_work(&self) -> super::testing::Working {
        self.hub.inner.at_work.begin()
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        self.hub.listeners().remove(self.id);
    }
}

impl Listener {
    /// A connection of `who` (whose token is no more needed) that follows nothing yet,
    /// from no known address, sending to `outbox`.
    pub(super) fn new(who: subscribe::Who, outbox: mpsc::Sender<Bytes>) -> Self {
        Self {
            user_id: who.user_id,
            session_id: who.session_id,
            login: who.login,
            making: who.making,
            token: None,
            address: None,
            outbox,
            closed: CancellationToken::new(),
            closed_because: Arc::default(),
            workspaces: Vec::new(),
            workspace_roles: HashMap::new(),
            blocks: Vec::new(),
            shared_containers: Vec::new(),
            presence: presence::State::default(),
        }
    }
}
