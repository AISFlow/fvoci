//! What waits to be sent to one connection about the rooms it is in: held to a number
//! of messages and of bytes, so that a connection that does not read holds no room
//! and little memory.

use std::collections::VecDeque;
use std::sync::Mutex;

use bytes::Bytes;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

use super::frame;

/// A message of a room for one connection.
#[derive(Debug, Clone)]
pub(super) struct Queued {
    /// The room, as the connection knows it; 0 for a message of no room.
    pub(super) doc: u32,
    pub(super) what: What,
}

/// What a message is.
#[derive(Clone)]
pub(super) enum What {
    /// A JSON message.
    Text(String),
    /// A sync message of the document of `epoch`, shared by its receivers
    /// ([`frame::message`]).
    Sync { epoch: i64, message: Bytes },
    /// What is told of the caret the room knows as `number`, in the document of
    /// `epoch`: an awareness update, shared by its receivers.
    Caret {
        epoch: i64,
        number: u64,
        update: Bytes,
    },
}

impl std::fmt::Debug for What {
    /// What it is and how large, never what it says: a body, or where somebody's
    /// caret is, is written nowhere, be it by a log line of what waits.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (kind, bytes) = match self {
            Self::Text(text) => ("Text", text.len()),
            Self::Sync { message, .. } => ("Sync", message.len()),
            Self::Caret { update, .. } => ("Caret", update.len()),
        };
        f.debug_struct(kind).field("bytes", &bytes).finish()
    }
}

impl Queued {
    /// The bytes it counts as while it waits.
    fn bytes(&self) -> usize {
        match &self.what {
            What::Text(text) => text.len(),
            What::Sync { message, .. } => message.len(),
            What::Caret { update, .. } => update.len(),
        }
    }

    /// Whether it is a binary frame of the room `doc`: a sync message, or a caret.
    fn in_frames_of(&self, doc: u32) -> bool {
        self.doc == doc && !matches!(self.what, What::Text(_))
    }

    /// Whether it tells of the caret `number` in the room `doc`.
    fn tells_caret(&self, doc: u32, number: u64) -> bool {
        self.doc == doc && matches!(self.what, What::Caret { number: of, .. } if of == number)
    }

    pub(super) fn text(doc: u32, text: String) -> Self {
        Self {
            doc,
            what: What::Text(text),
        }
    }

    pub(super) fn sync(doc: u32, epoch: i64, message: &Bytes) -> Self {
        Self {
            doc,
            what: What::Sync {
                epoch,
                message: message.clone(),
            },
        }
    }

    fn caret(doc: u32, epoch: i64, number: u64, update: &Bytes) -> Self {
        Self {
            doc,
            what: What::Caret {
                epoch,
                number,
                update: update.clone(),
            },
        }
    }
}

/// A message as the websocket sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outgoing {
    /// A text frame: a JSON message.
    Text(String),
    /// A binary frame.
    Binary(Bytes),
}

/// Why the queue did not take what was pushed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NotQueued {
    /// It holds as much as it may.
    Full,
    /// The connection ended.
    Closed,
}

/// How much may wait for one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Capacity {
    /// How many messages.
    pub(super) messages: usize,
    /// How many bytes, of the messages together. A message past it alone is taken
    /// into an empty queue: the whole of a document is sent whatever its size.
    pub(super) bytes: usize,
}

/// The messages that wait for one connection.
#[derive(Debug)]
pub(super) struct Queue {
    waiting: Mutex<Waiting>,
    /// Woken when there is something to send, or nothing more will come.
    wake: Notify,
    capacity: Capacity,
    /// Cancelled when notices alone are past what may wait: the connection reads
    /// nothing, and is closed.
    overrun: CancellationToken,
}

#[derive(Debug, Default)]
struct Waiting {
    messages: VecDeque<Queued>,
    /// The bytes of the messages together.
    bytes: usize,
    closed: bool,
}

impl Waiting {
    /// Drops the messages `drops` picks.
    fn drop_where(&mut self, drops: impl Fn(&Queued) -> bool) {
        self.messages.retain(|queued| !drops(queued));
        self.bytes = self.messages.iter().map(Queued::bytes).sum();
    }

    /// Queues `messages`, all or none.
    fn push(
        &mut self,
        capacity: Capacity,
        messages: impl IntoIterator<Item = Queued, IntoIter: ExactSizeIterator + Clone>,
    ) -> Result<(), NotQueued> {
        if self.closed {
            return Err(NotQueued::Closed);
        }
        let messages = messages.into_iter();
        let bytes: usize = messages.clone().map(|queued| queued.bytes()).sum();
        let over_bytes = !self.messages.is_empty() && self.bytes + bytes > capacity.bytes;
        if self.messages.len() + messages.len() > capacity.messages || over_bytes {
            return Err(NotQueued::Full);
        }
        self.bytes += bytes;
        self.messages.extend(messages);
        Ok(())
    }

    /// Queues what is told of a caret where there is room for a caret: while less than
    /// half of the messages and of the bytes that may wait do. Whether it was queued.
    fn push_caret(&mut self, capacity: Capacity, told: Queued) -> bool {
        let fits = self.messages.len() < capacity.messages / 2
            && self.bytes + told.bytes() <= capacity.bytes / 2;
        if fits && !self.closed {
            self.bytes += told.bytes();
            self.messages.push_back(told);
        }
        fits && !self.closed
    }
}

impl Queue {
    pub(super) fn new(capacity: Capacity) -> Self {
        Self {
            waiting: Mutex::new(Waiting::default()),
            wake: Notify::new(),
            capacity,
            overrun: CancellationToken::new(),
        }
    }

    fn waiting(&self) -> std::sync::MutexGuard<'_, Waiting> {
        self.waiting
            .lock()
            .expect("the queue is never held across a panic")
    }

    /// Queues `messages`, all or none.
    ///
    /// # Errors
    ///
    /// Returns why they were not queued: they do not fit under what may wait, or the
    /// connection ended.
    pub(super) fn push(
        &self,
        messages: impl IntoIterator<Item = Queued, IntoIter: ExactSizeIterator + Clone>,
    ) -> Result<(), NotQueued> {
        self.waiting().push(self.capacity, messages)?;
        self.wake.notify_one();
        Ok(())
    }

    /// Queues `messages`, the document of `epoch` anew, in place of the sync messages
    /// that wait for the room: what a connection that is sent the document anew no
    /// longer needs of the one before. The carets of that document that wait for the
    /// room wait on, after it: a caret is a place in a document the connection holds.
    /// Those of another making of the document are dropped.
    ///
    /// # Errors
    ///
    /// Returns why they were not queued, if they do not fit even so; the sync messages
    /// and carets that waited are dropped all the same, and the caller puts the
    /// connection out of the room.
    pub(super) fn push_anew(
        &self,
        doc: u32,
        epoch: i64,
        messages: impl IntoIterator<Item = Queued, IntoIter: ExactSizeIterator + Clone>,
    ) -> Result<(), NotQueued> {
        {
            let mut waiting = self.waiting();
            let (carets, others): (Vec<Queued>, Vec<Queued>) =
                std::mem::take(&mut waiting.messages)
                    .into_iter()
                    .filter(|queued| {
                        !(queued.doc == doc && matches!(queued.what, What::Sync { .. }))
                    })
                    .partition(|queued| {
                        queued.doc == doc && matches!(queued.what, What::Caret { .. })
                    });
            waiting.messages = others.into();
            waiting.bytes = waiting.messages.iter().map(Queued::bytes).sum();
            waiting.push(self.capacity, messages)?;
            for caret in carets {
                if matches!(caret.what, What::Caret { epoch: of, .. } if of == epoch) {
                    waiting.push_caret(self.capacity, caret);
                }
            }
        }
        self.wake.notify_one();
        Ok(())
    }

    /// Queues what is told of the caret `number` of the room `doc`, an awareness
    /// update of the document of `epoch`, in place of what waits of the same caret:
    /// of each caret only the last telling waits, however often it moved: a later one
    /// takes the place of the one that waits, a withdrawal too. A telling of a caret
    /// none waits of is not queued for a connection that ended, or for which half of
    /// what may wait waits already: a caret is said again, nobody is put out of a
    /// room for one, and carets alone never fill what the room needs to send of its
    /// document.
    pub(super) fn push_caret(&self, doc: u32, epoch: i64, number: u64, update: &Bytes) {
        {
            let mut waiting = self.waiting();
            if waiting.closed {
                return;
            }
            let told = Queued::caret(doc, epoch, number, update);
            let waits = waiting
                .messages
                .iter_mut()
                .find(|queued| queued.tells_caret(doc, number));
            if let Some(before) = waits {
                // In its place, whatever waits: one telling for another adds none.
                let bytes = (before.bytes(), told.bytes());
                *before = told;
                waiting.bytes = waiting.bytes.saturating_sub(bytes.0) + bytes.1;
            } else if !waiting.push_caret(self.capacity, told) {
                return;
            }
        }
        self.wake.notify_one();
    }

    /// Queues an answer to something the connection sent that the room has dealt with
    /// (`docAck`, `docNack`), whatever waits: what became of an update that was
    /// logged is told, also to a connection that reads nothing.
    ///
    /// # Errors
    ///
    /// Returns [`NotQueued::Full`] if it is past what may wait: it is queued all the
    /// same, and the caller puts the connection out of the room after it. Returns
    /// [`NotQueued::Closed`], with nothing queued, if the connection ended.
    pub(super) fn answer(&self, answer: Queued) -> Result<(), NotQueued> {
        let fits = {
            let mut waiting = self.waiting();
            if waiting.closed {
                return Err(NotQueued::Closed);
            }
            let fits = waiting.messages.len() < self.capacity.messages
                && (waiting.messages.is_empty()
                    || waiting.bytes + answer.bytes() <= self.capacity.bytes);
            waiting.bytes += answer.bytes();
            waiting.messages.push_back(answer);
            if waiting.messages.len() > self.capacity.messages.saturating_mul(2) {
                self.overrun.cancel();
            }
            fits
        };
        self.wake.notify_one();
        if fits { Ok(()) } else { Err(NotQueued::Full) }
    }

    /// Drops what waits for a room the connection left of its own accord.
    pub(super) fn forget(&self, doc: u32) {
        self.waiting().drop_where(|queued| queued.doc == doc);
    }

    /// Drops the sync messages and carets that wait for a room the connection is no
    /// longer in. The answers it was given there (`docJoined`, `docAck`, `docNack`) wait on:
    /// each says what became of something the connection sent.
    pub(super) fn forget_frames(&self, doc: u32) {
        self.waiting().drop_where(|queued| queued.in_frames_of(doc));
    }

    /// Queues a notice whatever waits: the answer to a connection that is refused or
    /// put out, be it because its queue is full. With `doc`, the sync messages and
    /// carets that wait for that room are dropped first; the answers it was given there wait on,
    /// before the notice. Messages past twice what may wait end the connection.
    pub(super) fn notice(&self, doc: Option<u32>, text: String) {
        {
            let mut waiting = self.waiting();
            if waiting.closed {
                return;
            }
            if let Some(doc) = doc {
                waiting.drop_where(|queued| queued.in_frames_of(doc));
            }
            waiting.bytes += text.len();
            waiting.messages.push_back(Queued::text(0, text));
            if waiting.messages.len() > self.capacity.messages.saturating_mul(2) {
                self.overrun.cancel();
            }
        }
        self.wake.notify_one();
    }

    /// Nothing more is queued, and what waits is dropped: the connection ended.
    pub(super) fn close(&self) {
        {
            let mut waiting = self.waiting();
            waiting.closed = true;
            waiting.messages.clear();
            waiting.bytes = 0;
        }
        self.wake.notify_one();
    }

    /// Cancelled when the connection is to be closed for reading nothing.
    pub(super) fn overrun(&self) -> &CancellationToken {
        &self.overrun
    }

    /// The next message to send; `None` once the connection ended. Waiting for it may
    /// be given up at any time: nothing is lost.
    pub(super) async fn next(&self) -> Option<Outgoing> {
        loop {
            // Registered before the look, so that a push between the look and the
            // wait wakes it.
            let woken = self.wake.notified();
            {
                let mut waiting = self.waiting();
                if let Some(queued) = waiting.messages.pop_front() {
                    waiting.bytes = waiting.bytes.saturating_sub(queued.bytes());
                    return Some(match queued.what {
                        What::Text(text) => Outgoing::Text(text),
                        What::Sync { epoch, message } => {
                            Outgoing::Binary(frame::framed(queued.doc, epoch, &message))
                        }
                        What::Caret { epoch, update, .. } => {
                            Outgoing::Binary(frame::carets(queued.doc, epoch, &update))
                        }
                    });
                }
                if waiting.closed {
                    return None;
                }
            }
            woken.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(doc: u32, text: &str) -> Queued {
        Queued::text(doc, text.to_owned())
    }

    fn sync(doc: u32) -> Queued {
        Queued::sync(doc, 1, &Bytes::from_static(&[2, 0]))
    }

    fn queue_of(messages: usize) -> Queue {
        Queue::new(Capacity {
            messages,
            bytes: 1024,
        })
    }

    /// What waits, in order, without ending the queue.
    async fn drained(queue: &Queue) -> Vec<Outgoing> {
        let waits = queue.waiting().messages.len();
        let mut sent = Vec::new();
        for _ in 0..waits {
            sent.extend(queue.next().await);
        }
        sent
    }

    #[tokio::test]
    async fn what_does_not_fit_is_not_queued_and_a_notice_takes_the_place_of_a_room() {
        let queue = queue_of(3);
        queue.push([text(1, "a"), sync(1)]).expect("room for two");
        queue.push([sync(2)]).expect("room for a third");
        assert_eq!(
            queue.push([sync(1)]),
            Err(NotQueued::Full),
            "no room for a fourth"
        );
        assert_eq!(
            queue.push_anew(2, 1, [text(2, "anew"), sync(2)]),
            Err(NotQueued::Full),
            "two in place of one do not fit"
        );
        queue.notice(Some(1), "left".to_owned());
        assert!(!queue.overrun().is_cancelled());
        let sent = drained(&queue).await;
        assert_eq!(
            sent,
            [
                Outgoing::Text("a".to_owned()),
                Outgoing::Text("left".to_owned())
            ],
            "the answer waits on, before the notice"
        );
        queue.push([text(1, "a"), sync(1), sync(2)]).expect("room");
        queue.forget_frames(1);
        assert_eq!(
            drained(&queue).await.len(),
            2,
            "the answer, and the other room's"
        );
        queue.push([text(1, "a"), sync(1), sync(2)]).expect("room");
        queue.forget(1);
        assert_eq!(drained(&queue).await.len(), 1, "the other room's");
    }

    #[tokio::test]
    async fn an_answer_is_queued_whatever_waits_and_says_when_it_was_one_too_many() {
        let queue = queue_of(2);
        queue.answer(text(1, "ack 1")).expect("room");
        queue.answer(text(1, "ack 2")).expect("room");
        assert_eq!(queue.answer(text(1, "ack 3")), Err(NotQueued::Full));
        queue.notice(Some(1), "left".to_owned());
        let sent = drained(&queue).await;
        let texts: Vec<Outgoing> = ["ack 1", "ack 2", "ack 3", "left"]
            .map(|text| Outgoing::Text(text.to_owned()))
            .into();
        assert_eq!(sent, texts, "every answer, and then the notice");
        queue.close();
        assert_eq!(queue.answer(text(1, "ack")), Err(NotQueued::Closed));
    }

    #[tokio::test]
    async fn the_bytes_that_wait_are_held_to_their_limit_too() {
        let queue = Queue::new(Capacity {
            messages: 100,
            bytes: 10,
        });
        let large = Bytes::from_static(&[0; 8]);
        // Into an empty queue whatever its size: the whole of a document.
        queue
            .push([Queued::sync(1, 1, &Bytes::from_static(&[0; 64]))])
            .expect("alone");
        assert_eq!(queue.push([sync(1)]), Err(NotQueued::Full));
        assert_eq!(drained(&queue).await.len(), 1);
        queue.push([Queued::sync(1, 1, &large)]).expect("8 of 10");
        queue.push([sync(1)]).expect("10 of 10");
        assert_eq!(queue.push([text(1, "x")]), Err(NotQueued::Full));
        // What is sent, dropped or replaced no longer counts.
        assert!(queue.next().await.is_some());
        queue.push([Queued::sync(2, 1, &large)]).expect("10 of 10");
        assert_eq!(queue.push([sync(2)]), Err(NotQueued::Full));
        queue.forget(2);
        queue.push([Queued::sync(1, 1, &large)]).expect("10 of 10");
        queue
            .push_anew(1, 2, [Queued::sync(1, 2, &large), sync(1)])
            .expect("in place of both");
        assert_eq!(drained(&queue).await.len(), 2);
    }

    #[tokio::test]
    async fn the_document_anew_takes_the_place_of_what_waited_of_it() {
        let queue = queue_of(4);
        queue
            .push([text(1, "ack"), sync(1), sync(2), sync(1)])
            .expect("room");
        queue
            .push_anew(1, 1, [text(1, "reset"), sync(1)])
            .expect("in place of two");
        let sent = drained(&queue).await;
        let texts: Vec<&str> = sent
            .iter()
            .filter_map(|outgoing| match outgoing {
                Outgoing::Text(text) => Some(text.as_str()),
                Outgoing::Binary(_) => None,
            })
            .collect();
        assert_eq!(texts, ["ack", "reset"], "the messages in words stay");
        assert_eq!(sent.len(), 4, "{sent:?}");
        assert_eq!(
            sent[1],
            Outgoing::Binary(frame::framed(2, 1, &Bytes::from_static(&[2, 0]))),
            "the other room's waits on"
        );
    }

    /// Tells the caret `number` of the room `doc`; a byte tells two tellings apart.
    fn tell(queue: &Queue, doc: u32, epoch: i64, number: u64, telling: u8) {
        queue.push_caret(doc, epoch, number, &Bytes::copy_from_slice(&[telling]));
    }

    fn caret_frame(doc: u32, epoch: i64, telling: u8) -> Outgoing {
        Outgoing::Binary(frame::carets(
            doc,
            epoch,
            &Bytes::copy_from_slice(&[telling]),
        ))
    }

    #[tokio::test]
    async fn of_a_caret_only_the_last_telling_waits_and_carets_take_half_the_room_at_most() {
        let queue = queue_of(8);
        queue.push([sync(1)]).expect("room");
        tell(&queue, 1, 1, 7, 1);
        tell(&queue, 1, 1, 8, 1);
        tell(&queue, 2, 1, 7, 1);
        // The caret 7 of the room 1 moved twice more, also with half the room taken:
        // the last telling waits where the first did. Carets take no fifth place of
        // the eight, with what else waits.
        tell(&queue, 1, 1, 7, 2);
        tell(&queue, 1, 1, 7, 3);
        tell(&queue, 1, 1, 9, 1);
        assert!(!queue.overrun().is_cancelled());
        let sent = drained(&queue).await;
        assert_eq!(
            sent[1..],
            [
                caret_frame(1, 1, 3),
                caret_frame(1, 1, 1),
                caret_frame(2, 1, 1)
            ],
            "the last of the caret 7, the caret 8 and the other room's 7; not the caret 9"
        );
        // What the room sends of its document has room after as many carets as may wait.
        for number in 0..8 {
            tell(&queue, 1, 1, number, 1);
        }
        queue
            .push([sync(1), sync(1), sync(1), sync(1)])
            .expect("half the room is the document's");
        assert_eq!(drained(&queue).await.len(), 8);
        queue.close();
        tell(&queue, 1, 2, 8, 3);
        assert_eq!(queue.next().await, None);
    }

    #[tokio::test]
    async fn the_document_anew_comes_before_the_carets_that_are_places_in_it() {
        let queue = queue_of(8);
        tell(&queue, 1, 1, 7, 4);
        tell(&queue, 1, 2, 8, 1);
        tell(&queue, 2, 1, 9, 1);
        queue
            .push_anew(1, 2, [text(1, "reset"), Queued::sync(1, 2, &Bytes::new())])
            .expect("room");
        let sent = drained(&queue).await;
        assert_eq!(
            sent,
            [
                caret_frame(2, 1, 1),
                Outgoing::Text("reset".to_owned()),
                Outgoing::Binary(frame::framed(1, 2, &Bytes::new())),
                caret_frame(1, 2, 1),
            ],
            "the other room's caret, the document, then the caret of this making; \
             not the caret of the one before"
        );
        // A room that is left takes its carets with it.
        tell(&queue, 1, 2, 8, 2);
        tell(&queue, 2, 2, 8, 2);
        queue.notice(Some(1), "left".to_owned());
        assert_eq!(
            drained(&queue).await,
            [caret_frame(2, 2, 2), Outgoing::Text("left".to_owned())]
        );
    }

    #[tokio::test]
    async fn notices_alone_past_what_may_wait_end_the_connection() {
        let queue = queue_of(2);
        for _ in 0..4 {
            queue.notice(None, "refused".to_owned());
        }
        assert!(!queue.overrun().is_cancelled(), "twice what may wait");
        queue.notice(None, "refused".to_owned());
        assert!(queue.overrun().is_cancelled());
        queue.close();
        assert_eq!(queue.next().await, None, "a closed queue sends nothing");
        assert_eq!(
            queue.push([sync(1)]),
            Err(NotQueued::Closed),
            "and takes nothing"
        );
        queue.notice(None, "refused".to_owned());
        assert_eq!(queue.next().await, None);
    }

    #[tokio::test]
    async fn a_wait_is_woken_by_a_push() {
        let queue = std::sync::Arc::new(queue_of(2));
        let waiting = tokio::spawn({
            let queue = std::sync::Arc::clone(&queue);
            async move { queue.next().await }
        });
        tokio::task::yield_now().await;
        queue.push([text(1, "woken")]).expect("room");
        assert_eq!(
            waiting.await.expect("the wait ends"),
            Some(Outgoing::Text("woken".to_owned()))
        );
    }
}
