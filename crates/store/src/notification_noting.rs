//! Whom a write of a block by a person may be told to, noted on the list
//! (`notification_outbox`) in the transaction of that write, and who watches what that
//! write made or touched (`subscriptions`).
//!
//! A write notes the users its body mentions that the body before did not, the users a
//! person property of a card takes that it did not hold, and, of a new comment, the
//! watchers of the card or the page it is on. Whoever writes a card or a page, or
//! comments on either, watches it from then on, until they stop; who is given a card
//! comes to watch it when the server works off that row, if they read the card then
//! ([`Store::watch_given`]). A row of the list says who may be told, never that they
//! are (`notification_outbox.rs`).

use std::collections::BTreeSet;

use fvoci_model::{
    Block, BlockType, CardPropertyDefinition, NotificationKind, OrgKind, assigned_users,
};
use serde_json::Value;

use crate::body_docs::{self, TypedUpdate, TypingAgent};
use crate::body_versions::AgentOfHandle;
use crate::containers;
use crate::db::{Dialect, Id, Noting, OptId, Param, Scope, Sql};
use crate::error::StoreError;
use crate::{ByAgent, Store};

/// How many identifiers one statement binds.
const BATCH: usize = 300;

/// The most people one write of a block notes for one kind: a body may hold as many
/// mentions as it has nodes, and each would be a row. Those that come first in the
/// body, or in the properties, are noted.
const MOST_NAMED_BY_A_WRITE: usize = 100;

/// Who is told of what a write of a block newly names, and as whose doing.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Telling {
    /// The people it names, as its writer's doing.
    Written,
    /// The people its body newly names, each as the doing of whoever typed the name:
    /// a body written from its document, by the last of several who typed it. The
    /// updates of the document past `since` (the last the block's body held) say who
    /// sent what: the first of them that holds a user's identifier is where that
    /// user was named. A name no update holds any more (they were folded into the
    /// document's state before the body was written) is the writer's doing.
    Typed {
        /// The last update the body of the block held before this write.
        since: i64,
    },
    /// Nobody, as its writer asked: what it writes was said before (a copy the web
    /// makes, a change taken back or made again). Who writes a new card, page or
    /// comment watches it all the same; nobody else comes to.
    Quiet,
    /// Nobody: a copy, a card or a board made from a template, what comes back from
    /// the trash or from an archive. Nobody comes to watch anything by it either.
    Nobody,
}

impl Telling {
    /// The people a write names, as its writer's doing, unless the scope tells nobody.
    pub(crate) fn of(scope: &Scope<'_>) -> Self {
        match scope.notes() {
            Noting::Named => Self::Written,
            Noting::Quietly => Self::Quiet,
            #[cfg(feature = "test-util")]
            Noting::Nobody => Self::Nobody,
        }
    }

    /// As [`Telling::of`], for a body written from its document.
    pub(crate) fn of_typed(scope: &Scope<'_>, since: i64) -> Self {
        match Self::of(scope) {
            Self::Written => Self::Typed { since },
            telling => telling,
        }
    }
}

/// The users a property value of a card held before a write: none for a block that is
/// new.
fn assigned_before<'b>(
    before: Option<&'b Block>,
    definitions: &[CardPropertyDefinition],
) -> BTreeSet<&'b str> {
    before
        .and_then(|before| before.fields.get("properties"))
        .and_then(Value::as_object)
        .map_or_default(|values| assigned_users(values, definitions).into_iter().collect())
}

/// Notes whom the write of `block` by `writer` newly names, in the transaction of the
/// write: the users its body mentions that the body of `before` did not, and, of a
/// card, the users its person properties take that those of `before` did not hold.
/// `before` is the block as it was stored, `None` for a new one. A new comment notes
/// the watchers of what it is on, but for those it names: they are told of that.
///
/// A comment is about the card, page or unit it is on; a card that is a template of its
/// board tells nobody, and neither does anything else that is no card, page, unit or
/// comment (a chapter and a diary entry tell nobody). Who writes a new card, page, unit
/// or comment watches it, quietly too; who gives a card to themselves watches it. Who is
/// given a card by somebody else comes to watch it as they are told of it
/// ([`Store::watch_given`]): only if they read it then.
pub(crate) async fn note_named(
    scope: &mut Scope<'_>,
    block: &Block,
    before: Option<&Block>,
    writer: &str,
    telling: Telling,
) -> Result<(), StoreError> {
    if matches!(telling, Telling::Nobody) {
        return Ok(());
    }
    let about = match block.block_type {
        BlockType::Card | BlockType::Page | BlockType::Unit => block.id.as_str(),
        BlockType::Comment => block.parent_id.as_str(),
        _ => return Ok(()),
    };
    if block.fields.get("isTemplate") == Some(&Value::Bool(true)) {
        return Ok(());
    }
    // Owned: the scope is written to while the agent is asked of.
    let agent = scope.agent().map(AgentOfHandle::of);
    if matches!(telling, Telling::Quiet) {
        if before.is_none() {
            let event = Noted {
                block,
                about,
                writer,
                agent: None,
                typed: &[],
            };
            event.watch(scope, &[writer]).await?;
        }
        return Ok(());
    }
    let mentioned_before: BTreeSet<&str> = before
        .and_then(|before| before.body.as_ref())
        .map_or_default(|body| body.mention_ids().into_iter().collect());
    let mentioned: Vec<&str> = block
        .body
        .as_ref()
        .map_or_default(|body| body.mention_ids())
        .into_iter()
        .filter(|user| !mentioned_before.contains(user))
        .collect();
    // Who typed what is read only for a body that names somebody new.
    let typed = match telling {
        Telling::Typed { since } if !mentioned.is_empty() => {
            body_docs::updates_since(scope, &block.id, since).await?
        }
        _ => Vec::new(),
    };
    let event = Noted {
        block,
        about,
        writer,
        agent: agent.as_ref().map(AgentOfHandle::by),
        typed: &typed,
    };
    event
        .note(scope, NotificationKind::Mention, &mentioned)
        .await?;
    if before.is_none() && matches!(telling, Telling::Written) {
        if block.block_type == BlockType::Comment {
            event.note_watchers(scope, &mentioned).await?;
        }
        event.watch(scope, &[writer]).await?;
    }

    // The definitions are read only for a card whose values changed: most writes
    // change none.
    let values = block.fields.get("properties").and_then(Value::as_object);
    let values_before = before.and_then(|before| before.fields.get("properties"));
    let (BlockType::Card, Some(values)) = (&block.block_type, values) else {
        return Ok(());
    };
    if values.is_empty() || values_before.and_then(Value::as_object) == Some(values) {
        return Ok(());
    }
    let Some(board) = containers::find_container(scope, &block.container_id).await? else {
        return Ok(());
    };
    let held = assigned_before(before, &board.card_properties);
    let assigned: Vec<&str> = assigned_users(values, &board.card_properties)
        .into_iter()
        .filter(|user| !held.contains(user))
        .collect();
    event
        .note(scope, NotificationKind::Assignment, &assigned)
        .await?;
    // Nobody is told of their own doing, and so nothing else makes them watch it.
    if assigned.contains(&writer) {
        event.watch(scope, &[writer]).await?;
    }
    Ok(())
}

/// Whether `bytes` hold `word` somewhere. An update of a document holds the text of
/// what it puts in as it is, an identifier that an attribute takes among it.
fn holds(bytes: &[u8], word: &[u8]) -> bool {
    !word.is_empty() && bytes.windows(word.len()).any(|window| window == word)
}

/// Ends a statement that notes rows of the list: a row that is there already (the
/// same person, kind and place) moves its `seq` on and is due again now. In one order,
/// whoever notes: two writers that name the same people wait for one another's rows,
/// and never each for the other's.
const NOTED_AGAIN: &str = " ORDER BY u.id ON CONFLICT (org_id, recipient_id, kind, source_id) \
     DO UPDATE SET seq = CASE WHEN notification_outbox.seq >= excluded.seq \
     THEN notification_outbox.seq + 1 ELSE excluded.seq END, \
     due_at = excluded.due_at, attempts = 0, actor_id = excluded.actor_id, \
     actor_token_name = excluded.actor_token_name, block_id = excluded.block_id, \
     container_id = excluded.container_id";

/// A write of a block that names people.
struct Noted<'a> {
    /// The block that was written: where it happened.
    block: &'a Block,
    /// The card or the page it is about.
    about: &'a str,
    writer: &'a str,
    /// The agent the writer wrote through ([`crate::Store::by_agent`]), if one.
    agent: Option<ByAgent<'a>>,
    /// The updates of its document that its body did not hold before, the oldest
    /// first, each with who sent it ([`Telling::Typed`]); none for a block that was
    /// written as it is.
    typed: &'a [TypedUpdate],
}

/// Who did what a row of the list tells: the user, and the agent they did it through.
type Doer<'a> = (&'a str, Option<&'a str>);

impl Noted<'_> {
    /// Who named `user`: whoever sent the first update that holds the user's
    /// identifier, or the writer where no update does, with the name of the token
    /// they sent it through, if any. `None` where that is the user: nobody is told of
    /// their own doing, but of an agent's of theirs that tells them
    /// ([`ByAgent::notify_self`]).
    fn doer_for(&self, user: &str) -> Option<Doer<'_>> {
        let (doer, agent) = self
            .typed
            .iter()
            .find(|update| holds(&update.bytes, user.as_bytes()))
            .map_or((self.writer, self.agent), |update| {
                (
                    update.written_by.as_str(),
                    update.agent.as_ref().map(TypingAgent::by),
                )
            });
        let told_of_own = agent.is_some_and(|agent| agent.notify_self);
        (doer != user || told_of_own).then_some((doer, agent.map(|agent| agent.token_name)))
    }

    /// Begins a statement that notes rows of `kind` by `doer` on the list, for the
    /// block's board or wiki as `bo`, its workspace as `w` and its organization as
    /// `o`; the recipients follow as `u`.
    fn noting(&self, scope: &Scope<'_>, kind: NotificationKind, (doer, token): Doer<'_>) -> Sql {
        let now = scope.now_millis();
        let mut sql = scope.sql(
            "INSERT INTO notification_outbox (org_id, recipient_id, kind, source_id, \
             block_id, container_id, actor_id, actor_token_name, seq, due_at) \
             SELECT w.org_id, u.id, ",
        );
        sql.bind(kind.as_str())
            .push_bind(", ", Id(&self.block.id))
            .push_bind(", ", Id(self.about))
            .push_bind(", bo.id, ", OptId(doer))
            .push_bind(", ", Param::OptText(token.map(str::to_owned)))
            .push_bind(", ", now)
            .push_bind(", ", now)
            .push(" FROM containers AS bo JOIN workspaces AS w ON w.id = bo.workspace_id")
            .push(&scope.same_organization(&[("w", "bo")]))
            .push(" JOIN organizations AS o ON o.id = w.org_id");
        sql
    }

    /// Narrows a statement of [`Noted::noting`] to what is noted
    /// at all: a board or a wiki that is no template, of an organization people work
    /// in together (in the space of one account nobody else reads anything), for users
    /// that are accounts (one that was erased is none).
    fn push_noted(&self, sql: &mut Sql, kind: NotificationKind) {
        sql.push_bind(
            " WHERE u.erased_at = 0 AND bo.id = ",
            Id(&self.block.container_id),
        )
        .push_bind(" AND bo.is_template = ", false)
        .push_bind(" AND (o.kind = ", OrgKind::Shared.as_str())
        .push_bind(" OR ", kind == NotificationKind::DueSoon)
        .push(")");
    }

    /// Notes that `users` may be told of `kind`: each as the doing of whoever named
    /// them, for no one of their own ([`Noted::doer_for`]).
    async fn note(
        &self,
        scope: &mut Scope<'_>,
        kind: NotificationKind,
        users: &[&str],
    ) -> Result<(), StoreError> {
        let told: Vec<(&str, Doer<'_>)> = users
            .iter()
            .filter(|user| fvoci_model::is_id(user))
            .filter_map(|user| Some((*user, self.doer_for(user)?)))
            .take(MOST_NAMED_BY_A_WRITE)
            .collect();
        let doers: BTreeSet<Doer<'_>> = told.iter().map(|(_, doer)| *doer).collect();
        for doer in doers {
            let users: Vec<&str> = told
                .iter()
                .filter(|(_, by)| *by == doer)
                .map(|(user, _)| *user)
                .collect();
            for batch in users.chunks(BATCH) {
                let mut sql = self.noting(scope, kind, doer);
                sql.push(" JOIN users AS u ON u.id IN (")
                    .bind_list(batch.iter().map(|user| Id(user)))
                    .push(")");
                self.push_noted(&mut sql, kind);
                sql.push(NOTED_AGAIN);
                scope.execute(sql).await?;
            }
        }
        Ok(())
    }

    /// Notes that the watchers of the card or the page a new comment is on may be told
    /// of it, as its writer's doing: all of them but the writer (unless an agent of
    /// theirs wrote it that tells them) and those of `mentioned`, who are told of the
    /// mention.
    async fn note_watchers(
        &self,
        scope: &mut Scope<'_>,
        mentioned: &[&str],
    ) -> Result<(), StoreError> {
        let doer = (self.writer, self.agent.map(|agent| agent.token_name));
        let mut sql = self.noting(scope, NotificationKind::Comment, doer);
        sql.push(" JOIN subscriptions AS s ON s.block_id = ")
            .bind(Id(self.about))
            .push(&scope.same_organization(&[("s", "bo")]))
            .push(" JOIN users AS u ON u.id = s.user_id");
        self.push_noted(&mut sql, NotificationKind::Comment);
        if !self.agent.is_some_and(|agent| agent.notify_self) {
            sql.push_bind(" AND u.id <> ", Id(self.writer));
        }
        let named: Vec<&str> = mentioned
            .iter()
            .copied()
            .filter(|user| fvoci_model::is_id(user))
            .take(MOST_NAMED_BY_A_WRITE)
            .collect();
        if !named.is_empty() {
            sql.push(" AND u.id NOT IN (")
                .bind_list(named.iter().map(|user| Id(user)))
                .push(")");
        }
        sql.push(NOTED_AGAIN);
        scope.execute(sql).await?;
        Ok(())
    }

    /// Makes `users` watchers of the card or the page the write is about, where they
    /// are not already ([`watch`]).
    async fn watch(&self, scope: &mut Scope<'_>, users: &[&str]) -> Result<(), StoreError> {
        watch(scope, self.about, users).await
    }
}

/// Makes `users` watchers of `block_id`, where they are not already: on a board or a
/// wiki that is no template, of an organization people work in together, for users
/// that are accounts, as [`Noted::push_noted`] narrows what is noted.
async fn watch(scope: &mut Scope<'_>, block_id: &str, users: &[&str]) -> Result<(), StoreError> {
    let users: Vec<&str> = users
        .iter()
        .copied()
        .filter(|user| fvoci_model::is_id(user))
        .collect();
    let now = scope.now_millis();
    let postgres = scope.dialect() == Dialect::Postgres;
    for batch in users.chunks(BATCH) {
        let mut sql = scope.sql(if postgres {
            "INSERT INTO subscriptions (org_id, block_type, block_id, \
             user_id, notified_at, created_at) SELECT b.org_id, b.type, b.id, u.id, "
        } else {
            "INSERT INTO subscriptions (block_type, block_id, \
             user_id, notified_at, created_at) SELECT b.type, b.id, u.id, "
        });
        sql.bind(now)
            .push_bind(", ", now)
            .push(" FROM blocks AS b JOIN containers AS bo ON bo.id = b.container_id")
            .push(&scope.same_organization(&[("bo", "b")]))
            .push(" JOIN workspaces AS w ON w.id = bo.workspace_id")
            .push(&scope.same_organization(&[("w", "bo")]))
            .push(" JOIN organizations AS o ON o.id = w.org_id")
            .push(" JOIN users AS u ON u.id IN (")
            .bind_list(batch.iter().map(|user| Id(user)))
            .push(")")
            .push_bind(" WHERE u.erased_at = 0 AND b.id = ", Id(block_id))
            .push_bind(" AND bo.is_template = ", false)
            .push_bind(" AND o.kind = ", OrgKind::Shared.as_str())
            .push(" ORDER BY u.id ON CONFLICT (block_id, user_id) DO NOTHING");
        scope.execute(sql).await?;
    }
    Ok(())
}

/// Who added whom to which shared-organization container, for [`Store::note_added_to_container`].
#[derive(Debug, Clone, Copy)]
pub struct AddedToContainerNotice<'a> {
    /// Target board, wiki, or course identifier.
    pub container_id: &'a str,
    /// User who received membership.
    pub recipient_id: &'a str,
    /// User who granted membership.
    pub actor_id: &'a str,
}

impl Store {
    /// Makes a user who was given a card a watcher of it, where they are not already:
    /// for the server that tells them of it, once it found that they read the card.
    /// Nothing is made of a card of a template, of one's own space, for an erased user
    /// or of a card that is gone.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn watch_given(&self, block_id: &str, user_id: &str) -> Result<(), StoreError> {
        self.with_write_conn(async |scope| watch(scope, block_id, &[user_id]).await)
            .await
    }

    /// Notes in `notification_outbox` that `notice.recipient_id` was added to a
    /// non-template shared-organization container `notice.container_id` by
    /// `notice.actor_id`.
    ///
    /// # Errors
    ///
    /// Returns [`StoreError::Database`] if the statement fails.
    pub async fn note_added_to_container(
        &self,
        notice: AddedToContainerNotice<'_>,
    ) -> Result<(), StoreError> {
        let AddedToContainerNotice {
            container_id,
            recipient_id,
            actor_id,
        } = notice;
        if actor_id.is_empty()
            || actor_id == recipient_id
            || !fvoci_model::is_id(recipient_id)
            || !fvoci_model::is_id(container_id)
        {
            return Ok(());
        }
        self.with_write_conn(async |scope| {
            note_added_to_container_in_tx(scope, container_id, recipient_id, actor_id).await
        })
        .await
    }
}

pub(crate) async fn note_added_to_container_in_tx(
    scope: &mut Scope<'_>,
    container_id: &str,
    recipient_id: &str,
    actor_id: &str,
) -> Result<(), StoreError> {
    if actor_id.is_empty() || actor_id == recipient_id {
        return Ok(());
    }
    let now = scope.now_millis();
    let mut sql = scope.sql(
        "INSERT INTO notification_outbox (org_id, recipient_id, kind, source_id, \
         block_id, container_id, actor_id, seq, due_at) SELECT w.org_id, u.id, ",
    );
    sql.bind(NotificationKind::AddedToContainer.as_str())
        .push_bind(", ", Id(container_id))
        .push_bind(", ", Id(container_id))
        .push_bind(", bo.id, ", OptId(actor_id))
        .push_bind(", ", now)
        .push_bind(", ", now)
        .push(" FROM containers AS bo JOIN workspaces AS w ON w.id = bo.workspace_id")
        .push(&scope.same_organization(&[("w", "bo")]))
        .push(" JOIN organizations AS o ON o.id = w.org_id")
        .push_bind(" JOIN users AS u ON u.id = ", Id(recipient_id))
        .push_bind(" WHERE u.erased_at = 0 AND bo.id = ", Id(container_id))
        .push_bind(" AND bo.is_template = ", false)
        .push_bind(" AND o.kind = ", OrgKind::Shared.as_str())
        .push(NOTED_AGAIN);
    scope.execute(sql).await?;
    Ok(())
}
