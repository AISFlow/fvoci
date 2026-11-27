//! The built-in template boards: listed in a language, stored once, and copied with
//! their files.

use std::time::Instant;

use fvoci_model::{Block, Container, ContainerKind, ContainerVisibility};
use serde_json::{Value, json};

use crate::common::ROOT_WS;
use crate::common::{ROOT_WORKSPACE_ID, Setup};

/// How many built-in templates there are, and in how many languages each is written.
const TEMPLATES: usize = 9;
const LANGUAGES: usize = 2;

/// The built-in templates are listed, each with blocks, and each can be duplicated into
/// a non-template board.
#[tokio::test]
async fn get_templates_returns_the_built_in_templates() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let boards = ts
        .client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>();

    assert_eq!(
        boards.len(),
        TEMPLATES,
        "every template once, in one language"
    );
    assert!(
        boards.iter().all(|board| board.is_template),
        "all are templates"
    );
    assert!(
        boards
            .iter()
            .all(|board| board.visibility == ContainerVisibility::Open),
        "all are open"
    );
    assert!(
        boards.iter().all(|board| board.template_version == 13),
        "all are at the current version (13)"
    );
    assert!(
        boards
            .iter()
            .any(|board| board.title == "Getting started with fvoci"),
        "the welcome board is present"
    );

    let first = &boards[0];
    let blocks = ts
        .client
        .get(&format!("{ROOT_WS}/boards/{}/blocks", first.id))
        .await
        .ok_json::<Vec<Block>>();
    assert!(!blocks.is_empty(), "the template has blocks");

    let duplicate = ts
        .client
        .post(
            &format!(
                "{ROOT_WS}/containers/{}/duplicate?toWorkspace={ROOT_WORKSPACE_ID}",
                first.id
            ),
            &json!({}),
        )
        .await
        .created_json::<Value>();
    let copy = &duplicate["containers"][0];
    assert_eq!(
        copy["isTemplate"],
        json!(false),
        "the copy is not a template"
    );
    assert!(
        duplicate["blocks"]
            .as_array()
            .is_some_and(|blocks| !blocks.is_empty()),
        "the copy has blocks"
    );
    ts.teardown().await;
}

/// The titles of the built-in templates listed for the client's user when the request
/// accepts `language` (says nothing when empty).
async fn titles_listed(client: &crate::common::Client, language: &str) -> Vec<String> {
    let boards = templates_accepting(client, language).await;
    boards
        .into_iter()
        .filter(|board| board.is_builtin)
        .map(|board| board.title)
        .collect()
}

/// `getTemplates` of the root workspace by `client`, with an `Accept-Language` unless
/// `language` is empty.
async fn templates_accepting(client: &crate::common::Client, language: &str) -> Vec<Container> {
    let mut request = client.request(reqwest::Method::GET, &format!("{ROOT_WS}/templates"));
    if !language.is_empty() {
        request = request.header(reqwest::header::ACCEPT_LANGUAGE, language);
    }
    crate::common::Client::send(request).await.ok_json()
}

/// The built-in templates are listed in one language: that of the user's settings;
/// where the settings leave it to the device, or name a language no template is written
/// in, the one the request accepts first; failing both, English. Each language has
/// every template.
#[tokio::test]
async fn the_templates_are_listed_in_the_language_of_the_settings_or_else_of_the_request() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let client = &ts.client;
    let english = titles_listed(client, "").await;
    let korean = titles_listed(client, "ko-KR").await;
    assert_eq!(english.len(), TEMPLATES, "{english:?}");
    assert_eq!(korean.len(), TEMPLATES, "{korean:?}");
    assert_eq!(
        english[0], "Getting started with fvoci",
        "the first in English"
    );
    assert_eq!(korean[0], "fvoci 시작하기", "the first in Korean");
    assert!(
        english.iter().all(|title| !korean.contains(title)),
        "no template is in both lists: {english:?} {korean:?}"
    );
    assert_eq!(titles_listed(client, "en").await, english, "en");
    assert_eq!(titles_listed(client, "ja").await, english, "ja");
    assert_eq!(
        titles_listed(client, "ja, en;q=0.5, ko;q=0.8").await,
        korean,
        "the heaviest of the server's languages"
    );

    client
        .patch("/users/me/settings", &json!({"language": "ko"}))
        .await
        .check_ok();
    assert_eq!(titles_listed(client, "").await, korean, "set to ko");
    assert_eq!(
        titles_listed(client, "en").await,
        korean,
        "set to ko, where English is accepted"
    );
    // The other user's settings are their own.
    assert_eq!(
        titles_listed(&ts.client2, "").await,
        english,
        "another user"
    );
    ts.teardown().await;
}

/// `GET /w/0/templates` without a session is a 401.
#[tokio::test]
async fn get_templates_without_a_session_is_401() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    ts.anonymous()
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .check_unauthorized();
    ts.teardown().await;
}

/// A server on a database that already has current templates does not import them
/// again, and starts without them when not asked for. Also measures the startup cost of
/// template initialization.
#[tokio::test]
async fn template_initialization_is_idempotent_and_measured() {
    // A server without templates starts quickly.
    let without = Instant::now();
    let plain = Setup::native().start().await;
    let plain_cost = without.elapsed();
    assert_eq!(
        ts_template_count(&plain).await,
        0,
        "no templates without initialization"
    );
    plain.teardown().await;

    // A server with templates imports them.
    let with = Instant::now();
    let ts = Setup::native().with_templates().start().await;
    let with_cost = with.elapsed();
    let first = ts_template_count(&ts).await;
    assert_eq!(
        first,
        TEMPLATES * LANGUAGES,
        "every template was imported in every language"
    );

    // Re-running initialization on the same, already-current database is a no-op.
    let done = ts.app.init_templates().await.expect("init is idempotent");
    assert!(!done, "no re-import was needed");
    assert_eq!(
        ts_template_count(&ts).await,
        first,
        "the template count is unchanged"
    );

    println!(
        "TEMPLATE_INIT_COST without={plain_cost:?} with={with_cost:?} delta={:?}",
        with_cost.saturating_sub(plain_cost)
    );
    ts.teardown().await;
}

/// The number of built-in templates that are stored, each language counted.
async fn ts_template_count(ts: &crate::common::TestServer) -> usize {
    ts.app
        .store()
        .get_builtin_templates()
        .await
        .expect("the built-in templates")
        .len()
}

/// Built-in templates among which a language has no marked welcome board are imported
/// again at the next start, so that a new account finds its first board. Once each
/// language has a marked one, nothing is imported.
#[tokio::test]
async fn templates_without_the_onboarding_mark_are_imported_again() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let marked = |boards: &[Container]| {
        boards
            .iter()
            .filter(|board| board.properties.get("onboarding") == Some(&serde_json::json!(true)))
            .map(|board| board.id.clone())
            .collect::<Vec<_>>()
    };
    let before: Vec<Container> = ts
        .client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json();
    let marked_before = marked(&before);
    assert_eq!(
        marked_before.len(),
        1,
        "one marked template after the first start"
    );
    assert!(
        !ts.app.init_templates().await.expect("the check runs"),
        "current templates are not imported again"
    );

    // As a database looks whose welcome board lost its mark.
    let unmark = fvoci_model::ContainerPatch {
        deleted_properties: vec!["onboarding".to_owned()],
        ..fvoci_model::ContainerPatch::default()
    };
    ts.system_store()
        .patch_container(&marked_before[0], &unmark, "")
        .await
        .expect("the mark is removed");

    assert!(
        ts.app.init_templates().await.expect("the import runs"),
        "templates without a marked one are imported again"
    );
    let after: Vec<Container> = ts
        .client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json();
    assert_eq!(after.len(), before.len(), "the same number of templates");
    let marked_after = marked(&after);
    assert_eq!(marked_after.len(), 1, "one marked template again");
    assert_ne!(marked_after, marked_before, "a newly imported board");
    ts.client
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .check_ok();
    ts.teardown().await;
}

/// The built-in templates are open boards that every user is offered. Could one be
/// joined, anyone would become its editor and change what every new user's first board
/// is copied from. A template is not joined, and so nobody becomes a member of a
/// built-in one.
#[tokio::test]
async fn a_built_in_template_cannot_be_joined_or_changed() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let templates = ts
        .client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>();
    let welcome = templates
        .iter()
        .find(|board| board.properties.get("onboarding") == Some(&json!(true)))
        .expect("the welcome template is built in");

    let response = ts
        .client
        .post_empty(&format!("{ROOT_WS}/containers/{}/join", welcome.id))
        .await;
    response.check_error(
        reqwest::StatusCode::FORBIDDEN,
        "a template cannot be joined",
    );

    // Without a membership there is no way to write to it.
    ts.client
        .patch(
            &format!("{ROOT_WS}/containers/{}", welcome.id),
            &serde_json::json!({"title": "taken over"}),
        )
        .await
        .check_forbidden();
    ts.client
        .delete(&format!("{ROOT_WS}/containers/{}", welcome.id))
        .await
        .check_forbidden();
    let blocks = ts
        .client
        .get(&format!("{ROOT_WS}/boards/{}/blocks", welcome.id))
        .await
        .ok_json::<Vec<fvoci_model::Block>>();
    let card = blocks
        .iter()
        .find(|block| block.block_type == fvoci_model::BlockType::Card)
        .expect("the template has cards");
    ts.client
        .patch(
            &format!("{ROOT_WS}/boards/{}/blocks/{}", welcome.id, card.id),
            &serde_json::json!({"title": "taken over"}),
        )
        .await
        .check_forbidden();
    ts.client
        .post_empty(&format!("{ROOT_WS}/containers/{}/members", welcome.id))
        .await
        .check_forbidden();

    // Reading and copying it is what it is for.
    ts.client
        .post_empty(&format!("{ROOT_WS}/containers/{}/duplicate", welcome.id))
        .await
        .check_created();
    ts.teardown().await;
}

/// The built-in template files (`assets/templates/`), in the order of their names.
fn template_files() -> Vec<Value> {
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/templates");
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&folder)
        .expect("the folder of the templates")
        .map(|entry| entry.expect("an entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let text = std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            serde_json::from_str(&text)
                .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()))
        })
        .collect()
}

/// How many cards or units of the template files have a body, and how many of their
/// property values are dates (the only values that are objects).
fn bodies_and_dates(files: &[Value]) -> (usize, usize) {
    let cards = || {
        files
            .iter()
            .flat_map(|template| template["cards"].as_array().into_iter().flatten())
    };
    let units = || {
        files
            .iter()
            .flat_map(|template| template["units"].as_array().into_iter().flatten())
    };
    let bodies = cards()
        .chain(units())
        .filter(|block| block["body"].is_object())
        .count();
    let dates = cards()
        .flat_map(|card| {
            card["fields"]["properties"]
                .as_object()
                .into_iter()
                .flat_map(serde_json::Map::values)
        })
        .filter(|value| value.is_object())
        .count();
    (bodies, dates)
}

/// The built-in templates listed in English are the English files of `assets/templates/`,
/// every one of them: one marked as the welcome board, each with its key, the cards' bodies
/// as documents whose files are files of the board, dates in the shape of 5.7, and views
/// whose card order names cards of their board.
#[tokio::test]
async fn the_built_in_templates_are_the_template_files() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    for language in ["en", "ko"] {
        the_templates_listed_are_the_files_of(&ts, language).await;
    }
    ts.teardown().await;
}

/// What the blocks of templates held between them: bodies, dates, cards that a view
/// orders, and files that a body shows.
#[derive(Default)]
struct Seen {
    bodies: usize,
    dates: usize,
    ordered: usize,
    /// Each file once, however many bodies of its board show it.
    shown: std::collections::BTreeSet<(String, String)>,
}

impl Seen {
    /// Counts the blocks of `board` in, reading each file a body shows and holding each
    /// card a view orders to the cards of the board.
    async fn read(&mut self, ts: &crate::common::TestServer, board: &Container) {
        if board.kind == ContainerKind::Course {
            let nodes = ts
                .client
                .get(&format!("{ROOT_WS}/courses/{}/units", board.id))
                .await
                .ok_json::<Vec<Value>>();
            assert!(
                nodes.iter().any(|node| node["kind"] == json!("chapter")),
                "course template has chapters"
            );
            assert!(
                nodes.iter().any(|node| node["kind"] == json!("unit")),
                "course template has units"
            );
            for node in nodes.iter().filter(|node| node["kind"] == json!("unit")) {
                let unit_id = node["id"].as_str().expect("unit id");
                let unit = ts
                    .client
                    .get(&format!("{ROOT_WS}/courses/{}/units/{unit_id}", board.id))
                    .await
                    .ok_json::<Value>();
                if unit["body"].is_object() {
                    self.bodies += 1;
                }
            }
            return;
        }
        let blocks = ts
            .client
            .get(&format!("{ROOT_WS}/boards/{}/blocks", board.id))
            .await
            .ok_json::<Vec<Value>>();
        let cards: Vec<&Value> = blocks
            .iter()
            .filter(|block| block["type"] == json!("card"))
            .map(|block| &block["id"])
            .collect();
        for block in &blocks {
            if block["body"].is_object() {
                self.bodies += 1;
                let block: Block =
                    serde_json::from_value(block.clone()).expect("a block of the contract");
                for file in block.shown_files() {
                    let place = format!("{ROOT_WS}/containers/{}/files/{file}", board.id);
                    let info = ts
                        .client
                        .get(&format!("{place}/info"))
                        .await
                        .ok_json::<Value>();
                    let read = ts.client.get(&place).await;
                    read.check_ok();
                    assert_eq!(
                        Some(read.body.len()),
                        info["size"].as_u64().and_then(|size| size.try_into().ok()),
                        "{file} is read whole"
                    );
                    self.shown.insert((board.id.clone(), file));
                }
            }
            self.dates += block["fields"]["properties"]
                .as_object()
                .into_iter()
                .flat_map(serde_json::Map::values)
                .filter(|value| value.is_object())
                .count();
            for card in block["fields"]["cardOrder"]
                .as_array()
                .into_iter()
                .flatten()
            {
                assert!(
                    cards.contains(&card),
                    "{card} in the order of {}",
                    block["title"]
                );
                self.ordered += 1;
            }
        }
    }
}

/// Holds the templates listed for a request that accepts `language` to the files
/// written in it.
async fn the_templates_listed_are_the_files_of(ts: &crate::common::TestServer, language: &str) {
    let boards = templates_accepting(&ts.client, language).await;
    let files: Vec<Value> = template_files()
        .into_iter()
        .filter(|template| template["language"] == json!(language))
        .collect();
    let mut keys: Vec<&str> = boards
        .iter()
        .filter_map(|board| board.properties.get("templateKey").and_then(Value::as_str))
        .collect();
    keys.sort_unstable();
    let mut expected: Vec<&str> = files
        .iter()
        .filter_map(|template| template["key"].as_str())
        .collect();
    expected.sort_unstable();
    assert_eq!(keys.len(), TEMPLATES, "{language}: {keys:?}");
    assert_eq!(
        keys, expected,
        "{language}: the templates of the files, each once"
    );
    assert_eq!(
        boards
            .iter()
            .filter(|board| board.properties.get("onboarding") == Some(&json!(true)))
            .count(),
        1,
        "{language}: one is marked as the welcome board"
    );

    let mut seen = Seen::default();
    for board in &boards {
        seen.read(ts, board).await;
    }
    let Seen {
        bodies,
        dates,
        ordered,
        shown,
    } = seen;
    let (files_bodies, files_dates) = bodies_and_dates(&files);
    assert!(
        files_bodies > 0 && files_dates > 0,
        "the files have bodies and dates"
    );
    assert_eq!(
        bodies, files_bodies,
        "{language}: the cards have their bodies"
    );
    assert_eq!(
        dates, files_dates,
        "{language}: the dates of the templates, in the new shape"
    );
    assert!(ordered > 0, "{language}: the views have card orders");
    let named: usize = files
        .iter()
        .map(|template| template["files"].as_array().map_or(0, Vec::len))
        .sum();
    assert!(
        named > 0,
        "{language}: the files of the templates name files"
    );
    assert_eq!(
        shown.len(),
        named,
        "{language}: every file named is shown and read"
    );
}

/// The files of the stored templates are there, each at the place of its record: files
/// that bodies show, which a purge of the files that nothing shows leaves.
async fn the_files_of_the_templates_are_kept(ts: &crate::common::TestServer) {
    let purged = ts
        .system_store()
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the purge runs");
    assert!(purged.is_empty(), "no file of a template: {purged:?}");
    let mut new_files = 0;
    for board in ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("the built-in templates")
    {
        for block in ts
            .system_store()
            .get_blocks(&fvoci_store::BlockFilter::container(&board.id))
            .await
            .expect("the blocks of a template")
        {
            for file in block.shown_files() {
                let record = ts
                    .system_store()
                    .get_file_record(&board.id, &file)
                    .await
                    .expect("a file a template shows is recorded");
                assert!(
                    ts.app
                        .files()
                        .exists(&record.path)
                        .await
                        .expect("the storage answers"),
                    "{file} of {} is stored",
                    board.title
                );
                new_files += 1;
            }
        }
    }
    assert!(new_files > 0, "the new templates show files");
}

/// On PostgreSQL an organization reads the records of the templates' files and changes
/// none of them: its purge of the files that nothing shows passes over a file of the
/// system organization's, also one that no body shows. (An SQLite database holds one
/// organization besides the system's, and its purge is the database's.)
#[tokio::test]
async fn an_organizations_purge_leaves_the_files_of_the_templates() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    if fvoci_store::testing::db_type_of_the_tests() != fvoci_store::DbType::Postgres {
        ts.teardown().await;
        return;
    }
    let template = ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("the built-in templates")
        .into_iter()
        .next()
        .expect("a built-in template");
    // A file of a template that no body shows: what a purge takes, of its own.
    let unshown = fvoci_model::FileRecord {
        id: fvoci_model::new_id(),
        container_id: template.id.clone(),
        stored_name: "unshown.png".to_owned(),
        name: "unshown.png".to_owned(),
        size: 3,
        path: format!("{}/unshown.png", template.id),
        created_at: 1,
        ..fvoci_model::FileRecord::default()
    };
    ts.system_store()
        .save_file_record(&unshown)
        .await
        .expect("the file is recorded");

    let ours = ts.app.store();
    assert!(
        matches!(ours.tenant(), fvoci_store::Tenant::Org(_)),
        "the store of an organization"
    );
    ours.get_file_record(&template.id, &unshown.stored_name)
        .await
        .expect("an organization reads the record");
    let purged = ours
        .collect_unreferenced_files(i64::MAX)
        .await
        .expect("the purge runs");
    assert!(purged.is_empty(), "nothing of the system's: {purged:?}");
    ts.system_store()
        .get_file_record(&template.id, &unshown.stored_name)
        .await
        .expect("the record is still there");
    ts.teardown().await;
}

/// Templates that are stored again take the files of the old ones with them, from the
/// storage too: the records go with the boards and their objects are doomed, which the
/// housekeeping removes.
#[tokio::test]
async fn templates_stored_again_remove_the_old_files() {
    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let old = ts
        .app
        .store()
        .get_builtin_templates()
        .await
        .expect("the built-in templates")
        .into_iter()
        .find(|board| board.properties.get("onboarding") == Some(&json!(true)))
        .expect("the marked one");
    let file = fvoci_model::FileRecord {
        id: fvoci_model::new_id(),
        container_id: old.id.clone(),
        stored_name: "picture.png".to_owned(),
        name: "picture.png".to_owned(),
        size: 3,
        path: format!("{}/picture.png", old.id),
        created_at: 1,
        ..fvoci_model::FileRecord::default()
    };
    ts.app
        .files()
        .write(&file.path, &mut &b"png"[..])
        .await
        .expect("the file is stored");
    ts.system_store()
        .save_file_record(&file)
        .await
        .expect("the file is recorded");
    // As a database looks whose templates are of an older version.
    let unmark = fvoci_model::ContainerPatch {
        deleted_properties: vec!["onboarding".to_owned()],
        ..fvoci_model::ContainerPatch::default()
    };
    ts.system_store()
        .patch_container(&old.id, &unmark, "")
        .await
        .expect("the mark is removed");

    assert!(
        ts.app
            .init_templates()
            .await
            .expect("the templates are stored"),
        "they are stored again"
    );
    // The old file is doomed with its record, and the housekeeping removes it.
    ts.app
        .housekeeping()
        .remove_doomed_objects(fvoci_server::auth::now_millis())
        .await
        .expect("the doomed objects are removed");
    assert!(
        !ts.app
            .files()
            .exists(&file.path)
            .await
            .expect("the storage answers"),
        "the old template's file is gone"
    );
    the_files_of_the_templates_are_kept(&ts).await;
    ts.teardown().await;
}

/// Templates whose files cannot be written are not stored: no template is offered
/// without its file. What was being written is given up and left to the housekeeping,
/// and the next attempt stores them.
#[cfg(unix)]
#[tokio::test]
async fn templates_whose_files_cannot_be_written_are_not_stored() {
    use std::os::unix::fs::PermissionsExt as _;

    use fvoci_store::LooseState;

    let ts = Setup::native().start().await;
    let directory = std::path::Path::new(&ts.app.config().file_storage_path);
    std::fs::create_dir_all(directory).expect("the test's directory exists");
    let permissions = std::fs::metadata(directory)
        .expect("the original permissions")
        .permissions();
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o555))
        .expect("the test removes write permission");
    let stored = ts.app.init_templates().await;
    std::fs::set_permissions(directory, permissions).expect("restore before cleanup");

    assert!(
        stored.is_err(),
        "run as an unprivileged user so the write fails"
    );
    assert_eq!(ts_template_count(&ts).await, 0, "no template is stored");
    let left = ts
        .system_store()
        .loose_objects()
        .await
        .expect("the listed objects");
    assert!(!left.is_empty(), "the place of the failed write is listed");
    assert!(
        left.iter().all(|object| object.state == LooseState::Doomed),
        "nothing is left as being written: {left:?}"
    );

    assert!(
        ts.app
            .init_templates()
            .await
            .expect("the templates are stored"),
        "with a storage that takes them, they are stored"
    );
    assert_eq!(ts_template_count(&ts).await, TEMPLATES * LANGUAGES);
    ts.teardown().await;
}

/// A guest role follows the workspace, not the account shared by its memberships.
#[tokio::test]
async fn a_guest_uses_templates_in_the_workspace_where_they_are_a_member() {
    use crate::common::TEST_WORKSPACE_ID;

    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let user = ts.client2.me().await;
    ts.client
        .put(
            &format!("{ROOT_WS}/members/{}", user.id),
            &json!({"role": "guest"}),
        )
        .await
        .check_ok();
    ts.client2
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .check_not_found();
    let elsewhere = format!("/w/{TEST_WORKSPACE_ID}");
    let templates: Vec<Container> = ts
        .client2
        .get(&format!("{elsewhere}/templates"))
        .await
        .ok_json();
    assert!(
        !templates.is_empty(),
        "a member can still use templates elsewhere"
    );
    let board = &templates[0].id;
    for path in [
        format!("{ROOT_WS}/containers/{board}"),
        format!("{ROOT_WS}/boards/{board}/blocks"),
        format!("{ROOT_WS}/boards/{board}/cards"),
        format!("{ROOT_WS}/containers/{board}/members"),
    ] {
        ts.client2.get(&path).await.check_not_found();
    }
    ts.client2
        .post(
            &format!("{ROOT_WS}/containers/{board}/duplicate"),
            &json!({}),
        )
        .await
        .check_not_found();
    ts.client2
        .get(&format!("{elsewhere}/boards/{board}/blocks"))
        .await
        .check_ok();
    ts.client2
        .post(
            &format!("{elsewhere}/containers/{board}/duplicate"),
            &json!({}),
        )
        .await
        .check_created();

    // An invited guest on a workspace-owned template still reads its blocks (200).
    let invited_template: Container = ts
        .client
        .post(
            &format!("{ROOT_WS}/boards"),
            &json!({
                "workspaceId": crate::common::ROOT_WORKSPACE_ID,
                "visibility": "private",
                "title": "Guest Invited Template",
                "isTemplate": true,
            }),
        )
        .await
        .created_json();
    ts.client
        .post(
            &format!("{ROOT_WS}/containers/{}/members", invited_template.id),
            &json!({"userId": user.id, "role": "viewer"}),
        )
        .await
        .check_ok();
    ts.client2
        .get(&format!("{ROOT_WS}/boards/{}/blocks", invited_template.id))
        .await
        .check_ok();
    ts.teardown().await;
}

/// The cards of a board, by title.
async fn cards_by_title(
    client: &crate::common::Client,
    workspace: &str,
    board_id: &str,
) -> std::collections::BTreeMap<String, Block> {
    client
        .get(&format!("{workspace}/boards/{board_id}/blocks?type=card"))
        .await
        .ok_json::<Vec<Block>>()
        .into_iter()
        .map(|card| (card.title.clone(), card))
        .collect()
}

/// A board made from a built-in template has files of its own, with the bytes of the
/// template's: the picture and the document of the welcome board are copied from the
/// system's organization into the user's. What its cards name, by a card relation and
/// in their bodies, are the cards of the copy.
#[tokio::test]
async fn a_board_made_from_a_template_has_its_own_files_and_names_its_own_cards() {
    use crate::common::TEST_WS;

    let ts = Setup::native()
        .with_templates()
        .start()
        .await
        .init_basic()
        .await;
    let templates = ts
        .client
        .get(&format!("{TEST_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>();
    let welcome = templates
        .iter()
        .find(|board| board.properties.get("onboarding") == Some(&json!(true)))
        .expect("the welcome template is built in");
    let of_template = cards_by_title(&ts.client, TEST_WS, &welcome.id).await;

    let made = ts
        .client
        .post_empty(&format!("{TEST_WS}/onboard"))
        .await
        .ok_json::<Value>();
    let board_id = made["boardId"].as_str().expect("a board id");
    let cards = cards_by_title(&ts.client, TEST_WS, board_id).await;
    assert_eq!(
        cards.keys().collect::<Vec<_>>(),
        of_template.keys().collect::<Vec<_>>(),
        "the cards of the template"
    );
    let ids: Vec<&str> = cards.values().map(|card| card.id.as_str()).collect();

    // The files: the document of one card, the pictures of another.
    let folder = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/templates/files");
    for (title, assets) in [
        (
            "Attach files and open them right here",
            vec!["viewer-sample.en.docx"],
        ),
        (
            "See the same cards five ways",
            vec!["board.png", "timeline.png"],
        ),
    ] {
        let files = cards[title].shown_files();
        assert_eq!(files.len(), assets.len(), "the files of {title}");
        assert!(
            files
                .iter()
                .all(|file| !of_template[title].shown_files().contains(file)),
            "{title} shows files of its own, not the template's"
        );
        for (file, asset) in files.iter().zip(assets) {
            let response = ts
                .client
                .get(&format!("{TEST_WS}/containers/{board_id}/files/{file}"))
                .await;
            response.check_ok();
            let expected = std::fs::read(folder.join(asset)).expect("the asset is in the folder");
            assert_eq!(response.body, expected, "the bytes of {asset}");
        }
    }

    // A card relation names cards of the copy, and so does a body.
    let relation = welcome
        .card_properties
        .iter()
        .find(|property| property.property_type == fvoci_model::PropertyType::Relation)
        .map(|property| property.id.as_str())
        .expect("the welcome board has a card relation");
    let arranging = &cards["Filter, sort and group"];
    let links = arranging.fields["properties"][relation]
        .as_array()
        .expect("a relation value");
    let named: Vec<&str> = links
        .iter()
        .filter_map(|item| {
            assert_eq!(item.get("kind"), Some(&json!("fs")));
            assert_eq!(item.get("lagDays"), Some(&json!(0)));
            item.get("card").and_then(Value::as_str)
        })
        .collect();
    assert_eq!(named.len(), 2, "the two cards it comes after");
    assert!(
        named.iter().all(|id| ids.contains(id)),
        "the relation names cards of the copy: {named:?}"
    );
    let writing = serde_json::to_value(&cards["Write in the body"].body).expect("a body");
    let mut referred = Vec::new();
    references(&writing, &mut referred);
    assert_eq!(
        referred,
        [cards["Open a card"].id.as_str()],
        "the body names the copy of the card it named"
    );
    ts.teardown().await;
}

/// The identifiers that the references of a body name, in the order they are written in.
fn references<'b>(node: &'b Value, named: &mut Vec<&'b str>) {
    if node["type"] == json!("reference") {
        named.extend(node["attrs"]["id"].as_str());
    }
    for child in node["content"].as_array().into_iter().flatten() {
        references(child, named);
    }
}
