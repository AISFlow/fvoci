//! The body of a card as a Y document, for people to edit it together.
//!
//! A body is stored as JSON (`fvoci_model::Body`). While it is edited by several people
//! at once it is also a Y document, which their editors change through the binding of
//! the web app's editor (`@tiptap/y-tiptap`). This crate is what the server needs to
//! stand between the two, with no input or output of its own:
//!
//! - [`from_body`] makes the Y document of a stored body, in the shape the editors read
//!   ([`new_doc`] the one of a card without a body), and [`state_of`] gives a Y
//!   document as it is stored.
//! - [`take_update`] takes a client's update into a Y document, and [`from_state`]
//!   opens a stored one. Both give a document that is a body or nothing: the update is
//!   applied to a copy, which is read again from its state, projected back to the JSON
//!   an editor would store from it, checked as any body is checked, and repaired where
//!   that fails (two edits that are each valid can merge into a document that is not,
//!   and the editors delete what their schema refuses, as the repair does). The
//!   document an update is given for is never changed. The steps are not given on
//!   their own, so that none is left out or taken in another order.
//!
//! Free functions that borrow a document (`&Doc`) never mutate it; [`BodyDoc::take_in_place`]
//! consumes a document to apply and repair in place, returning the untouched document on
//! pre-apply refusal or requiring [`BodyDoc::rebuild`] from the caller's checkpoint on
//! post-apply refusal. Each reads a document under its lock, and waits for that: do not
//! call one while the same thread holds a transaction on that document. The Y library is
//! given on as [`yrs`], so that what holds a document names its types from the version
//! this crate is built with.
//!
//! What the shape is, and what an editor does with shared content, is not assumed here:
//! the web app states it (`apps/web/tools/ydoc/vectors.ts` writes it down as the vectors
//! of `contracts/ydoc-vectors.json`, and `apps/web/tests/editor/ydoc-vectors.test.ts`
//! holds its editor to them), and this crate's tests read those.

mod append;
mod block_cache;
mod build;
mod doc;
mod mend;
mod placing;
mod project;
mod read;
mod reading;
mod repair;
mod scan;
mod schema;
mod settle;
mod take;
mod update;

pub use append::FIRST_APPENDER;
pub use build::{from_body, from_body_by, new_doc};
pub use doc::{
    BodyDoc, BodyTakeError, Changed, MadeBody, SettleError, SettledDoc, StateCounts, TakenInPlace,
    client_changes, reading_steps, state_counts, steps_ahead,
};
pub use repair::{InPlaceRepair, Repair};
pub use settle::same_state;
pub use take::{TakeError, Taken, from_state, take_update};
pub use update::{
    MAX_CLIENT_CHANGES, MAX_CLIENTS, MAX_TOTAL_CHANGES, PLACING_STEPS, READING_STEPS, UpdateError,
    UpdateLimits, state_of,
};
pub use yrs;

/// The steps of [`take_update`] one by one, for this crate's tests: what each does
/// alone is held to the vectors. Built with the feature `test-util` only.
#[cfg(feature = "test-util")]
#[doc(hidden)]
pub mod testing {
    use fvoci_model::{Body, BodyError};
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, Transact, Update};

    use crate::{Repair, UpdateError, UpdateLimits};

    /// Projects a Y document to its body.
    ///
    /// # Errors
    ///
    /// Returns why the document is no body.
    pub fn to_body(doc: &Doc) -> Result<Option<Body>, BodyError> {
        crate::project::to_body(doc)
    }

    /// Repairs a Y document in place: runs the in-place repair on a copy and applies its
    /// update to `doc` only if the repair succeeds, so a repair that fails leaves `doc`
    /// as it was.
    ///
    /// # Errors
    ///
    /// Returns why the document cannot be repaired.
    ///
    /// # Panics
    ///
    /// Panics if the changes of the repair do not apply to `doc`, which would be a
    /// defect.
    pub fn repair(doc: &Doc) -> Result<Repair, BodyError> {
        repair_isolated(doc, None)
    }

    /// [`repair`], with what the repair writes written as `writer`: the same changes from
    /// any document, where a repair writes as a client the document names.
    ///
    /// # Errors
    ///
    /// As [`repair`].
    ///
    /// # Panics
    ///
    /// As [`repair`].
    pub fn repair_by(doc: &Doc, writer: u64) -> Result<Repair, BodyError> {
        repair_isolated(doc, Some(writer))
    }

    /// Runs [`crate::repair::repaired_by`] on a copy of `doc` and applies the resulting
    /// repair update back to `doc` on success.
    fn repair_isolated(doc: &Doc, writer: Option<u64>) -> Result<Repair, BodyError> {
        let copy = crate::update::copy_of(doc, None).map_err(|_| BodyError::InvalidNode {
            path: String::new(),
            problem: "the document cannot be read again to be repaired",
        })?;
        let repair = crate::repair::repaired_by(&copy, writer)?;
        if let Some(changes) = &repair.update {
            let changes = Update::decode_v1(changes).expect("the changes are an update");
            doc.transact_mut()
                .apply_update(changes)
                .expect("the changes apply to the document they were made from");
        }
        Ok(repair)
    }

    /// The Y document with an update, not yet projected or repaired.
    ///
    /// # Errors
    ///
    /// Returns why the update cannot be taken at all.
    pub fn with_update(
        doc: &Doc,
        update: &[u8],
        limits: &UpdateLimits,
    ) -> Result<Doc, UpdateError> {
        crate::update::with_update(doc, update, limits)
    }

    /// A copy of a Y document that shares nothing with it.
    ///
    /// # Errors
    ///
    /// Returns why the document cannot be read again from its state.
    pub fn copy_of(doc: &Doc) -> Result<Doc, UpdateError> {
        crate::update::copy_of(doc, None)
    }

    /// How many steps the changes of an update may take to place in a document,
    /// counted list by list and cut by cut whatever the update holds
    /// ([`UpdateLimits::placing_steps`]).
    ///
    /// # Errors
    ///
    /// Returns why the update, or the document, cannot be read.
    pub fn placing_steps(doc: &Doc, update: &[u8]) -> Result<u64, UpdateError> {
        crate::update::placing_steps(doc, update)
    }

    /// The JSON a Y document projects to, before it is checked as a body: the walk of
    /// [`to_body`] alone, without `Body::parse`.
    ///
    /// # Errors
    ///
    /// Returns why the walk refuses the document.
    pub fn projected(doc: &Doc) -> Result<Option<serde_json::Value>, BodyError> {
        use yrs::ReadTxn as _;
        let txn = doc.transact();
        txn.get_xml_fragment(crate::FIELD)
            .map_or(Ok(None), |root| crate::project::projected(&txn, &root))
    }

    /// What the changes of `update` are charged in `copy`, the document read from
    /// `state` of `changes` changes, as [`crate::take_update`] charges them within
    /// `limit`: the update read again, and its steps counted, reading the lists of the
    /// state only where the cheap count is past `limit`.
    ///
    /// # Errors
    ///
    /// Returns why the update, or the state, cannot be read.
    pub fn charged(
        copy: &Doc,
        state: &[u8],
        changes: u64,
        update: &[u8],
        limit: u64,
    ) -> Result<u64, UpdateError> {
        let (sent, _, _) = crate::update::read(copy, update)?;
        crate::update::charged(copy, (state, changes), (update, &sent), limit)
    }

    /// Whether two states are the same as [`crate::take_update`] tells it.
    #[must_use]
    pub fn same_state(one: &[u8], other: &[u8]) -> bool {
        crate::settle::same_state(one, other)
    }

    /// The size of a body as its limit is measured.
    #[must_use]
    pub fn size_of(body: &Body) -> usize {
        crate::take::size_of(body)
    }

    /// Handle to the incremental block + reading caches for pass-by-pass benchmarks (`tests/cost.rs`).
    #[derive(Debug)]
    pub struct CacheProbe {
        cache: crate::block_cache::BlockCache,
        reading: Option<Box<crate::reading::ReadingCache>>,
    }

    /// Pre-decoded update with its newly inserted shared-type IDs (output of `update::read`,
    /// prepared outside pass `4b` so `cost.rs` does not double-count `update::read`).
    #[derive(Debug)]
    pub struct PreparedUpdate {
        sent: Update,
        new_types: Vec<yrs::ID>,
    }

    /// Captured delta and transaction update of one in-place update application for
    /// [`CacheProbe::count_reading`] and [`CacheProbe::validate`].
    #[derive(Debug)]
    pub struct AppliedDelta {
        delta: crate::block_cache::ApplyDelta,
        txn_update: Vec<u8>,
        before_sv: yrs::StateVector,
        after_sv: yrs::StateVector,
    }

    /// Outcome of [`CacheProbe::validate`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum CacheValidation {
        /// Valid body with its compact JSON byte length (`None` for an empty body).
        Valid(Option<usize>),
        /// Validation failed; repair is needed.
        NeedsRepair,
    }

    impl CacheProbe {
        /// Builds the initial block + reading caches from `doc` and its encoded `state`.
        ///
        /// # Errors
        ///
        /// Returns why `doc` is not a valid body.
        pub fn build(doc: &Doc, state: &[u8]) -> Result<Self, BodyError> {
            let cache = crate::block_cache::BlockCache::build(doc)?;
            let reading = crate::reading::count_strict_with_cache(state, crate::READING_STEPS)
                .ok()
                .and_then(|(_, rc)| rc);
            Ok(Self { cache, reading })
        }

        /// Reads and decodes `update` against `doc` (the part already timed in pass `4a`).
        ///
        /// # Errors
        ///
        /// Returns why the update cannot be read.
        pub fn prepare_update(doc: &Doc, update: &[u8]) -> Result<PreparedUpdate, UpdateError> {
            let (sent, _, flags) = crate::update::read(doc, update)?;
            Ok(PreparedUpdate {
                sent,
                new_types: flags.new_types,
            })
        }

        /// Applies a pre-decoded `prepared` update to `doc` in place and captures the
        /// incremental delta + committed transaction update via `update::apply_and_capture`.
        ///
        /// # Errors
        ///
        /// Returns why the update cannot be applied.
        pub fn apply_prepared(
            doc: &Doc,
            prepared: PreparedUpdate,
        ) -> Result<AppliedDelta, UpdateError> {
            let applied = crate::update::apply_and_capture(
                doc,
                prepared.sent,
                prepared.new_types,
                crate::update::CaptureMode {
                    delta: crate::update::DeltaMode::Capture,
                    txn_update: crate::update::TxnUpdateMode::Capture,
                },
            )?;
            Ok(AppliedDelta {
                delta: applied.delta.expect("delta requested"),
                txn_update: applied.txn_update.expect("txn_update requested"),
                before_sv: applied.before_sv,
                after_sv: applied.after_sv,
            })
        }

        /// Runs the incremental Pass 5–6 reading cache update for `applied`, returning
        /// `(state_bytes, reading_steps)` or `None` if it fell back.
        #[must_use]
        pub fn count_reading(
            &mut self,
            applied: &AppliedDelta,
            limits: &UpdateLimits,
        ) -> Option<(usize, u64)> {
            let rc = self.reading.take()?;
            match rc.try_apply(
                &applied.txn_update,
                &applied.before_sv,
                &applied.after_sv,
                limits,
            ) {
                Ok(crate::reading::FastApply::Accepted { cache, summary }) => {
                    self.reading = cache;
                    Some((summary.state_bytes, summary.reading_steps))
                }
                _ => None,
            }
        }

        /// Updates the cache incrementally from `applied`.
        #[must_use]
        pub fn validate(&mut self, doc: &Doc, applied: &AppliedDelta) -> CacheValidation {
            match self.cache.apply_delta(doc, &applied.delta) {
                Ok(()) => CacheValidation::Valid(self.cache.body_bytes()),
                Err(_) => CacheValidation::NeedsRepair,
            }
        }

        /// Document-order deduplicated file IDs from the cache.
        #[must_use]
        pub fn file_ids(&self, doc: &Doc) -> Vec<String> {
            self.cache.file_ids_in_order(doc)
        }
    }

    pub use crate::reading::List;

    /// How many steps the library takes to read a state into an empty document
    /// ([`UpdateLimits::reading_steps`]), counted to the end whatever it is, and the
    /// lists the reading that counts them leaves.
    ///
    /// # Errors
    ///
    /// Returns why the bytes are no whole state.
    pub fn reading(state: &[u8]) -> Result<(u64, Vec<List>), &'static str> {
        crate::reading::lists(state).map_err(crate::scan::Refusal::reason)
    }

    /// How many entries of the stack of waiting changes the reading of a state looks
    /// through, and how many changes and clients the state has: what
    /// [`crate::steps_ahead`] holds the first to by the others.
    ///
    /// # Errors
    ///
    /// Returns why the bytes are no whole state.
    pub fn looked(state: &[u8]) -> Result<(u64, (u64, u64)), &'static str> {
        crate::reading::looked(state).map_err(crate::scan::Refusal::reason)
    }

    /// How many changes a state or an update holds.
    ///
    /// # Panics
    ///
    /// Panics if the bytes are none the walk takes.
    #[must_use]
    pub fn changes_of(update: &[u8]) -> u64 {
        crate::scan::check(update)
            .expect("an update the walk takes")
            .changes
    }

    /// An editor that holds a body's document, as a client of the server does: it
    /// takes what the server sends and makes the updates of what it writes.
    #[derive(Debug)]
    pub struct Editor {
        doc: Doc,
    }

    impl Editor {
        /// An editor with the client identifier `client` that holds the document of
        /// `state`, a version 1 update the server sent.
        ///
        /// # Panics
        ///
        /// Panics if `state` is no update.
        #[must_use]
        pub fn join(client: u64, state: &[u8]) -> Self {
            let doc = crate::build::doc_of_client(Some(client));
            let editor = Self { doc };
            editor.receive(state);
            editor
        }

        /// Takes an update the server sent.
        ///
        /// # Panics
        ///
        /// Panics if `update` is no update.
        pub fn receive(&self, update: &[u8]) {
            let update = Update::decode_v1(update).expect("the server sends updates");
            self.doc
                .transact_mut()
                .apply_update(update)
                .expect("an update the server sent applies");
        }

        /// Appends a paragraph of `text` and gives the update of it.
        #[must_use]
        pub fn append_paragraph(&self, text: &str) -> Vec<u8> {
            use yrs::{ReadTxn as _, XmlElementPrelim, XmlFragment as _, XmlTextPrelim};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            {
                let mut txn = self.doc.transact_mut();
                let paragraph = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
                paragraph.push_back(&mut txn, XmlTextPrelim::new(text));
            }
            self.doc.transact().encode_diff_v1(&before)
        }

        /// Appends a paragraph that holds a mention of the user `user_id` and gives the
        /// update of it.
        #[must_use]
        pub fn append_mention(&self, user_id: &str) -> Vec<u8> {
            use yrs::{ReadTxn as _, Xml as _, XmlElementPrelim, XmlFragment as _};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            {
                let mut txn = self.doc.transact_mut();
                let paragraph = root.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
                let mention = paragraph.push_back(&mut txn, XmlElementPrelim::empty("mention"));
                mention.insert_attribute(&mut txn, "userId", user_id);
            }
            self.doc.transact().encode_diff_v1(&before)
        }

        /// Appends a board view of the view `view_id` and gives the update of it.
        #[must_use]
        pub fn append_board_view(&self, view_id: &str) -> Vec<u8> {
            use yrs::{ReadTxn as _, Xml as _, XmlElementPrelim, XmlFragment as _};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            {
                let mut txn = self.doc.transact_mut();
                let view = root.push_back(&mut txn, XmlElementPrelim::empty("boardView"));
                view.insert_attribute(&mut txn, "viewId", view_id);
            }
            self.doc.transact().encode_diff_v1(&before)
        }

        /// Appends an empty element `name` and gives the update of it: of a node no body
        /// has, an update the server repairs.
        #[must_use]
        pub fn append_element(&self, name: &str) -> Vec<u8> {
            use yrs::{ReadTxn as _, XmlElementPrelim, XmlFragment as _};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            root.push_back(&mut self.doc.transact_mut(), XmlElementPrelim::empty(name));
            self.doc.transact().encode_diff_v1(&before)
        }

        /// Types `letters` at the end of the text of the last paragraph and gives the
        /// update of it.
        ///
        /// # Panics
        ///
        /// Panics if what is last is no paragraph of a text.
        #[must_use]
        pub fn type_at_end(&self, letters: &str) -> Vec<u8> {
            use yrs::{ReadTxn as _, Text as _, XmlFragment as _, XmlOut};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            {
                let mut txn = self.doc.transact_mut();
                let last = root
                    .len(&txn)
                    .checked_sub(1)
                    .and_then(|at| root.get(&txn, at));
                let Some(XmlOut::Element(paragraph)) = last else {
                    panic!("the last node is a paragraph: {last:?}");
                };
                let Some(XmlOut::Text(text)) = paragraph.first_child() else {
                    panic!("the last paragraph has a text");
                };
                let end = text.len(&txn);
                text.insert(&mut txn, end, letters);
            }
            self.doc.transact().encode_diff_v1(&before)
        }

        /// Adds a column to the first table at the top: an empty cell at the end of
        /// each row, and gives the update of it.
        ///
        /// # Panics
        ///
        /// Panics if there is no table at the top.
        #[must_use]
        pub fn add_table_column(&self) -> Vec<u8> {
            self.edit_table(|txn, table| {
                use yrs::{XmlFragment as _, XmlOut};
                let rows: Vec<_> = table.children(txn).collect();
                for row in rows {
                    if let XmlOut::Element(row) = row {
                        let end = row.len(txn);
                        let cell = serde_json::json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
                        crate::build::insert_node(txn, &row, end, &cell);
                    }
                }
            })
        }

        /// Adds a row of `cells` empty cells at the end of the first table at the top,
        /// and gives the update of it.
        ///
        /// # Panics
        ///
        /// Panics if there is no table at the top.
        #[must_use]
        pub fn add_table_row(&self, cells: usize) -> Vec<u8> {
            self.edit_table(|txn, table| {
                use yrs::XmlFragment as _;
                let cell =
                    serde_json::json!({"type": "tableCell", "content": [{"type": "paragraph"}]});
                let row = serde_json::json!({"type": "tableRow", "content": vec![cell; cells]});
                let end = table.len(txn);
                crate::build::insert_node(txn, table, end, &row);
            })
        }

        fn edit_table(
            &self,
            edit: impl FnOnce(&mut yrs::TransactionMut<'_>, &yrs::XmlElementRef),
        ) -> Vec<u8> {
            use yrs::{ReadTxn as _, XmlFragment as _, XmlOut};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            {
                let mut txn = self.doc.transact_mut();
                let table = root
                    .children(&txn)
                    .find_map(|child| match child {
                        XmlOut::Element(element) if element.tag().as_ref() == "table" => {
                            Some(element)
                        }
                        _ => None,
                    })
                    .expect("a table at the top");
                edit(&mut txn, &table);
            }
            self.doc.transact().encode_diff_v1(&before)
        }

        /// Deletes the last paragraph, or whatever is last, and gives the update of it.
        #[must_use]
        pub fn delete_last(&self) -> Vec<u8> {
            use yrs::{ReadTxn as _, XmlFragment as _};
            let before = self.doc.transact().state_vector();
            let root = self.doc.get_or_insert_xml_fragment(crate::FIELD);
            {
                let mut txn = self.doc.transact_mut();
                let len = root.len(&txn);
                if len > 0 {
                    root.remove_range(&mut txn, len - 1, 1);
                }
            }
            self.doc.transact().encode_diff_v1(&before)
        }

        /// The state vector of what the editor holds, version 1.
        #[must_use]
        pub fn state_vector(&self) -> Vec<u8> {
            use yrs::ReadTxn as _;
            use yrs::updates::encoder::Encode as _;
            self.doc.transact().state_vector().encode_v1()
        }

        /// What the editor holds that a holder of `state_vector` (version 1) lacks, as
        /// an update: every deletion it knows of, whoever made it, is in it.
        ///
        /// # Panics
        ///
        /// Panics if `state_vector` is none.
        #[must_use]
        pub fn missing_from(&self, state_vector: &[u8]) -> Vec<u8> {
            use yrs::updates::decoder::Decode as _;
            use yrs::{ReadTxn as _, StateVector};
            let theirs = StateVector::decode_v1(state_vector).expect("a state vector");
            self.doc.transact().encode_diff_v1(&theirs)
        }

        /// The body of what the editor holds.
        ///
        /// # Panics
        ///
        /// Panics if it is no body.
        #[must_use]
        pub fn body(&self) -> Option<Body> {
            crate::project::to_body(&self.doc).expect("an editor holds a body")
        }
    }
}

/// The name of the XML fragment at the root of the Y document that holds the body: the
/// one the editor's collaboration extension uses when it is given none.
pub const FIELD: &str = "default";
