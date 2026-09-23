//! Offline tests for the Hocuspocus 4.6.0 wire codec.
//!
//! `lib.rs` does not export `collab` yet; include the module directly until the
//! coordinator registers it after prerequisite slices land.

#[path = "../src/collab/wire.rs"]
mod wire;

use std::fs;

use wire::{
    decode, decode_with_limits, encode, AuthMessage, CollabKind, CollabRoomName, ConnectionMessage,
    DocumentMessage, Limits, SyncStep, WireError, WireFrame,
};

fn hex_decode(hex: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(hex.len() / 2);
    let bytes = hex.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        let pair = std::str::from_utf8(&bytes[i..i + 2]).expect("ascii hex");
        out.push(u8::from_str_radix(pair, 16).expect("hex digit"));
        i += 2;
    }
    out
}

fn roundtrip(frame: &WireFrame) {
    let encoded = encode(frame).expect("encode");
    let decoded = decode(&encoded).expect("decode roundtrip");
    assert_eq!(decoded, *frame);
    let reencoded = encode(&decoded).expect("re-encode");
    assert_eq!(reencoded, encoded);
}

#[test]
fn collab_room_name_parse_workspace_document_task() {
    let ws = "11111111-1111-4111-8111-111111111111";
    let doc = "22222222-2222-4222-8222-222222222222";
    let parsed = CollabRoomName::parse(&format!("{ws}:document:{doc}")).expect("document");
    assert_eq!(parsed.kind, CollabKind::Document);
    assert_eq!(parsed.routing_key(), format!("{ws}:document:{doc}"));

    let task = CollabRoomName::parse(&format!("{ws}:task:{doc}")).expect("task");
    assert_eq!(task.kind, CollabKind::Task);

    assert!(CollabRoomName::parse(&format!("{ws}:{doc}")).is_none());
    assert!(CollabRoomName::parse(&format!("{ws}:page:{doc}")).is_none());
    assert!(CollabRoomName::parse(&format!("{ws}:document:{doc}:extra")).is_none());
}

#[test]
fn connection_ping_pong_roundtrip() {
    roundtrip(&WireFrame::Connection(ConnectionMessage::Ping));
    roundtrip(&WireFrame::Connection(ConnectionMessage::Pong));
    assert_eq!(
        decode(&[9]).unwrap(),
        WireFrame::Connection(ConnectionMessage::Ping)
    );
    assert_eq!(
        decode(&[10]).unwrap(),
        WireFrame::Connection(ConnectionMessage::Pong)
    );
}

#[test]
fn auth_and_stateless_roundtrip_korean_emoji() {
    let key = "11111111-1111-4111-8111-111111111111:document:22222222-2222-4222-8222-222222222222";
    roundtrip(&WireFrame::Document {
        routing_key: key.to_string(),
        room: CollabRoomName::parse(key),
        message: DocumentMessage::Auth(AuthMessage::Token {
            token: "12345".into(),
            provider_version: Some("4.6.0".into()),
        }),
    });
    roundtrip(&WireFrame::Document {
        routing_key: key.to_string(),
        room: CollabRoomName::parse(key),
        message: DocumentMessage::Auth(AuthMessage::Authenticated {
            scope: "readonly".into(),
        }),
    });
    roundtrip(&WireFrame::Document {
        routing_key: key.to_string(),
        room: CollabRoomName::parse(key),
        message: DocumentMessage::Stateless("persist:안녕-🚀✨".into()),
    });
}

#[test]
fn golden_fixtures_match_js_serialization() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/compat/fixtures/hocus-wire.json"
    );
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).expect("read fixtures")).expect("json");
    let cases = json["cases"].as_array().expect("cases array");

    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let hex = case["hex"].as_str().expect("hex");
        let bytes = hex_decode(hex);
        let decoded = decode(&bytes).unwrap_or_else(|err| panic!("decode {id}: {err}"));

        match id {
            "connection_ping" => {
                assert_eq!(decoded, WireFrame::Connection(ConnectionMessage::Ping));
            }
            "connection_pong" => {
                assert_eq!(decoded, WireFrame::Connection(ConnectionMessage::Pong));
            }
            "auth_token_client" => {
                let WireFrame::Document { message, room, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert!(room.is_some());
                let DocumentMessage::Auth(AuthMessage::Token {
                    token,
                    provider_version,
                }) = message
                else {
                    panic!("expected auth token for {id}");
                };
                assert!(!token.is_empty());
                assert_eq!(provider_version.as_deref(), Some("4.6.0"));
            }
            "stateless_persist" | "stateless_persisted" | "stateless_persist_failed" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                let DocumentMessage::Stateless(payload) = message else {
                    panic!("expected stateless for {id}");
                };
                assert!(payload.starts_with("persist"));
            }
            "sync_update_korean_emoji" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                let DocumentMessage::Sync(sync) = message else {
                    panic!("expected sync for {id}");
                };
                assert_eq!(sync.step, SyncStep::Update);
            }
            "awareness_session_routing_key" => {
                let WireFrame::Document { routing_key, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert!(routing_key.contains('\0'));
            }
            _ => {
                // Every golden case must decode and re-encode to identical bytes.
                let reencoded = encode(&decoded).expect("re-encode");
                assert_eq!(reencoded, bytes, "roundtrip bytes for {id}");
            }
        }
    }
}

#[test]
fn malformed_boundaries_rejected() {
    assert!(matches!(decode(&[]), Err(WireError::EmptyFrame)));

    let limits = Limits {
        max_frame_bytes: 32,
        max_routing_key_bytes: 16,
        max_string_bytes: 8,
        max_binary_payload_bytes: 8,
    };

    assert!(matches!(
        decode_with_limits(&[0xFF; 40], limits),
        Err(WireError::FrameTooLarge { .. })
    ));

    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/compat/fixtures/hocus-wire.json"
    );
    let json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).expect("read fixtures")).expect("json");
    for case in json["malformed"].as_array().expect("malformed") {
        let id = case["id"].as_str().expect("id");
        let hex = case["hex"].as_str().unwrap_or("");
        let bytes = hex_decode(hex);
        let err = decode(&bytes).unwrap_err();
        assert!(
            matches!(
                err,
                WireError::EmptyFrame
                    | WireError::Truncated
                    | WireError::UnknownMessageType(_)
                    | WireError::InvalidVarUint
                    | WireError::StringTooLong { .. }
                    | WireError::BinaryTooLong { .. }
            ),
            "unexpected error for {id}: {err}"
        );
    }
}

#[test]
fn unknown_message_type_is_typed_error() {
    let key = "11111111-1111-4111-8111-111111111111:document:22222222-2222-4222-8222-222222222222";
    let frame = WireFrame::Document {
        routing_key: key.to_string(),
        room: CollabRoomName::parse(key),
        message: DocumentMessage::QueryAwareness,
    };
    let mut bytes = encode(&frame).expect("encode");
    // Flip the message type byte (after routing key) to an unsupported opcode.
    let type_index = bytes.iter().position(|b| *b == 3).expect("query type byte");
    bytes[type_index] = 99;
    assert!(matches!(
        decode(&bytes),
        Err(WireError::UnknownMessageType(99))
    ));
}
