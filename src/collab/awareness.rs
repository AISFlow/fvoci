use std::collections::BTreeMap;

const MAX_AWARENESS_BYTES: usize = 65_536;
const MAX_AWARENESS_CLIENTS: usize = 128;
const LIB0_MAX_SAFE_UINT: u64 = (1_u64 << 53) - 1;
const NULL_JSON: &[u8] = b"null";

const PRESENCE_COLORS: [&str; 8] = [
    "#b91c1c", "#c2410c", "#b45309", "#15803d", "#0f766e", "#1d4ed8", "#6d28d9", "#be185d",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AwarenessError {
    TooLarge,
    Truncated,
    TrailingBytes,
    InvalidVarUint,
    InvalidClientId,
    InvalidUtf8,
    InvalidJson,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AwarenessUpdate {
    pub client_id: u32,
    pub clock: u64,
    pub state: Option<Vec<u8>>,
}

struct Cursor<'a> {
    input: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a [u8]) -> Self {
        Self { input, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.pos)
    }

    fn read_byte(&mut self) -> Result<u8, AwarenessError> {
        if self.pos >= self.input.len() {
            return Err(AwarenessError::Truncated);
        }
        let byte = self.input[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], AwarenessError> {
        if self.remaining() < len {
            return Err(AwarenessError::Truncated);
        }
        let slice = &self.input[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_var_uint(&mut self) -> Result<u64, AwarenessError> {
        let mut result = 0u64;
        let mut shift = 0u32;
        for _ in 0..10 {
            let byte = self.read_byte()?;
            let bits = u64::from(byte & 0x7f);
            if shift >= 64 || bits > (u64::MAX >> shift) {
                return Err(AwarenessError::InvalidVarUint);
            }
            result |= bits << shift;
            if byte & 0x80 == 0 {
                if result > LIB0_MAX_SAFE_UINT {
                    return Err(AwarenessError::InvalidVarUint);
                }
                return Ok(result);
            }
            shift += 7;
        }
        Err(AwarenessError::InvalidVarUint)
    }

    fn read_var_string(&mut self) -> Result<&'a str, AwarenessError> {
        let len = self.read_var_uint()?;
        let len = usize::try_from(len).map_err(|_| AwarenessError::TooLarge)?;
        if len > MAX_AWARENESS_BYTES {
            return Err(AwarenessError::TooLarge);
        }
        let bytes = self.read_exact(len)?;
        std::str::from_utf8(bytes).map_err(|_| AwarenessError::InvalidUtf8)
    }
}

fn write_var_uint(out: &mut Vec<u8>, value: u64) {
    let mut remaining = value;
    loop {
        if remaining < 0x80 {
            out.push(remaining as u8);
            return;
        }
        out.push((remaining as u8 & 0x7f) | 0x80);
        remaining >>= 7;
    }
}

fn write_var_bytes(out: &mut Vec<u8>, value: &[u8]) {
    write_var_uint(out, value.len() as u64);
    out.extend_from_slice(value);
}

/// Parse a bounded y-protocol awareness frame: count, then clientID/clock/JSON.
pub fn decode_awareness(input: &[u8]) -> Result<Vec<AwarenessUpdate>, AwarenessError> {
    if input.len() > MAX_AWARENESS_BYTES {
        return Err(AwarenessError::TooLarge);
    }
    if input.is_empty() {
        return Err(AwarenessError::Truncated);
    }
    let mut cursor = Cursor::new(input);
    let count = cursor.read_var_uint()?;
    if count > MAX_AWARENESS_CLIENTS as u64 {
        return Err(AwarenessError::TooLarge);
    }
    let mut updates = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let client_id = cursor.read_var_uint()?;
        if client_id > u64::from(u32::MAX) {
            return Err(AwarenessError::InvalidClientId);
        }
        let clock = cursor.read_var_uint()?;
        let json = cursor.read_var_string()?;
        let value: serde_json::Value =
            serde_json::from_str(json).map_err(|_| AwarenessError::InvalidJson)?;
        let state = if value.is_null() {
            None
        } else {
            Some(json.as_bytes().to_vec())
        };
        updates.push(AwarenessUpdate {
            client_id: client_id as u32,
            clock,
            state,
        });
    }
    if cursor.remaining() != 0 {
        return Err(AwarenessError::TrailingBytes);
    }
    Ok(updates)
}

pub fn encode_awareness(updates: &[AwarenessUpdate]) -> Vec<u8> {
    let mut out = Vec::new();
    write_var_uint(&mut out, updates.len() as u64);
    for update in updates {
        write_var_uint(&mut out, u64::from(update.client_id));
        write_var_uint(&mut out, update.clock);
        match &update.state {
            Some(state) => write_var_bytes(&mut out, state),
            None => write_var_bytes(&mut out, NULL_JSON),
        }
    }
    out
}

fn json_id_pair(value: &serde_json::Value) -> Option<serde_json::Value> {
    let obj = value.as_object()?;
    let client = obj.get("client")?;
    let clock = obj.get("clock")?;
    if !client.is_number() || !clock.is_number() {
        return None;
    }
    let mut out = serde_json::Map::new();
    out.insert("client".into(), client.clone());
    out.insert("clock".into(), clock.clone());
    Some(serde_json::Value::Object(out))
}

/// Y.RelativePosition JSON: type/item are `{client,clock}|null`, tname string|null,
/// assoc number|null. Empty/all-null positions crash Tiptap's decoration path.
fn sanitize_relative_position(value: &serde_json::Value) -> Option<serde_json::Value> {
    let obj = value.as_object()?;
    let type_v = match obj.get("type") {
        None => None,
        Some(serde_json::Value::Null) => Some(serde_json::Value::Null),
        Some(v) => Some(json_id_pair(v)?),
    };
    let tname_v = match obj.get("tname") {
        None => None,
        Some(serde_json::Value::Null) => Some(serde_json::Value::Null),
        Some(serde_json::Value::String(s)) => Some(serde_json::Value::String(s.clone())),
        Some(_) => return None,
    };
    let item_v = match obj.get("item") {
        None => None,
        Some(serde_json::Value::Null) => Some(serde_json::Value::Null),
        Some(v) => Some(json_id_pair(v)?),
    };
    let assoc_v = match obj.get("assoc") {
        None => None,
        Some(serde_json::Value::Null) => Some(serde_json::Value::Null),
        Some(v) if v.is_number() => Some(v.clone()),
        Some(_) => return None,
    };
    let has_target = matches!(&type_v, Some(v) if !v.is_null())
        || matches!(&tname_v, Some(v) if !v.is_null())
        || matches!(&item_v, Some(v) if !v.is_null());
    if !has_target {
        return None;
    }
    let mut out = serde_json::Map::new();
    if let Some(v) = type_v {
        out.insert("type".into(), v);
    }
    if let Some(v) = tname_v {
        out.insert("tname".into(), v);
    }
    if let Some(v) = item_v {
        out.insert("item".into(), v);
    }
    if let Some(v) = assoc_v {
        out.insert("assoc".into(), v);
    }
    Some(serde_json::Value::Object(out))
}

fn sanitize_cursor(value: &serde_json::Value) -> Option<serde_json::Value> {
    let obj = value.as_object()?;
    let anchor = sanitize_relative_position(obj.get("anchor")?)?;
    let head = sanitize_relative_position(obj.get("head")?)?;
    let mut out = serde_json::Map::new();
    out.insert("anchor".into(), anchor);
    out.insert("head".into(), head);
    Some(serde_json::Value::Object(out))
}

fn sanitize_block(value: &serde_json::Value) -> Option<serde_json::Value> {
    let obj = value.as_object()?;
    let id = obj.get("id")?.as_str()?;
    if id.is_empty() {
        return None;
    }
    let mut out = serde_json::Map::new();
    out.insert("id".into(), serde_json::Value::String(id.to_string()));
    Some(serde_json::Value::Object(out))
}

/// Filter awareness state JSON to the FVOCI allowlist and inject verified user fields.
pub fn sanitize_user_state(
    raw: &[u8],
    user_id: &str,
    display_name: &str,
    color: &str,
) -> Option<Vec<u8>> {
    let value = serde_json::from_slice::<serde_json::Value>(raw).ok()?;
    let obj = value.as_object()?;
    let user = obj.get("user")?.as_object()?;
    let claimed_id = user.get("id")?.as_str()?;
    if claimed_id != user_id {
        return None;
    }
    let mut verified_user = serde_json::Map::new();
    verified_user.insert("id".into(), serde_json::Value::String(user_id.to_string()));
    verified_user.insert(
        "name".into(),
        serde_json::Value::String(display_name.to_string()),
    );
    verified_user.insert("color".into(), serde_json::Value::String(color.to_string()));
    let mut out = serde_json::Map::new();
    out.insert("user".into(), serde_json::Value::Object(verified_user));
    for (key, val) in obj {
        match key.as_str() {
            "user" => {}
            "cursor" if val.is_null() => {
                out.insert("cursor".into(), serde_json::Value::Null);
            }
            "cursor" => {
                if let Some(cursor) = sanitize_cursor(val) {
                    out.insert("cursor".into(), cursor);
                }
            }
            "block" if val.is_null() => {
                out.insert("block".into(), serde_json::Value::Null);
            }
            "block" => {
                if let Some(block) = sanitize_block(val) {
                    out.insert("block".into(), block);
                }
            }
            "title" if val.is_null() => {
                out.insert("title".into(), serde_json::Value::Null);
            }
            "title" if val.as_bool() == Some(true) => {
                out.insert("title".into(), serde_json::Value::Bool(true));
            }
            _ => {}
        }
    }
    serde_json::to_vec(&serde_json::Value::Object(out)).ok()
}

pub fn user_color(user_id: &uuid::Uuid) -> String {
    let simple = user_id.as_simple().to_string();
    let tail = &simple[simple.len().saturating_sub(6)..];
    let parsed = u32::from_str_radix(tail, 16).unwrap_or(0);
    let index = (parsed as usize) % PRESENCE_COLORS.len();
    PRESENCE_COLORS[index].to_string()
}

pub fn display_name(given: &str, family: Option<&str>) -> String {
    match family {
        Some(f) if !f.trim().is_empty() => format!("{} {}", given.trim(), f.trim()),
        _ => given.trim().to_string(),
    }
}

struct ClientRecord {
    clock: u64,
    generation: u64,
    state: Option<Vec<u8>>,
    seq: u64,
}

pub struct AwarenessRegistry {
    by_client: BTreeMap<u32, ClientRecord>,
    generation: u64,
    seq: u64,
}

impl Default for AwarenessRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl AwarenessRegistry {
    pub fn new() -> Self {
        Self {
            by_client: BTreeMap::new(),
            generation: 0,
            seq: 0,
        }
    }

    pub fn connection_generation(&mut self) -> u64 {
        self.generation = self.generation.saturating_add(1);
        self.generation
    }

    fn next_seq(&mut self) -> u64 {
        self.seq = self.seq.saturating_add(1);
        self.seq
    }

    fn evict_oldest_tombstone(&mut self) -> bool {
        let oldest = self
            .by_client
            .iter()
            .filter(|(_, record)| record.state.is_none())
            .min_by_key(|(_, record)| record.seq)
            .map(|(client_id, _)| *client_id);
        if let Some(client_id) = oldest {
            self.by_client.remove(&client_id);
            true
        } else {
            false
        }
    }

    fn fits_new_client(&mut self, client_id: u32) -> bool {
        if self.by_client.contains_key(&client_id) {
            return true;
        }
        while self.by_client.len() >= MAX_AWARENESS_CLIENTS {
            if !self.evict_oldest_tombstone() {
                return false;
            }
        }
        true
    }

    fn clock_applies(current: Option<&ClientRecord>, clock: u64, incoming_null: bool) -> bool {
        let curr_clock = current.map(|record| record.clock).unwrap_or(0);
        let has_state = current.is_some_and(|record| record.state.is_some());
        curr_clock < clock || (curr_clock == clock && incoming_null && has_state)
    }

    pub fn apply_connection_updates(
        &mut self,
        claimed_client_id: u32,
        updates: &[AwarenessUpdate],
        user_id: &str,
        display_name: &str,
        color: &str,
        conn_generation: u64,
    ) -> Option<Vec<u8>> {
        let mut changed = Vec::new();
        for update in updates {
            if update.client_id != claimed_client_id {
                continue;
            }
            let current = self.by_client.get(&update.client_id);
            if current.is_some_and(|record| record.generation > conn_generation) {
                continue;
            }
            let incoming_null = update.state.is_none();
            if !Self::clock_applies(current, update.clock, incoming_null) {
                continue;
            }
            let state = match &update.state {
                Some(raw) => sanitize_user_state(raw, user_id, display_name, color),
                None => None,
            };
            if update.state.is_some() && state.is_none() {
                continue;
            }
            if !self.fits_new_client(update.client_id) {
                continue;
            }
            let seq = self.next_seq();
            self.by_client.insert(
                update.client_id,
                ClientRecord {
                    clock: update.clock,
                    generation: conn_generation,
                    state: state.clone(),
                    seq,
                },
            );
            changed.push(AwarenessUpdate {
                client_id: update.client_id,
                clock: update.clock,
                state,
            });
        }
        if changed.is_empty() {
            None
        } else {
            Some(encode_awareness(&changed))
        }
    }

    pub fn remove_client(&mut self, client_id: u32, conn_generation: u64) -> Option<Vec<u8>> {
        let current = self.by_client.get(&client_id)?;
        if current.generation > conn_generation || current.state.is_none() {
            return None;
        }
        let clock = current.clock.saturating_add(1);
        let seq = self.next_seq();
        self.by_client.insert(
            client_id,
            ClientRecord {
                clock,
                generation: conn_generation,
                state: None,
                seq,
            },
        );
        Some(encode_awareness(&[AwarenessUpdate {
            client_id,
            clock,
            state: None,
        }]))
    }

    pub fn encode_all(&self) -> Vec<u8> {
        let updates = self
            .by_client
            .iter()
            .filter_map(|(client_id, record)| {
                record.state.as_ref().map(|state| AwarenessUpdate {
                    client_id: *client_id,
                    clock: record.clock,
                    state: Some(state.clone()),
                })
            })
            .collect::<Vec<_>>();
        if updates.is_empty() {
            Vec::new()
        } else {
            encode_awareness(&updates)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collab::wire::{DocumentMessage, WireFrame};

    const GOLDEN_INNER: &str = "01b96001297b226e616d65223a22ed858cec8aa4ed8ab8e29ca8222c22636f6c6f72223a2223666630306666227d";
    const JS_NULL: &str = "01b96002046e756c6c";
    const JS_TWO: &str = "020701137b2275736572223a7b226964223a2261227d7d0903137b2275736572223a7b226964223a2262227d7d";
    const JS_NESTED_OK: &str = "010701f9017b2275736572223a7b226964223a227531222c226e616d65223a226e222c22636f6c6f72223a2223623931633163227d2c22637572736f72223a7b22616e63686f72223a7b2274797065223a7b22636c69656e74223a312c22636c6f636b223a307d2c22746e616d65223a6e756c6c2c226974656d223a7b22636c69656e74223a312c22636c6f636b223a347d2c226173736f63223a307d2c2268656164223a7b22746e616d65223a2264656661756c74222c2274797065223a6e756c6c2c226974656d223a6e756c6c2c226173736f63223a2d317d7d2c22626c6f636b223a7b226964223a226231227d2c227469746c65223a747275657d";
    const JS_EMPTY_CURSOR: &str = "010701527b2275736572223a7b226964223a227531222c226e616d65223a226e222c22636f6c6f72223a2223623931633163227d2c22637572736f72223a7b22616e63686f72223a7b7d2c2268656164223a7b7d7d7d";
    const JS_ALLNULL_CURSOR: &str = "0107019a017b2275736572223a7b226964223a227531222c226e616d65223a226e222c22636f6c6f72223a2223623931633163227d2c22637572736f72223a7b22616e63686f72223a7b2274797065223a6e756c6c2c22746e616d65223a6e756c6c2c226974656d223a6e756c6c7d2c2268656164223a7b2274797065223a6e756c6c2c22746e616d65223a6e756c6c2c226974656d223a6e756c6c7d7d7d";

    fn hx(s: &str) -> Vec<u8> {
        hex::decode(s).expect("hex")
    }

    fn fixture_case_hex(id: &str) -> Vec<u8> {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../compat/fixtures/hocus-wire.json"))
                .expect("fixture json");
        let cases = json.get("cases").and_then(|v| v.as_array()).expect("cases");
        let hex = cases
            .iter()
            .find(|row| row.get("id").and_then(|v| v.as_str()) == Some(id))
            .and_then(|row| row.get("hex"))
            .and_then(|v| v.as_str())
            .unwrap_or_else(|| panic!("missing fixture {id}"));
        hx(hex)
    }

    fn inner_from_outer(frame: &[u8]) -> Vec<u8> {
        match crate::collab::wire::decode(frame).expect("outer wire") {
            WireFrame::Document {
                message: DocumentMessage::Awareness(payload),
                ..
            } => payload,
            other => panic!("expected awareness, got {other:?}"),
        }
    }

    fn parsed(bytes: &[u8]) -> serde_json::Value {
        serde_json::from_slice(bytes).expect("json")
    }

    fn live(
        client_id: u32,
        clock: u64,
        user_id: &str,
        extra: serde_json::Value,
    ) -> AwarenessUpdate {
        let mut obj = extra.as_object().cloned().unwrap_or_default();
        obj.insert(
            "user".into(),
            serde_json::json!({"id": user_id, "name": "forged", "color": "#000000"}),
        );
        AwarenessUpdate {
            client_id,
            clock,
            state: Some(serde_json::to_vec(&serde_json::Value::Object(obj)).unwrap()),
        }
    }

    #[test]
    fn golden_outer_wire_decodes_to_inner_y_protocol() {
        let inner = inner_from_outer(&fixture_case_hex("awareness_update"));
        assert_eq!(inner, hx(GOLDEN_INNER));
        let routed = inner_from_outer(&fixture_case_hex("awareness_session_routing_key"));
        assert_eq!(routed, inner);
        let updates = decode_awareness(&inner).expect("inner");
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].client_id, 12345);
        assert_eq!(updates[0].clock, 1);
        let state = parsed(updates[0].state.as_ref().expect("state"));
        assert_eq!(state["name"], "테스트✨");
        assert_eq!(state["color"], "#ff00ff");
        assert!(state.get("user").is_none());
        assert_eq!(encode_awareness(&updates), inner);
    }

    #[test]
    fn roundtrip_js_oracle_null_and_two_ids() {
        let null_update = decode_awareness(&hx(JS_NULL)).expect("null");
        assert_eq!(null_update.len(), 1);
        assert_eq!(null_update[0].client_id, 12345);
        assert_eq!(null_update[0].clock, 2);
        assert_eq!(null_update[0].state, None);
        assert_eq!(encode_awareness(&null_update), hx(JS_NULL));

        let two = decode_awareness(&hx(JS_TWO)).expect("two");
        assert_eq!(two.len(), 2);
        assert_eq!(two[0].client_id, 7);
        assert_eq!(two[1].client_id, 9);
        assert_eq!(parsed(two[0].state.as_ref().unwrap())["user"]["id"], "a");
        assert_eq!(encode_awareness(&two), hx(JS_TWO));
    }

    #[test]
    fn malformed_trailing_truncated_count_integer_utf8_json() {
        let inner = hx(GOLDEN_INNER);
        let mut trailing = inner.clone();
        trailing.push(0x00);
        assert_eq!(
            decode_awareness(&trailing),
            Err(AwarenessError::TrailingBytes)
        );
        assert_eq!(decode_awareness(&[]), Err(AwarenessError::Truncated));
        assert_eq!(decode_awareness(&[0x01]), Err(AwarenessError::Truncated));
        assert_eq!(decode_awareness(&[0x81]), Err(AwarenessError::Truncated));

        let mut over_count = Vec::new();
        write_var_uint(&mut over_count, (MAX_AWARENESS_CLIENTS as u64) + 1);
        assert_eq!(decode_awareness(&over_count), Err(AwarenessError::TooLarge));

        let mut huge = Vec::new();
        huge.resize(MAX_AWARENESS_BYTES + 1, 0);
        assert_eq!(decode_awareness(&huge), Err(AwarenessError::TooLarge));

        assert_eq!(
            decode_awareness(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x01]),
            Err(AwarenessError::InvalidVarUint)
        );

        let mut client_too_wide = Vec::new();
        write_var_uint(&mut client_too_wide, 1);
        write_var_uint(&mut client_too_wide, u64::from(u32::MAX) + 1);
        write_var_uint(&mut client_too_wide, 1);
        write_var_bytes(&mut client_too_wide, NULL_JSON);
        assert_eq!(
            decode_awareness(&client_too_wide),
            Err(AwarenessError::InvalidClientId)
        );

        let mut bad_utf8 = Vec::new();
        write_var_uint(&mut bad_utf8, 1);
        write_var_uint(&mut bad_utf8, 1);
        write_var_uint(&mut bad_utf8, 1);
        write_var_bytes(&mut bad_utf8, &[0xff, 0xfe]);
        assert_eq!(
            decode_awareness(&bad_utf8),
            Err(AwarenessError::InvalidUtf8)
        );

        let mut bad_json = Vec::new();
        write_var_uint(&mut bad_json, 1);
        write_var_uint(&mut bad_json, 1);
        write_var_uint(&mut bad_json, 1);
        write_var_bytes(&mut bad_json, b"{");
        assert_eq!(
            decode_awareness(&bad_json),
            Err(AwarenessError::InvalidJson)
        );

        assert_eq!(decode_awareness(&[0x00]).unwrap(), Vec::new());
    }

    #[test]
    fn sanitize_nested_user_cursor_block_title_and_stripping() {
        let nested = decode_awareness(&hx(JS_NESTED_OK)).unwrap();
        let cleaned =
            sanitize_user_state(nested[0].state.as_ref().unwrap(), "u1", "김연구", "#b91c1c")
                .expect("nested");
        let value = parsed(&cleaned);
        assert_eq!(value["user"]["id"], "u1");
        assert_eq!(value["user"]["name"], "김연구");
        assert_eq!(value["user"]["color"], "#b91c1c");
        assert!(value.get("id").is_none());
        assert!(value.get("name").is_none());
        assert!(value.get("color").is_none());
        assert_eq!(value["block"]["id"], "b1");
        assert_eq!(value["title"], true);
        assert_eq!(value["cursor"]["head"]["tname"], "default");
        assert_eq!(value["cursor"]["anchor"]["item"]["clock"], 4);

        let empty = decode_awareness(&hx(JS_EMPTY_CURSOR)).unwrap();
        let stripped = parsed(
            &sanitize_user_state(empty[0].state.as_ref().unwrap(), "u1", "n", "#b91c1c").unwrap(),
        );
        assert!(stripped.get("cursor").is_none());

        let allnull = decode_awareness(&hx(JS_ALLNULL_CURSOR)).unwrap();
        let stripped = parsed(
            &sanitize_user_state(allnull[0].state.as_ref().unwrap(), "u1", "n", "#b91c1c").unwrap(),
        );
        assert!(stripped.get("cursor").is_none());

        let raw = serde_json::to_vec(&serde_json::json!({
            "id": "top",
            "name": "leak",
            "color": "#ffffff",
            "user": {"id": "u1", "name": "forged", "color": "#000000", "role": "admin"},
            "block:extra": {"id": "nope"},
            "block": {"id": ""},
            "title": "secret",
            "cursor": null,
            "unknown": 1
        }))
        .unwrap();
        let cleaned = parsed(&sanitize_user_state(&raw, "u1", "n", "#1d4ed8").unwrap());
        assert_eq!(cleaned["user"]["name"], "n");
        assert_eq!(cleaned["user"]["color"], "#1d4ed8");
        assert!(cleaned["user"].get("role").is_none());
        assert!(cleaned.get("id").is_none());
        assert!(cleaned.get("block:extra").is_none());
        assert!(cleaned.get("block").is_none());
        assert!(cleaned.get("title").is_none());
        assert!(cleaned.get("unknown").is_none());
        assert!(cleaned["cursor"].is_null());

        let clearing = serde_json::to_vec(&serde_json::json!({
            "user": {"id": "u1", "name": "x", "color": "#000000"},
            "block": null,
            "title": null
        }))
        .unwrap();
        let cleaned = parsed(&sanitize_user_state(&clearing, "u1", "n", "#1d4ed8").unwrap());
        assert!(cleaned["block"].is_null());
        assert!(cleaned["title"].is_null());

        assert!(sanitize_user_state(&raw, "other", "n", "#1d4ed8").is_none());
        assert!(sanitize_user_state(b"{}", "u1", "n", "#1d4ed8").is_none());
        let golden_state = decode_awareness(&hx(GOLDEN_INNER)).unwrap();
        assert!(sanitize_user_state(
            golden_state[0].state.as_ref().unwrap(),
            "u1",
            "n",
            "#b91c1c"
        )
        .is_none());
    }

    #[test]
    fn presence_color_matches_shared_helper() {
        let id = uuid::Uuid::parse_str("01a01f00-0000-7000-8000-000000000001").unwrap();
        assert_eq!(user_color(&id), "#c2410c");
        let other = uuid::Uuid::parse_str("01a01f00-0000-7000-8000-00000000000b").unwrap();
        assert_eq!(user_color(&other), "#15803d");
        assert_ne!(user_color(&id), user_color(&other));
    }

    #[test]
    fn registry_two_ids_forgery_null_clocks_generation_and_old_close() {
        let mut reg = AwarenessRegistry::new();
        let g1 = reg.connection_generation();
        let g2 = reg.connection_generation();
        assert!(g1 >= 1);
        assert!(g2 > g1);

        let peer9 = live(9, 3, "peer", serde_json::json!({}));
        assert!(reg
            .apply_connection_updates(9, &[peer9.clone()], "peer", "B", "#c2410c", g1)
            .is_some());
        assert_eq!(decode_awareness(&reg.encode_all()).unwrap().len(), 1);

        let mixed = decode_awareness(&hx(JS_TWO)).unwrap();
        let encoded = reg
            .apply_connection_updates(7, &mixed, "a", "A", "#b91c1c", g2)
            .expect("claimed 7");
        let applied = decode_awareness(&encoded).unwrap();
        assert_eq!(applied.len(), 1);
        assert_eq!(applied[0].client_id, 7);
        let snapshot = decode_awareness(&reg.encode_all()).unwrap();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.iter().any(|u| u.client_id == 9));
        assert_eq!(
            parsed(
                snapshot
                    .iter()
                    .find(|u| u.client_id == 9)
                    .unwrap()
                    .state
                    .as_ref()
                    .unwrap()
            )["user"]["id"],
            "peer"
        );

        assert!(reg
            .apply_connection_updates(
                7,
                &[live(7, 2, "forged", serde_json::json!({}))],
                "a",
                "A",
                "#b91c1c",
                g2
            )
            .is_none());

        let stale = AwarenessUpdate {
            client_id: 7,
            clock: 0,
            state: live(7, 5, "a", serde_json::json!({})).state,
        };
        assert!(reg
            .apply_connection_updates(7, &[stale], "a", "A", "#b91c1c", g2)
            .is_none());

        let duplicate = live(7, 1, "a", serde_json::json!({"title": true}));
        assert!(reg
            .apply_connection_updates(7, &[duplicate], "a", "A", "#b91c1c", g2)
            .is_none());

        let stale_tombstone = AwarenessUpdate {
            client_id: 7,
            clock: 1,
            state: None,
        };
        let tomb = decode_awareness(
            &reg.apply_connection_updates(7, &[stale_tombstone], "a", "A", "#b91c1c", g2)
                .expect("same-clock null"),
        )
        .unwrap();
        assert_eq!(tomb[0].state, None);
        let live_ids: Vec<_> = decode_awareness(&reg.encode_all())
            .unwrap()
            .into_iter()
            .map(|u| u.client_id)
            .collect();
        assert!(!live_ids.contains(&7));
        assert!(live_ids.contains(&9));

        let g3 = reg.connection_generation();
        let takeover = live(9, 4, "peer", serde_json::json!({"title": true}));
        assert!(reg
            .apply_connection_updates(9, &[takeover], "peer", "B", "#c2410c", g3)
            .is_some());
        assert!(reg.remove_client(9, g1).is_none());
        let snapshot = decode_awareness(&reg.encode_all()).unwrap();
        assert!(snapshot
            .iter()
            .any(|u| u.client_id == 9 && u.state.is_some()));

        let old_update = live(9, 99, "peer", serde_json::json!({}));
        assert!(reg
            .apply_connection_updates(9, &[old_update], "peer", "B", "#c2410c", g1)
            .is_none());

        let removal = decode_awareness(&reg.remove_client(9, g3).expect("tombstone")).unwrap();
        assert_eq!(removal.len(), 1);
        assert_eq!(removal[0].client_id, 9);
        assert_eq!(removal[0].state, None);
        assert_eq!(removal[0].clock, 5);
        assert!(!decode_awareness(&reg.encode_all())
            .unwrap_or_default()
            .iter()
            .any(|u| u.client_id == 9 && u.state.is_some()));
        assert!(reg.remove_client(9, g3).is_none());
    }

    #[test]
    fn registry_tombstones_stay_bounded() {
        let mut reg = AwarenessRegistry::new();
        let gen = reg.connection_generation();
        for i in 0..(MAX_AWARENESS_CLIENTS as u32 * 2) {
            let update = live(i, 1, "u", serde_json::json!({}));
            let _ = reg.apply_connection_updates(i, &[update], "u", "n", "#b91c1c", gen);
            let _ = reg.remove_client(i, gen);
        }
        assert!(reg.by_client.len() <= MAX_AWARENESS_CLIENTS);
        assert!(reg.encode_all().is_empty());
    }

    #[test]
    fn display_name_joins_family() {
        assert_eq!(display_name("김", Some("연구")), "김 연구");
        assert_eq!(display_name("김", Some("  ")), "김");
        assert_eq!(display_name(" 김 ", None), "김");
    }
}
