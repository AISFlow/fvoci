use std::io::{self, Read, Write};

use crate::limits::Limits;
use crate::outcome::{EngineStatus, LimitKind};

pub fn write_frame<W: Write>(w: &mut W, payload: &[u8], max: u64) -> io::Result<()> {
    if payload.len() as u64 > max {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame exceeds bound",
        ));
    }
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame exceeds u32"))?;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

/// Read one length-prefixed frame. `Ok(None)` is clean EOF before a header.
pub fn read_frame<R: Read>(r: &mut R, max: u64) -> Result<Option<Vec<u8>>, FrameError> {
    let mut hdr = [0u8; 4];
    match r.read_exact(&mut hdr) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(FrameError::Io(err.to_string())),
    }
    let len = u32::from_le_bytes(hdr) as u64;
    if len > max {
        return Err(FrameError::TooLarge { len, max });
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)
        .map_err(|err| FrameError::Io(err.to_string()))?;
    Ok(Some(buf))
}

#[derive(Debug)]
pub enum FrameError {
    TooLarge { len: u64, max: u64 },
    Io(String),
}

impl FrameError {
    pub fn into_status(self, _limits: &Limits) -> EngineStatus {
        match self {
            Self::TooLarge { len, max } => EngineStatus::ResourceLimit {
                kind: LimitKind::Frame,
                detail: format!("frame {len} bytes exceeds {max}-byte limit"),
            },
            Self::Io(detail) => EngineStatus::WorkerFailure {
                reason: crate::outcome::WorkerFailureReason::Protocol,
                detail,
            },
        }
    }
}
