//! Linear `O(N)` multi-client V1 wire encoder for [`crate::from_body`].
//!
//! Writes every parent's children and every `XmlText`'s formatting/string items directly
//! into lib0 v1 chunks in `push_front` order (`left == None`, `right == prev_head`) across
//! strictly descending `ClientID`s (`C_0 > C_1 > ... > C_{K-1}`), guaranteeing:
//! - strictly linear `O(N)` encoding (no quadratic `split_block` or `Vec::insert`),
//! - per-client change counts bounded by [`CHANGES_PER_CLIENT`] (`MAX_CLIENT_CHANGES / 4`),
//! - zero conflicts, zero cuts, zero `BlockPicker` waits (`reading_steps == 0`), and
//! - canonical output identical byte-for-byte to `state_of(&doc)`.

use fvoci_model::Body;
use serde_json::Value;
use yrs::encoding::write::Write as _;
use yrs::updates::encoder::Encoder;
use yrs::{Any, ClientID, ID, Number};

use super::{any_of, content_of, is_text, letters_of};
use crate::FIELD;
use crate::doc::StateCounts;
use crate::reading::{DirectItem, Holds, LOOKED_PER_STEP, ReadingCache, ReadingCacheBuilder};
use crate::schema::{self, Fallback};
use crate::update::MAX_CLIENT_CHANGES;

/// Largest client identifier accepted by JavaScript and [`crate::scan`] (53 bits).
pub(super) const MAX_WRITER: u64 = (1 << 53) - 1;

/// Lower bound for random 32-bit writers in [`crate::from_body`], leaving >1M descending
/// client IDs below the initial client while remaining within 32 bits (`<= u32::MAX`,
/// strictly below `repair::FIRST_WRITER`).
pub(super) const MIN_RANDOM_WRITER: u64 = 1 << 20;

/// Maximum clock accepted by [`crate::scan`] (31 bits).
const MAX_CLIENT_CLOCK: u32 = 0x7fff_ffff;

/// Maximum changes written under one client ID before rolling to the next descending ID.
/// Chosen as `MAX_CLIENT_CHANGES / 4` (131,072) so large bodies spread over a small
/// handful of clients (3 clients for 50k-node marked shapes) while staying far below both
/// `MAX_CLIENT_CHANGES` (512 Ki) and the `OpenDoc::worn` `steps_ahead` threshold.
pub(super) const CHANGES_PER_CLIENT: u32 = 128 * 1024;

const _: () = {
    assert!(
        MAX_CLIENT_CHANGES / 4 == 128 * 1024,
        "CHANGES_PER_CLIENT must equal MAX_CLIENT_CHANGES / 4"
    );
};

/// lib0 v1 item header flags and content kind tags matching `yrs::block`.
const HAS_RIGHT_ORIGIN: u8 = 0x40;
const HAS_PARENT_SUB: u8 = 0x20;
const CONTENT_STRING: u8 = 4;
const CONTENT_FORMAT: u8 = 6;
const CONTENT_TYPE: u8 = 7;
const CONTENT_ANY: u8 = 8;
const TYPE_XML_ELEMENT: u8 = 3;
const TYPE_XML_TEXT: u8 = 6;

/// Whether [`encode_body_with_plan`] also captures [`ReadingCache`] and top-level block IDs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CacheMode {
    None,
    Full,
}

/// Output of [`encode_body_with_plan`]: canonical v1 state bytes, exact counts, and
/// optional caches.
pub(super) struct EncodedBody {
    pub(super) state: Vec<u8>,
    pub(super) last_client: u64,
    pub(super) counts: StateCounts,
    pub(super) reading_cache: Option<Box<ReadingCache>>,
    pub(super) top_level_ids: Vec<ID>,
    /// Set when unvalidated `Body::from_stored` values (nested/out-of-range attrs or
    /// non-canonical mark attrs) require `state_of(&doc)` + `scan` verification.
    pub(super) needs_verify: bool,
}

/// Reference to an item already emitted into an earlier or current client chunk.
#[derive(Debug, Clone, Copy)]
struct EmittedRef {
    id: ID,
    client_idx: u16,
    piece_idx: u32,
}

/// Parent target of an item emitted with `right == None`.
#[derive(Debug, Clone, Copy)]
enum ParentRef {
    Root,
    Item(EmittedRef),
}

/// Metadata for one wire item emitted into the active [`ClientChunk`].
#[derive(Debug, Clone, Copy)]
struct ItemSpec<'a> {
    parent: ParentRef,
    right: Option<EmittedRef>,
    member: Option<&'a str>,
    clock_len: u32,
    content_kind: u8,
    holds: Holds,
    utf8_len: u32,
    has_non_bmp: bool,
}

/// Zero-overhead lib0 v1 byte buffer implementing `yrs`'s [`Encoder`] directly onto a
/// single contiguous `Vec<u8>` so item byte lengths can be measured with `buf.len()`.
struct ChunkEncoder(Vec<u8>);

impl ChunkEncoder {
    fn with_capacity(cap: usize) -> Self {
        Self(Vec::with_capacity(cap))
    }

    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn as_slice(&self) -> &[u8] {
        &self.0
    }

    #[inline]
    fn write_id(&mut self, id: &ID) {
        self.write_var(id.client.get());
        self.write_var(id.clock);
    }
}

impl yrs::encoding::write::Write for ChunkEncoder {
    #[inline]
    fn write_all(&mut self, buf: &[u8]) {
        self.0.extend_from_slice(buf);
    }

    #[inline]
    fn write_u8(&mut self, value: u8) {
        self.0.push(value);
    }
}

impl Encoder for ChunkEncoder {
    #[inline]
    fn to_vec(self) -> Vec<u8> {
        self.0
    }

    #[inline]
    fn reset_ds_cur_val(&mut self) {}

    #[inline]
    fn write_ds_clock(&mut self, clock: u32) {
        self.write_var(clock);
    }

    #[inline]
    fn write_ds_len(&mut self, len: u32) {
        self.write_var(len);
    }

    #[inline]
    fn write_left_id(&mut self, id: &ID) {
        self.write_id(id);
    }

    #[inline]
    fn write_right_id(&mut self, id: &ID) {
        self.write_id(id);
    }

    #[inline]
    fn write_client(&mut self, client: ClientID) {
        self.write_var(client.get());
    }

    #[inline]
    fn write_info(&mut self, info: u8) {
        self.write_u8(info);
    }

    #[inline]
    fn write_parent_info(&mut self, is_y_key: bool) {
        self.write_var(u32::from(is_y_key));
    }

    #[inline]
    fn write_type_ref(&mut self, info: u8) {
        self.write_u8(info);
    }

    #[inline]
    fn write_len(&mut self, len: u32) {
        self.write_var(len);
    }

    #[inline]
    fn write_any(&mut self, any: &Any) {
        any.encode(self);
    }

    fn write_json(&mut self, any: &Any) {
        let mut buf = String::new();
        any.to_json(&mut buf);
        self.write_string(buf.as_str());
    }

    #[inline]
    fn write_key(&mut self, key: &str) {
        self.write_string(key);
    }
}

struct ClientChunk {
    client: u64,
    client_idx: u16,
    count: u32,
    clock: u32,
    enc: ChunkEncoder,
}

impl ClientChunk {
    fn new(client: u64, client_idx: u16, capacity: usize) -> Self {
        Self {
            client,
            client_idx,
            count: 0,
            clock: 0,
            enc: ChunkEncoder::with_capacity(capacity.max(4096)),
        }
    }
}

/// Summary of one pass over a stored [`Body`]'s JSON tree used to pick the encoding path,
/// starting client ID, and buffer capacities.
#[derive(Debug, Clone, Copy)]
pub(super) struct BodyPlan {
    pub(super) max_changes: u64,
    pub(super) utf8_bytes: u64,
    pub(super) max_chunks: u64,
    pub(super) has_marks: bool,
}

impl BodyPlan {
    /// Whether the body has no inline marks and fits inside a single client chunk, allowing
    /// direct single-transaction `Doc` construction with identical canonical bytes.
    #[must_use]
    pub(super) fn can_build_direct(self) -> bool {
        !self.has_marks && self.max_chunks == 1
    }

    fn initial_chunk_capacity(self) -> usize {
        let est = self
            .utf8_bytes
            .saturating_add(self.max_changes.saturating_mul(16));
        let per_chunk = est.div_ceil(self.max_chunks.max(1));
        usize::try_from(per_chunk.min(8 * 1024 * 1024)).unwrap_or(64 * 1024)
    }

    fn estimated_items(self) -> usize {
        usize::try_from(self.max_changes.min(1024 * 1024)).unwrap_or(1024 * 1024)
    }
}

/// Zero-allocation scan over `document` computing upper-bound changes, UTF-8 text bytes,
/// chunk count, and whether any text node carries marks.
pub(super) fn scan_body_plan(document: &Value) -> BodyPlan {
    fn walk(node: &Value, plan: &mut BodyPlan) {
        if is_text(node) {
            let text = letters_of(node);
            plan.utf8_bytes = plan
                .utf8_bytes
                .saturating_add(u64::try_from(text.len()).unwrap_or(u64::MAX));
            let marks = node
                .get("marks")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if marks > 0 {
                plan.has_marks = true;
            }
            // Worst case: own XmlText + String + open & close per mark.
            plan.max_changes = plan
                .max_changes
                .saturating_add(2 + u64::try_from(marks).unwrap_or(0) * 2);
            return;
        }
        let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
        let attrs = schema::node_attributes(kind).map_or(0, <[schema::Attribute]>::len);
        plan.max_changes = plan
            .max_changes
            .saturating_add(1 + u64::try_from(attrs).unwrap_or(0));
        for child in content_of(node) {
            walk(child, plan);
        }
    }
    let mut plan = BodyPlan {
        max_changes: 0,
        utf8_bytes: 0,
        max_chunks: 1,
        has_marks: false,
    };
    for child in content_of(document) {
        walk(child, &mut plan);
    }
    let by_changes = plan.max_changes.div_ceil(u64::from(CHANGES_PER_CLIENT));
    let by_clock = plan.utf8_bytes / u64::from(MAX_CLIENT_CLOCK / 2);
    plan.max_chunks = by_changes.saturating_add(by_clock).max(1);
    plan
}

/// Encodes `body` into canonical lib0 v1 bytes in linear `O(N)` time, splitting across
/// descending `ClientID`s whenever a chunk reaches [`CHANGES_PER_CLIENT`].
pub(super) fn encode_body_with_plan(
    body: &Body,
    writer: u64,
    plan: BodyPlan,
    mode: CacheMode,
) -> EncodedBody {
    let start_client = start_client_for(plan, writer);
    let mut builder = WireBuilder::new(start_client, plan, mode);
    let root_nodes = content_of(body.document());
    builder.emit_children(ParentRef::Root, root_nodes);
    builder.finish()
}

/// Picks a deterministic starting `ClientID` (`C_0`) so `C_0 > C_1 > ... > C_{K-1} >= 0`
/// never underflows even when a caller passes a tiny `writer` (e.g. `0` or `1`) on a
/// large body. Small bodies (`max_chunks == 1`) and random writers (`>= 1 << 20`) keep
/// `writer & MAX_WRITER` unchanged without shifting.
fn start_client_for(plan: BodyPlan, writer: u64) -> u64 {
    let base = writer & MAX_WRITER;
    if plan.max_chunks > 1 && base < plan.max_chunks - 1 {
        base + (plan.max_chunks - 1)
    } else {
        base
    }
}

#[derive(Clone, PartialEq)]
enum MarkAttrs<'a> {
    Empty,
    LinkHref(&'a str),
    Other(Any),
}

#[derive(Clone, PartialEq)]
struct MarkSpec<'a> {
    rank: usize,
    name: &'a str,
    attrs: MarkAttrs<'a>,
}

#[derive(Clone, Copy)]
enum SpanText<'a> {
    Single(&'a str),
    Joined(usize, usize),
}

struct TextSpan<'a> {
    text: SpanText<'a>,
    marks: (usize, usize),
}

#[derive(Clone, Copy)]
enum TextOp<'a> {
    Text(SpanText<'a>),
    OpenMark(usize),
    CloseMark(&'a str),
}

/// Owns client chunks and optional [`ReadingCacheBuilder`] independently of
/// [`WireBuilder`]'s scratch buffers so item emission never splits borrows.
struct ChunkWriter {
    start_client: u64,
    chunk_capacity: usize,
    chunks: Vec<ClientChunk>,
    reading: Option<ReadingCacheBuilder>,
    needs_verify: bool,
}

impl ChunkWriter {
    fn new(start_client: u64, plan: BodyPlan, mode: CacheMode) -> Self {
        let mut reading = (mode == CacheMode::Full)
            .then(|| ReadingCacheBuilder::with_capacity(FIELD.as_bytes(), plan.estimated_items()));
        let first_idx = reading
            .as_mut()
            .and_then(|rb| rb.start_client(start_client))
            .unwrap_or(0);
        let chunk_capacity = plan.initial_chunk_capacity();
        Self {
            start_client,
            chunk_capacity,
            chunks: vec![ClientChunk::new(start_client, first_idx, chunk_capacity)],
            reading,
            needs_verify: false,
        }
    }

    fn ensure_room(&mut self, clock_len: u32) {
        let chunk = self.chunks.last().expect("at least one chunk exists");
        let full = chunk.count >= CHANGES_PER_CLIENT
            || chunk.clock.saturating_add(clock_len) > MAX_CLIENT_CLOCK;
        if full && chunk.count > 0 && chunk.client > 0 {
            let next_client = chunk.client - 1;
            let next_idx = self
                .reading
                .as_mut()
                .and_then(|rb| rb.start_client(next_client))
                .unwrap_or_else(|| u16::try_from(self.chunks.len()).unwrap_or(u16::MAX));
            self.chunks
                .push(ClientChunk::new(next_client, next_idx, self.chunk_capacity));
        }
    }

    fn emit_item(
        &mut self,
        spec: ItemSpec<'_>,
        write_content: impl FnOnce(&mut ChunkEncoder),
    ) -> EmittedRef {
        self.ensure_room(spec.clock_len);
        let chunk = self.chunks.last_mut().expect("at least one chunk exists");
        let begin = chunk.clock;
        let client_idx = chunk.client_idx;
        let id = ID::new(ClientID::new(chunk.client), begin);
        let wire_bytes = write_wire_item(&mut chunk.enc, &spec, write_content);
        chunk.count += 1;
        chunk.clock = begin.saturating_add(spec.clock_len);
        let piece_idx = self.record_item_meta(&spec, client_idx, begin, wire_bytes);
        EmittedRef {
            id,
            client_idx,
            piece_idx,
        }
    }

    fn record_item_meta(
        &mut self,
        spec: &ItemSpec<'_>,
        client_idx: u16,
        begin: u32,
        wire_bytes: usize,
    ) -> u32 {
        let Some(rb) = self.reading.as_mut() else {
            return 0;
        };
        let (parent_piece, parent_dep) = match spec.parent {
            ParentRef::Root => (ReadingCacheBuilder::ROOT_DEFAULT, None),
            ParentRef::Item(p) => (
                p.piece_idx,
                spec.right.is_none().then_some((p.client_idx, p.id.clock)),
            ),
        };
        rb.record_item(DirectItem {
            client_idx,
            begin,
            len: spec.clock_len,
            parent: parent_piece,
            right: spec.right.map(|r| (r.client_idx, r.id.clock, r.piece_idx)),
            parent_dep,
            member: spec.member.map(str::as_bytes),
            holds: spec.holds,
            utf8_len: spec.utf8_len,
            has_non_bmp: spec.has_non_bmp,
            wire_bytes,
        })
        .unwrap_or(0)
    }

    fn finish(self, top_level_ids: Vec<ID>) -> EncodedBody {
        let non_empty: Vec<ClientChunk> = self
            .chunks
            .into_iter()
            .filter(|chunk| chunk.count > 0)
            .collect();
        let last_client = non_empty
            .last()
            .map_or(self.start_client, |chunk| chunk.client);
        let payload_sum: usize = non_empty.iter().map(|c| c.enc.len()).sum();
        let mut out = ChunkEncoder::with_capacity(payload_sum + non_empty.len() * 16 + 8);
        out.write_var(non_empty.len());
        let mut changes = 0_u64;
        let mut client_changes = 0_u64;
        for chunk in &non_empty {
            out.write_var(chunk.count);
            out.write_client(ClientID::new(chunk.client));
            out.write_var(0_u32);
            out.write_all(chunk.enc.as_slice());
            let count = u64::from(chunk.count);
            changes = changes.saturating_add(count);
            client_changes = client_changes.max(count);
        }
        // Empty delete set (0 clients).
        out.write_var(0_u32);
        let state = out.to_vec();
        let clients = u64::try_from(non_empty.len()).unwrap_or(u64::MAX);
        let steps_ahead = changes.saturating_mul(clients).div_ceil(LOOKED_PER_STEP);
        let had_reading = self.reading.is_some();
        let reading_cache = self.reading.and_then(|rb| rb.finish(state.len()));
        let needs_verify = self.needs_verify || (had_reading && reading_cache.is_none());
        EncodedBody {
            state,
            last_client,
            counts: StateCounts {
                changes,
                client_changes,
                steps_ahead,
            },
            reading_cache,
            top_level_ids,
            needs_verify,
        }
    }
}

struct WireBuilder<'doc> {
    writer: ChunkWriter,
    capture_top_ids: bool,
    top_level_ids: Vec<ID>,
    /// Reusable scratch buffers for `emit_text_stretch`.
    tmp_marks: Vec<MarkSpec<'doc>>,
    mark_pool: Vec<MarkSpec<'doc>>,
    spans: Vec<TextSpan<'doc>>,
    joined_buf: String,
    text_ops: Vec<TextOp<'doc>>,
    json_buf: Vec<u8>,
}

impl<'doc> WireBuilder<'doc> {
    fn new(start_client: u64, plan: BodyPlan, mode: CacheMode) -> Self {
        Self {
            writer: ChunkWriter::new(start_client, plan, mode),
            capture_top_ids: mode == CacheMode::Full,
            top_level_ids: Vec::new(),
            tmp_marks: Vec::new(),
            mark_pool: Vec::new(),
            spans: Vec::new(),
            joined_buf: String::new(),
            text_ops: Vec::new(),
            json_buf: Vec::new(),
        }
    }

    /// Emits all children of `parent` in reverse (`push_front`) order so each child has
    /// `left == None` and `right == prev_head`.
    fn emit_children(&mut self, parent: ParentRef, nodes: &'doc [Value]) {
        let is_root = matches!(parent, ParentRef::Root);
        let mut right: Option<EmittedRef> = None;
        let mut rest = nodes;
        while let Some(last) = rest.last() {
            let stretch = if is_text(last) {
                rest.iter().rev().take_while(|node| is_text(node)).count()
            } else {
                1
            };
            let (before, these) = rest.split_at(rest.len() - stretch);
            let emitted = match these {
                [node] if !is_text(node) => {
                    let elem = self.emit_element(parent, right, node);
                    if is_root && self.capture_top_ids {
                        self.top_level_ids.push(elem.id);
                    }
                    elem
                }
                texts => self.emit_text_stretch(parent, right, texts),
            };
            right = Some(emitted);
            rest = before;
        }
    }

    fn emit_element(
        &mut self,
        parent: ParentRef,
        right: Option<EmittedRef>,
        node: &'doc Value,
    ) -> EmittedRef {
        let kind = node.get("type").and_then(Value::as_str).unwrap_or_default();
        let elem = self.writer.emit_item(
            ItemSpec {
                parent,
                right,
                member: None,
                clock_len: 1,
                content_kind: CONTENT_TYPE,
                holds: Holds::Type,
                utf8_len: 0,
                has_non_bmp: false,
            },
            |enc| {
                enc.write_type_ref(TYPE_XML_ELEMENT);
                enc.write_string(kind);
            },
        );
        self.emit_children(ParentRef::Item(elem), content_of(node));
        let given = node.get("attrs");
        for attribute in schema::node_attributes(kind).unwrap_or(&[]) {
            let attr_spec = ItemSpec {
                parent: ParentRef::Item(elem),
                right: None,
                member: Some(attribute.name),
                clock_len: 1,
                content_kind: CONTENT_ANY,
                holds: Holds::Values,
                utf8_len: 0,
                has_non_bmp: false,
            };
            match given.and_then(|attrs| attrs.get(attribute.name)) {
                Some(value) => {
                    let mut complex = false;
                    self.writer.emit_item(attr_spec, |enc| {
                        enc.write_len(1);
                        complex = encode_value_as_any(enc, value);
                    });
                    if complex {
                        self.writer.needs_verify = true;
                    }
                }
                None => match attribute.fallback {
                    Fallback::None => {}
                    Fallback::Integer(number) => {
                        self.writer.emit_item(attr_spec, |enc| {
                            enc.write_len(1);
                            Any::Number(Number::Int(number)).encode(enc);
                        });
                    }
                    Fallback::Boolean(truth) => {
                        self.writer.emit_item(attr_spec, |enc| {
                            enc.write_len(1);
                            enc.write_u8(if truth { 120 } else { 121 });
                        });
                    }
                    Fallback::Text(text) => {
                        self.writer.emit_item(attr_spec, |enc| {
                            enc.write_len(1);
                            Any::from(text).encode(enc);
                        });
                    }
                },
            }
        }
        elem
    }

    /// Emits an `XmlText` and its coalesced runs + minimal format transitions in linear
    /// `O(N)` time using reverse (`push_front`) child order.
    fn emit_text_stretch(
        &mut self,
        parent: ParentRef,
        right: Option<EmittedRef>,
        nodes: &'doc [Value],
    ) -> EmittedRef {
        let text_ref = self.writer.emit_item(
            ItemSpec {
                parent,
                right,
                member: None,
                clock_len: 1,
                content_kind: CONTENT_TYPE,
                holds: Holds::Type,
                utf8_len: 0,
                has_non_bmp: false,
            },
            |enc| enc.write_type_ref(TYPE_XML_TEXT),
        );
        self.prepare_text_ops(nodes);
        let mut child_right: Option<EmittedRef> = None;
        for op_idx in (0..self.text_ops.len()).rev() {
            let emitted = match self.text_ops[op_idx] {
                TextOp::Text(span_text) => self.emit_text_leaf(text_ref, child_right, span_text),
                TextOp::OpenMark(mark_idx) => {
                    self.emit_format_open(text_ref, child_right, mark_idx)
                }
                TextOp::CloseMark(name) => self.writer.emit_item(
                    ItemSpec {
                        parent: ParentRef::Item(text_ref),
                        right: child_right,
                        member: None,
                        clock_len: 1,
                        content_kind: CONTENT_FORMAT,
                        holds: Holds::One,
                        utf8_len: 0,
                        has_non_bmp: false,
                    },
                    |enc| {
                        enc.write_key(name);
                        enc.write_string("null");
                    },
                ),
            };
            child_right = Some(emitted);
        }
        text_ref
    }

    fn emit_text_leaf(
        &mut self,
        text_ref: EmittedRef,
        right: Option<EmittedRef>,
        span_text: SpanText<'doc>,
    ) -> EmittedRef {
        let slice = match span_text {
            SpanText::Single(s) => s,
            SpanText::Joined(start, end) => &self.joined_buf[start..end],
        };
        let (utf16_len, has_non_bmp) = utf16_len_and_non_bmp(slice);
        let utf8_len = u32::try_from(slice.len()).unwrap_or(u32::MAX);
        self.writer.emit_item(
            ItemSpec {
                parent: ParentRef::Item(text_ref),
                right,
                member: None,
                clock_len: utf16_len,
                content_kind: CONTENT_STRING,
                holds: Holds::Letters,
                utf8_len,
                has_non_bmp,
            },
            |enc| enc.write_string(slice),
        )
    }

    fn emit_format_open(
        &mut self,
        text_ref: EmittedRef,
        right: Option<EmittedRef>,
        mark_idx: usize,
    ) -> EmittedRef {
        let spec = ItemSpec {
            parent: ParentRef::Item(text_ref),
            right,
            member: None,
            clock_len: 1,
            content_kind: CONTENT_FORMAT,
            holds: Holds::One,
            utf8_len: 0,
            has_non_bmp: false,
        };
        let mark = &self.mark_pool[mark_idx];
        let name = mark.name;
        match &mark.attrs {
            MarkAttrs::Empty => self.writer.emit_item(spec, |enc| {
                enc.write_key(name);
                enc.write_string("{}");
            }),
            MarkAttrs::LinkHref(href) => {
                self.json_buf.clear();
                self.json_buf.extend_from_slice(br#"{"href":"#);
                let _ = serde_json::to_writer(&mut self.json_buf, *href);
                self.json_buf.push(b'}');
                let json = self.json_buf.as_slice();
                self.writer.emit_item(spec, |enc| {
                    enc.write_key(name);
                    enc.write_var(json.len());
                    enc.write_all(json);
                })
            }
            MarkAttrs::Other(any) => {
                self.writer.needs_verify = true;
                self.writer.emit_item(spec, |enc| {
                    enc.write_key(name);
                    enc.write_json(any);
                })
            }
        }
    }

    /// Populates `self.text_ops` in visual left-to-right order for a stretch of text nodes,
    /// coalescing consecutive runs with identical marks and emitting minimal transitions.
    fn prepare_text_ops(&mut self, nodes: &'doc [Value]) {
        self.mark_pool.clear();
        self.spans.clear();
        self.joined_buf.clear();
        self.text_ops.clear();

        for node in nodes {
            let letters = letters_of(node);
            if letters.is_empty() {
                continue;
            }
            extract_marks(node, &mut self.tmp_marks);
            let same_as_prev = self.spans.last().is_some_and(|prev| {
                &self.mark_pool[prev.marks.0..prev.marks.1] == self.tmp_marks.as_slice()
            });
            if same_as_prev && let Some(prev) = self.spans.last_mut() {
                match &mut prev.text {
                    SpanText::Single(first) => {
                        let start = self.joined_buf.len();
                        self.joined_buf.push_str(first);
                        self.joined_buf.push_str(letters);
                        prev.text = SpanText::Joined(start, self.joined_buf.len());
                    }
                    SpanText::Joined(_, end) => {
                        self.joined_buf.push_str(letters);
                        *end = self.joined_buf.len();
                    }
                }
            } else {
                let m_start = self.mark_pool.len();
                self.mark_pool.append(&mut self.tmp_marks);
                let m_end = self.mark_pool.len();
                self.spans.push(TextSpan {
                    text: SpanText::Single(letters),
                    marks: (m_start, m_end),
                });
            }
        }

        let mut prev_range: (usize, usize) = (0, 0);
        for span in &self.spans {
            let prev = &self.mark_pool[prev_range.0..prev_range.1];
            let curr = &self.mark_pool[span.marks.0..span.marks.1];
            for pm in prev {
                if !curr.iter().any(|cm| cm.name == pm.name) {
                    self.text_ops.push(TextOp::CloseMark(pm.name));
                }
            }
            for (offset, cm) in curr.iter().enumerate() {
                let unchanged = prev
                    .iter()
                    .any(|pm| pm.name == cm.name && pm.attrs == cm.attrs);
                if !unchanged {
                    self.text_ops.push(TextOp::OpenMark(span.marks.0 + offset));
                }
            }
            self.text_ops.push(TextOp::Text(span.text));
            prev_range = span.marks;
        }
        for pm in &self.mark_pool[prev_range.0..prev_range.1] {
            self.text_ops.push(TextOp::CloseMark(pm.name));
        }
    }

    fn finish(mut self) -> EncodedBody {
        self.top_level_ids.reverse();
        self.writer.finish(self.top_level_ids)
    }
}

/// Encodes one lib0 v1 item directly into `enc` and returns the exact wire bytes written.
fn write_wire_item(
    enc: &mut ChunkEncoder,
    spec: &ItemSpec<'_>,
    write_content: impl FnOnce(&mut ChunkEncoder),
) -> usize {
    let start = enc.len();
    let mut info = spec.content_kind;
    if spec.right.is_some() {
        info |= HAS_RIGHT_ORIGIN;
    }
    if spec.member.is_some() {
        info |= HAS_PARENT_SUB;
    }
    enc.write_info(info);
    if let Some(right_ref) = spec.right {
        enc.write_right_id(&right_ref.id);
    } else {
        match spec.parent {
            ParentRef::Root => {
                enc.write_parent_info(true);
                enc.write_string(FIELD);
            }
            ParentRef::Item(parent_ref) => {
                enc.write_parent_info(false);
                enc.write_left_id(&parent_ref.id);
            }
        }
        if let Some(key) = spec.member {
            enc.write_string(key);
        }
    }
    write_content(enc);
    enc.len() - start
}

/// Encodes a JSON attribute value directly in lib0 Any wire format without heap-allocating
/// an intermediate [`Any`] for primitives (`Null`, `Bool`, `Number`, `String`). Returns
/// `true` if `value` is complex or outside 53-bit integer bounds and needs post-decode
/// verification (`scan` / canonical re-encoding).
fn encode_value_as_any(enc: &mut ChunkEncoder, value: &Value) -> bool {
    match value {
        Value::Null => {
            enc.write_u8(126);
            false
        }
        Value::Bool(truth) => {
            enc.write_u8(if *truth { 120 } else { 121 });
            false
        }
        Value::String(text) => {
            enc.write_u8(119);
            enc.write_string(text);
            false
        }
        Value::Number(number) => {
            let num = number.as_i64().map_or_else(
                || Number::try_i64(number.as_f64().unwrap_or(f64::NAN)),
                Number::Int,
            );
            Any::Number(num).encode(enc);
            matches!(
                num,
                Number::Int(n)
                    if !(Number::I64_MIN_SAFE_INTEGER..=Number::I64_MAX_SAFE_INTEGER).contains(&n)
            )
        }
        Value::Array(_) | Value::Object(_) => {
            any_of(value).encode(enc);
            true
        }
    }
}

fn extract_marks<'doc>(node: &'doc Value, out: &mut Vec<MarkSpec<'doc>>) {
    out.clear();
    let Some(marks) = node.get("marks").and_then(Value::as_array) else {
        return;
    };
    for mark in marks {
        let Some(name) = mark.get("type").and_then(Value::as_str) else {
            continue;
        };
        if out.iter().any(|existing| existing.name == name) {
            continue;
        }
        let Some(attrs) = mark_attrs_of(mark.get("attrs")) else {
            continue;
        };
        let rank = schema::mark_rank(name).unwrap_or(usize::MAX);
        out.push(MarkSpec { rank, name, attrs });
    }
    out.sort_unstable_by(|a, b| (a.rank, a.name).cmp(&(b.rank, b.name)));
}

fn mark_attrs_of(given: Option<&Value>) -> Option<MarkAttrs<'_>> {
    match given {
        None => Some(MarkAttrs::Empty),
        Some(Value::Null) => None,
        Some(Value::Object(map)) if map.is_empty() => Some(MarkAttrs::Empty),
        Some(val @ Value::Object(map)) if map.len() == 1 => {
            if let Some(Value::String(href)) = map.get("href") {
                Some(MarkAttrs::LinkHref(href.as_str()))
            } else {
                Some(MarkAttrs::Other(any_of(val)))
            }
        }
        Some(other) => {
            let any = any_of(other);
            // `Any::Null` / `Undefined` / non-finite floats serialize to JSON `"null"`
            // and are deleted by `yrs::cleanup_text_fmt` and dropped by `read::marks_of`.
            if matches!(any, Any::Null | Any::Undefined)
                || matches!(any, Any::Number(Number::Float(f)) if !f.is_finite())
            {
                None
            } else {
                Some(MarkAttrs::Other(any))
            }
        }
    }
}

/// Computes `(utf16_len, has_non_bmp)` in one linear pass over UTF-8 bytes.
fn utf16_len_and_non_bmp(text: &str) -> (u32, bool) {
    if text.is_ascii() {
        return (u32::try_from(text.len()).unwrap_or(u32::MAX), false);
    }
    let mut len = 0_usize;
    let mut has_non_bmp = false;
    for &b in text.as_bytes() {
        len += usize::from(b & 0xc0 != 0x80);
        if b >= 0xf0 {
            len += 1;
            has_non_bmp = true;
        }
    }
    (u32::try_from(len).unwrap_or(u32::MAX), has_non_bmp)
}
