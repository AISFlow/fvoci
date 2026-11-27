//! The frames on the pipes between the server and its worker: a length of four bytes,
//! the lowest first, and that many bytes.
//!
//! A length is held to the largest frame before anything is made for it: neither side
//! takes the other's word for how much to make room for.

use std::io::{self, Read, Write};

use thiserror::Error;

/// The bytes of the length before every frame.
pub const HEADER_BYTES: usize = 4;

/// Why a frame cannot be read or written.
#[derive(Debug, Error)]
pub enum FrameError {
    /// The length is past the largest frame.
    #[error("a frame of {length} bytes is larger than the {limit} allowed")]
    TooLarge {
        /// How many bytes the frame holds, as its head says or as it would be written.
        length: u64,
        /// The largest frame.
        limit: u64,
    },
    /// The pipe ended within a frame.
    #[error("the pipe ended within a frame")]
    Cut,
    /// Reading or writing failed.
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// The header of a frame of `length` bytes.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] past `limit`, or past what four bytes count.
pub fn header(length: usize, limit: u64) -> Result<[u8; HEADER_BYTES], FrameError> {
    let length = u64::try_from(length).unwrap_or(u64::MAX);
    let too_large = || FrameError::TooLarge { length, limit };
    if length > limit {
        return Err(too_large());
    }
    let length = u32::try_from(length).map_err(|_| too_large())?;
    Ok(length.to_le_bytes())
}

/// The length a header says, held to `limit`.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] past `limit`.
pub fn length_of(header: [u8; HEADER_BYTES], limit: u64) -> Result<usize, FrameError> {
    let length = u32::from_le_bytes(header);
    let too_large = || FrameError::TooLarge {
        length: u64::from(length),
        limit,
    };
    if u64::from(length) > limit {
        return Err(too_large());
    }
    usize::try_from(length).map_err(|_| too_large())
}

/// Reads a frame; `None` if the pipe ended before one began.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] for a length past `limit`, [`FrameError::Cut`] if
/// the pipe ended within the frame, and [`FrameError::Io`] if reading failed.
pub fn read_frame(input: &mut impl Read, limit: u64) -> Result<Option<Vec<u8>>, FrameError> {
    let mut head = [0; HEADER_BYTES];
    let mut filled = 0;
    while filled < HEADER_BYTES {
        match input.read(&mut head[filled..]) {
            Ok(0) if filled == 0 => return Ok(None),
            Ok(0) => return Err(FrameError::Cut),
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error.into()),
        }
    }
    let length = length_of(head, limit)?;
    let mut payload = vec![0; length];
    input.read_exact(&mut payload).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            FrameError::Cut
        } else {
            FrameError::Io(error)
        }
    })?;
    Ok(Some(payload))
}

/// Writes a frame and flushes it.
///
/// # Errors
///
/// Returns [`FrameError::TooLarge`] for a payload past `limit`, and [`FrameError::Io`]
/// if writing failed.
pub fn write_frame(output: &mut impl Write, payload: &[u8], limit: u64) -> Result<(), FrameError> {
    output.write_all(&header(payload.len(), limit)?)?;
    output.write_all(payload)?;
    output.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_reads_back_as_it_was_written() {
        let mut pipe = Vec::new();
        write_frame(&mut pipe, b"one", 16).expect("written");
        write_frame(&mut pipe, b"", 16).expect("written");
        let mut input = pipe.as_slice();
        assert_eq!(
            read_frame(&mut input, 16).expect("read"),
            Some(b"one".to_vec())
        );
        assert_eq!(read_frame(&mut input, 16).expect("read"), Some(Vec::new()));
        assert_eq!(read_frame(&mut input, 16).expect("the end"), None);
    }

    #[test]
    fn a_length_past_the_limit_is_refused_before_room_is_made_for_it() {
        let mut input: &[u8] = &u32::MAX.to_le_bytes();
        assert!(matches!(
            read_frame(&mut input, 1024),
            Err(FrameError::TooLarge { length, limit: 1024 }) if length == u64::from(u32::MAX)
        ));
        assert!(write_frame(&mut Vec::new(), &[0; 17], 16).is_err());
    }

    #[test]
    fn a_frame_cut_short_is_told_from_a_pipe_that_ended_between_frames() {
        let mut half_header: &[u8] = &[3, 0];
        assert!(matches!(
            read_frame(&mut half_header, 16),
            Err(FrameError::Cut)
        ));
        let mut half_payload: &[u8] = &[3, 0, 0, 0, b'o'];
        assert!(matches!(
            read_frame(&mut half_payload, 16),
            Err(FrameError::Cut)
        ));
    }
}
