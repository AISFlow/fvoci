//! Stdio JSON-lines bridge: apply/encode Yjs update V1 through Yrs.
//! This is not a Hocuspocus/y-websocket adapter and does not speak provider frames.

use std::io::{self, BufRead, Write};

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use serde_json::{json, Value};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{
    Doc, GetString, Options, ReadTxn, StateVector, Transact, Update, XmlFragment, XmlFragmentRef,
};

const FRAGMENT: &str = "prosemirror";

fn new_doc() -> Doc {
    // Yrs 0.28 added formatting cleanup; 0.23.5 had none, so keep it off.
    Doc::with_options(Options {
        skip_gc: true,
        cleanup_formatting: false,
        ..Options::default()
    })
}

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    B64.decode(s.trim()).map_err(|e| e.to_string())
}

fn apply_v1(doc: &Doc, bytes: &[u8]) -> Result<(), String> {
    let update = Update::decode_v1(bytes).map_err(|e| format!("decode_v1: {e}"))?;
    let mut txn = doc.transact_mut();
    txn.apply_update(update)
        .map_err(|e| format!("apply_update: {e}"))
}

fn xml_text(doc: &Doc) -> (u32, String) {
    let xml: XmlFragmentRef = doc.get_or_insert_xml_fragment(FRAGMENT);
    let txn = doc.transact();
    let len = xml.len(&txn);
    let text = xml.get_string(&txn);
    (len, text)
}

fn handle(doc: &mut Doc, req: &Value) -> Value {
    let cmd = match req.get("cmd").and_then(Value::as_str) {
        Some(c) => c,
        None => return json!({ "ok": false, "error": "missing cmd" }),
    };
    match cmd {
        "ping" => json!({
            "ok": true,
            "skip_gc": true,
            "fragment": FRAGMENT,
            "encoding": "updateV1",
        }),
        "reset" => {
            *doc = new_doc();
            json!({ "ok": true })
        }
        "apply_v1" => {
            let Some(b64) = req.get("b64").and_then(Value::as_str) else {
                return json!({ "ok": false, "error": "missing b64" });
            };
            match b64_decode(b64).and_then(|bytes| apply_v1(doc, &bytes)) {
                Ok(()) => json!({ "ok": true }),
                Err(error) => json!({ "ok": false, "error": error }),
            }
        }
        "encode_state_v1" => {
            let txn = doc.transact();
            let bytes = txn.encode_state_as_update_v1(&StateVector::default());
            json!({ "ok": true, "b64": B64.encode(bytes) })
        }
        "encode_sv" => {
            let txn = doc.transact();
            json!({ "ok": true, "b64": B64.encode(txn.state_vector().encode_v1()) })
        }
        "diff_v1" => {
            let Some(sv_b64) = req.get("sv_b64").and_then(Value::as_str) else {
                return json!({ "ok": false, "error": "missing sv_b64" });
            };
            match b64_decode(sv_b64).and_then(|bytes| {
                StateVector::decode_v1(&bytes).map_err(|e| format!("sv decode: {e}"))
            }) {
                Ok(sv) => {
                    let txn = doc.transact();
                    json!({ "ok": true, "b64": B64.encode(txn.encode_state_as_update_v1(&sv)) })
                }
                Err(error) => json!({ "ok": false, "error": error }),
            }
        }
        "inspect" => {
            let (len, text) = xml_text(doc);
            json!({
                "ok": true,
                "skip_gc": true,
                "fragment": FRAGMENT,
                "xml_len": len,
                "xml_string": text,
            })
        }
        other => json!({ "ok": false, "error": format!("unknown cmd {other}") }),
    }
}

fn main() {
    let mut doc = new_doc();
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                let _ = writeln!(stdout, "{}", json!({ "ok": false, "error": e.to_string() }));
                let _ = stdout.flush();
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let resp = match serde_json::from_str::<Value>(&line) {
            Ok(req) => handle(&mut doc, &req),
            Err(e) => json!({ "ok": false, "error": e.to_string() }),
        };
        if writeln!(stdout, "{}", resp).is_err() {
            break;
        }
        if stdout.flush().is_err() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // `crates/collab-engine/fixtures/revision_before.v1` as standard padded base64.
    const UPDATE_B64: &str = "AQEpAAgBAXQBdxDsiqTrg4Xsg7ct67O466y4AA==";

    fn b64_reply(doc: &mut Doc, req: Value) -> String {
        let resp = handle(doc, &req);
        assert_eq!(resp["ok"], true, "{req} -> {resp}");
        resp["b64"].as_str().expect("b64").to_owned()
    }

    #[test]
    fn bridge_base64_bytes_are_stable() {
        let mut doc = new_doc();
        assert_eq!(b64_reply(&mut doc, json!({ "cmd": "encode_sv" })), "AA==");
        assert_eq!(
            b64_reply(&mut doc, json!({ "cmd": "encode_state_v1" })),
            "AAA="
        );
        let applied = handle(
            &mut doc,
            &json!({ "cmd": "apply_v1", "b64": format!(" {UPDATE_B64}\n") }),
        );
        assert_eq!(applied, json!({ "ok": true }));
        assert_eq!(
            b64_reply(&mut doc, json!({ "cmd": "encode_state_v1" })),
            UPDATE_B64
        );
        assert_eq!(b64_reply(&mut doc, json!({ "cmd": "encode_sv" })), "ASkB");
        assert_eq!(
            b64_reply(&mut doc, json!({ "cmd": "diff_v1", "sv_b64": "AA==" })),
            UPDATE_B64
        );
        for bad in ["AAA", "AA A=", "-w==", "+x==", "AAA=="] {
            let resp = handle(&mut doc, &json!({ "cmd": "apply_v1", "b64": bad }));
            assert_eq!(resp["ok"], false, "{bad:?} must be refused");
        }
    }

    #[test]
    fn bridge_uses_the_standard_alphabet() {
        // `crates/collab-engine/fixtures/korean_emoji_mid_edit.v1`; it encodes back unchanged.
        const PLUS_B64: &str = "AQEVC8QVBBUFA+ykkQA=";
        let mut doc = new_doc();
        let applied = handle(&mut doc, &json!({ "cmd": "apply_v1", "b64": PLUS_B64 }));
        assert_eq!(applied, json!({ "ok": true }));
        assert_eq!(
            b64_reply(&mut doc, json!({ "cmd": "encode_state_v1" })),
            PLUS_B64
        );
        assert_eq!(b64_decode("+/8=").unwrap(), [0xfb, 0xff]);
        assert!(b64_decode("-_8=").is_err());
    }

    // Stored Yjs 13.6.33 updates and the oracle that `js/generate.mjs` wrote with them.
    fn fixture(name: &str) -> Vec<u8> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/collab-engine/fixtures")
            .join(name);
        std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    fn oracle() -> Value {
        let oracle: Value =
            serde_json::from_slice(&fixture("expectations.json")).expect("expectations.json");
        assert_eq!(oracle["yjs"], "13.6.33");
        oracle
    }

    fn strings(list: &Value) -> Vec<&str> {
        list.as_array()
            .expect("oracle list")
            .iter()
            .map(|s| s.as_str().expect("oracle string"))
            .collect()
    }

    /// Applies the fixtures in order through the bridge; returns the doc and its inspect reply.
    fn bridge_apply(names: &[&str]) -> (Doc, Value) {
        let mut doc = new_doc();
        for name in names {
            let b64 = B64.encode(fixture(name));
            let resp = handle(&mut doc, &json!({ "cmd": "apply_v1", "b64": b64 }));
            assert_eq!(resp, json!({ "ok": true }), "{name}");
        }
        let inspect = handle(&mut doc, &json!({ "cmd": "inspect" }));
        (doc, inspect)
    }

    fn xml(names: &[&str]) -> String {
        let (_, inspect) = bridge_apply(names);
        inspect["xml_string"]
            .as_str()
            .expect("xml_string")
            .to_owned()
    }

    /// Text leaves of an oracle ProseMirror document.
    fn texts(node: &Value) -> Vec<&str> {
        let mut out: Vec<&str> = node["text"].as_str().into_iter().collect();
        for child in node["content"].as_array().into_iter().flatten() {
            out.extend(texts(child));
        }
        out
    }

    fn array_t(doc: &Doc) -> Value {
        use yrs::types::ToJson;
        let txn = doc.transact();
        let arr = txn.get_array("t").expect("array t");
        serde_json::to_value(arr.to_json(&txn)).expect("array json")
    }

    #[test]
    fn stored_yjs_updates_decode_to_the_oracle_content() {
        let oracle = oracle();

        let structured = xml(&["structured.v1"]);
        for s in strings(&oracle["structured"]["must_include"]) {
            assert!(
                structured.contains(s),
                "structured lacks {s:?}: {structured}"
            );
        }
        let followup = xml(&["structured.v1", "followup_edit.v1"]);
        for s in strings(&oracle["followup"]["must_include"]) {
            assert!(followup.contains(s), "followup lacks {s:?}: {followup}");
        }

        let korean = xml(&[
            "korean_emoji_base.v1",
            "korean_emoji_mid_edit.v1",
            "korean_emoji_delete.v1",
        ]);
        let ko = &oracle["korean_emoji"];
        for s in strings(&ko["after_mid_and_delete_includes"]) {
            assert!(korean.contains(s), "korean lacks {s:?}: {korean}");
        }
        for s in strings(&ko["after_mid_and_delete_excludes"]) {
            assert!(!korean.contains(s), "korean keeps deleted {s:?}: {korean}");
        }

        let utf8 = xml(&["utf8_korean.v1"]);
        let marker = oracle["utf8_marker"].as_str().expect("utf8_marker");
        assert!(utf8.contains(marker), "{utf8}");

        // Out of order: u2 stays pending until its u1 dependency arrives.
        let (u1_has, u2_has) = (
            oracle["pending"]["u1_has"].as_str().expect("u1_has"),
            oracle["pending"]["u2_has"].as_str().expect("u2_has"),
        );
        let pending = xml(&["pending_u2.v1"]);
        assert!(!pending.contains(u2_has), "{pending}");
        let both = xml(&["pending_u2.v1", "pending_u1.v1"]);
        assert!(both.contains(u1_has) && both.contains(u2_has), "{both}");

        // A delete-only update changes the delete set, not the state vector.
        let del = &oracle["delete_only"];
        let (mut doc, base) = bridge_apply(&["delete_only_base.v1"]);
        assert_eq!(
            b64_reply(&mut doc, json!({ "cmd": "encode_sv" })),
            del["sv_before_b64"]
        );
        let b64 = B64.encode(fixture("delete_only.v1"));
        let applied = handle(&mut doc, &json!({ "cmd": "apply_v1", "b64": b64 }));
        assert_eq!(applied, json!({ "ok": true }));
        assert_eq!(
            b64_reply(&mut doc, json!({ "cmd": "encode_sv" })),
            del["sv_after_b64"]
        );
        let deleted = handle(&mut doc, &json!({ "cmd": "inspect" }));
        let after = deleted["xml_string"].as_str().expect("xml_string");
        for s in texts(&del["prosemirror_json_after"]) {
            assert!(after.contains(s), "delete_only lacks {s:?}: {after}");
        }
        for s in texts(&del["prosemirror_json_before"]) {
            assert!(base["xml_string"].as_str().unwrap().contains(s), "{base}");
            assert!(!after.contains(s), "delete_only keeps {s:?}: {after}");
        }

        // The stored Yjs state vector still selects the delete for a peer that missed it.
        // The paragraph has one attribute, so the XML string does not depend on map order.
        let sv_b64 = B64.encode(fixture("sv_before_delete.bin"));
        let diff = b64_reply(&mut doc, json!({ "cmd": "diff_v1", "sv_b64": sv_b64 }));
        let (mut peer, _) = bridge_apply(&["delete_only_base.v1"]);
        let applied = handle(&mut peer, &json!({ "cmd": "apply_v1", "b64": diff }));
        assert_eq!(applied, json!({ "ok": true }));
        assert_eq!(handle(&mut peer, &json!({ "cmd": "inspect" })), deleted);

        // Yjs builds this fragment as paragraph, Y.Map, paragraph; only its ProseMirror
        // projection is malformed (oracle `map_child`).
        let (_, map_child) = bridge_apply(&["map_child.v1"]);
        assert_eq!(map_child["xml_len"], 3, "{map_child}");
    }

    #[test]
    fn invalid_utf8_in_an_update_is_refused() {
        // Yrs 0.28 checks wire strings; 0.23.5 read them with from_utf8_unchecked.
        for (name, text) in [("revision_before.v1", "스"), ("utf8_korean.v1", "안")] {
            let mut bytes = fixture(name);
            let at = bytes
                .windows(text.len())
                .position(|w| w == text.as_bytes())
                .expect("string bytes");
            bytes[at] = 0xff;
            let mut doc = new_doc();
            let b64 = B64.encode(&bytes);
            let resp = handle(&mut doc, &json!({ "cmd": "apply_v1", "b64": b64 }));
            assert_eq!(resp["ok"], false, "{name}: {resp}");
            assert_eq!(b64_reply(&mut doc, json!({ "cmd": "encode_sv" })), "AA==");
        }
    }

    #[test]
    fn state_vector_client_ids_truncate_to_32_bits() {
        // small-client, as in collab-engine, reads client 0x1_0000_0029 as 41, so the
        // diff omits client 41's update. 0.23.5 kept the u64 id and sent the update.
        // Yjs 13.6.33 client ids fit in u32.
        let sv = |client: u64| {
            let mut out = vec![1];
            for mut v in [client, 1] {
                while v >= 0x80 {
                    out.push(v as u8 | 0x80);
                    v >>= 7;
                }
                out.push(v as u8);
            }
            B64.encode(out)
        };
        let mut doc = new_doc();
        let applied = handle(&mut doc, &json!({ "cmd": "apply_v1", "b64": UPDATE_B64 }));
        assert_eq!(applied, json!({ "ok": true }));
        assert_eq!(sv(41), "ASkB");
        for client in [41, (1 << 32) + 41] {
            let diff = b64_reply(&mut doc, json!({ "cmd": "diff_v1", "sv_b64": sv(client) }));
            assert_eq!(diff, "AAA=", "client {client:#x}");
        }
    }

    #[test]
    fn stored_yjs_snapshot_restores_the_revision() {
        use yrs::updates::encoder::{Encoder, EncoderV1};
        let revision = &oracle()["revision"];
        let (before, _) = bridge_apply(&["revision_before.v1"]);
        assert_eq!(array_t(&before), revision["before"]);
        let (after, _) = bridge_apply(&["revision_after.v1"]);
        assert_eq!(array_t(&after), revision["after"]);

        // skip_gc keeps the deleted entry, so the Yjs snapshot taken before restores it.
        // These array entries are whole items. A snapshot that cuts inside a text block
        // needs split_by_snapshot first (crates/vendor/yrs-0.28.0/PATCHES.md); not covered.
        let snapshot =
            yrs::Snapshot::decode_v1(&fixture("revision_snapshot.bin")).expect("snapshot");
        let mut encoder = EncoderV1::new();
        after
            .transact()
            .encode_state_from_snapshot(&snapshot, &mut encoder)
            .expect("encode_state_from_snapshot");
        let restored = new_doc();
        let update = Update::decode_v1(&encoder.to_vec()).expect("restored update");
        restored
            .transact_mut()
            .apply_update(update)
            .expect("restored apply");
        assert_eq!(array_t(&restored), revision["before"]);
    }
}
