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
    let mut opts = Options::default();
    opts.skip_gc = true;
    Doc::with_options(opts)
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
