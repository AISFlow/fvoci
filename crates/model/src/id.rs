//! Identifiers.
//!
//! Every entity (workspace, board, block, user, category, session, file record) is named by a
//! version 7 UUID in its canonical text form: 36 characters, lower-case hexadecimal
//! digits with hyphens. The first 48 bits are the time the identifier was made, so new
//! identifiers land next to each other in an index.
//!
//! An identifier is a name and not a secret, and its order is not the order of
//! creation: identifiers made by one process sort in the order they were made, but two
//! processes (two servers, or one before and after a restart) order theirs by their
//! clocks. Sorting by creation uses the creation time, with the identifier to break
//! ties. Secrets (session tokens, read tokens, the tokens of invitations, stored file
//! names) do not come from here.

use uuid::Uuid;

/// Returns a new identifier.
pub fn new_id() -> String {
    Uuid::now_v7().hyphenated().to_string()
}

/// Whether `text` has the form of an identifier: a UUID in canonical text form, other
/// than the nil UUID.
///
/// Other spellings of a UUID (upper case, braces, no hyphens) are not identifiers. A
/// database that stores identifiers as text would not find a row under them, and one
/// that stores them as UUIDs would; refusing them keeps the two the same. The nil UUID
/// (all zeros) is the identifier of nothing: no row can have it, which lets the store
/// use it as the value that matches no row.
pub fn is_id(text: &str) -> bool {
    text.len() == uuid::fmt::Hyphenated::LENGTH
        && Uuid::try_parse(text).is_ok_and(|uuid| {
            !uuid.is_nil() && uuid.hyphenated().encode_lower(&mut Uuid::encode_buffer()) == text
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ids_are_version_7_uuids_in_canonical_form() {
        let id = new_id();
        assert!(is_id(&id), "id {id}");
        let uuid = Uuid::try_parse(&id).expect("an identifier is a UUID");
        assert_eq!(uuid.get_version_num(), 7, "id {id}");
    }

    #[test]
    fn ids_of_one_process_sort_in_the_order_they_were_made() {
        let ids: Vec<String> = (0..1000).map(|_| new_id()).collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(
            ids, sorted,
            "identifiers made in a row are already in order"
        );
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "and no two are the same");
    }

    #[test]
    fn only_the_canonical_form_is_an_identifier() {
        let id = "01890a5d-ac96-774b-bcce-b302099a8057";
        assert!(is_id(id), "the canonical form");
        for other in [
            "",
            "0",
            "system",
            "01890A5D-AC96-774B-BCCE-B302099A8057",
            "01890a5dac96774bbcceb302099a8057",
            "{01890a5d-ac96-774b-bcce-b302099a8057}",
            "urn:uuid:01890a5d-ac96-774b-bcce-b302099a8057",
            "01890a5d-ac96-774b-bcce-b302099a805",
            "01890a5d-ac96-774b-bcce-b302099a8057 ",
            "01890a5d-ac96-774b-bcce-b302099a805g",
            "bjqnmgfbzbjd9zrqoqjgapxfmyo",
            "00000000-0000-0000-0000-000000000000",
        ] {
            assert!(!is_id(other), "{other:?} is not an identifier");
        }
    }
}
