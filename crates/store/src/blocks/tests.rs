//! Unit tests of [`super`].

use super::*;

fn block(id: &str, parent_id: &str) -> Block {
    Block {
        id: id.to_owned(),
        parent_id: parent_id.to_owned(),
        ..Block::default()
    }
}

fn ids(blocks: &[Block]) -> Vec<&str> {
    parents_first(blocks)
        .into_iter()
        .map(|index| blocks[index].id.as_str())
        .collect()
}

#[test]
fn the_listed_columns_are_where_the_list_reads_them() {
    let columns: Vec<&str> = LISTED_BLOCK_COLUMNS.split(", ").map(str::trim).collect();
    let block_columns = BLOCK_COLUMNS.split(", ").count();
    assert_eq!(columns[block_columns - 1], "NULL", "the body is not read");
    assert_eq!(columns[LISTED_HAS_BODY], "body IS NOT NULL");
    assert_eq!(columns[LISTED_TASKS_TOTAL], "body_tasks_total");
    assert_eq!(columns[LISTED_TASKS_DONE], "body_tasks_done");
    assert_eq!(columns.len(), LISTED_TASKS_DONE + 1);
    assert_eq!(LISTED_LAST_COMMENT_AT, columns.len(), "read after them");
    // The text, then the first image identifier, right after the last comment's time.
    assert_eq!(
        PREVIEW_COLUMNS,
        "SUBSTR(body_text, 1, 201), body_first_image"
    );
    assert_eq!(LISTED_BODY_TEXT, LISTED_LAST_COMMENT_AT + 1);
    assert_eq!(LISTED_FIRST_IMAGE, LISTED_BODY_TEXT + 1);
}

#[test]
fn edited_at_is_the_column_after_the_block_columns() {
    assert_eq!(BLOCK_COLUMNS.split(", ").count(), EDITED_AT);
    let read_columns = [BLOCK_COLUMNS, ", ", EDITED_AT_COLUMN].concat();
    assert!(read_columns.ends_with(EDITED_AT_COLUMN));
}

#[test]
fn parents_come_before_their_children_whatever_the_given_order() {
    let blocks = [
        block("text", "card"),
        block("reply", "comment"),
        block("comment", "card"),
        block("card", ""),
        block("view", "elsewhere"),
    ];
    assert_eq!(
        ids(&blocks),
        ["card", "view", "text", "comment", "reply"],
        "what waits for nothing keeps its place; children follow their parents"
    );
}

#[test]
fn blocks_in_a_cycle_of_parents_come_last_as_given() {
    let blocks = [
        block("a", "b"),
        block("b", "a"),
        block("own", "own"),
        block("top", ""),
    ];
    assert_eq!(ids(&blocks), ["own", "top", "a", "b"]);
}
