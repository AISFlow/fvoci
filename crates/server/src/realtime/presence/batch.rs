//! Each connection's batch: every 250 ms, its board's roster as the reader may see it,
//! sent when it changed.
//!
//! The store is asked what the reader may see (its own access, then whom of those
//! present it may name) when the reader enters a board, at the periodic check, and
//! when an access change reached the visit. In between, a change of the board's
//! visits looks up only the people the reader has not met yet, and a read token's
//! reader, who is sent nothing, asks nothing.

use std::collections::HashSet;
use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;

use fvoci_model::WorkspaceRole;
use tokio::time::Instant;

use super::super::{Outgoing as _, text_of};
use super::{Hub, LEASE, LOOKUP_WAIT, ListenerId, Snapshot, Visit, snapshot};
use crate::app::{App, Asker, PresenceAccess, SessionMade};
use crate::error::AppError;

const BATCH: Duration = Duration::from_millis(250);

/// What a batch reads of the connection, under the lock.
struct Batch {
    visit: Visit,
    revision: u64,
    /// An access change reached the visit since the last batch took this.
    recheck: bool,
    /// The store is asked anew what the reader may see.
    due: bool,
    user_id: String,
    /// How the connection's session was made, and what it had confirmed; `None` without a session.
    making: Option<SessionMade>,
    /// The board's roster before the reader's sight, and the number of its board's
    /// latest change.
    raw: Snapshot,
    changed: Option<u64>,
}

/// What a batch found of the connection.
enum Taken {
    /// No visit: the connection is not on a board, or is gone.
    Away,
    /// The visit was not renewed in time; it ends unless a command came since.
    Expired {
        revision: u64,
    },
    /// Nothing to do: the board's visits did not change and no check is due.
    Unchanged,
    Batch(Box<Batch>),
}

/// What the loop keeps of a reader between batches, for one entry into a board.
#[derive(Default)]
struct Reader {
    entry: Option<u64>,
    sight: Option<Sight>,
    /// The number of the board's change the last projection was of.
    seen: Option<u64>,
    sent: Option<Snapshot>,
    next_check: Option<Instant>,
}

/// What the store said the reader may see, at the last check.
enum Sight {
    /// A read token: counted on the board, sent nothing.
    Token,
    Member {
        /// Boxed: an application handle is far larger than the token's nothing.
        app: Box<App>,
        role: WorkspaceRole,
        /// The people met so far, by whether the reader may name them.
        named: HashSet<String>,
        unnamed: HashSet<String>,
    },
}

impl Sight {
    fn of(access: PresenceAccess) -> Self {
        match access.role {
            None => Self::Token,
            Some(role) => Self::Member {
                app: Box::new(access.app),
                role,
                named: HashSet::new(),
                unnamed: HashSet::new(),
            },
        }
    }

    /// Looks up which of `raw`'s people not met yet the reader may name.
    async fn meet(&mut self, batch: &Batch) -> Result<(), AppError> {
        let Self::Member {
            app,
            role,
            named,
            unnamed,
        } = self
        else {
            return Ok(());
        };
        let new: Vec<String> = batch
            .raw
            .people
            .iter()
            .map(|person| &person.user_id)
            .filter(|id| !named.contains(*id) && !unnamed.contains(*id))
            .cloned()
            .collect();
        if new.is_empty() {
            return Ok(());
        }
        let visible = app
            .users_in_workspace(&batch.user_id, &batch.visit.workspace_id, *role, &new)
            .await?;
        for id in new {
            if visible.iter().any(|user| user.id == id) {
                named.insert(id);
            } else {
                unnamed.insert(id);
            }
        }
        Ok(())
    }

    /// `raw` as the reader may see it; `None` for a token's reader.
    fn project(&self, mut raw: Snapshot) -> Option<Snapshot> {
        let Self::Member { named, .. } = self else {
            return None;
        };
        let before = raw.people.len();
        raw.people.retain(|person| named.contains(&person.user_id));
        raw.hidden_count = before - raw.people.len();
        Some(raw)
    }
}

/// How a batch's lookup ended.
enum Looked {
    /// The reader may read the board; the open card is the one it is still on (empty
    /// if none).
    Read {
        card_id: String,
    },
    /// The reader's access was not asked again; at most whom it may name of the
    /// people it had not met.
    Met,
    Refused,
}

impl Hub {
    /// Sends the connection its board's roster as it changes, until the connection
    /// ends (it never returns).
    pub(in crate::realtime) async fn presence_loop(&self, app: &App, id: ListenerId) -> Infallible {
        let mut ticks = tokio::time::interval(BATCH);
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut reader = Reader::default();
        loop {
            ticks.tick().await;
            self.batch(app, id, &mut reader).await;
        }
    }

    async fn batch(&self, app: &App, id: ListenerId, reader: &mut Reader) {
        let batch = match self.take_batch(id, reader) {
            Taken::Batch(batch) => batch,
            Taken::Unchanged => return,
            Taken::Expired { revision } => {
                self.clear_presence(id, Some(revision));
                return;
            }
            Taken::Away => {
                *reader = Reader::default();
                return;
            }
        };
        let due = batch.due;
        let looked = if due {
            tokio::time::timeout(LOOKUP_WAIT, read_anew(app, &batch, reader)).await
        } else {
            tokio::time::timeout(LOOKUP_WAIT, meet_new(&batch, reader)).await
        };
        let looked = match looked {
            Ok(Ok(looked)) => looked,
            Ok(Err(error)) => {
                tracing::warn!(%error, "presence authority could not be read");
                Looked::Refused
            }
            Err(_) => Looked::Refused,
        };
        if let Looked::Refused = looked {
            self.clear_presence(id, Some(batch.revision));
            return;
        }
        // Its sight was read for this reader alone: no other check applies, and it is
        // queued under the lock that settles it, before an access change can come.
        let projected = reader
            .sight
            .as_ref()
            .and_then(|sight| sight.project(batch.raw.clone()))
            .filter(|projected| reader.sent.as_ref() != Some(projected));
        let text = projected.as_ref().and_then(text_of);
        let Some(queued) = self.settle(id, &batch, &looked, text.as_ref()) else {
            return;
        };
        if let Some(projected) = projected {
            self.inner
                .metrics
                .websocket_queued(projected.kind(), queued);
            reader.sent = Some(projected);
        }
        if due {
            reader.next_check = Some(
                Instant::now() + Duration::from_secs(app.config().collab.collab_recheck_seconds),
            );
        }
        reader.seen = batch.changed;
    }

    /// Reads the connection's visit and, if the batch has anything to do, its board's
    /// roster, and takes the recheck flag, under one lock. The roster is built only
    /// when the board's visits changed since the reader's last projection or the
    /// store is to be asked again: an idle board costs a lookup of its number.
    fn take_batch(&self, id: ListenerId, reader: &mut Reader) -> Taken {
        let mut guard = self.listeners();
        let listeners = &mut *guard;
        let Some(listener) = listeners.by_id.get_mut(&id) else {
            return Taken::Away;
        };
        let state = &mut listener.presence;
        let Some(visit) = state.visit.as_ref() else {
            return Taken::Away;
        };
        if state
            .renewed
            .is_none_or(|renewed| renewed.elapsed() >= LEASE)
        {
            return Taken::Expired {
                revision: state.revision,
            };
        }
        if reader.entry != Some(state.entry) {
            *reader = Reader {
                entry: Some(state.entry),
                ..Reader::default()
            };
        }
        let changed = listeners.visits.changed(&visit.container());
        let due = state.recheck
            || reader.sight.is_none()
            || reader.next_check.is_none_or(|at| Instant::now() >= at);
        if !due && reader.seen == changed {
            return Taken::Unchanged;
        }
        let visit = visit.clone();
        let recheck = std::mem::take(&mut state.recheck);
        let revision = state.revision;
        let (user_id, making) = (listener.user_id.clone(), listener.made());
        let raw = snapshot(&visit, listeners);
        Taken::Batch(Box::new(Batch {
            visit,
            revision,
            recheck,
            due,
            user_id,
            making,
            raw,
            changed,
        }))
    }

    /// Whether what the batch read stands, to be sent: no command came meanwhile and
    /// no access change reached the visit while the store was asked. If it does, the
    /// reader's `roster` (if any) is queued under the same lock, and how many it was
    /// queued for is returned; `None` if it does not. If the open card is gone, the
    /// visit is moved to the board with no card, and the next batch, which builds
    /// the roster again, sends.
    fn settle(
        &self,
        id: ListenerId,
        batch: &Batch,
        looked: &Looked,
        roster: Option<&Bytes>,
    ) -> Option<usize> {
        let mut listeners = self.listeners();
        let listener = listeners.by_id.get_mut(&id)?;
        let state = &mut listener.presence;
        if state.revision != batch.revision {
            // A recheck the batch was for is still owed to the roster's other people.
            state.recheck |= batch.recheck;
            return None;
        }
        // An access change reached the visit after the batch took the flag: what was
        // read may be from before it, and the next batch asks again.
        if state.recheck {
            return None;
        }
        if let Looked::Read { card_id } = looked
            && *card_id != batch.visit.card_id
        {
            // The card was deleted or moved: the reader is on the board, with no card.
            let visit = Visit {
                card_id: card_id.clone(),
                ..batch.visit.clone()
            };
            listeners.place(id, Some(visit));
            return None;
        }
        Some(roster.map_or(0, |roster| listeners.queue(&[id], roster)))
    }
}

/// Asks the store anew what the reader may see.
async fn read_anew(app: &App, batch: &Batch, reader: &mut Reader) -> Result<Looked, AppError> {
    let asker = batch.making.as_ref().map(|making| Asker {
        user_id: &batch.user_id,
        making: making.making(),
        delegation: None,
    });
    let visit = &batch.visit;
    let access = app
        .presence_access(
            asker,
            &visit.workspace_id,
            &visit.container_id,
            &visit.card_id,
            &visit.read_token,
        )
        .await?;
    let Some(access) = access else {
        return Ok(Looked::Refused);
    };
    let card_id = access.card_id.clone();
    let mut sight = Sight::of(access);
    sight.meet(batch).await?;
    reader.sight = Some(sight);
    Ok(Looked::Read { card_id })
}

/// Asks the store only about the people of the roster the reader has not met.
async fn meet_new(batch: &Batch, reader: &mut Reader) -> Result<Looked, AppError> {
    if let Some(sight) = &mut reader.sight {
        sight.meet(batch).await?;
    }
    Ok(Looked::Met)
}
