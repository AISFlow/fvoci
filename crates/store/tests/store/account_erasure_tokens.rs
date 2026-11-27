//! Erasing an account takes the names of its tokens out of every copy of them: the
//! versions and documents an agent wrote through one, the comments that say which
//! agent wrote them, live and in the trash, and what others were told, or are to be
//! told, of what the account did through one. In every organization, also one the
//! account had left before (PostgreSQL, which holds several). The names of somebody
//! else's tokens stay.

use fvoci_model::{ApiTokenScope, Block, BlockPatch, BlockType, Body, BodyChange, new_id};
use fvoci_store::{
    ByAgent, DbType, Erasing, GrantedWorkspace, Issuance, Store, Tenant, TokenToIssue,
};
use serde_json::json;

use crate::account_erasure::{NOBODY, T0, board_of, erase, founder, holding};
use crate::common;
use crate::provider_tokens::session;

/// The name the leaver gave their token: what they wrote, as their own name is.
const LEAVERS_TOKEN: &str = "leaver-laptop-token";

/// The name the founder gave theirs, which stays.
const FOUNDERS_TOKEN: &str = "founder-desk-token";

/// Where a token's name is copied.
const COPIES: [&str; 4] = [
    "body_docs.agent_token_name",
    "body_versions.actor_token_name",
    "notification_outbox.actor_token_name",
    "notifications.actor_token_name",
];

/// A token of `user_id` named `name`, made with a password session and reaching
/// `workspace_id`: its identifier.
async fn token_of(store: &Store, user_id: &str, workspace_id: &str, name: &str) -> String {
    let session_id = session(store, user_id, None).await;
    let org_id = store
        .tenant()
        .org_id()
        .expect("a handle of an organization");
    let granted = [GrantedWorkspace {
        workspace_id,
        org_id: &org_id,
    }];
    let id = new_id();
    let issued = store
        .issue_api_token(&TokenToIssue {
            id: &id,
            user_id,
            session_id: &session_id,
            name,
            token_prefix: "fvoci_pat_abcdef",
            token_digest: &new_id(),
            scope: ApiTokenScope::Write,
            workspaces: &granted,
            allow_personal_space: false,
            notify_self: true,
            created_at: T0,
            expires_at: T0 + 1000,
        })
        .await
        .expect("asked");
    match issued {
        Issuance::Issued(token) => token.id,
        refused => panic!("{refused:?}"),
    }
}

/// What an agent wrote through a token on a card, as the store keeps it.
struct Written<'a> {
    board: &'a str,
    card: &'a str,
    /// The version of the card's body the agent wrote.
    version: i64,
    token_id: &'a str,
    token_name: &'a str,
    /// Whose token it is.
    actor_id: &'a str,
    /// Who was told of it.
    recipient_id: &'a str,
    /// What they were told of.
    kind: &'a str,
    /// Whether the card's document is still waiting with what the agent typed.
    document: Document,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Document {
    Waiting,
    Kept,
}

/// Stores, in the organization of `store`, the version `written` names, the
/// notification sent of it and the one to come, and the card's document if it waits.
/// (The bodies' tables name their organization on PostgreSQL alone, by the handle's.)
async fn store_written(store: &Store, written: &Written<'_>) {
    let Written {
        board,
        card,
        version,
        token_id,
        token_name: name,
        actor_id,
        recipient_id,
        kind,
        document,
    } = written;
    let org_id = store
        .tenant()
        .org_id()
        .expect("a handle of an organization");
    let digest = match common::db_type() {
        DbType::Postgres => "'\\x00'::bytea",
        DbType::Sqlite => "x'00'",
    };
    let mut statements = vec![
        format!(
            "INSERT INTO body_versions (block_id, version, body_digest, files, kept_at, cause, \
             actor_token_id, actor_token_name) \
             VALUES ('{card}', {version}, {digest}, '[]', {T0}, 'agent', '{token_id}', '{name}')"
        ),
        format!(
            "INSERT INTO notifications (org_id, id, recipient_id, kind, source_id, block_id, \
             container_id, actor_id, created_at, actor_token_name) VALUES ('{org_id}', '{}', \
             '{recipient_id}', '{kind}', '{card}', '{card}', '{board}', '{actor_id}', {T0}, \
             '{name}')",
            new_id()
        ),
        format!(
            "INSERT INTO notification_outbox (org_id, recipient_id, kind, source_id, block_id, \
             container_id, actor_id, seq, due_at, actor_token_name) VALUES ('{org_id}', '{recipient_id}', \
             '{kind}', '{card}', '{card}', '{board}', '{actor_id}', {version}, {T0}, '{name}')"
        ),
    ];
    if *document == Document::Waiting {
        statements.push(format!(
            "INSERT INTO body_docs (block_id, state, state_seq, log_end, projected_seq, \
             body_digest, epoch, created_at, updated_at, agent_token_id, agent_token_name) \
             VALUES ('{card}', {digest}, 0, 0, 0, {digest}, 0, {T0}, {T0}, '{token_id}', \
             '{name}')"
        ));
    }
    for statement in statements {
        store
            .execute_unchecked(&statement)
            .await
            .unwrap_or_else(|error| panic!("{statement}: {error}"));
    }
}

/// A paragraph of `words`, as the text of a comment.
fn text(words: &str) -> Body {
    Body::parse(json!({"type": "doc", "content": [
        {"type": "paragraph", "content": [{"type": "text", "text": words}]}
    ]}))
    .expect("the text may be stored")
    .expect("a paragraph is some text")
}

/// The agent that writes through the token `token_id` named `token_name`.
fn through<'a>(token_id: &'a str, token_name: &'a str) -> ByAgent<'a> {
    ByAgent {
        token_id,
        token_name,
        notify_self: true,
    }
}

/// A comment on `card` of `board` that `user_id` writes, through `agent` if there is
/// one, telling nobody: as it is stored.
async fn comment(
    store: &Store,
    (board, card): (&str, &str),
    user_id: &str,
    agent: Option<ByAgent<'_>>,
) -> Block {
    let mut comment = Block {
        id: new_id(),
        container_id: board.to_owned(),
        parent_id: card.to_owned(),
        block_type: BlockType::Comment,
        body: Some(text("first")),
        ..Block::default()
    };
    agent
        .map_or_else(|| store.clone(), |agent| store.by_agent(agent))
        .telling_nobody()
        .insert_block(&mut comment, user_id)
        .await
        .expect("the comment is stored");
    comment
}

/// The comments whose agents are named: two of the leaver's agent (one put in the
/// trash), the founder's comment whose text the leaver's agent wrote and the founder
/// changed after, and one of the founder's agent, which stays.
async fn comments_by_agents(
    store: &Store,
    on_card: (&str, &str),
    (leaver, leavers): (&str, &str),
    (founder, founders): (&str, &str),
) {
    let by_leaver = through(leavers, LEAVERS_TOKEN);
    comment(store, on_card, leaver, Some(by_leaver)).await;
    let trashed = comment(store, on_card, leaver, Some(by_leaver)).await;
    store
        .delete_block(&trashed.id, leaver)
        .await
        .expect("the comment is put in the trash");
    let rewritten = comment(store, on_card, founder, None).await;
    let new_text = BlockPatch {
        body: BodyChange::Set(text("second")),
        ..BlockPatch::default()
    };
    store
        .by_agent(by_leaver)
        .telling_nobody()
        .patch_block(&rewritten.id, &new_text, leaver)
        .await
        .expect("the leaver's agent writes its text");
    let renamed = BlockPatch {
        title: Some("kept".to_owned()),
        ..BlockPatch::default()
    };
    let changed = store
        .telling_nobody()
        .patch_block(&rewritten.id, &renamed, founder)
        .await
        .expect("the founder changes it last");
    assert_eq!(
        changed.fields.get("agentTokenName"),
        Some(&json!(LEAVERS_TOKEN)),
        "a change of no text keeps who wrote it"
    );
    comment(
        store,
        on_card,
        founder,
        Some(through(founders, FOUNDERS_TOKEN)),
    )
    .await;
}

#[tokio::test]
async fn an_erased_account_leaves_no_name_of_its_tokens() {
    let test = common::setup().await;
    let system = test.system();
    let founder = founder(&test).await;
    let leaver = common::user(&test, "leaver").await;
    let workspace = common::workspace_of(&test, &[&founder, &leaver]).await;
    let leavers = token_of(&test, &leaver, &workspace, LEAVERS_TOKEN).await;
    let founders = token_of(&test, &founder, &workspace, FOUNDERS_TOKEN).await;
    let (board, card) = board_of(&test, &workspace, &founder).await;
    let by_the_leaver = Written {
        board: &board,
        card: &card,
        version: 1,
        token_id: &leavers,
        token_name: LEAVERS_TOKEN,
        actor_id: &leaver,
        recipient_id: &founder,
        kind: "comment",
        document: Document::Waiting,
    };
    store_written(&test, &by_the_leaver).await;
    // The founder's agent wrote the next version, and the founder is told of it.
    let by_the_founder = Written {
        version: 2,
        token_id: &founders,
        token_name: FOUNDERS_TOKEN,
        actor_id: &founder,
        kind: "mention",
        document: Document::Kept,
        ..by_the_leaver
    };
    store_written(&test, &by_the_founder).await;
    comments_by_agents(
        &test,
        (&board, &card),
        (&leaver, &leavers),
        (&founder, &founders),
    )
    .await;
    // On PostgreSQL, an organization the leaver had left before they were erased,
    // where their agent wrote on a card and its people were told.
    if common::db_type() == DbType::Postgres {
        let (other_org, other_workspace) =
            common::organization(&test.scoped(Tenant::Unset), "Other").await;
        let other = test.for_org(&other_org).expect("an identifier");
        let their_founder = common::user(&other, "their-founder").await;
        common::works_in(&other, &other_workspace, &their_founder).await;
        let (their_board, their_card) = board_of(&other, &other_workspace, &their_founder).await;
        let there = Written {
            board: &their_board,
            card: &their_card,
            recipient_id: &their_founder,
            ..by_the_leaver
        };
        store_written(&other, &there).await;
    }
    let mut before = [
        &["api_tokens.name", "blocks.fields", "deleted_blocks.block"][..],
        &COPIES,
    ]
    .concat();
    before.sort_unstable();
    assert_eq!(
        holding(&system, LEAVERS_TOKEN).await,
        before,
        "every copy of the name, before"
    );
    assert!(
        !holding(&system, FOUNDERS_TOKEN).await.is_empty(),
        "the founder's token's name is copied"
    );

    erase(&test, &leaver, Erasing::AtOnce, T0 + 1).await;

    assert_eq!(
        holding(&system, LEAVERS_TOKEN).await,
        NOBODY,
        "the leaver's token's name is nowhere"
    );
    assert_eq!(
        holding(&system, FOUNDERS_TOKEN).await,
        [
            "api_tokens.name",
            "blocks.fields",
            "body_versions.actor_token_name",
            "notification_outbox.actor_token_name",
            "notifications.actor_token_name",
        ],
        "the founder's token's name stays"
    );
    let kept: Vec<String> = system
        .texts_unchecked(&format!(
            "SELECT cause FROM body_versions WHERE block_id = '{card}' ORDER BY version"
        ))
        .await
        .expect("read");
    assert_eq!(kept, ["agent", "agent"], "what the agents wrote stays");
    test.teardown().await;
}
