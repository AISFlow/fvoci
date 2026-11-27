//! Editing a card's body together over the websocket: the rooms of the connections
//! that hold a card's document, over real connections.

use std::collections::VecDeque;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use fvoci_collab::testing::Editor;
use fvoci_model::{Card, ContainerMember, ContainerVisibility};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

use crate::collab::{body_of, card};
use crate::common::{Client, ROOT_WORKSPACE_ID, Setup, TEST_WORKSPACE_ID, TestServer, WebSocket};

/// How long to wait for a message that should arrive.
pub(crate) const PATIENCE: Duration = Duration::from_secs(10);
/// How long to wait before deciding that no message arrives.
pub(crate) const QUIET: Duration = Duration::from_millis(300);

/// The types of a sync message.
pub(crate) const STEP_1: u64 = 0;
pub(crate) const STEP_2: u64 = 1;
pub(crate) const UPDATE: u64 = 2;

pub(crate) fn paragraphs(texts: &[&str]) -> Value {
    let content: Vec<Value> = texts
        .iter()
        .map(|text| json!({"type": "paragraph", "content": [{"type": "text", "text": text}]}))
        .collect();
    json!({"type": "doc", "content": content})
}

fn put_varuint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push(u8::try_from(value & 0x7f).expect("seven bits") | 0x80);
        value >>= 7;
    }
    out.push(u8::try_from(value).expect("seven bits"));
}

pub(crate) fn take_varuint(bytes: &mut &[u8]) -> u64 {
    let mut value = 0;
    let mut shift = 0;
    loop {
        let (byte, rest) = bytes.split_first().expect("a varuint ends in the frame");
        *bytes = rest;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return value;
        }
        shift += 7;
    }
}

/// An awareness update of one state, as an editor's library writes it: whose it
/// says it is, which telling, and the state as JSON.
pub(crate) fn awareness(client: u64, clock: u64, state: &Value) -> Vec<u8> {
    let state = state.to_string();
    let mut out = vec![1];
    put_varuint(&mut out, client);
    put_varuint(&mut out, clock);
    put_varuint(&mut out, u64::try_from(state.len()).expect("a length"));
    out.extend_from_slice(state.as_bytes());
    out
}

/// A binary frame of carets as `contracts/websocket.md` has it.
pub(crate) fn caret_frame(doc: u32, epoch: i64, update: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varuint(&mut out, u64::from(doc));
    put_varuint(&mut out, 1);
    put_varuint(&mut out, u64::try_from(epoch).expect("an epoch"));
    put_varuint(&mut out, u64::try_from(update.len()).expect("a length"));
    out.extend_from_slice(update);
    out
}

/// A binary frame of a sync message as `contracts/websocket.md` has it.
pub(crate) fn frame(doc: u32, epoch: i64, sync: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    put_varuint(&mut out, u64::from(doc));
    put_varuint(&mut out, 0);
    put_varuint(&mut out, u64::try_from(epoch).expect("an epoch"));
    put_varuint(&mut out, sync);
    put_varuint(&mut out, u64::try_from(payload.len()).expect("a length"));
    out.extend_from_slice(payload);
    out
}

/// A binary frame the server sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    pub(crate) doc: u32,
    pub(crate) epoch: i64,
    pub(crate) sync: u64,
    pub(crate) payload: Vec<u8>,
}

/// What the server told of one caret: a frame of carets with its one state.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Told {
    pub(crate) doc: u32,
    pub(crate) epoch: i64,
    /// The number the server tells the caret under, and which telling this is.
    pub(crate) number: u64,
    pub(crate) clock: u64,
    /// The awareness state; `null` for a caret that is gone.
    pub(crate) state: Value,
}

fn read_frame(mut bytes: &[u8]) -> Incoming {
    let doc = u32::try_from(take_varuint(&mut bytes)).expect("a room");
    let kind = take_varuint(&mut bytes);
    let epoch = i64::try_from(take_varuint(&mut bytes)).expect("an epoch");
    let sync = match kind {
        0 => take_varuint(&mut bytes),
        1 => 0,
        _ => panic!("a frame of no kind there is: {kind}"),
    };
    let length = usize::try_from(take_varuint(&mut bytes)).expect("a length");
    assert_eq!(bytes.len(), length, "the payload is the rest of the frame");
    if kind == 0 {
        return Incoming::Frame(Frame {
            doc,
            epoch,
            sync,
            payload: bytes.to_vec(),
        });
    }
    assert_eq!(take_varuint(&mut bytes), 1, "one caret a frame");
    let number = take_varuint(&mut bytes);
    let clock = take_varuint(&mut bytes);
    let length = usize::try_from(take_varuint(&mut bytes)).expect("a length");
    assert_eq!(bytes.len(), length, "the state is the rest of the update");
    Incoming::Caret(Told {
        doc,
        epoch,
        number,
        clock,
        state: serde_json::from_slice(bytes).expect("a state is JSON"),
    })
}

/// What a connection yields next.
#[derive(Debug, PartialEq)]
pub(crate) enum Incoming {
    Text(Value),
    Frame(Frame),
    Caret(Told),
    Closed,
    Nothing,
}

/// A websocket connection that edits one card's body, as the web app will: it holds
/// the document the server sent, takes what the server sends of it and sends what it
/// writes.
pub(crate) struct Peer {
    stream: WebSocket,
    /// The client identifier of its editor.
    client: u64,
    /// The room it is in, as the server named it; 0 for none.
    pub(crate) doc: u32,
    /// Which making of the document it holds.
    pub(crate) epoch: i64,
    pub(crate) editor: Option<Editor>,
    /// The JSON messages it read that no test asked for yet.
    texts: VecDeque<Value>,
    /// The sync messages it took into its document, in the order they came.
    pub(crate) taken: Vec<Frame>,
    /// What it was told of carets that no test asked for yet, in the order it came:
    /// of whatever room the frame named.
    pub(crate) carets: VecDeque<Told>,
    /// The state vector the server last sent it (a sync step 1), if it sent one.
    pub(crate) asked: Option<Vec<u8>>,
    /// Its last join named the document it holds: the server may send a step 1.
    named: bool,
}

impl Peer {
    /// Waits until the server has carried out what this connection sent before
    /// ([`crate::common::carried_out`]), without changing document state.
    pub(crate) async fn barrier(&mut self, th: &TestServer) {
        let came = crate::common::carried_out(th.app.ws(), &mut self.stream).await;
        assert!(
            came.is_empty(),
            "unexpected frames before barrier: {came:?}"
        );
    }

    /// A connection of the client's session, whose editor is `client`.
    pub(crate) async fn connect(of: &Client, client: u64) -> Self {
        Self::on(of.connect_websocket().await, client)
    }

    pub(crate) fn on(stream: WebSocket, client: u64) -> Self {
        Self {
            stream,
            client,
            doc: 0,
            epoch: 0,
            editor: None,
            texts: VecDeque::new(),
            taken: Vec::new(),
            carets: VecDeque::new(),
            asked: None,
            named: false,
        }
    }

    /// The client identifier of its editor.
    pub(crate) fn client(&self) -> u64 {
        self.client
    }

    /// Sends an awareness update as a frame of carets of the room it is in.
    pub(crate) async fn send_caret(&mut self, update: &[u8]) {
        self.send_binary(caret_frame(self.doc, self.epoch, update))
            .await;
    }

    /// What it is told next of a caret, reading until it comes.
    pub(crate) async fn caret(&mut self) -> Told {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            if let Some(told) = self.carets.pop_front() {
                return told;
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !left.is_zero() && self.step(left).await,
                "no caret came; read: {:?}",
                self.texts
            );
        }
    }

    pub(crate) async fn send_json(&mut self, message: Value) {
        self.stream
            .send(Message::Text(message.to_string().into()))
            .await
            .expect("the message is sent");
    }

    pub(crate) async fn send_binary(&mut self, bytes: Vec<u8>) {
        self.stream
            .send(Message::Binary(bytes.into()))
            .await
            .expect("the frame is sent");
    }

    /// Sends a frame the server may close the connection over before all of it is
    /// written.
    pub(crate) async fn try_send_binary(&mut self, bytes: Vec<u8>) {
        let _ = self.stream.send(Message::Binary(bytes.into())).await;
    }

    /// What comes next, without taking it: pings are passed over.
    pub(crate) async fn incoming(&mut self, wait: Duration) -> Incoming {
        loop {
            match tokio::time::timeout(wait, self.stream.next()).await {
                Err(_) => return Incoming::Nothing,
                Ok(None | Some(Err(_) | Ok(Message::Close(_)))) => return Incoming::Closed,
                Ok(Some(Ok(Message::Text(text)))) => {
                    return Incoming::Text(serde_json::from_str(&text).expect("JSON"));
                }
                Ok(Some(Ok(Message::Binary(bytes)))) => return read_frame(&bytes),
                Ok(Some(Ok(_))) => {}
            }
        }
    }

    /// Reads one message and does with it what an editor does: a `docReset` of
    /// another epoch drops the document, a sync message of the epoch it holds is
    /// taken into the document (the first step 2 makes it), any other JSON is kept
    /// for the test. Whether there was a message.
    pub(crate) async fn step(&mut self, wait: Duration) -> bool {
        match self.incoming(wait).await {
            Incoming::Nothing => return false,
            Incoming::Closed => {
                self.texts.push_back(json!({"type": "closed"}));
                return false;
            }
            Incoming::Text(message) => {
                if message["type"] == json!("docReset") && message["doc"] == json!(self.doc) {
                    let epoch = message["epoch"].as_i64().expect("an epoch");
                    if epoch != self.epoch {
                        self.epoch = epoch;
                        self.editor = None;
                    }
                }
                self.texts.push_back(message);
            }
            Incoming::Frame(frame) => {
                if frame.doc != self.doc || frame.epoch != self.epoch {
                    return true;
                }
                if frame.sync == STEP_1 {
                    // The server's state vector: what it holds, for the peer to send
                    // what it lacks. The document follows. Only who named the
                    // document it holds is sent one.
                    assert!(self.named, "a state vector nobody asked for: {frame:?}");
                    self.asked = Some(frame.payload.clone());
                    self.taken.push(frame);
                    return true;
                }
                assert!(matches!(frame.sync, STEP_2 | UPDATE), "{frame:?}");
                if let Some(editor) = &self.editor {
                    editor.receive(&frame.payload);
                } else {
                    assert_eq!(frame.sync, STEP_2, "the whole document comes first");
                    self.editor = Some(Editor::join(self.client, &frame.payload));
                }
                self.taken.push(frame);
            }
            Incoming::Caret(told) => {
                assert!(
                    told.doc != self.doc || self.editor.is_some(),
                    "a caret of a room comes after its document: {told:?}"
                );
                self.carets.push_back(told);
            }
        }
        true
    }

    /// Reads until nothing comes for a while.
    pub(crate) async fn settle(&mut self) {
        while self.step(QUIET).await {}
    }

    /// The next JSON message of `kind`, reading until it comes.
    pub(crate) async fn text(&mut self, kind: &str) -> Value {
        self.one_of(&[kind]).await
    }

    /// The next JSON message of one of `kinds`, reading until it comes.
    pub(crate) async fn one_of(&mut self, kinds: &[&str]) -> Value {
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let found = self.texts.iter().position(|message| {
                message["type"]
                    .as_str()
                    .is_some_and(|kind| kinds.contains(&kind))
            });
            if let Some(index) = found {
                return self.texts.remove(index).expect("it is there");
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !left.is_zero() && self.step(left).await,
                "none of {kinds:?} came; read: {:?}",
                self.texts
            );
        }
    }

    /// The JSON messages read so far that no test asked for.
    pub(crate) fn unread(&self) -> Vec<Value> {
        self.texts.iter().cloned().collect()
    }

    /// Asks to come into the room of a card, and waits for the answer: `docJoined`,
    /// after which it holds the document, or `docRefused`.
    pub(crate) async fn join(&mut self, workspace_id: &str, card_id: &str) -> Value {
        self.send_json(json!({
            "type": "joinDoc", "workspaceId": workspace_id, "cardId": card_id,
        }))
        .await;
        self.named = false;
        let deadline = tokio::time::Instant::now() + PATIENCE;
        loop {
            let answer = self.texts.iter().position(|message| {
                [json!("docJoined"), json!("docRefused")].contains(&message["type"])
            });
            if let Some(index) = answer {
                let answer = self.texts.remove(index).expect("it is there");
                if answer["type"] == json!("docJoined") {
                    self.doc =
                        u32::try_from(answer["doc"].as_u64().expect("a room")).expect("32 bits");
                    self.epoch = answer["epoch"].as_i64().expect("an epoch");
                    self.editor = None;
                    while self.editor.is_none() {
                        assert!(self.step(PATIENCE).await, "the document follows");
                    }
                }
                return answer;
            }
            let left = deadline.saturating_duration_since(tokio::time::Instant::now());
            assert!(
                !left.is_zero() && self.step(left).await,
                "no answer to joinDoc; read: {:?}",
                self.texts
            );
        }
    }

    /// Asks to come into the room of a card as a connection that holds the document
    /// already (it was away): the join names the epoch and the state vector of what
    /// its editor holds. Waits for `docJoined` and the document that follows it,
    /// which is taken into the editor it holds unless it is of another epoch; the
    /// answer.
    pub(crate) async fn join_holding(&mut self, workspace_id: &str, card_id: &str) -> Value {
        use base64::Engine as _;
        let held = base64::engine::general_purpose::STANDARD.encode(self.editor().state_vector());
        self.join_saying(workspace_id, card_id, self.epoch, &held)
            .await
    }

    /// As [`Self::join_holding`], saying what `epoch` and `state_vector` say it holds.
    pub(crate) async fn join_saying(
        &mut self,
        workspace_id: &str,
        card_id: &str,
        epoch: i64,
        state_vector: &str,
    ) -> Value {
        self.send_json(json!({
            "type": "joinDoc", "workspaceId": workspace_id, "cardId": card_id,
            "epoch": epoch, "stateVector": state_vector,
        }))
        .await;
        self.asked = None;
        self.named = true;
        let joined = self.text("docJoined").await;
        self.doc = u32::try_from(joined["doc"].as_u64().expect("a room")).expect("32 bits");
        let epoch = joined["epoch"].as_i64().expect("an epoch");
        if epoch != self.epoch {
            self.epoch = epoch;
            self.editor = None;
        }
        let before = self.taken.len();
        let document = |frame: &Frame| frame.sync == STEP_2;
        while !self.taken[before..].iter().any(document) {
            assert!(self.step(PATIENCE).await, "the document follows");
        }
        joined
    }

    /// Comes into the room of a card, which it is let into.
    pub(crate) async fn joined(&mut self, workspace_id: &str, card_id: &str) -> Value {
        let answer = self.join(workspace_id, card_id).await;
        assert_eq!(answer["type"], json!("docJoined"), "{answer}");
        answer
    }

    pub(crate) async fn leave(&mut self) {
        self.send_json(json!({"type": "leaveDoc", "doc": self.doc}))
            .await;
        self.doc = 0;
        self.editor = None;
    }

    pub(crate) fn editor(&self) -> &Editor {
        self.editor.as_ref().expect("it holds a document")
    }

    /// Sends an update of the document it holds.
    pub(crate) async fn send_update(&mut self, update: &[u8]) {
        self.send_binary(frame(self.doc, self.epoch, UPDATE, update))
            .await;
    }

    /// Appends a paragraph and sends the update of it.
    pub(crate) async fn write(&mut self, text: &str) -> Vec<u8> {
        let update = self.editor().append_paragraph(text);
        self.send_update(&update).await;
        update
    }

    /// Appends a paragraph, sends it, and waits for the server to say it has it:
    /// the number it has in the log.
    pub(crate) async fn write_acked(&mut self, text: &str) -> i64 {
        self.write(text).await;
        let ack = self.text("docAck").await;
        assert_eq!(
            (&ack["doc"], &ack["epoch"]),
            (&json!(self.doc), &json!(self.epoch)),
            "{ack}"
        );
        ack["seq"].as_i64().expect("a number in the log")
    }

    /// The body of the document it holds.
    pub(crate) fn body(&self) -> Option<Value> {
        self.editor().body().map(|body| body.document().clone())
    }

    /// The texts of the paragraphs of the document it holds.
    pub(crate) fn lines(&self) -> Vec<String> {
        lines_of(self.body().as_ref())
    }
}

/// The texts of the paragraphs of a body.
pub(crate) fn lines_of(body: Option<&Value>) -> Vec<String> {
    body.and_then(|body| body["content"].as_array())
        .map(|content| {
            content
                .iter()
                .map(|node| node["content"][0]["text"].as_str().unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// A board for the invited only in the test workspace, made by the first client's
/// user, with a card whose body is `body`.
pub(crate) struct Table {
    pub(crate) board_id: String,
    pub(crate) card_id: String,
}

pub(crate) async fn table(th: &TestServer, body: &Value) -> Table {
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let card: Card = th
        .client
        .post(
            &format!("/w/{TEST_WORKSPACE_ID}/boards/{}/cards", board.id),
            &json!({"title": "Edited together", "body": body}),
        )
        .await
        .created_json();
    Table {
        board_id: board.id,
        card_id: card.id,
    }
}

/// The two clients' users in the room of a card with `body`, both writing: the
/// second is given the editor's role on the board.
pub(crate) async fn two_in_a_room(th: &TestServer, body: &Value) -> (Table, Peer, Peer) {
    let t = table(th, body).await;
    let user2 = th.client2.me().await.id;
    give(th, &t.board_id, &user2, "editor").await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    (t, one, two)
}

/// Gives a user a role on the board, as the first client's user: `viewer`,
/// `commenter` or `editor`. A user who has a role is given this one in its place.
pub(crate) async fn give(th: &TestServer, board_id: &str, user_id: &str, role: &str) {
    give_in(th, TEST_WORKSPACE_ID, board_id, user_id, role).await;
}

/// As [`give`], on a board of the workspace `workspace_id`.
pub(crate) async fn give_in(
    th: &TestServer,
    workspace_id: &str,
    board_id: &str,
    user_id: &str,
    role: &str,
) {
    let member = ContainerMember {
        container_id: board_id.to_owned(),
        user_id: user_id.to_owned(),
        role: role.parse().expect("a valid role"),
        synthetic: false,
        inherited_role: None,
    };
    let is_member = th
        .app
        .store()
        .get_member_for_container(board_id, user_id)
        .await
        .is_ok();
    if is_member {
        th.client
            .put(
                &format!("/w/{workspace_id}/containers/{board_id}/members/{user_id}"),
                &member,
            )
            .await
            .check_ok();
    } else {
        th.client
            .add_member_in(workspace_id, &member)
            .await
            .check_ok();
    }
}

/// Writes a card's body whole over HTTP, as a client that reads the card and patches
/// it: the first patch is refused if the card's editors typed since (their body is
/// written to the card first), and the client reads again.
pub(crate) async fn write_whole(client: &Client, card_id: &str, body: &Value) {
    for _ in 0..3 {
        let read = card(client, card_id).await;
        let patched = client
            .patch(
                &format!("/w/{TEST_WORKSPACE_ID}/cards/{card_id}"),
                &json!({"body": body, "readAt": read.updated_at}),
            )
            .await;
        if patched.status != reqwest::StatusCode::CONFLICT {
            patched.check_ok();
            return;
        }
    }
    panic!("the card kept changing while its body was written whole");
}

/// Waits until the card's body is `body`.
pub(crate) async fn card_holds(client: &Client, card_id: &str, body: &Value) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    loop {
        let held = body_of(&card(client, card_id).await);
        if held.as_ref() == Some(body) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the card holds {held:?}, not {body}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Two connections come into the room of a card and write, in turn and at once: both
/// hold the same document, each is told the number its update has in the log, the
/// card is given the body of both when they leave, and the server holds the same
/// after it stopped and started again. A third that comes late is sent all of it, and
/// one that left and comes back what was written meanwhile.
#[tokio::test]
async fn two_connections_edit_a_card_together_and_converge() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &t.board_id, &user2, "editor").await;

    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    let joined = one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(
        joined,
        json!({
            "type": "docJoined", "workspaceId": TEST_WORKSPACE_ID, "cardId": t.card_id,
            "doc": 1, "epoch": 1, "mode": "write",
        })
    );
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(one.lines(), ["first"]);
    assert_eq!(two.lines(), ["first"]);

    // In turn: each has the other's before it writes.
    let mut seqs = vec![one.write_acked("one writes").await];
    two.settle().await;
    assert_eq!(two.lines(), ["first", "one writes"]);
    seqs.push(two.write_acked("two answers").await);
    one.settle().await;
    // At once: each writes before it has the other's.
    one.write("one at once").await;
    two.write("two at once").await;
    for peer in [&mut one, &mut two] {
        let ack = peer.text("docAck").await;
        seqs.push(ack["seq"].as_i64().expect("a number"));
        peer.settle().await;
    }
    seqs.sort_unstable();
    assert_eq!(seqs, [1, 2, 3, 4], "each update has its number in the log");
    let merged = one.body().expect("a body");
    assert_eq!(two.body(), Some(merged.clone()), "both hold the same");
    assert_eq!(lines_of(Some(&merged)).len(), 5, "{merged}");
    assert!(one.unread().is_empty(), "{:?}", one.unread());
    assert!(
        one.taken
            .iter()
            .all(|frame| frame.sync == UPDATE || frame == &one.taken[0]),
        "after the whole document, updates"
    );

    // The last to leave closes the document, which writes its body to the card.
    one.leave().await;
    two.leave().await;
    card_holds(&th.client, &t.card_id, &merged).await;

    // The server stops and starts again: the document is as it was.
    drop((one, two));
    let th = th.killed_and_started_again(async |_| {}).await;
    card_holds(&th.client, &t.card_id, &merged).await;
    let mut one = Peer::connect(&th.client, 201).await;
    let mut two = Peer::connect(&th.client2, 202).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_eq!(one.body(), Some(merged.clone()));
    assert_eq!(two.body(), Some(merged.clone()));
    one.write_acked("after the restart").await;

    // A third comes late, and is sent all of it.
    let mut three = Peer::connect(&th.client, 203).await;
    three.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.settle().await;
    assert_eq!(three.body(), one.body());
    assert_eq!(three.body(), two.body());
    assert_eq!(three.lines().len(), 6);

    // It leaves, misses an update, and comes back to all of it; what it knew the
    // room as before is no room any more.
    let before = three.doc;
    three.leave().await;
    two.write_acked("while three was away").await;
    let again = three.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    assert_ne!(again["doc"], json!(before), "{again}");
    one.settle().await;
    assert_eq!(three.body(), two.body());
    assert_eq!(three.body(), one.body());
    assert_eq!(three.lines().len(), 7);
    let all = two.body().expect("a body");
    for peer in [&mut one, &mut two, &mut three] {
        peer.leave().await;
    }
    card_holds(&th.client, &t.card_id, &all).await;
    th.teardown().await;
}

/// An update the document does not take goes to nobody but its sender, who is told
/// why, and is not in the card; and what the others are sent is in the log before it
/// reaches them.
#[tokio::test]
async fn a_refused_update_goes_to_nobody_and_what_goes_out_is_logged_first() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    give(&th, &t.board_id, &user2, "editor").await;
    let mut one = Peer::connect(&th.client, 101).await;
    let mut two = Peer::connect(&th.client2, 102).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    two.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let app = th
        .app
        .enter_workspace_as(
            fvoci_server::app::Asker::with_password(&user2),
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("the membership is read")
        .expect("a member")
        .app;
    let logged = async || {
        app.store()
            .get_body_doc(&t.card_id)
            .await
            .expect("read")
            .expect("a document")
            .updates
    };

    // Bytes that are no update; and an update the worker takes and the log refuses
    // (it names a file the board does not have).
    let image = crate::collab_images::showing("7nothing.png");
    for refused in [&b"no update"[..], &image] {
        one.send_update(refused).await;
        let nack = one.text("docNack").await;
        assert_eq!(
            nack,
            json!({"type": "docNack", "doc": one.doc, "epoch": 1, "reason": "update_refused"})
        );
    }
    assert_eq!(
        two.incoming(QUIET).await,
        Incoming::Nothing,
        "sent to nobody"
    );
    assert!(logged().await.is_empty(), "and not logged");

    // What is taken is in the log, under the number its sender is told, when it
    // reaches the other.
    let update = one.write("typed").await;
    match two.incoming(PATIENCE).await {
        Incoming::Frame(frame) => {
            assert_eq!(
                frame,
                Frame {
                    doc: two.doc,
                    epoch: 1,
                    sync: UPDATE,
                    payload: update.clone()
                }
            );
            two.editor().receive(&frame.payload);
        }
        other => panic!("the update reaches the other: {other:?}"),
    }
    let log = logged().await;
    assert_eq!(log.len(), 1, "{log:?}");
    assert_eq!(log[0].bytes, update);
    let ack = one.text("docAck").await;
    assert_eq!(ack["seq"], json!(log[0].seq), "{ack}");
    assert_eq!(
        one.incoming(QUIET).await,
        Incoming::Nothing,
        "not sent back"
    );

    // An update made against another making of the document than the one there is.
    let stale = one.editor().append_paragraph("of another epoch");
    one.send_binary(frame(one.doc, 7, UPDATE, &stale)).await;
    let nack = one.text("docNack").await;
    assert_eq!(
        (&nack["epoch"], &nack["reason"]),
        (&json!(7), &json!("update_out_of_step")),
        "{nack}"
    );
    assert_eq!(two.incoming(QUIET).await, Incoming::Nothing);

    let body = two.body().expect("a body");
    assert_eq!(two.lines(), ["first", "typed"]);
    one.leave().await;
    two.leave().await;
    card_holds(&th.client, &t.card_id, &body).await;
    th.teardown().await;
}

/// A viewer and a commenter come into the room to read: they are sent what the others
/// write, and what they send is refused and goes to nobody.
#[tokio::test]
async fn readers_read_and_what_they_send_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let user2 = th.client2.me().await.id;
    let commenter = th.add_user("commenter").await;
    give(&th, &t.board_id, &user2, "viewer").await;
    give(&th, &t.board_id, &commenter.me().await.id, "commenter").await;

    let mut writer = Peer::connect(&th.client, 101).await;
    writer.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    let mut readers = [
        Peer::connect(&th.client2, 102).await,
        Peer::connect(&commenter, 103).await,
    ];
    for reader in &mut readers {
        let joined = reader.joined(TEST_WORKSPACE_ID, &t.card_id).await;
        assert_eq!(joined["mode"], json!("read"), "{joined}");
    }
    writer.write_acked("from the writer").await;
    for reader in &mut readers {
        reader.settle().await;
        assert_eq!(reader.lines(), ["first", "from the writer"]);
        reader.write("from a reader").await;
        let nack = reader.text("docNack").await;
        assert_eq!(nack["reason"], json!("document_read_only"), "{nack}");
    }
    assert_eq!(writer.incoming(QUIET).await, Incoming::Nothing);
    // A reader may ask for what it is missing.
    let [reader, _] = &mut readers;
    reader
        .send_binary(frame(reader.doc, reader.epoch, STEP_1, &[0]))
        .await;
    match reader.incoming(PATIENCE).await {
        Incoming::Frame(frame) => {
            assert_eq!(frame.sync, STEP_2, "{frame:?}");
            let whole = Editor::join(104, &frame.payload);
            assert_eq!(
                whole.body().map(|body| body.document().clone()),
                writer.body()
            );
        }
        other => panic!("the answer to a sync step 1: {other:?}"),
    }
    th.teardown().await;
}

/// Who may not read the board is refused as for a card that is not there, and so is a
/// connection without a session, whatever read token it follows the board's blocks
/// with.
#[tokio::test]
async fn who_may_not_read_is_refused_as_for_no_card() {
    let th = Setup::local_mode().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut writer = Peer::connect(&th.client, 101).await;
    writer.joined(TEST_WORKSPACE_ID, &t.card_id).await;

    // Nobody is let in who may not read the board: the answer is that of a card
    // that is not there, whichever of the two it is.
    let no_card = fvoci_model::new_id();
    let comment = fvoci_model::Block {
        id: fvoci_model::new_id(),
        container_id: t.board_id.clone(),
        parent_id: t.card_id.clone(),
        block_type: fvoci_model::BlockType::Comment,
        title: "no card".to_owned(),
        created_at: 1,
        updated_at: 1,
        ..fvoci_model::Block::default()
    };
    let stored: Vec<fvoci_model::Block> = th
        .client
        .insert_blocks(&t.board_id, &[comment])
        .await
        .created_json();
    let comment_id = stored[0].id.clone();
    let outsider = th.add_user("outsider").await;
    let mut outside = Peer::connect(&outsider, 105).await;
    let mut anonymous = Peer::on(th.anonymous().as_browser().connect_websocket().await, 106);
    let token = th.share(&t.board_id).await;
    anonymous
        .send_json(json!({
            "type": "subscribeBlocks", "workspaceId": TEST_WORKSPACE_ID,
            "readToken": token, "blockIds": [t.card_id],
        }))
        .await;
    for peer in [&mut outside, &mut anonymous] {
        for (workspace_id, card_id) in [
            (TEST_WORKSPACE_ID, t.card_id.as_str()),
            (TEST_WORKSPACE_ID, no_card.as_str()),
            ("no workspace", t.card_id.as_str()),
        ] {
            peer.send_json(json!({
                "type": "joinDoc", "workspaceId": workspace_id, "cardId": card_id,
                "readToken": token,
            }))
            .await;
            let refused = peer.text("docRefused").await;
            assert_eq!(
                refused,
                json!({
                    "type": "docRefused", "workspaceId": workspace_id, "cardId": card_id,
                    "reason": "not_found",
                })
            );
        }
    }
    // Nor into the room of a block that is no card, or of a card named with another
    // workspace than its board's, by who may write the card.
    for (workspace_id, card_id) in [
        (TEST_WORKSPACE_ID, comment_id.as_str()),
        (ROOT_WORKSPACE_ID, t.card_id.as_str()),
    ] {
        let refused = writer.join(workspace_id, card_id).await;
        assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    }
    // What a refused connection sends as a frame of a room it is not in is dropped.
    outside
        .send_binary(frame(1, 1, UPDATE, &writer.editor().append_paragraph("x")))
        .await;
    assert_eq!(outside.incoming(QUIET).await, Incoming::Nothing);
    assert_eq!(writer.incoming(QUIET).await, Incoming::Nothing);
    th.teardown().await;
}

/// Waits until there is no room, and sees that none comes after.
pub(crate) async fn no_rooms(th: &TestServer) {
    let deadline = tokio::time::Instant::now() + PATIENCE;
    while th.app.collab().rooms() > 0 {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} rooms are left",
            th.app.collab().rooms()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    tokio::time::sleep(QUIET).await;
    assert_eq!(th.app.collab().rooms(), 0, "a room came after");
}

/// A connection that ends without leaving is out of its rooms all the same: the room
/// closes, which writes the body to the card. So is one that ends while it comes in.
#[tokio::test]
async fn a_connection_that_ends_leaves_its_rooms() {
    let th = Setup::native().start().await.init_basic().await;
    let t = table(&th, &paragraphs(&["first"])).await;
    let mut one = Peer::connect(&th.client, 101).await;
    one.joined(TEST_WORKSPACE_ID, &t.card_id).await;
    one.write_acked("typed").await;
    let body = one.body();
    assert_eq!(th.app.collab().rooms(), 1);
    drop(one);
    no_rooms(&th).await;
    assert_eq!(
        body_of(&card(&th.client, &t.card_id).await),
        body,
        "written as the room closed"
    );

    for _ in 0..20 {
        let mut peer = Peer::connect(&th.client, 102).await;
        peer.send_json(json!({
            "type": "joinDoc", "workspaceId": TEST_WORKSPACE_ID, "cardId": t.card_id,
        }))
        .await;
        drop(peer);
    }
    no_rooms(&th).await;
    th.teardown().await;
}

/// A card of another organization is not there for who is no member of its
/// workspace, whatever workspace they name; who is let into that workspace and onto
/// the card's board edits it there, in its organization.
#[tokio::test]
async fn a_card_of_another_organization_is_reached_through_its_workspace_only() {
    use fvoci_model::{Block, BlockType, WorkspaceRole};
    // SQLite holds one organization.
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        return;
    }
    let th = Setup::native().start().await.init_basic().await;
    let (theirs, workspace_id, board_id) = crate::workspaces::another_organization(&th).await;
    let members = theirs.get_members_for_container(&board_id).await;
    let owner = members.expect("their members")[0].user_id.clone();
    let mut theirs_card = Block {
        id: fvoci_model::new_id(),
        container_id: board_id.clone(),
        block_type: BlockType::Card,
        title: "theirs".to_owned(),
        ..Block::default()
    };
    theirs
        .insert_block(&mut theirs_card, &owner)
        .await
        .expect("their card is stored");
    let user1 = th.client.me().await.id;
    let mut peer = Peer::connect(&th.client, 101).await;
    for named in [workspace_id.as_str(), TEST_WORKSPACE_ID] {
        let refused = peer.join(named, &theirs_card.id).await;
        assert_eq!(
            (&refused["type"], &refused["reason"]),
            (&json!("docRefused"), &json!("not_found")),
            "{refused}"
        );
    }

    theirs
        .add_workspace_member(&workspace_id, &user1, WorkspaceRole::Guest)
        .await
        .expect("let into their workspace");
    let member = ContainerMember {
        container_id: board_id,
        user_id: user1,
        role: fvoci_model::ContainerRole::Editor,
        synthetic: false,
        inherited_role: None,
    };
    theirs.save_member(&member).await.expect("invited");
    let joined = peer.joined(&workspace_id, &theirs_card.id).await;
    assert_eq!(joined["mode"], json!("write"), "{joined}");
    peer.write_acked("written in their organization").await;
    let refused = peer.join(TEST_WORKSPACE_ID, &theirs_card.id).await;
    assert_eq!(refused["reason"], json!("not_found"), "{refused}");
    let body = peer.body();
    peer.leave().await;
    no_rooms(&th).await;
    let stored = theirs.get_block(&theirs_card.id).await.expect("their card");
    assert_eq!(stored.body.map(|body| body.document().clone()), body);
    th.teardown().await;
}

/// An update is sent on, and answered, only once the log has it: held between the
/// worker that took it and the log, it reaches nobody.
#[tokio::test]
async fn nothing_is_sent_on_before_the_log_has_it() {
    let th = Setup::native().start().await.init_basic().await;
    let (t, mut one, mut two) = two_in_a_room(&th, &paragraphs(&["first"])).await;
    let user1 = th.client.me().await.id;
    let app = th
        .app
        .enter_workspace_as(
            fvoci_server::app::Asker::with_password(&user1),
            TEST_WORKSPACE_ID,
        )
        .await
        .expect("the membership is read")
        .expect("a member")
        .app;
    let logged = async || {
        let stored = app.store().get_body_doc(&t.card_id).await.expect("read");
        stored.expect("a document").updates
    };

    let (held, go) = th.app.collab().pause_next_log();
    let update = one.write("typed").await;
    held.notified().await;
    assert_eq!(two.incoming(QUIET).await, Incoming::Nothing, "not sent on");
    assert_eq!(one.incoming(QUIET).await, Incoming::Nothing, "nor answered");
    assert!(logged().await.is_empty(), "the log does not have it yet");
    go.notify_one();
    match two.incoming(PATIENCE).await {
        Incoming::Frame(frame) => assert_eq!(frame.payload, update, "sent on once logged"),
        other => panic!("the update reaches the other: {other:?}"),
    }
    let log = logged().await;
    assert_eq!(log.len(), 1, "{log:?}");
    let ack = one.text("docAck").await;
    assert_eq!(ack["seq"], json!(log[0].seq), "{ack}");
    th.teardown().await;
}
