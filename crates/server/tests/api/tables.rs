//! The table of a body: found by the words of its cells, read by a public link, carried
//! by an archive and by every copy as it is stored; refused when it does not fill its
//! grid or is past a limit; and mended, by the server alone, when two edits together
//! leave it so, once, also when its log is taken again.

use fvoci_collab::testing::Editor;
use fvoci_model::{ContainerVisibility, SearchHit};
use reqwest::StatusCode;
use serde_json::{Value, json};

use crate::collab::{Editing, body_of, card, editing, editing_app, joined};
use crate::common::{Setup, TEST_WORKSPACE_ID, TEST_WS};
use crate::content_search::search_with;
use crate::export::{boards_of, import};
use crate::wiki_copies::{page, wiki, write};

fn line(text: &str) -> Value {
    json!({"type": "paragraph", "content": [{"type": "text", "text": text}]})
}

/// A table of rows of cells, each cell one line of its text (an empty paragraph for an
/// empty text); a header row first if `header` is given.
fn table(header: Option<&[&str]>, rows: &[&[&str]]) -> Value {
    let row = |kind: &str, cells: &[&str]| {
        let cells: Vec<Value> = cells
            .iter()
            .map(|text| {
                let paragraph = if text.is_empty() {
                    json!({"type": "paragraph"})
                } else {
                    line(text)
                };
                json!({"type": kind, "content": [paragraph]})
            })
            .collect();
        json!({"type": "tableRow", "content": cells})
    };
    let rows: Vec<Value> = header
        .map(|cells| row("tableHeader", cells))
        .into_iter()
        .chain(rows.iter().map(|cells| row("tableCell", cells)))
        .collect();
    json!({"type": "table", "content": rows})
}

/// A body of a line, a table with a header row, a cell that spans two columns and one
/// that spans three, and a line after it.
fn with_table() -> Value {
    let mut grid = table(
        Some(&["Name", "Role", "Office"]),
        &[
            &["Ada", "Engineer", "Lisbon"],
            &["Grace", "Admiral"],
            &["Linus"],
        ],
    );
    grid["content"][2]["content"][1]["attrs"] = json!({"colspan": 2});
    grid["content"][3]["content"][0]["attrs"] = json!({"colspan": 3});
    json!({"type": "doc", "content": [line("before"), grid, line("after")]})
}

/// A search of the database finds a card and a page by a word that only a cell of
/// their tables says.
#[tokio::test]
async fn a_table_is_found_by_the_words_of_its_cells() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let body = with_table();
    let card: Value = th
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "People", "body": body}),
        )
        .await
        .created_json();
    let wiki_id = wiki(&th.client, "Handbook").await;
    let page_id = page(&th.client, &wiki_id, "", "Crew").await;
    write(&th.client, &wiki_id, &page_id, &body).await;

    for (query, kind, id) in [
        ("Admiral", "card", card["id"].as_str().expect("an id")),
        ("Engineer", "page", page_id.as_str()),
    ] {
        let hits: Vec<SearchHit> =
            search_with(&th.client, TEST_WS, &[("q", query), ("kind", kind)])
                .await
                .ok_json();
        assert_eq!(
            hits.iter().map(|hit| hit.id.as_str()).collect::<Vec<_>>(),
            [id],
            "{query} in a cell"
        );
    }
    th.teardown().await;
}

/// Whoever reads a board or a wiki by its public link reads the table as it is stored.
#[tokio::test]
async fn a_public_link_reader_reads_a_table_as_it_is_stored() {
    // Public links are a switch of the organization, on in local mode.
    let ts = Setup::local_mode().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let body = with_table();
    ts.client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card", "body": body}),
        )
        .await
        .check_status(StatusCode::CREATED);
    let wiki_id = wiki(&ts.client, "Handbook").await;
    let page_id = page(&ts.client, &wiki_id, "", "Crew").await;
    write(&ts.client, &wiki_id, &page_id, &body).await;

    let reader = ts.anonymous();
    let board_token = ts.share(&board.id).await;
    let blocks: Vec<Value> = reader
        .get(&format!(
            "/shared/{}/blocks?readToken={board_token}",
            board.id
        ))
        .await
        .ok_json();
    let shown = blocks
        .iter()
        .find(|block| block["type"] == json!("card"))
        .expect("the card");
    assert_eq!(shown["body"], body, "the card's table as it is");
    let wiki_token = ts.share(&wiki_id).await;
    let read: Value = reader
        .get(&format!(
            "/shared/{wiki_id}/pages/{page_id}?readToken={wiki_token}"
        ))
        .await
        .ok_json();
    assert_eq!(read["body"], body, "the page's table as it is");
    ts.teardown().await;
}

/// A board's archive carries its table, and the board it is brought back as has it;
/// the copy of a card, of a board and of a page each have it as it is.
#[tokio::test]
async fn an_archive_and_every_copy_keep_a_table() {
    let ts = Setup::native().start().await.init_basic().await;
    let board = ts
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let body = with_table();
    let made: Value = ts
        .client
        .post(
            &format!("{TEST_WS}/boards/{}/cards", board.id),
            &json!({"title": "Card", "body": body}),
        )
        .await
        .created_json();

    let exported = ts
        .client
        .get(&format!("{TEST_WS}/containers/{}/archive/export", board.id))
        .await;
    exported.check_ok();
    import(&ts.client, TEST_WORKSPACE_ID, &exported.body)
        .await
        .check_ok();
    let imported = boards_of(&ts)
        .await
        .into_iter()
        .find(|other| other.id != board.id)
        .expect("the imported board");
    let cards: Vec<Value> = ts
        .client
        .get(&format!(
            "{TEST_WS}/boards/{}/blocks?type=card",
            imported.id
        ))
        .await
        .ok_json();
    assert_eq!(cards[0]["body"], body, "brought back from the archive");

    let copies: Vec<Value> = ts
        .client
        .post_empty(&format!(
            "{TEST_WS}/boards/{}/blocks/{}/duplicate",
            board.id,
            made["id"].as_str().expect("an id")
        ))
        .await
        .created_json();
    let copy = copies
        .iter()
        .find(|block| block["type"] == json!("card"))
        .expect("the copy of the card");
    let stored = card(&ts.client, copy["id"].as_str().expect("an id")).await;
    assert_eq!(body_of(&stored), Some(body.clone()), "the copy of the card");

    let board_copy: Value = ts
        .client
        .post_empty(&format!("{TEST_WS}/containers/{}/duplicate", board.id))
        .await
        .created_json();
    let board_card = board_copy["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .find(|block| block["type"] == json!("card"))
        .expect("the card of the copied board");
    assert_eq!(board_card["body"], body, "the copy of the board");

    let wiki_id = wiki(&ts.client, "Handbook").await;
    let page_id = page(&ts.client, &wiki_id, "", "Crew").await;
    write(&ts.client, &wiki_id, &page_id, &body).await;
    let pages = format!("{TEST_WS}/wikis/{wiki_id}/pages");
    let copy: Value = ts
        .client
        .post(
            &format!("{pages}/{page_id}/duplicate"),
            &json!({"title": "Crew (copy)"}),
        )
        .await
        .created_json();
    let read: Value = ts
        .client
        .get(&format!("{pages}/{}", copy["id"].as_str().expect("an id")))
        .await
        .ok_json();
    assert_eq!(read["body"], body, "the copy of the page");
    ts.teardown().await;
}

/// A body written whole is stored as it is or refused, never mended: a table that does
/// not fill its grid is refused at the table, and so is one past a limit, with the
/// limit as the reason.
#[tokio::test]
async fn a_table_that_does_not_fill_its_grid_or_is_past_a_limit_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let board = th
        .create_board(TEST_WORKSPACE_ID, ContainerVisibility::Open)
        .await;
    let short = table(None, &[&["a", "b"], &["c"]]);
    let wide = table(None, &[&["x"; 21]]);
    let tall_rows: Vec<&[&str]> = vec![&["x"]; usize::from(fvoci_model::TABLE_MAX_ROWS) + 1];
    let tall = table(None, &tall_rows);
    for (grid, reason, problem_reason) in [
        (short, "spans past the table or over another", None),
        (
            wide,
            "more than 200 rows or more than 20 columns",
            Some("table_too_large"),
        ),
        (
            tall,
            "more than 200 rows or more than 20 columns",
            Some("table_too_large"),
        ),
    ] {
        let body = json!({"type": "doc", "content": [line("before"), grid]});
        let refused = th
            .client
            .post(
                &format!("{TEST_WS}/boards/{}/cards", board.id),
                &json!({"title": "Card", "body": body}),
            )
            .await;
        refused.check_status(StatusCode::BAD_REQUEST);
        assert_eq!(
            refused.refused_at(),
            "/body/content/1",
            "{}",
            refused.text()
        );
        assert!(
            refused.error_message().contains(reason),
            "{}",
            refused.text()
        );
        assert_eq!(
            refused.value()["reason"].as_str(),
            problem_reason,
            "{}",
            refused.text()
        );
    }
    th.teardown().await;
}

/// Two editors of a table that each write before they have the other's update, one a
/// column and the other a row, leave the new row a cell short: the server gives it the
/// cell, sends that to both, and the card is given the mended table.
#[tokio::test]
async fn a_column_and_a_row_added_together_are_mended_by_the_server() {
    let th = Setup::native().start().await.init_basic().await;
    let start = table(None, &[&["a", "b"], &["c", "d"]]);
    let e = editing(&th, &json!({"type": "doc", "content": [start]})).await;
    let collab = e.app.collab();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let one = Editor::join(101, &loaded.state);
    let other = Editor::join(102, &loaded.state);
    let column = one.add_table_column();
    let row = other.add_table_row(2);

    let taken = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user1, &column)
        .await
        .expect("taken");
    assert!(taken.repair.is_none(), "a column alone fills the grid");
    let taken = collab
        .take(&e.app, &e.card_id, loaded.epoch, &e.user2, &row)
        .await
        .expect("taken");
    assert!(taken.repair.is_some(), "the row is mended");
    for editor in [&one, &other] {
        let missing = collab
            .diff(&e.app, &e.card_id, loaded.epoch, &editor.state_vector())
            .await
            .expect("what it is missing");
        editor.receive(&missing);
    }
    let mended = json!({"type": "doc", "content": [
        table(None, &[&["a", "b", ""], &["c", "d", ""], &["", "", ""]]),
    ]});
    for editor in [&one, &other] {
        assert_eq!(
            editor.body().map(|body| body.document().clone()),
            Some(mended.clone()),
            "each editor holds the mended table"
        );
    }
    collab
        .write_body(&e.app, &e.card_id)
        .await
        .expect("written");
    assert_eq!(
        body_of(&card(&th.client, &e.card_id).await),
        Some(mended),
        "the card holds the mended table"
    );
    th.teardown().await;
}

/// A table at its limit of columns is mended as any other: a column and a row added
/// together leave the new row a cell short of twenty, and the server gives it the cell.
/// A column more is refused, with the limits of a body as the reason.
#[tokio::test]
async fn a_table_at_its_limit_is_mended_and_a_column_past_it_is_refused() {
    let th = Setup::native().start().await.init_basic().await;
    let start = table(None, &[&["a"; 19], &["b"; 19]]);
    let e = editing(&th, &json!({"type": "doc", "content": [start]})).await;
    let collab = e.app.collab();
    let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
    let column = Editor::join(101, &loaded.state).add_table_column();
    let row = Editor::join(102, &loaded.state).add_table_row(19);
    for (user, update) in [(&e.user1, column), (&e.user2, row)] {
        collab
            .take(&e.app, &e.card_id, loaded.epoch, user, &update)
            .await
            .expect("taken");
    }
    let widened = |text: &'static str| {
        let mut cells = vec![text; 19];
        cells.push("");
        cells
    };
    let rows = [widened("a"), widened("b"), vec![""; 20]];
    let rows: Vec<&[&str]> = rows.iter().map(Vec::as_slice).collect();
    let mended = json!({"type": "doc", "content": [table(None, &rows)]});
    let reopened = collab.open(&e.app, &e.card_id).await.expect("opened");
    let editor = Editor::join(103, &reopened.state);
    assert_eq!(
        editor.body().map(|body| body.document().clone()),
        Some(mended.clone()),
        "the row is given the cell it lacks"
    );

    let refused = collab
        .take(
            &e.app,
            &e.card_id,
            reopened.epoch,
            &e.user1,
            &editor.add_table_column(),
        )
        .await
        .expect_err("a twenty-first column");
    assert_eq!(refused.status(), 400, "{refused}");
    assert_eq!(
        refused.reason().map(fvoci_server::error::Reason::as_str),
        Some("table_too_large")
    );
    let after = collab.open(&e.app, &e.card_id).await.expect("reopened");
    assert_eq!(
        Editor::join(104, &after.state)
            .body()
            .map(|body| body.document().clone()),
        Some(mended),
        "refused 21st column leaves the document unpolluted"
    );
    th.teardown().await;
}

/// How many cells each row of the first table of a body has.
fn widths(body: Option<&Value>) -> Vec<usize> {
    body.and_then(|body| body["content"][0]["content"].as_array())
        .map(|rows| {
            rows.iter()
                .map(|row| row["content"].as_array().map_or(0, Vec::len))
                .collect()
        })
        .unwrap_or_default()
}

/// A table the server mended as it took two edits is the same table after a server
/// that was killed before it folded its log: the next takes each logged update again,
/// and the repair it makes of one is the repair logged after it, so the logged one adds
/// no cell of its own. At the limit of columns the log is taken again too, past it none.
#[tokio::test]
async fn a_mended_table_is_mended_once_when_its_log_is_taken_again() {
    let th = Setup::native().start().await.init_basic().await;
    let cases = [
        (table(None, &[&["a", "b"], &["c", "d"]]), 2),
        (table(None, &[&["a"; 19], &["b"; 19]]), 19),
    ];
    let mut wanted = Vec::new();
    for (start, columns) in cases {
        let e = editing(&th, &json!({"type": "doc", "content": [start]})).await;
        let collab = e.app.collab();
        let loaded = collab.open(&e.app, &e.card_id).await.expect("opened");
        let column = Editor::join(101, &loaded.state).add_table_column();
        let row = Editor::join(102, &loaded.state).add_table_row(columns);
        for (user, update) in [(&e.user1, column), (&e.user2, row)] {
            collab
                .take(&e.app, &e.card_id, loaded.epoch, user, &update)
                .await
                .expect("taken");
        }
        let (mended, _) = joined(collab, &e.app, &e.card_id, 103).await;
        let body = mended.body().map(|body| body.document().clone());
        assert_eq!(
            widths(body.as_ref()),
            vec![columns + 1; 3],
            "mended as it was taken: {body:?}"
        );
        wanted.push((e.card_id.clone(), body));
    }

    let th = th
        .killed_and_started_again(async |app| app.collab().abandon().await)
        .await;
    for (card_id, body) in &wanted {
        let e = Editing {
            card_id: card_id.clone(),
            ..editing_app(&th).await
        };
        let (again, _) = joined(e.app.collab(), &e.app, card_id, 109).await;
        assert_eq!(
            &again.body().map(|body| body.document().clone()),
            body,
            "the log taken again, {card_id}"
        );
    }
    th.teardown().await;
}
