use crate::collab::wire::SyncStep;

const LIB0_MAX_SAFE_UINT: u64 = (1_u64 << 53) - 1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YSyncError {
    Truncated,
    InvalidVarUint,
    BinaryTooLong,
}

struct Cursor<'a> {
    input: &'a [u8],
    pos: usize,
    max_binary: usize,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a [u8], max_binary: usize) -> Self {
        Self {
            input,
            pos: 0,
            max_binary,
        }
    }

    fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.pos)
    }

    fn read_byte(&mut self) -> Result<u8, YSyncError> {
        if self.pos >= self.input.len() {
            return Err(YSyncError::Truncated);
        }
        let byte = self.input[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], YSyncError> {
        if self.remaining() < len {
            return Err(YSyncError::Truncated);
        }
        let slice = &self.input[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_var_uint(&mut self) -> Result<u64, YSyncError> {
        let mut result = 0u64;
        let mut shift = 0u32;
        for _ in 0..10 {
            let byte = self.read_byte()?;
            let bits = u64::from(byte & 0x7f);
            if shift >= 64 || bits > (u64::MAX >> shift) {
                return Err(YSyncError::InvalidVarUint);
            }
            result |= bits << shift;
            if byte & 0x80 == 0 {
                if result > LIB0_MAX_SAFE_UINT {
                    return Err(YSyncError::InvalidVarUint);
                }
                return Ok(result);
            }
            shift += 7;
        }
        Err(YSyncError::InvalidVarUint)
    }

    fn read_var_bytes(&mut self) -> Result<Vec<u8>, YSyncError> {
        let len = self.read_var_uint()?;
        let len = usize::try_from(len).map_err(|_| YSyncError::BinaryTooLong)?;
        if len > self.max_binary {
            return Err(YSyncError::BinaryTooLong);
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

/// Parse the y-protocol sync envelope inside a Hocuspocus Sync payload.
pub fn parse_sync_payload(
    y_protocol: &[u8],
    max_binary: usize,
) -> Result<(SyncStep, Vec<u8>), YSyncError> {
    let mut cursor = Cursor::new(y_protocol, max_binary);
    let step_value = cursor.read_var_uint()?;
    let step = match step_value {
        0 => SyncStep::Step1,
        1 => SyncStep::Step2,
        2 => SyncStep::Update,
        _ => return Err(YSyncError::InvalidVarUint),
    };
    let payload = cursor.read_var_bytes()?;
    if cursor.remaining() != 0 {
        return Err(YSyncError::Truncated);
    }
    Ok((step, payload))
}

pub fn encode_sync_payload(step: SyncStep, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 8);
    let step_value = match step {
        SyncStep::Step1 => 0,
        SyncStep::Step2 => 1,
        SyncStep::Update => 2,
    };
    write_var_uint(&mut out, step_value);
    write_var_bytes(&mut out, payload);
    out
}

/// Canonical empty Yjs updateV1 (`[0, 0]`) and byte-emptiness.
pub fn is_empty_update(payload: &[u8]) -> bool {
    payload.is_empty() || payload == [0, 0]
}
