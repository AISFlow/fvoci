//! Nodes an agent adds after the last of a body, while its editors may type in it: the
//! nodes are written into the card's document as one more editor writes, and the
//! editors are sent the update as another's, so that what they typed meanwhile stays
//! where they typed it and nobody loads the document anew. The body is not written
//! whole: what the editors typed and did not send yet is not lost to it.

use serde_json::Value;

use fvoci_store::ByAgent;

use super::place::{Held, Unanswered};
use super::upkeep::Sent;
use super::{Collab, Taken, whole};
use crate::app::App;
use crate::error::AppError;

/// Who adds nodes to a body: a user, by a session or through the personal API token
/// they gave an agent.
#[derive(Debug, Clone, Copy)]
pub struct Appender<'a> {
    /// The user who has the session, or who gave the agent the token.
    pub user_id: &'a str,
    /// The agent, by its token; `None` for a session.
    pub agent: Option<ByAgent<'a>>,
}

impl Appender<'_> {
    /// The key a death of the worker over what it sent is put down to
    /// ([`super::senders`]): the token, so that one agent's updates held back hold back
    /// none of its owner's own, nor another agent's; the user, for a session.
    fn sender(self) -> String {
        self.agent.map_or_else(
            || self.user_id.to_owned(),
            |agent| format!("token:{}", agent.token_id),
        )
    }
}

/// What a document took of the nodes: its update, the epoch it was of, and what the
/// room is to be told.
type Appended = (Vec<u8>, i64, Taken);

impl Collab {
    /// Adds `nodes`, top-level nodes of a body, after the last of the body of the card
    /// `card_id` of `app`'s organization, by `by`, and writes the body to the
    /// card. Opens the document if it is not open; an open room is sent the update. An
    /// agent's addition is kept as its version, after the body before, which keeps what
    /// the editors typed until then.
    ///
    /// # Errors
    ///
    /// As [`Collab::take`]: the card is gone, the document would be larger than it may
    /// (`document_too_large`), the worker ended over it (`document_unavailable`), or
    /// the token's updates are held back for a while.
    pub async fn append(
        &self,
        app: &App,
        card_id: &str,
        by: Appender<'_>,
        nodes: &[Value],
    ) -> Result<(), AppError> {
        let sender = by.sender();
        self.held_back(app, &sender)?;
        let nodes = serde_json::to_vec(nodes)
            .map_err(|error| AppError::internal(format_args!("the nodes as JSON: {error}")))?;
        let user_id = by.user_id.to_owned();
        let agent = by.agent.map(|agent| {
            (
                agent.token_id.to_owned(),
                agent.token_name.to_owned(),
                agent.notify_self,
            )
        });
        let appended = self
            .on_slot_while(
                app,
                card_id,
                None,
                move |collab, app, card_id, mut slot| async move {
                    let (collab, app, card_id, slot) = (&collab, &app, card_id.as_str(), &mut slot);
                    let by = Added {
                        user_id: &user_id,
                        sender: &sender,
                        agent: agent
                            .as_ref()
                            .map(|(token_id, token_name, notify_self)| ByAgent {
                                token_id,
                                token_name,
                                notify_self: *notify_self,
                            }),
                    };
                    let appended = collab.append_held(app, card_id, slot, by, &nodes).await;
                    collab.retire(slot);
                    appended
                },
            )
            .await?
            .map_err(Unanswered::into_error)?;
        let (update, epoch, taken) = appended;
        self.send_appended(app, card_id, epoch, &update, &taken)
            .await;
        Ok(())
    }
}

/// Who adds to a held card's body, as [`Collab::append_held`] is told.
#[derive(Debug, Clone, Copy)]
struct Added<'a> {
    user_id: &'a str,
    sender: &'a str,
    agent: Option<ByAgent<'a>>,
}

impl Collab {
    /// [`Collab::append`] on a held slot: the nodes added and written to the card, and
    /// for an agent the versions around them kept. Once the nodes are added, what fails
    /// after is logged, not answered: they are added.
    async fn append_held(
        &self,
        app: &App,
        card_id: &str,
        slot: &mut Held,
        by: Added<'_>,
        nodes: &[u8],
    ) -> Result<Result<Appended, Unanswered>, AppError> {
        if by.agent.is_some() {
            self.keep_before_agent(app, card_id, slot).await?;
        }
        let sent = Sent {
            user_id: by.user_id,
            agent: by.agent,
        };
        let appended = self
            .append_in(app, card_id, slot, sent, by.sender, nodes)
            .await;
        if appended.is_ok() {
            let written = match by.agent {
                Some(agent) => {
                    self.keep_agent_version(app, card_id, slot, by.user_id, agent)
                        .await
                }
                None => {
                    self.write_pending(app, card_id, slot, whole::DueVersion::Keep)
                        .await
                }
            };
            // The nodes are in the document and its log by now: the room is sent them
            // and the caller told they were added. The card is written from the log as
            // a document is, and the agent's version kept with the next one.
            if let Err(error) = written {
                tracing::warn!(
                    %error,
                    card_id,
                    "nodes added to a body were not written to its card yet"
                );
            }
        }
        Ok(appended)
    }
}
