use std::collections::HashMap;

const MAX_AWARENESS_BYTES: usize = 65_536;
const MAX_AWARENESS_CLIENTS: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AwarenessError {
    TooLarge,
    Truncated,
    InvalidVarUint,
    InvalidClientId,
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
                return Ok(result);
            }
            shift += 7;
        }
        Err(AwarenessError::InvalidVarUint)
    }

    fn read_var_bytes(&mut self) -> Result<Vec<u8>, AwarenessError> {
        let len = self.read_var_uint()?;
        let len = usize::try_from(len).map_err(|_| AwarenessError::TooLarge)?;
        if len > MAX_AWARENESS_BYTES {
            return Err(AwarenessError::TooLarge);
        }
        Ok(self.read_exact(len)?.to_vec())
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

/// Parse a bounded y-protocol awareness frame.
pub fn decode_awareness(input: &[u8]) -> Result<Vec<AwarenessUpdate>, AwarenessError> {
    if input.len() > MAX_AWARENESS_BYTES {
        return Err(AwarenessError::TooLarge);
    }
    let mut cursor = Cursor::new(input);
    let mut updates = Vec::new();
    while cursor.remaining() > 0 {
        if updates.len() >= MAX_AWARENESS_CLIENTS {
            return Err(AwarenessError::TooLarge);
        }
        let len = cursor.read_var_uint()?;
        let len = usize::try_from(len).map_err(|_| AwarenessError::TooLarge)?;
        if len > MAX_AWARENESS_BYTES {
            return Err(AwarenessError::TooLarge);
        }
        let chunk = cursor.read_exact(len)?;
        let mut inner = Cursor::new(chunk);
        let client_id = inner.read_var_uint()?;
        if client_id > u32::MAX as u64 {
            return Err(AwarenessError::InvalidClientId);
        }
        let clock = inner.read_var_uint()?;
        let state = if inner.remaining() > 0 {
            Some(inner.read_var_bytes()?)
        } else {
            None
        };
        if inner.remaining() != 0 {
            return Err(AwarenessError::Truncated);
        }
        updates.push(AwarenessUpdate {
            client_id: client_id as u32,
            clock,
            state,
        });
    }
    Ok(updates)
}

pub fn encode_awareness(updates: &[AwarenessUpdate]) -> Vec<u8> {
    let mut out = Vec::new();
    for update in updates {
        let mut inner = Vec::new();
        write_var_uint(&mut inner, update.client_id as u64);
        write_var_uint(&mut inner, update.clock);
        if let Some(state) = &update.state {
            write_var_bytes(&mut inner, state);
        }
        write_var_uint(&mut out, inner.len() as u64);
        out.extend_from_slice(&inner);
    }
    out
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
    let mut out = serde_json::Map::new();
    out.insert("id".into(), serde_json::Value::String(user_id.to_string()));
    out.insert(
        "name".into(),
        serde_json::Value::String(display_name.to_string()),
    );
    out.insert("color".into(), serde_json::Value::String(color.to_string()));
    if let Some(cursor) = obj.get("cursor") {
        out.insert("cursor".into(), cursor.clone());
    }
    for (key, val) in obj {
        if key.starts_with("block:") {
            out.insert(key.clone(), val.clone());
        }
        if key == "title" && val.as_bool() == Some(true) {
            out.insert("title".into(), serde_json::Value::Bool(true));
        }
    }
    serde_json::to_vec(&serde_json::Value::Object(out)).ok()
}

pub fn user_color(user_id: &uuid::Uuid) -> String {
    let bytes = user_id.as_bytes();
    let hue = ((bytes[0] as u16) << 8 | bytes[1] as u16) % 360;
    format!("hsl({hue}, 70%, 45%)")
}

pub fn display_name(given: &str, family: Option<&str>) -> String {
    match family {
        Some(f) if !f.trim().is_empty() => format!("{} {}", given.trim(), f.trim()),
        _ => given.trim().to_string(),
    }
}

pub struct AwarenessRegistry {
    by_client: HashMap<u32, (u64, Vec<u8>, u64)>,
    generation: u64,
}

impl AwarenessRegistry {
    pub fn new() -> Self {
        Self {
            by_client: HashMap::new(),
            generation: 0,
        }
    }

    pub fn connection_generation(&self) -> u64 {
        self.generation
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
        let mut changed = false;
        for update in updates {
            if update.client_id != claimed_client_id {
                continue;
            }
            let state = match &update.state {
                Some(raw) => sanitize_user_state(raw, user_id, display_name, color),
                None => None,
            };
            match state {
                Some(bytes) => {
                    self.by_client
                        .insert(update.client_id, (update.clock, bytes, conn_generation));
                    changed = true;
                }
                None if update.state.is_none() => {
                    if let Some((_, _, gen)) = self.by_client.get(&update.client_id) {
                        if *gen <= conn_generation {
                            self.by_client.remove(&update.client_id);
                            changed = true;
                        }
                    }
                }
                None => {}
            }
        }
        if changed {
            Some(self.encode_all())
        } else {
            None
        }
    }

    pub fn remove_client(&mut self, client_id: u32, conn_generation: u64) -> Option<Vec<u8>> {
        if let Some((_, _, gen)) = self.by_client.get(&client_id) {
            if *gen <= conn_generation {
                self.by_client.remove(&client_id);
                return Some(self.encode_all());
            }
        }
        None
    }

    pub fn encode_all(&self) -> Vec<u8> {
        let updates = self
            .by_client
            .iter()
            .map(|(client_id, (clock, state, _))| AwarenessUpdate {
                client_id: *client_id,
                clock: *clock,
                state: Some(state.clone()),
            })
            .collect::<Vec<_>>();
        encode_awareness(&updates)
    }
}
