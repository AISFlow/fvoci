//! The vectors of `contracts/ydoc-vectors.json`, which the web app writes with the
//! editor's own binding: this crate reads the same JSON from every Y document, makes Y
//! documents an editor reads the same JSON from, and repairs as an editor repairs.

mod support;

use fvoci_collab::testing::{repair, repair_by, to_body};
use fvoci_collab::{from_body, from_body_by, state_of};
use fvoci_model::BodyError;
use serde_json::{Value, json};
use support::{body_of, body_vectors, doc_of, json_of, to_base64, vectors};

/// Every Y document the binding makes of a body projects to the JSON an editor reads
/// from it, and so does shared content no editor writes and every editor keeps.
#[test]
fn a_document_of_the_binding_projects_to_what_an_editor_reads() {
    for section in ["documents", "bodies", "readings"] {
        for vector in vectors(section) {
            let body = to_body(&doc_of(&vector.update))
                .unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            assert_eq!(json_of(body.as_ref()), vector.json, "{}", vector.name);
        }
    }
}

/// The Y document made of a body projects to the JSON an editor reads from the
/// binding's document of that body, also after it has travelled as an update, and is
/// no larger than the body.
#[test]
fn a_document_made_here_projects_to_what_an_editor_reads() {
    let all = body_vectors();
    assert!(all.len() > 50, "the accepted bodies and the vectors' own");
    for one in all {
        let name = &one.vector.name;
        let made = from_body(&body_of(&one.body));
        let direct = to_body(&made).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(json_of(direct.as_ref()), one.vector.json, "{name}");
        let size = |json: &Value| serde_json::to_vec(json).expect("JSON").len();
        assert!(
            size(&one.vector.json) <= size(&one.body),
            "{name}: no larger than the body it was made of"
        );
        let travelled =
            to_body(&doc_of(&state_of(&made))).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            json_of(travelled.as_ref()),
            one.vector.json,
            "{name}, as an update"
        );
    }
}

/// The same writer makes the same Y document of a body, byte for byte; left to
/// itself, every document has a writer of its own.
#[test]
fn a_document_is_made_the_same_by_the_same_writer() {
    for one in body_vectors() {
        let body = body_of(&one.body);
        assert_eq!(
            state_of(&from_body_by(&body, 1)),
            state_of(&from_body_by(&body, 1)),
            "{}",
            one.vector.name
        );
        assert_ne!(
            state_of(&from_body(&body)),
            state_of(&from_body(&body)),
            "{}: a writer of its own each time",
            one.vector.name
        );
    }
}

/// What a repair leaves of a vector that only the server's rules refuse, or that every
/// editor shortens: no editor says what is left, so these tables do.
enum Left {
    /// How many nodes it deletes, how many cells it mends, and the body it leaves.
    Body {
        deleted: usize,
        mended: usize,
        body: Value,
    },
    /// A table past a limit, which no repair brings under it.
    TableTooLarge,
}

const fn left(deleted: usize, mended: usize, body: Value) -> Left {
    Left::Body {
        deleted,
        mended,
        body,
    }
}

fn line(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

fn doc(blocks: &[Value]) -> Value {
    json!({"type": "doc", "content": blocks})
}

/// A table of rows of cells, each cell given as its text (empty: an empty paragraph),
/// a `#` before it for a header, and a rowspan after a `|`.
fn table(rows: &[&[&str]]) -> Value {
    let cell = |given: &str| {
        let (kind, given) = given
            .strip_prefix('#')
            .map_or(("tableCell", given), |rest| ("tableHeader", rest));
        let (text, rowspan) = given.split_once('|').map_or((given, None), |(text, span)| {
            (text, span.parse::<u16>().ok())
        });
        let paragraph = if text.is_empty() {
            json!({"type": "paragraph"})
        } else {
            line(text)
        };
        let mut cell = json!({"type": kind, "content": [paragraph]});
        if let Some(rowspan) = rowspan {
            cell["attrs"] = json!({"rowspan": rowspan});
        }
        cell
    };
    let rows: Vec<Value> = rows
        .iter()
        .map(|cells| json!({"type": "tableRow", "content": cells.iter().map(|one| cell(one)).collect::<Vec<_>>()}))
        .collect();
    json!({"type": "table", "content": rows})
}

/// The vectors of a section repair to what the table says, each a body that a second
/// repair leaves as it is and that another holder of the document, handed the changes
/// alone, holds the same.
fn assert_repairs_to(section: &str, table: Vec<(&str, Left)>) {
    let vectors = vectors(section);
    assert_eq!(
        vectors
            .iter()
            .map(|one| one.name.as_str())
            .collect::<Vec<_>>(),
        table.iter().map(|(name, _)| *name).collect::<Vec<_>>(),
        "the table has every vector of {section}"
    );
    for (vector, (name, left)) in vectors.iter().zip(table) {
        let doc = doc_of(&vector.update);
        assert!(to_body(&doc).is_err(), "{name}: refused as it is");
        let Left::Body {
            deleted,
            mended,
            body,
        } = left
        else {
            assert!(
                matches!(repair(&doc), Err(BodyError::TableTooLarge { .. })),
                "{name}: past a limit"
            );
            continue;
        };
        let repaired = repair(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            (repaired.deleted, repaired.mended),
            (deleted, mended),
            "{name}: deletions and mends"
        );
        assert_eq!(json_of(repaired.body.as_ref()), body, "{name}");
        let now = to_body(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(now, repaired.body, "{name}: the document is the body given");

        let again = repair(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            (again.deleted, again.mended, &again.update),
            (0, 0, &None),
            "{name}: once is enough"
        );

        let other = doc_of(&vector.update);
        let changes = repaired.update.expect("the changes as an update");
        let other = fvoci_collab::testing::with_update(&other, &changes, &support::ROOMY)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let theirs = to_body(&other).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            theirs, repaired.body,
            "{name}: another holder after the changes"
        );
    }
}

/// What the editors keep though it is no body is refused here, and a repair deletes
/// what the allowlist refuses in it and nothing else, or mends the grid of a table.
#[test]
fn what_only_the_servers_rules_refuse_is_refused_and_repaired() {
    let a = doc(&[line("a")]);
    let two_empty =
        json!({"type": "doc", "content": [{"type": "paragraph"}, {"type": "paragraph"}]});
    let null = Value::Null;
    let mut vectors = vec![
        ("a heading of level 7", left(1, 0, a.clone())),
        ("a heading whose level is a text", left(1, 0, null.clone())),
        ("a heading whose level is null", left(1, 0, null.clone())),
        ("an image without a file", left(1, 0, a.clone())),
        ("an image whose file is empty", left(1, 0, null.clone())),
        ("a file block without a file", left(1, 0, a.clone())),
        ("a file block whose file is empty", left(1, 0, null.clone())),
        // The mentions go, the paragraphs they stood in stay.
        (
            "a mention of nobody, and of somebody who is no identifier",
            left(2, 0, two_empty.clone()),
        ),
        // As the mentions: the references go, their paragraphs stay.
        (
            "a reference to nothing, and to something that is no identifier",
            left(2, 0, two_empty),
        ),
        (
            "formulas without a source and with an empty one",
            left(3, 0, a.clone()),
        ),
        // The text goes, and a document of one empty paragraph is no body.
        ("a link without a target", left(1, 0, null.clone())),
        ("a link whose value is true", left(1, 0, null.clone())),
        ("a link whose value is a text", left(1, 0, null.clone())),
        ("a link that runs a script", left(1, 0, a.clone())),
        (
            "an ordered list that starts below zero",
            left(1, 0, null.clone()),
        ),
        // The item, and with it the list it leaves without one: one deletion.
        (
            "a task item whose checked is a text",
            left(1, 0, null.clone()),
        ),
        (
            "a code block whose language has a space",
            left(1, 0, null.clone()),
        ),
        ("a text with the character U+0000", left(1, 0, null.clone())),
        (
            "a mark given twice, once under a name with a hash",
            left(1, 0, null.clone()),
        ),
        (
            "two links on one text, under names with a hash",
            left(1, 0, null),
        ),
    ];
    vectors.extend(refused_tables(a.clone()));
    vectors.extend(refused_toggles_and_callouts(a));
    assert_repairs_to("refused", vectors);
}

/// A toggle of a summary of one text and what it hides.
fn toggle(summary: &str, hidden: &[Value]) -> Value {
    json!({"type": "details", "content": [
        {"type": "detailsSummary", "content": [{"type": "text", "text": summary}]},
        {"type": "detailsContent", "content": hidden},
    ]})
}

/// The refused vectors of toggles and callouts, in their order: a callout of a tone not
/// on the list goes, as a heading of a level not on it does, and a table in a toggle is
/// mended as one in the document.
fn refused_toggles_and_callouts(a: Value) -> Vec<(&'static str, Left)> {
    vec![
        ("a callout of a tone not on the list", left(1, 0, a.clone())),
        ("a callout whose tone is a number", left(1, 0, Value::Null)),
        (
            "a callout of a tone not on the list, in a toggle",
            left(1, 0, doc(&[toggle("s", &[line("kept")])])),
        ),
        // A board view that names no view goes, as an image without its file does.
        (
            "a board view of a view that is no identifier",
            left(1, 0, a.clone()),
        ),
        ("a board view without its view", left(1, 0, a)),
        (
            "in a toggle, one adds a column while the other adds a row",
            left(
                0,
                1,
                doc(&[
                    line("before"),
                    toggle(
                        "s",
                        &[table(&[
                            &["a", "b", "new 0"],
                            &["c", "d", "new 1"],
                            &["e", "f", ""],
                        ])],
                    ),
                ]),
            ),
        ),
    ]
}

/// The refused vectors of tables, in their order: a table that does not fill its grid
/// is mended, one that has no cell goes, one past a limit is refused.
fn refused_tables(a: Value) -> Vec<(&'static str, Left)> {
    let before = line("before");
    vec![
        // A row is given what it lacks at its end, of the kind of its last cell.
        (
            "a table whose second row is a cell short",
            left(0, 1, doc(&[table(&[&["a", "b"], &["c", ""]])])),
        ),
        (
            "a row of headers a cell short",
            left(0, 1, doc(&[table(&[&["#a", "#"], &["b", "c"]])])),
        ),
        // A row with no cell is a row short of every cell: it stays, and is given them.
        (
            "a table whose second row has no cell, which no cell above covers",
            left(0, 2, doc(&[table(&[&["a", "b"], &["", ""]])])),
        ),
        // A span is narrowed to what fits.
        (
            "a cell whose rowspan reaches past the last row",
            left(0, 1, doc(&[table(&[&["a|2", "b"], &["c"]])])),
        ),
        (
            "a cell whose colspan runs into a cell that reaches down from above",
            left(0, 1, doc(&[table(&[&["a", "b|2"], &["c"]])])),
        ),
        (
            "spans that are no whole number from one up",
            left(0, 3, doc(&[table(&[&["a", "b"], &["c", "d"]])])),
        ),
        // Nothing to mend a table without a cell from: it goes.
        ("a table whose only row has no cell", left(1, 0, a)),
        ("a table of twenty-one columns", Left::TableTooLarge),
        (
            "one adds a column while the other adds a row",
            left(
                0,
                1,
                doc(&[
                    before.clone(),
                    table(&[&["a", "b", "new 0"], &["c", "d", "new 1"], &["e", "f", ""]]),
                ]),
            ),
        ),
        // What was deleted comes back empty: a grid cannot be narrower than a row.
        (
            "one deletes a column while the other adds a row",
            left(
                0,
                2,
                doc(&[
                    before.clone(),
                    table(&[&["a", ""], &["c", ""], &["e", "f"]]),
                ]),
            ),
        ),
        (
            "one joins the two cells of a column while the other deletes the lower row",
            left(0, 1, doc(&[before, table(&[&["a", "b"]])])),
        ),
    ]
}

/// A cell that fails is deleted as every editor deletes it, which leaves its row a cell
/// short, and an empty one is put in its place: the cells after it stay in their
/// columns.
#[test]
fn a_cell_that_fails_is_deleted_and_an_empty_one_put_in_its_place() {
    let after = doc(&[table(&[&["", "b"], &["c", "d"]])]);
    let second = doc(&[table(&[&["a", ""], &["c", "d"]])]);
    let numbers: Vec<String> = (0..20).map(|column| column.to_string()).collect();
    let all: Vec<&str> = numbers.iter().map(String::as_str).collect();
    let mut first = all.clone();
    first[0] = "";
    let wide = doc(&[table(&[&first, &all])]);
    assert_repairs_to(
        "shortened",
        vec![
            ("a cell with nothing in it", left(1, 1, after.clone())),
            (
                "a cell that holds a text with no paragraph",
                left(1, 1, second.clone()),
            ),
            ("a cell that holds a table", left(1, 1, second)),
            // The empty text goes too, which an editor keeps: the cell put in goes
            // between the cells it was between.
            (
                "a cell with nothing in it, after a text with nothing in it",
                left(2, 1, doc(&[table(&[&["a", "", "c"], &["d", "e", "f"]])])),
            ),
            (
                "a cell with nothing in it, in a table of twenty columns",
                left(1, 1, wide),
            ),
            (
                "two people each delete one of the two paragraphs of a cell",
                left(1, 1, after),
            ),
        ],
    );
}

/// Shared content that does not fit is refused as it is, and repaired to what is left
/// when an editor has deleted what fails. A second repair finds nothing to do, and an
/// editor that is handed the deletions holds the same.
#[test]
fn a_repair_leaves_what_an_editor_leaves() {
    for vector in vectors("repairs") {
        let name = &vector.name;
        let doc = doc_of(&vector.update);
        assert!(to_body(&doc).is_err(), "{name}: refused as it is");

        let repaired = repair(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(json_of(repaired.body.as_ref()), vector.json, "{name}");
        assert!(repaired.deleted > 0, "{name}: something was deleted");
        let now = to_body(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(now, repaired.body, "{name}: the document is the body given");

        let again = repair(&doc).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            (again.deleted, &again.update),
            (0, &None),
            "{name}: once is enough"
        );
        assert_eq!(again.body, repaired.body, "{name}: and changes nothing");

        // Another holder of the same document, handed the deletions alone.
        let other = doc_of(&vector.update);
        let deletions = repaired.update.expect("the deletions as an update");
        let other = fvoci_collab::testing::with_update(&other, &deletions, &support::ROOMY)
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        let theirs = to_body(&other).unwrap_or_else(|error| panic!("{name}: {error}"));
        assert_eq!(
            json_of(theirs.as_ref()),
            vector.json,
            "{name}: after the deletions"
        );
    }
}

/// The client the repairs of the file for the web app write as, for the same file each
/// time: past the 32 bits of an editor's, so that no vector's editor has it.
const MENDER: u64 = 1 << 32;

/// What this crate makes, for the web app's test to read with the editor's binding
/// (`apps/web/tests/editor/ydoc-vectors.test.ts`): the Y document of every accepted
/// body, and every repair's document after the repair.
fn made_here() -> Value {
    let documents: Vec<Value> = body_vectors()
        .iter()
        .map(|one| {
            let update = to_base64(&state_of(&from_body_by(&body_of(&one.body), 1)));
            json!({"name": one.vector.name, "update": update})
        })
        .collect();
    let repairs: Vec<Value> = vectors("repairs")
        .iter()
        .map(|vector| {
            let doc = doc_of(&vector.update);
            repair_by(&doc, MENDER).unwrap_or_else(|error| panic!("{}: {error}", vector.name));
            json!({"name": vector.name, "update": to_base64(&state_of(&doc))})
        })
        .collect();
    // What no editor's repair leaves: the server's, with the body it reads.
    let mended: Vec<Value> = vectors("refused")
        .into_iter()
        .chain(vectors("shortened"))
        .filter_map(|vector| {
            let doc = doc_of(&vector.update);
            let repaired = match repair_by(&doc, MENDER) {
                Ok(repaired) => repaired,
                // A table past a limit is refused, not mended.
                Err(BodyError::TableTooLarge { .. }) => return None,
                Err(error) => panic!("{}: {error}", vector.name),
            };
            let json = json_of(repaired.body.as_ref());
            Some(json!({"name": vector.name, "update": to_base64(&state_of(&doc)), "json": json}))
        })
        .collect();
    json!({
        "description": "Y documents made by crates/collab, written by its test \
            (FVOCI_WRITE_VECTORS=1 cargo test -p fvoci-collab --test vectors). documents: \
            from_body of every body contracts/ydoc-vectors.json has under documents and \
            bodies. repairs: every repair of that file after repair. The web app's test reads each \
            with the editor's binding and expects the JSON of the vector of the same name, and \
            that the editor deletes nothing more. mended: every vector of that file under refused \
            and shortened that the server repairs, after repair, with json, the body the server \
            reads from it; the web app's test expects an editor to read the same, and to delete \
            nothing.",
        "documents": documents,
        "repairs": repairs,
        "mended": mended,
    })
}

/// The file the web app's test reads is what this crate makes now. Run with
/// `FVOCI_WRITE_VECTORS=1` to write it.
#[test]
fn the_file_for_the_web_app_is_up_to_date() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/made-here.json");
    let made = format!(
        "{}\n",
        serde_json::to_string_pretty(&made_here()).expect("JSON")
    );
    if std::env::var_os("FVOCI_WRITE_VECTORS").is_some() {
        std::fs::write(&path, &made).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed == made,
        "{} is stale: run FVOCI_WRITE_VECTORS=1 cargo test -p fvoci-collab --test vectors",
        path.display()
    );
}

/// One editor's session, as the vectors recorded it from the binding: the document the
/// server opens, then every update the editor sends. Each is taken in its turn, taking
/// one twice changes nothing, one that comes before the one it builds on is refused,
/// and the end is the body an editor reads there.
#[test]
fn the_updates_of_an_editing_session_are_taken_in_order() {
    let file = support::read_json(&support::contract("ydoc-vectors.json"));
    let session = &file["session"];
    let bytes = |text: &Value| support::from_base64(text.as_str().expect("base64"));
    let updates: Vec<Vec<u8>> = session["updates"]
        .as_array()
        .expect("the updates of the session")
        .iter()
        .map(bytes)
        .collect();
    assert!(updates.len() >= 5, "an update for every edit");
    let opened = doc_of(&bytes(&session["start"]));

    let ahead = updates.last().expect("a last update");
    assert_eq!(
        fvoci_collab::testing::with_update(&opened, ahead, &support::ROOMY).map(|_| ()),
        Err(fvoci_collab::UpdateError::MissingChanges),
        "the last update first"
    );

    let mut doc = opened;
    for (index, update) in updates.iter().enumerate() {
        // What the binding sends is placed in a few steps, counted list by list.
        let steps = fvoci_collab::testing::placing_steps(&doc, update)
            .unwrap_or_else(|error| panic!("update {index}: {error}"));
        assert!(steps < 1000, "update {index}: {steps} steps");
        doc = fvoci_collab::testing::with_update(&doc, update, &support::ROOMY)
            .unwrap_or_else(|error| panic!("update {index}: {error}"));
        assert!(
            to_body(&doc).is_ok(),
            "update {index}: a body after every edit"
        );
        let again = fvoci_collab::testing::with_update(&doc, update, &support::ROOMY)
            .unwrap_or_else(|error| panic!("update {index}, again: {error}"));
        assert_eq!(
            state_of(&again),
            state_of(&doc),
            "update {index}: twice is once"
        );
    }
    let end = to_body(&doc).expect("a body at the end");
    assert_eq!(json_of(end.as_ref()), session["json"]);
}
