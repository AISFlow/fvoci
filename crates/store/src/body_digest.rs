//! The digest of a body: what tells whether two bodies are the same, whatever the order
//! of the members of their objects (`body_docs.body_digest`, `body_versions.body_digest`).

use fvoci_model::Body;
use serde_json::Value;
use sha2::{Digest as _, Sha256};

/// The digest of a body as the store holds it: SHA-256 of its JSON with
/// the members of every object in the order of their names and no space; of nothing for
/// no body.
pub fn body_digest(body: Option<&Body>) -> Vec<u8> {
    let mut json = Vec::new();
    if let Some(body) = body {
        write_canonical(body.document(), &mut json);
    }
    Sha256::digest(&json).to_vec()
}

/// Writes a JSON value with the members of every object in the order of their names.
fn write_canonical(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Object(members) => {
            let mut names: Vec<&String> = members.keys().collect();
            names.sort_unstable();
            out.push(b'{');
            for (index, name) in names.into_iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(&Value::String(name.clone()), out);
                out.push(b':');
                write_canonical(&members[name], out);
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b',');
                }
                write_canonical(item, out);
            }
            out.push(b']');
        }
        // A value with no members is written the same whatever the order of a map.
        other => out.extend_from_slice(other.to_string().as_bytes()),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn the_digest_of_a_body_does_not_depend_on_the_order_of_its_members() {
        let one = Body::from_stored(json!({"type": "doc", "content": [{"type": "paragraph"}]}));
        let other = Body::from_stored(
            serde_json::from_str(r#"{"content":[{"type":"paragraph"}],"type":"doc"}"#)
                .expect("JSON"),
        );
        assert_eq!(body_digest(Some(&one)), body_digest(Some(&other)));
        assert_ne!(
            body_digest(Some(&one)),
            body_digest(None),
            "a body is not none"
        );
        let mut written = Vec::new();
        write_canonical(
            &json!({"b": [1, "x"], "a": {"d": null, "c": true}}),
            &mut written,
        );
        assert_eq!(
            String::from_utf8(written).expect("UTF-8"),
            r#"{"a":{"c":true,"d":null},"b":[1,"x"]}"#
        );
    }
}
