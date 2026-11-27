//! The built-in templates replaced: what goes and what is handed back, once however many
//! replace them at once.

use crate::boards::new_board;
use crate::common;
use fvoci_model::{Container, ContainerVisibility, FileRecord, new_id};

/// Replacing the built-in templates is decided, done and handed back in one go: it keeps
/// the templates when `needed` says so, and otherwise removes them with their file
/// records, which it returns so that the files can go too, and stores the new ones.
#[tokio::test]
async fn replacing_the_built_in_templates_hands_back_the_old_files() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let template = |title: &str| Container {
        is_template: true,
        title: title.to_owned(),
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    let mut old = template("old");
    test.insert_container(&mut old, "")
        .await
        .expect("the old template is stored");
    let file = FileRecord {
        id: new_id(),
        container_id: old.id.clone(),
        stored_name: "picture.png".to_owned(),
        name: "picture.png".to_owned(),
        size: 1,
        path: format!("{}/picture.png", old.id),
        created_at: 1,
        ..FileRecord::default()
    };
    test.save_file_record(&file)
        .await
        .expect("the template's file is recorded");

    let mut new = fvoci_model::ContainersAndBlocks {
        containers: vec![template("new")],
        blocks: Vec::new(),
    };
    let kept = test
        .replace_builtin_templates(&mut new, &[], <[Container]>::is_empty)
        .await
        .expect("the check runs");
    assert_eq!(kept, None, "the stored templates are kept when not needed");

    let files = test
        .replace_builtin_templates(&mut new, &[], |stored| {
            stored.iter().any(|board| board.title == "old")
        })
        .await
        .expect("the templates are replaced")
        .expect("they were replaced");
    assert_eq!(
        files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        [file.path.as_str()],
        "the old template's file is handed back"
    );
    let titles: Vec<String> = test
        .get_builtin_templates()
        .await
        .expect("the built-in templates are read")
        .into_iter()
        .map(|board| board.title)
        .collect();
    assert_eq!(titles, ["new"], "the new templates are stored, built in");
    let error = test
        .get_file_record(&old.id, &file.stored_name)
        .await
        .expect_err("the record went with its board");
    assert!(error.is_not_found(), "got {error:?}");

    test.teardown().await;
}

/// Two replacements of the built-in templates that start at once, each finding none,
/// store them once between them: the second decides after the first has committed. On
/// PostgreSQL that is the advisory lock, on SQLite the write transaction.
#[tokio::test]
async fn replacements_of_the_built_in_templates_at_once_store_them_once() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let templates = || fvoci_model::ContainersAndBlocks {
        containers: vec![Container {
            is_template: true,
            ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
        }],
        blocks: Vec::new(),
    };
    let (mut first, mut second) = (templates(), templates());
    let (first, second) = tokio::join!(
        test.replace_builtin_templates(&mut first, &[], <[Container]>::is_empty),
        test.replace_builtin_templates(&mut second, &[], <[Container]>::is_empty),
    );
    let replaced = [first, second]
        .into_iter()
        .map(|result| result.expect("a replacement runs").is_some())
        .filter(|&replaced| replaced)
        .count();
    assert_eq!(replaced, 1, "one of them stores the templates");
    assert_eq!(
        test.get_builtin_templates()
            .await
            .expect("the built-in templates are read")
            .len(),
        1,
        "and they are there once"
    );
    test.teardown().await;
}

/// The files of the new templates are recorded in the transaction that stores them:
/// each has to be marked as being written, and its mark goes as it is recorded. With a
/// mark missing nothing is stored at all, and the stored templates stay.
#[tokio::test]
async fn the_files_of_new_templates_are_recorded_with_them_or_nothing_is_stored() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let template = |title: &str| Container {
        is_template: true,
        title: title.to_owned(),
        ..new_board(&new_id(), &workspace_id, ContainerVisibility::Open)
    };
    let file_of = |board: &Container, name: &str| FileRecord {
        id: new_id(),
        container_id: board.id.clone(),
        stored_name: name.to_owned(),
        name: name.to_owned(),
        size: 1,
        path: format!("boards/{}/{name}", board.id),
        created_at: 1,
        ..FileRecord::default()
    };

    let first = template("first");
    let marked = file_of(&first, "marked.png");
    test.mark_writing(&marked.path, i64::MAX)
        .await
        .expect("the file is marked as being written");
    let mut templates = fvoci_model::ContainersAndBlocks {
        containers: vec![first],
        blocks: Vec::new(),
    };
    test.replace_builtin_templates(
        &mut templates,
        std::slice::from_ref(&marked),
        <[Container]>::is_empty,
    )
    .await
    .expect("the templates are stored")
    .expect("there were none");
    let recorded = test
        .get_file_record(&marked.container_id, &marked.stored_name)
        .await
        .expect("the file is recorded with its board");
    assert_eq!(recorded.path, marked.path, "at its place");
    assert!(
        test.loose_objects()
            .await
            .expect("the loose objects are read")
            .is_empty(),
        "and its mark is gone"
    );

    // A file whose mark is gone, as when the housekeeping took it for a write that
    // stopped: the replacement fails as a whole.
    let second = template("second");
    let unmarked = file_of(&second, "unmarked.png");
    let mut templates = fvoci_model::ContainersAndBlocks {
        containers: vec![second],
        blocks: Vec::new(),
    };
    let error = test
        .replace_builtin_templates(&mut templates, std::slice::from_ref(&unmarked), |_| true)
        .await
        .expect_err("a file that is not marked is not recorded");
    assert!(error.is_not_found(), "got {error:?}");
    let titles: Vec<String> = test
        .get_builtin_templates()
        .await
        .expect("the built-in templates are read")
        .into_iter()
        .map(|board| board.title)
        .collect();
    assert_eq!(titles, ["first"], "the stored templates stay");
    test.get_file_record(&marked.container_id, &marked.stored_name)
        .await
        .expect("and so does the record of their file");
    assert!(
        test.loose_objects()
            .await
            .expect("the loose objects are read")
            .is_empty(),
        "nothing is doomed"
    );
    test.teardown().await;
}
