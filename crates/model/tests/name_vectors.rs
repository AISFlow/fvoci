//! The shared test vectors of `contracts/name-vectors.json`: the name each case is
//! shown by is the one the case says.

use std::path::Path;

use fvoci_model::PersonName;
use serde::Deserialize;

#[derive(Deserialize)]
struct Case {
    note: String,
    name: PersonName,
    shown: String,
}

#[derive(Deserialize)]
struct Vectors {
    cases: Vec<Case>,
}

#[test]
fn every_name_is_shown_as_the_vectors_say() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../contracts/name-vectors.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("{} is read: {error}", path.display()));
    let vectors: Vectors = serde_json::from_str(&text).expect("the vectors are well formed");
    assert!(vectors.cases.len() > 10, "the vectors are found");
    for case in vectors.cases {
        assert_eq!(
            case.name.shown(),
            case.shown,
            "{}: {:?}",
            case.note,
            case.name
        );
    }
}
