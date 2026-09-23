//! Offline tests for the Hocuspocus 4.6.0 wire codec.
//!
//! Exercise the shared codec module; no WebSocket route is enabled.

use fvoci_server::collab::wire;

use std::fs;
use std::panic::{self, AssertUnwindSafe};

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

fn encode_varuint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        if value < 0x80 {
            out.push(value as u8);
            return out;
        }
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
}

fn roundtrip(frame: &WireFrame) {
    let encoded = encode(frame).expect("encode");
    let decoded = decode(&encoded).expect("decode roundtrip");
    assert_eq!(decoded, *frame);
    let reencoded = encode(&decoded).expect("re-encode");
    assert_eq!(reencoded, encoded);
}

fn fixture_json() -> serde_json::Value {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/compat/fixtures/hocus-wire.json"
    );
    serde_json::from_str(&fs::read_to_string(path).expect("read fixtures")).expect("json")
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
    assert!(CollabRoomName::parse(&format!("{ws}:document:")).is_none());
    assert!(CollabRoomName::parse("not-a-uuid:document:also-not").is_none());
    assert!(CollabRoomName::parse(&format!("{}:document:{doc}", "a".repeat(32))).is_none());
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
    assert_eq!(
        encode(&WireFrame::Connection(ConnectionMessage::Ping)).unwrap(),
        vec![9]
    );
    assert_eq!(
        encode(&WireFrame::Connection(ConnectionMessage::Pong)).unwrap(),
        vec![10]
    );
}

#[test]
fn auth_token_is_awareness_client_id_not_session() {
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
        message: DocumentMessage::Auth(AuthMessage::TokenRequest),
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
        message: DocumentMessage::Auth(AuthMessage::Authenticated {
            scope: "read-write".into(),
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
    let json = fixture_json();
    assert_eq!(json["clientID"], 12345);
    let cases = json["cases"].as_array().expect("cases array");
    assert!(!cases.is_empty(), "golden cases must not be empty");

    for case in cases {
        let id = case["id"].as_str().expect("case id");
        let hex = case["hex"].as_str().expect("hex");
        let bytes = hex_decode(hex);
        let decoded = decode(&bytes).unwrap_or_else(|err| panic!("decode {id}: {err}"));
        let reencoded = encode(&decoded).unwrap_or_else(|err| panic!("re-encode {id}: {err}"));
        assert_eq!(reencoded, bytes, "roundtrip bytes for {id}");

        match id {
            "connection_ping" => {
                assert_eq!(decoded, WireFrame::Connection(ConnectionMessage::Ping));
            }
            "connection_pong" => {
                assert_eq!(decoded, WireFrame::Connection(ConnectionMessage::Pong));
            }
            "auth_token_client" => {
                let WireFrame::Document { message, room, .. } = &decoded else {
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
                assert_eq!(token, "12345");
                assert_eq!(provider_version.as_deref(), Some("4.6.0"));
            }
            "auth_token_request_server" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(message, DocumentMessage::Auth(AuthMessage::TokenRequest));
            }
            "auth_authenticated_readonly" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(
                    message,
                    DocumentMessage::Auth(AuthMessage::Authenticated {
                        scope: "readonly".into(),
                    })
                );
            }
            "auth_authenticated_readwrite" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(
                    message,
                    DocumentMessage::Auth(AuthMessage::Authenticated {
                        scope: "read-write".into(),
                    })
                );
            }
            "stateless_persist" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(
                    message,
                    DocumentMessage::Stateless(
                        "persist:33333333-3333-4333-8333-333333333333".into()
                    )
                );
            }
            "stateless_persisted" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(
                    message,
                    DocumentMessage::Stateless(
                        "persisted:33333333-3333-4333-8333-333333333333".into()
                    )
                );
            }
            "stateless_persist_failed" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(
                    message,
                    DocumentMessage::Stateless(
                        "persist-failed:33333333-3333-4333-8333-333333333333".into()
                    )
                );
            }
            "sync_step1" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                let DocumentMessage::Sync(sync) = message else {
                    panic!("expected sync for {id}");
                };
                assert_eq!(sync.step, SyncStep::Step1);
            }
            "sync_step2" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                let DocumentMessage::Sync(sync) = message else {
                    panic!("expected sync for {id}");
                };
                assert_eq!(sync.step, SyncStep::Step2);
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
                let WireFrame::Document {
                    routing_key,
                    room,
                    message,
                } = decoded
                else {
                    panic!("expected document frame for {id}");
                };
                assert!(routing_key.contains('\0'));
                assert!(routing_key.ends_with("session-abc"));
                assert!(
                    room.is_some(),
                    "NUL session suffix must not break room parse"
                );
                assert!(matches!(message, DocumentMessage::Awareness(_)));
            }
            "close_client" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(message, DocumentMessage::Close { reason: None });
            }
            "close_server_reason" => {
                let WireFrame::Document { message, .. } = decoded else {
                    panic!("expected document frame for {id}");
                };
                assert_eq!(
                    message,
                    DocumentMessage::Close {
                        reason: Some("provider_initiated".into()),
                    }
                );
            }
            _ => {}
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

    let huge_string = encode_varuint(1_000_000);
    assert!(matches!(
        decode(&huge_string),
        Err(WireError::StringTooLong {
            size: 1_000_000,
            ..
        })
    ));

    let json = fixture_json();
    for case in json["malformed"].as_array().expect("malformed") {
        let id = case["id"].as_str().expect("id");
        let hex = case["hex"].as_str().unwrap_or("");
        let bytes = hex_decode(hex);
        let decoded = panic::catch_unwind(AssertUnwindSafe(|| decode(&bytes)));
        let err = match decoded {
            Ok(Ok(frame)) => panic!("{id} decoded unexpectedly: {frame:?}"),
            Ok(Err(err)) => err,
            Err(_) => panic!("{id} panicked instead of returning WireError"),
        };
        match id {
            "empty" => assert!(matches!(err, WireError::EmptyFrame), "{id}: {err}"),
            "truncated_varstring" => {
                assert!(matches!(err, WireError::Truncated), "{id}: {err}")
            }
            "unknown_message_type" => {
                assert!(
                    matches!(err, WireError::UnknownMessageType(99)),
                    "{id}: {err}"
                )
            }
            "oversized_length_prefix" | "varuint_overflow" => {
                assert!(matches!(err, WireError::InvalidVarUint), "{id}: {err}")
            }
            "document_ping" => {
                assert!(
                    matches!(err, WireError::DocumentPingNotAllowed),
                    "{id}: {err}"
                )
            }
            "document_pong" => {
                assert!(
                    matches!(err, WireError::DocumentPongNotAllowed),
                    "{id}: {err}"
                )
            }
            _ => panic!("unspecified malformed id {id}: {err}"),
        }
    }
}

#[test]
fn invented_sync_reply_and_broadcast_opcodes_are_unknown() {
    let key = "11111111-1111-4111-8111-111111111111:document:22222222-2222-4222-8222-222222222222";
    let frame = WireFrame::Document {
        routing_key: key.to_string(),
        room: CollabRoomName::parse(key),
        message: DocumentMessage::QueryAwareness,
    };
    let mut sync_reply = encode(&frame).expect("encode");
    let mut broadcast = sync_reply.clone();
    let type_index = sync_reply
        .iter()
        .position(|b| *b == 3)
        .expect("query type byte");
    sync_reply[type_index] = 4;
    broadcast[type_index] = 6;
    assert!(matches!(
        decode(&sync_reply),
        Err(WireError::UnknownMessageType(4))
    ));
    assert!(matches!(
        decode(&broadcast),
        Err(WireError::UnknownMessageType(6))
    ));
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
    let type_index = bytes.iter().position(|b| *b == 3).expect("query type byte");
    bytes[type_index] = 99;
    assert!(matches!(
        decode(&bytes),
        Err(WireError::UnknownMessageType(99))
    ));
}

#[test]
fn extra_trailing_bytes_and_utf8_rejected() {
    let key = "11111111-1111-4111-8111-111111111111:document:22222222-2222-4222-8222-222222222222";
    let mut bytes = encode(&WireFrame::Document {
        routing_key: key.to_string(),
        room: CollabRoomName::parse(key),
        message: DocumentMessage::QueryAwareness,
    })
    .expect("encode");
    bytes.push(0x00);
    assert!(matches!(decode(&bytes), Err(WireError::Truncated)));

    let mut invalid_utf8 = encode_varuint(1);
    invalid_utf8.push(0xff);
    invalid_utf8.push(0x03);
    assert!(matches!(decode(&invalid_utf8), Err(WireError::Utf8)));
}

#[test]
fn sync_declared_length_rejects_trailing_bytes() {
    let json = fixture_json();
    let cases = json["cases"].as_array().expect("cases");
    let sync = cases
        .iter()
        .find(|case| case["id"] == "sync_step1")
        .expect("sync_step1");
    let mut bytes = hex_decode(sync["hex"].as_str().expect("hex"));
    bytes.push(0x00);
    assert!(
        matches!(decode(&bytes), Err(WireError::Truncated)),
        "trailing byte after declared y-protocol length must not be swallowed"
    );
}

#[test]
fn varuint_above_lib0_safe_integer_is_invalid() {
    let too_big = encode_varuint((1_u64 << 53) + 1);
    let decoded = panic::catch_unwind(AssertUnwindSafe(|| decode(&too_big)));
    match decoded {
        Ok(Err(WireError::InvalidVarUint)) => {}
        Ok(other) => panic!("expected InvalidVarUint, got {other:?}"),
        Err(_) => panic!("varuint over MAX_SAFE_INTEGER panicked"),
    }
}

#[test]
fn routing_key_limit_does_not_allocate_unbounded() {
    let limits = Limits {
        max_frame_bytes: 64,
        max_routing_key_bytes: 4,
        max_string_bytes: 32,
        max_binary_payload_bytes: 32,
    };
    let mut bytes = encode_varuint(20);
    bytes.extend_from_slice(&[b'a'; 20]);
    bytes.push(3);
    assert!(matches!(
        decode_with_limits(&bytes, limits),
        Err(WireError::RoutingKeyTooLong { size: 20, .. })
    ));
}
