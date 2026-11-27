//! What the tests of this crate share: the vectors both sides are held to, and the
//! ways in and out of a Y document.

#![allow(
    dead_code,
    reason = "each test binary uses the part of this module it needs"
)]

pub(crate) mod explore;
pub(crate) mod honest;
pub(crate) mod random;
pub(crate) mod shapes;
pub(crate) mod slow;
pub(crate) mod wire;

use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fvoci_collab::testing::{copy_of, with_update};
use fvoci_collab::{FIELD, UpdateLimits, new_doc};
use fvoci_model::Body;
use serde_json::Value;
use yrs::{Doc, ReadTxn as _, Transact as _, TransactionMut, XmlFragmentRef};

/// Limits no vector comes near.
pub(crate) const ROOMY: UpdateLimits = UpdateLimits {
    update_bytes: 64 * 1024 * 1024,
    state_bytes: 64 * 1024 * 1024,
    body_bytes: 64 * 1024 * 1024,
    placing_steps: Some(fvoci_collab::PLACING_STEPS),
    clients: Some(fvoci_collab::MAX_CLIENTS),
    client_changes: Some(fvoci_collab::MAX_CLIENT_CHANGES),
    total_changes: Some(fvoci_collab::MAX_TOTAL_CHANGES),
    reading_steps: Some(fvoci_collab::READING_STEPS),
};

/// A file of the contracts both sides read.
pub(crate) fn contract(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../contracts")
        .join(name)
}

pub(crate) fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

/// A case of `contracts/ydoc-vectors.json`.
#[derive(Debug)]
pub(crate) struct Vector {
    pub(crate) name: String,
    /// The Y document, as a version 1 update.
    pub(crate) update: Vec<u8>,
    /// What an editor reads from it, in the stored form ([`stored_form`]); `Null` for
    /// no body.
    pub(crate) json: Value,
}

/// The JSON an editor reads, as the server stores it: the same, but that the start of
/// an ordered list at 1, which an editor fills in, is left out, as a body may leave it.
pub(crate) fn stored_form(json: &Value) -> Value {
    match json {
        Value::Array(items) => Value::Array(items.iter().map(stored_form).collect()),
        Value::Object(members) => {
            let mut stored: serde_json::Map<String, Value> = members
                .iter()
                .map(|(name, member)| (name.clone(), stored_form(member)))
                .collect();
            if stored.get("type").and_then(Value::as_str) == Some("orderedList")
                && let Some(Value::Object(attrs)) = stored.get_mut("attrs")
            {
                if attrs.get("start").and_then(Value::as_i64) == Some(1) {
                    attrs.remove("start");
                }
                if attrs.is_empty() {
                    stored.remove("attrs");
                }
            }
            Value::Object(stored)
        }
        other => other.clone(),
    }
}

/// The cases of one section of `contracts/ydoc-vectors.json`.
pub(crate) fn vectors(section: &str) -> Vec<Vector> {
    let file = read_json(&contract("ydoc-vectors.json"));
    let cases = file[section]
        .as_array()
        .unwrap_or_else(|| panic!("the vectors have a list of {section}"));
    assert!(!cases.is_empty(), "there are {section}");
    cases
        .iter()
        .map(|case| {
            let name = case["name"].as_str().expect("a case has a name").to_owned();
            let update = from_base64(case["update"].as_str().expect("a case has an update"));
            Vector {
                name,
                update,
                json: stored_form(&case["json"]),
            }
        })
        .collect()
}

/// The accepted bodies of `contracts/body-vectors.json` that are written out, by name.
pub(crate) fn accepted_bodies() -> Vec<(String, Value)> {
    let file = read_json(&contract("body-vectors.json"));
    file["cases"]
        .as_array()
        .expect("the body vectors have cases")
        .iter()
        .filter(|case| case["expect"] == "accept" && case.get("body").is_some())
        .map(|case| {
            let name = case["name"].as_str().expect("a case has a name").to_owned();
            (name, case["body"].clone())
        })
        .collect()
}

/// A body with the Y document the editor's binding makes of it.
#[derive(Debug)]
pub(crate) struct BodyVector {
    /// The body as it is stored before anybody edits it.
    pub(crate) body: Value,
    pub(crate) vector: Vector,
}

/// Every body the vectors have as a Y document: the accepted bodies of
/// `contracts/body-vectors.json` (`documents`) and those the vectors bring themselves
/// (`bodies`).
pub(crate) fn body_vectors() -> Vec<BodyVector> {
    let documents = vectors("documents");
    let accepted = accepted_bodies();
    assert_eq!(
        documents.iter().map(|one| &one.name).collect::<Vec<_>>(),
        accepted.iter().map(|(name, _)| name).collect::<Vec<_>>(),
        "the vectors hold every accepted body"
    );
    let file = read_json(&contract("ydoc-vectors.json"));
    let own = file["bodies"]
        .as_array()
        .expect("the vectors have bodies of their own")
        .iter()
        .map(|case| case["body"].clone());
    documents
        .into_iter()
        .zip(accepted.into_iter().map(|(_, body)| body))
        .chain(vectors("bodies").into_iter().zip(own))
        .map(|(vector, body)| BodyVector { body, vector })
        .collect()
}

pub(crate) fn from_base64(text: &str) -> Vec<u8> {
    STANDARD.decode(text).expect("base64")
}

pub(crate) fn to_base64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// The Y document an update holds.
pub(crate) fn doc_of(update: &[u8]) -> Doc {
    with_update(&new_doc(), update, &ROOMY).expect("the update of a vector is taken")
}

/// A body as JSON, `Null` for none: the form the vectors give what an editor reads.
pub(crate) fn json_of(body: Option<&Body>) -> Value {
    body.map_or(Value::Null, |body| body.document().clone())
}

/// The body of a JSON document that is one.
pub(crate) fn body_of(document: &Value) -> Body {
    Body::parse(document.clone())
        .unwrap_or_else(|error| panic!("{document}: {error}"))
        .unwrap_or_else(|| panic!("{document}: no body"))
}

/// What a client that holds `doc` sends after it made the changes of `edit` to the
/// root of its copy: the update of those changes alone, as the Y library writes them.
pub(crate) fn edited(
    doc: &Doc,
    edit: impl FnOnce(&XmlFragmentRef, &mut TransactionMut<'_>),
) -> Vec<u8> {
    let client = copy_of(doc).expect("the document can be read again");
    let before = client.transact().state_vector();
    {
        let root = client.get_or_insert_xml_fragment(FIELD);
        let mut txn = client.transact_mut();
        edit(&root, &mut txn);
    }
    client.transact().encode_diff_v1(&before)
}
