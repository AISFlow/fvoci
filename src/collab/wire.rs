//! Bounded Hocuspocus 4.6.0 provider/server WebSocket frame codec.
//!
//! Framing matches `@hocuspocus/provider@4.6.0` and `@hocuspocus/server@4.6.0`:
//! - Connection-level `Ping` is a single `[9]` byte with no document prefix.
//! - Connection-level `Pong` is `writeVarUint(10)` (one byte `0x0a`).
//! - Document frames are `varString(routingKey) + varUint(type) + payload`.
//!
//! Inner sync/awareness bytes are y-protocol payloads kept opaque for Yrs.

use std::fmt;

use uuid::Uuid;

/// Hard limits for decode/encode. Product ACL/session policy is out of scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub max_frame_bytes: usize,
    pub max_routing_key_bytes: usize,
    pub max_string_bytes: usize,
    pub max_binary_payload_bytes: usize,
}

impl Limits {
    // Source collab-http.ts: default 1 MiB body budget × STATE_OVERSIZE_FACTOR 8.
    // The frame cap includes routing/envelope bytes; inner payload also has a cap.
    pub const DEFAULT: Self = Self {
        max_frame_bytes: 8 * 1_048_576,
        max_routing_key_bytes: 512,
        max_string_bytes: 65_536,
        max_binary_payload_bytes: 8 * 1_048_576,
    };
}

/// lib0 `readVarUint` throws above `Number.MAX_SAFE_INTEGER`.
const LIB0_MAX_SAFE_UINT: u64 = (1_u64 << 53) - 1;

/// Provider 4.6.0 multiplex message type after the routing key.
///
/// Installed `@hocuspocus/provider@4.6.0` `MessageType` has no opcode 4 or 6.
/// Connection-level Ping/Pong are not document-prefixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MessageType {
    Sync = 0,
    Awareness = 1,
    Auth = 2,
    QueryAwareness = 3,
    Stateless = 5,
    Close = 7,
    SyncStatus = 8,
    Ping = 9,
    Pong = 10,
}

impl MessageType {
    pub fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Sync),
            1 => Some(Self::Awareness),
            2 => Some(Self::Auth),
            3 => Some(Self::QueryAwareness),
            5 => Some(Self::Stateless),
            7 => Some(Self::Close),
            8 => Some(Self::SyncStatus),
            9 => Some(Self::Ping),
            10 => Some(Self::Pong),
            _ => None,
        }
    }

    fn as_u64(self) -> u64 {
        self as u64
    }
}

/// Auth sub-message inside `MessageType::Auth`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AuthMessageType {
    Token = 0,
    PermissionDenied = 1,
    Authenticated = 2,
}

impl AuthMessageType {
    fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Token),
            1 => Some(Self::PermissionDenied),
            2 => Some(Self::Authenticated),
            _ => None,
        }
    }

    fn as_u64(self) -> u64 {
        self as u64
    }
}

/// y-protocols/sync step discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncStep {
    Step1 = 0,
    Step2 = 1,
    Update = 2,
}

impl SyncStep {
    fn from_u64(value: u64) -> Option<Self> {
        match value {
            0 => Some(Self::Step1),
            1 => Some(Self::Step2),
            2 => Some(Self::Update),
            _ => None,
        }
    }
}

/// Parsed FVOCI room name (`workspaceId:document|task:id`). No auto-creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CollabRoomName {
    pub workspace_id: Uuid,
    pub kind: CollabKind,
    pub resource_id: Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CollabKind {
    Document,
    Task,
}

/// Source `z.uuid()` accepts hyphenated 8-4-4-4-12 form, not urn/simple/braced.
fn parse_source_uuid(value: &str) -> Option<Uuid> {
    if value.len() != 36 {
        return None;
    }
    Uuid::try_parse(value).ok()
}

impl CollabRoomName {
    /// Parse `workspaceId:document|task:id` exactly like source `parseCollabName`.
    ///
    /// Call this on the document-name half of a routing key (`parseRoutingKey`).
    pub fn parse(name: &str) -> Option<Self> {
        let mut parts = name.split(':');
        let workspace_id = parts.next()?;
        let kind = parts.next()?;
        let resource_id = parts.next()?;
        if parts.next().is_some() {
            return None;
        }
        if workspace_id.is_empty() || kind.is_empty() || resource_id.is_empty() {
            return None;
        }
        let kind = match kind {
            "document" => CollabKind::Document,
            "task" => CollabKind::Task,
            _ => return None,
        };
        let workspace_id = parse_source_uuid(workspace_id)?;
        let resource_id = parse_source_uuid(resource_id)?;
        Some(Self {
            workspace_id,
            kind,
            resource_id,
        })
    }

    pub fn routing_key(&self) -> String {
        let kind = match self.kind {
            CollabKind::Document => "document",
            CollabKind::Task => "task",
        };
        format!("{}:{}:{}", self.workspace_id, kind, self.resource_id)
    }
}

/// Top-level decoded frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireFrame {
    Connection(ConnectionMessage),
    Document {
        routing_key: String,
        room: Option<CollabRoomName>,
        message: DocumentMessage,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionMessage {
    Ping,
    Pong,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DocumentMessage {
    Sync(SyncMessage),
    Awareness(Vec<u8>),
    Auth(AuthMessage),
    QueryAwareness,
    Stateless(String),
    Close { reason: Option<String> },
    SyncStatus { applied: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncMessage {
    pub step: SyncStep,
    /// Full y-protocol payload after the Hocuspocus type byte, including the
    /// leading sync-step varUint and trailing bytes.
    pub y_protocol: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthMessage {
    /// Client -> server token claim (awareness clientID decimal, not session JWT).
    Token {
        token: String,
        provider_version: Option<String>,
    },
    /// Server -> client token re-sync request (no strings).
    TokenRequest,
    PermissionDenied {
        reason: String,
    },
    Authenticated {
        scope: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    EmptyFrame,
    FrameTooLarge { size: usize, max: usize },
    Truncated,
    Utf8,
    RoutingKeyTooLong { size: usize, max: usize },
    StringTooLong { size: usize, max: usize },
    BinaryTooLong { size: usize, max: usize },
    UnknownMessageType(u64),
    UnknownAuthMessageType(u64),
    UnknownSyncStep(u64),
    InvalidVarUint,
    DocumentPingNotAllowed,
    DocumentPongNotAllowed,
}

impl fmt::Display for WireError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFrame => f.write_str("empty frame"),
            Self::FrameTooLarge { size, max } => {
                write!(f, "frame size {size} exceeds max {max}")
            }
            Self::Truncated => f.write_str("truncated frame"),
            Self::Utf8 => f.write_str("invalid utf-8"),
            Self::RoutingKeyTooLong { size, max } => {
                write!(f, "routing key length {size} exceeds max {max}")
            }
            Self::StringTooLong { size, max } => {
                write!(f, "string length {size} exceeds max {max}")
            }
            Self::BinaryTooLong { size, max } => {
                write!(f, "binary length {size} exceeds max {max}")
            }
            Self::UnknownMessageType(value) => write!(f, "unknown message type {value}"),
            Self::UnknownAuthMessageType(value) => write!(f, "unknown auth message type {value}"),
            Self::UnknownSyncStep(value) => write!(f, "unknown sync step {value}"),
            Self::InvalidVarUint => f.write_str("invalid varuint"),
            Self::DocumentPingNotAllowed => {
                f.write_str("ping must be connection-level (single byte)")
            }
            Self::DocumentPongNotAllowed => {
                f.write_str("pong must be connection-level (writeVarUint 10)")
            }
        }
    }
}

impl std::error::Error for WireError {}

struct Cursor<'a> {
    input: &'a [u8],
    pos: usize,
    limits: Limits,
}

impl<'a> Cursor<'a> {
    fn new(input: &'a [u8], limits: Limits) -> Self {
        Self {
            input,
            pos: 0,
            limits,
        }
    }

    fn remaining(&self) -> usize {
        self.input.len().saturating_sub(self.pos)
    }

    fn read_byte(&mut self) -> Result<u8, WireError> {
        if self.pos >= self.input.len() {
            return Err(WireError::Truncated);
        }
        let byte = self.input[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    fn read_exact(&mut self, len: usize) -> Result<&'a [u8], WireError> {
        if self.remaining() < len {
            return Err(WireError::Truncated);
        }
        let slice = &self.input[self.pos..self.pos + len];
        self.pos += len;
        Ok(slice)
    }

    fn read_var_uint(&mut self) -> Result<u64, WireError> {
        let mut result = 0u64;
        let mut shift = 0u32;
        for _ in 0..10 {
            let byte = self.read_byte()?;
            let bits = u64::from(byte & 0x7f);
            if shift >= 64 || bits > (u64::MAX >> shift) {
                return Err(WireError::InvalidVarUint);
            }
            result |= bits << shift;
            if byte & 0x80 == 0 {
                if result > LIB0_MAX_SAFE_UINT {
                    return Err(WireError::InvalidVarUint);
                }
                return Ok(result);
            }
            shift += 7;
        }
        Err(WireError::InvalidVarUint)
    }

    fn bounded_len(len: u64, max: usize) -> Result<usize, usize> {
        match usize::try_from(len) {
            Ok(size) if size <= max => Ok(size),
            Ok(size) => Err(size),
            Err(_) => Err(usize::MAX),
        }
    }

    fn read_var_string(&mut self) -> Result<String, WireError> {
        let len = self.read_var_uint()?;
        let len = Self::bounded_len(len, self.limits.max_string_bytes).map_err(|size| {
            WireError::StringTooLong {
                size,
                max: self.limits.max_string_bytes,
            }
        })?;
        let bytes = self.read_exact(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| WireError::Utf8)
    }

    fn read_var_bytes(&mut self) -> Result<Vec<u8>, WireError> {
        let len = self.read_var_uint()?;
        let len = Self::bounded_len(len, self.limits.max_binary_payload_bytes).map_err(|size| {
            WireError::BinaryTooLong {
                size,
                max: self.limits.max_binary_payload_bytes,
            }
        })?;
        Ok(self.read_exact(len)?.to_vec())
    }
}

struct Encoder {
    out: Vec<u8>,
    limits: Limits,
}

impl Encoder {
    fn new(limits: Limits) -> Self {
        Self {
            out: Vec::new(),
            limits,
        }
    }

    fn ensure_frame_capacity(&self, additional: usize) -> Result<(), WireError> {
        if self.out.len() + additional > self.limits.max_frame_bytes {
            return Err(WireError::FrameTooLarge {
                size: self.out.len() + additional,
                max: self.limits.max_frame_bytes,
            });
        }
        Ok(())
    }

    fn write_var_uint(&mut self, value: u64) -> Result<(), WireError> {
        let mut remaining = value;
        loop {
            self.ensure_frame_capacity(1)?;
            if remaining < 0x80 {
                self.out.push(remaining as u8);
                return Ok(());
            }
            self.out.push((remaining as u8 & 0x7f) | 0x80);
            remaining >>= 7;
        }
    }

    fn write_var_string(&mut self, value: &str) -> Result<(), WireError> {
        let bytes = value.as_bytes();
        if bytes.len() > self.limits.max_string_bytes {
            return Err(WireError::StringTooLong {
                size: bytes.len(),
                max: self.limits.max_string_bytes,
            });
        }
        self.write_var_uint(bytes.len() as u64)?;
        self.ensure_frame_capacity(bytes.len())?;
        self.out.extend_from_slice(bytes);
        Ok(())
    }

    fn write_var_bytes(&mut self, value: &[u8]) -> Result<(), WireError> {
        if value.len() > self.limits.max_binary_payload_bytes {
            return Err(WireError::BinaryTooLong {
                size: value.len(),
                max: self.limits.max_binary_payload_bytes,
            });
        }
        self.write_var_uint(value.len() as u64)?;
        self.ensure_frame_capacity(value.len())?;
        self.out.extend_from_slice(value);
        Ok(())
    }

    fn write_raw(&mut self, value: &[u8]) -> Result<(), WireError> {
        self.ensure_frame_capacity(value.len())?;
        self.out.extend_from_slice(value);
        Ok(())
    }

    fn finish(self) -> Vec<u8> {
        self.out
    }
}

/// `@hocuspocus/common` `parseRoutingKey`: documentName is before the first NUL.
fn routing_key_base(key: &str) -> &str {
    match key.split_once('\0') {
        Some((document_name, _)) => document_name,
        None => key,
    }
}

fn decode_sync_payload(cursor: &mut Cursor<'_>) -> Result<SyncMessage, WireError> {
    let start = cursor.pos;
    let step_value = cursor.read_var_uint()?;
    let step = SyncStep::from_u64(step_value).ok_or(WireError::UnknownSyncStep(step_value))?;
    let _payload = cursor.read_var_bytes()?;
    Ok(SyncMessage {
        step,
        y_protocol: cursor.input[start..cursor.pos].to_vec(),
    })
}

fn decode_auth(cursor: &mut Cursor<'_>) -> Result<AuthMessage, WireError> {
    let auth_value = cursor.read_var_uint()?;
    let auth_type = AuthMessageType::from_u64(auth_value)
        .ok_or(WireError::UnknownAuthMessageType(auth_value))?;
    match auth_type {
        AuthMessageType::Token => {
            if cursor.remaining() == 0 {
                return Ok(AuthMessage::TokenRequest);
            }
            let token = cursor.read_var_string()?;
            let provider_version = if cursor.remaining() > 0 {
                Some(cursor.read_var_string()?)
            } else {
                None
            };
            if cursor.remaining() > 0 {
                return Err(WireError::Truncated);
            }
            Ok(AuthMessage::Token {
                token,
                provider_version,
            })
        }
        AuthMessageType::PermissionDenied => {
            let reason = cursor.read_var_string()?;
            if cursor.remaining() > 0 {
                return Err(WireError::Truncated);
            }
            Ok(AuthMessage::PermissionDenied { reason })
        }
        AuthMessageType::Authenticated => {
            let scope = cursor.read_var_string()?;
            if cursor.remaining() > 0 {
                return Err(WireError::Truncated);
            }
            Ok(AuthMessage::Authenticated { scope })
        }
    }
}

fn encode_auth(message: &AuthMessage, encoder: &mut Encoder) -> Result<(), WireError> {
    match message {
        AuthMessage::Token {
            token,
            provider_version,
        } => {
            encoder.write_var_uint(AuthMessageType::Token.as_u64())?;
            encoder.write_var_string(token)?;
            if let Some(version) = provider_version {
                encoder.write_var_string(version)?;
            }
        }
        AuthMessage::TokenRequest => {
            encoder.write_var_uint(AuthMessageType::Token.as_u64())?;
        }
        AuthMessage::PermissionDenied { reason } => {
            encoder.write_var_uint(AuthMessageType::PermissionDenied.as_u64())?;
            encoder.write_var_string(reason)?;
        }
        AuthMessage::Authenticated { scope } => {
            encoder.write_var_uint(AuthMessageType::Authenticated.as_u64())?;
            encoder.write_var_string(scope)?;
        }
    }
    Ok(())
}

/// Decode one Hocuspocus wire frame with default limits.
pub fn decode(input: &[u8]) -> Result<WireFrame, WireError> {
    decode_with_limits(input, Limits::DEFAULT)
}

/// Encode one Hocuspocus wire frame with default limits.
pub fn encode(frame: &WireFrame) -> Result<Vec<u8>, WireError> {
    encode_with_limits(frame, Limits::DEFAULT)
}

pub fn decode_with_limits(input: &[u8], limits: Limits) -> Result<WireFrame, WireError> {
    if input.is_empty() {
        return Err(WireError::EmptyFrame);
    }
    if input.len() > limits.max_frame_bytes {
        return Err(WireError::FrameTooLarge {
            size: input.len(),
            max: limits.max_frame_bytes,
        });
    }

    if input.len() == 1 && input[0] == MessageType::Ping as u8 {
        return Ok(WireFrame::Connection(ConnectionMessage::Ping));
    }

    if input.len() == 1 && input[0] == MessageType::Pong as u8 {
        return Ok(WireFrame::Connection(ConnectionMessage::Pong));
    }

    let mut cursor = Cursor::new(input, limits);
    let routing_key = cursor.read_var_string()?;
    if routing_key.len() > limits.max_routing_key_bytes {
        return Err(WireError::RoutingKeyTooLong {
            size: routing_key.len(),
            max: limits.max_routing_key_bytes,
        });
    }

    let type_value = cursor.read_var_uint()?;
    let message_type =
        MessageType::from_u64(type_value).ok_or(WireError::UnknownMessageType(type_value))?;

    let base = routing_key_base(&routing_key);
    let room = CollabRoomName::parse(base);

    let message = match message_type {
        MessageType::Ping => return Err(WireError::DocumentPingNotAllowed),
        MessageType::Pong => return Err(WireError::DocumentPongNotAllowed),
        MessageType::Sync => DocumentMessage::Sync(decode_sync_payload(&mut cursor)?),
        MessageType::Awareness => DocumentMessage::Awareness(cursor.read_var_bytes()?),
        MessageType::Auth => DocumentMessage::Auth(decode_auth(&mut cursor)?),
        MessageType::QueryAwareness => {
            if cursor.remaining() != 0 {
                return Err(WireError::Truncated);
            }
            DocumentMessage::QueryAwareness
        }
        MessageType::Stateless => DocumentMessage::Stateless(cursor.read_var_string()?),
        MessageType::Close => {
            let reason = if cursor.remaining() > 0 {
                Some(cursor.read_var_string()?)
            } else {
                None
            };
            if cursor.remaining() > 0 {
                return Err(WireError::Truncated);
            }
            DocumentMessage::Close { reason }
        }
        MessageType::SyncStatus => {
            let applied = cursor.read_var_uint()? == 1;
            if cursor.remaining() != 0 {
                return Err(WireError::Truncated);
            }
            DocumentMessage::SyncStatus { applied }
        }
    };

    if cursor.remaining() != 0 {
        return Err(WireError::Truncated);
    }

    Ok(WireFrame::Document {
        routing_key,
        room,
        message,
    })
}

pub fn encode_with_limits(frame: &WireFrame, limits: Limits) -> Result<Vec<u8>, WireError> {
    let mut encoder = Encoder::new(limits);
    match frame {
        WireFrame::Connection(ConnectionMessage::Ping) => {
            encoder.ensure_frame_capacity(1)?;
            encoder.out.push(MessageType::Ping as u8);
        }
        WireFrame::Connection(ConnectionMessage::Pong) => {
            encoder.write_var_uint(MessageType::Pong as u64)?;
        }
        WireFrame::Document {
            routing_key,
            message,
            ..
        } => {
            if routing_key.len() > limits.max_routing_key_bytes {
                return Err(WireError::RoutingKeyTooLong {
                    size: routing_key.len(),
                    max: limits.max_routing_key_bytes,
                });
            }
            encoder.write_var_string(routing_key)?;
            match message {
                DocumentMessage::Sync(sync) => {
                    encoder.write_var_uint(MessageType::Sync.as_u64())?;
                    encoder.write_raw(&sync.y_protocol)?;
                }
                DocumentMessage::Awareness(payload) => {
                    encoder.write_var_uint(MessageType::Awareness.as_u64())?;
                    encoder.write_var_bytes(payload)?;
                }
                DocumentMessage::Auth(auth) => {
                    encoder.write_var_uint(MessageType::Auth.as_u64())?;
                    encode_auth(auth, &mut encoder)?;
                }
                DocumentMessage::QueryAwareness => {
                    encoder.write_var_uint(MessageType::QueryAwareness.as_u64())?;
                }
                DocumentMessage::Stateless(payload) => {
                    encoder.write_var_uint(MessageType::Stateless.as_u64())?;
                    encoder.write_var_string(payload)?;
                }
                DocumentMessage::Close { reason } => {
                    encoder.write_var_uint(MessageType::Close.as_u64())?;
                    if let Some(reason) = reason {
                        encoder.write_var_string(reason)?;
                    }
                }
                DocumentMessage::SyncStatus { applied } => {
                    encoder.write_var_uint(MessageType::SyncStatus.as_u64())?;
                    encoder.write_var_uint(if *applied { 1 } else { 0 })?;
                }
            }
        }
    }
    Ok(encoder.finish())
}
