use super::*;

const FIRST: &str = "01890a5d-ac96-774b-bcce-b302099a8057";
const SECOND: &str = "01890a5d-ac96-774b-bcce-b302099a8058";
const SECRET: &str = "qM4nYvE7m0kXc1D_8sWb-3uZtLrA9hGfJ2oPiK5xN6Q";
/// Another secret, before [`SECRET`] in the order of names.
const OTHER_SECRET: &str = "Zb7Qx2LmP9sT4vW1yA6cE3gH8jK0nR5uDfGhIoU2wXe";

#[test]
fn identifiers_are_numbered_in_order_of_first_appearance() {
    let mut placeholders = Placeholders::default();
    let text = format!("{FIRST} not found, see {SECOND} and {FIRST}");
    assert_eq!(
        placeholders.text(&text),
        "<id:1> not found, see <id:2> and <id:1>",
        "input {text:?}"
    );
}

#[test]
fn secrets_are_numbered_apart_from_identifiers() {
    assert_eq!(SECRET.len(), SECRET_LENGTH, "the sample is a secret");
    let mut placeholders = Placeholders::default();
    let text = format!("/files/teams/{FIRST}/{SECOND}/{SECRET}.png and {SECRET}.png");
    assert_eq!(
        placeholders.text(&text),
        "/files/teams/<id:1>/<id:2>/<secret:1>.png and <secret:1>.png",
        "input {text:?}"
    );
}

/// A stored file name as the name of a member (an archive's `fileNames`) is the
/// same secret as where it is a value; an identifier as a name stays as it is.
#[test]
fn a_secret_in_the_name_of_a_member_is_replaced_and_an_identifier_kept() {
    let mut placeholders = Placeholders::default();
    let recorded = json!({
        "fileId": format!("{SECRET}.png"),
        "fileNames": {format!("{SECRET}.png"): "pixel.png"},
        "properties": {FIRST: "a value"},
    });
    assert_eq!(
        placeholders.value(&recorded, ""),
        json!({
            "fileId": "<secret:1>.png",
            "fileNames": {"<secret:1>.png": "pixel.png"},
            "properties": {FIRST: "a value"},
        }),
        "input {recorded}"
    );
}

/// Names and values number their secrets in one count; a name numbers no identifier.
#[test]
fn names_and_values_share_the_numbering_of_secrets() {
    let recorded = json!({
        "a": {format!("{OTHER_SECRET}.png"): "x", FIRST: "y"},
        "b": FIRST,
        "fileId": format!("{SECRET}.png"),
        "fileNames": {format!("{SECRET}.png"): "pixel.png"},
    });
    assert_eq!(
        Placeholders::default().value(&recorded, ""),
        json!({
            "a": {"<secret:1>.png": "x", FIRST: "y"},
            "b": "<id:1>",
            "fileId": "<secret:2>.png",
            "fileNames": {"<secret:2>.png": "pixel.png"},
        }),
        "input {recorded}"
    );
}

/// Two stored names new to the recording are numbered by what their members hold, so
/// their numbers do not follow the order of the names, which differs from run to run.
#[test]
fn new_secrets_in_names_are_numbered_by_what_their_members_hold() {
    for (first, second) in [(SECRET, OTHER_SECRET), (OTHER_SECRET, SECRET)] {
        let recorded = json!({
            "fileNames": {format!("{first}.png"): "b.pdf", format!("{second}.png"): "a.pdf"},
            "shown": [format!("{first}.png"), format!("{second}.png")],
        });
        assert_eq!(
            Placeholders::default().value(&recorded, ""),
            json!({
                "fileNames": {"<secret:1>.png": "a.pdf", "<secret:2>.png": "b.pdf"},
                "shown": ["<secret:2>.png", "<secret:1>.png"],
            }),
            "input {recorded}"
        );
    }
}

/// Two stored names new to the recording whose members hold the same cannot be told
/// apart: either could be the first, and what names them later would be numbered by the
/// names' own order, which differs from run to run. The recording stops instead.
#[test]
#[should_panic(expected = "hold the same")]
fn new_secrets_in_names_that_hold_the_same_stop_the_recording() {
    let recorded = json!({
        "fileNames": {format!("{SECRET}.png"): "a.pdf", format!("{OTHER_SECRET}.png"): "a.pdf"},
    });
    Placeholders::default().value(&recorded, "");
}

/// Names that hold the same are told apart once their secrets are known to the recording.
#[test]
fn known_secrets_in_names_may_hold_the_same() {
    for (first, second) in [(SECRET, OTHER_SECRET), (OTHER_SECRET, SECRET)] {
        let recorded = json!({
            // Members are read in the order of their names: `answered` comes first.
            "answered": [format!("{first}.png"), format!("{second}.png")],
            "fileNames": {format!("{first}.png"): "a.pdf", format!("{second}.png"): "a.pdf"},
        });
        assert_eq!(
            Placeholders::default().value(&recorded, ""),
            json!({
                "answered": ["<secret:1>.png", "<secret:2>.png"],
                "fileNames": {"<secret:1>.png": "a.pdf", "<secret:2>.png": "a.pdf"},
            }),
            "input {recorded}"
        );
    }
}

#[test]
fn only_whole_runs_are_identifiers_or_secrets() {
    let unchanged = |text: &str| {
        assert_eq!(
            Placeholders::default().text(text),
            text,
            "{text:?} holds nothing that differs between runs"
        );
    };
    // Other spellings of a UUID, the nil UUID, and an identifier inside a longer run.
    unchanged("01890A5D-AC96-774B-BCCE-B302099A8057");
    unchanged("01890a5dac96774bbcceb302099a8057");
    unchanged("00000000-0000-0000-0000-000000000000");
    unchanged("x01890a5d-ac96-774b-bcce-b302099a8057");
    unchanged("01890a5d-ac96-774b-bcce-b302099a8057-more");
    // As long as a secret, but a word: one letter case only.
    unchanged("a_setting_with_a_very_long_snake_case_name_x");
    unchanged("unknown-board");
    assert_eq!(
        Placeholders::default().text(&format!("block ID={FIRST}.")),
        "block ID=<id:1>.",
        "an identifier ends at a character outside the run"
    );
}

#[test]
fn lists_keep_their_order() {
    let mut placeholders = Placeholders::default();
    let value = json!([
        {"id": SECOND, "title": "b", "updatedAt": 1_700_000_000_001_i64},
        {"id": FIRST, "title": "a", "token": "x"},
    ]);
    assert_eq!(
        placeholders.value(&value, ""),
        json!([
            {"id": "<id:1>", "title": "b", "updatedAt": "<ts>"},
            {"id": "<id:2>", "title": "a", "token": "<secret>"},
        ]),
        "as they came, numbered in that order"
    );
}

#[test]
fn header_names_are_spelled_one_way() {
    assert_eq!(
        canonical_header_name("x-content-type-options"),
        "X-Content-Type-Options",
        "each part is capitalized"
    );
    assert_eq!(canonical_header_name("ETag"), "Etag", "the rest is lowered");
}
