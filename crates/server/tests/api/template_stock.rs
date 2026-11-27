//! A server whose file storage takes nothing when it starts: it starts all the same,
//! owes the built-in templates, and stores them once the storage takes their files.

#![cfg(unix)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use fvoci_model::{Container, OrgRole, User, WorkspaceRole, new_id};
use fvoci_server::config::Configuration;

use crate::common::{PASSWORD, ROOT_WORKSPACE_ID, ROOT_WS, Setup, TestServer, USER1_USERNAME};

/// How many built-in templates a reader is listed: each once, in one language.
const TEMPLATES: usize = 9;

/// A directory of the test `name` that exists and takes no file.
fn dead_storage(name: &str) -> PathBuf {
    let directory = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("api-tests")
        .join(format!("dead-storage-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("the directory is made");
    set_mode(&directory, READ_ONLY);
    directory
}

/// The directory's mode when it takes files, and when it takes none.
const WRITABLE: u32 = 0o755;
const READ_ONLY: u32 = 0o555;

fn set_mode(directory: &Path, mode: u32) {
    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(mode))
        .expect("the permissions of the test's directory are set");
}

/// The built-in templates the first user is listed.
async fn listed(ts: &TestServer) -> usize {
    ts.client
        .get(&format!("{ROOT_WS}/templates"))
        .await
        .ok_json::<Vec<Container>>()
        .iter()
        .filter(|board| board.is_builtin)
        .count()
}

/// Starts a server that stores the templates as it starts, on the file storage
/// `configure` names, which takes no file.
async fn started_on_dead_storage(configure: fn(&mut Configuration)) -> TestServer {
    let mut ts = Setup::native()
        .with_templates()
        .configure(configure)
        .try_start()
        .await
        .expect("run as an unprivileged user: the server starts though the write fails");
    // Seed user1 directly in the store without going through `/register` (which now
    // requests a welcome board and triggers `restock_templates_soon`), so these tests
    // control the exact moment owed templates are first requested.
    let mut user = User {
        id: new_id(),
        username: USER1_USERNAME.to_owned(),
        email: "user1@sample.com".to_owned(),
        password_hash: fvoci_server::auth::password::hash_password(PASSWORD).expect("a hash"),
        ..User::default()
    };
    ts.app
        .store()
        .create_user(&mut user)
        .await
        .expect("user1 is stored");
    if ts
        .app
        .store()
        .is_organization_member(&user.id)
        .await
        .expect("checked membership")
    {
        ts.app
            .store()
            .execute_unchecked(&format!(
                "UPDATE organization_members SET role = '{}' WHERE CAST(user_id AS TEXT) = '{}'",
                OrgRole::Admin.as_str(),
                user.id
            ))
            .await
            .expect("user1 promoted to organization admin");
    } else {
        ts.app
            .store()
            .add_organization_member(&user.id, OrgRole::Admin)
            .await
            .expect("user1 administers the organization");
    }
    if ts
        .app
        .store()
        .add_workspace_member(ROOT_WORKSPACE_ID, &user.id, WorkspaceRole::Admin)
        .await
        .is_err()
    {
        ts.app
            .store()
            .execute_unchecked(&format!(
                "UPDATE workspace_members SET role = '{}' \
                 WHERE CAST(workspace_id AS TEXT) = '{ROOT_WORKSPACE_ID}' \
                 AND CAST(user_id AS TEXT) = '{}'",
                WorkspaceRole::Admin.as_str(),
                user.id
            ))
            .await
            .expect("user1 promoted to root workspace admin");
    }
    ts.client = ts.login(USER1_USERNAME, PASSWORD).await;
    assert!(
        ts.app.owes_templates(),
        "the templates are owed (run as an unprivileged user: root writes where nobody may)"
    );
    ts
}

/// The storage is dead through the start and comes back: until then the list has no
/// built-in template and every attempt leaves them owed; then the next attempt stores
/// them. That the templates are owed does not make the server unready.
#[tokio::test]
async fn a_server_starts_without_its_file_storage_and_stores_the_templates_when_it_is_back() {
    fn configure(config: &mut Configuration) {
        config.file_storage_path = dead_storage("attempts").display().to_string();
    }
    let ts = started_on_dead_storage(configure).await;
    let storage = PathBuf::from(&ts.app.config().file_storage_path);

    assert_eq!(
        listed(&ts).await,
        0,
        "no template is listed without its files"
    );
    assert!(
        !ts.app.restock_templates().await,
        "the storage takes nothing yet"
    );
    assert!(ts.app.owes_templates(), "still owed");
    assert_eq!(listed(&ts).await, 0);

    set_mode(&storage, WRITABLE);
    ts.anonymous().get("/ready").await.check_ok();
    assert!(
        ts.app.owes_templates(),
        "ready, and the templates still owed"
    );

    assert!(
        ts.app.restock_templates().await,
        "the storage takes the files"
    );
    assert!(!ts.app.owes_templates(), "nothing is owed");
    assert_eq!(listed(&ts).await, TEMPLATES);
    assert!(
        ts.app.restock_templates().await,
        "and nothing is done again"
    );
    ts.teardown().await;
}

/// Somebody who would have been listed the templates makes the server try: the request
/// is answered with what there is, and the templates are there soon after.
#[tokio::test]
async fn a_list_of_templates_that_are_owed_has_them_stored_in_the_background() {
    fn configure(config: &mut Configuration) {
        config.file_storage_path = dead_storage("listed").display().to_string();
    }
    let ts = started_on_dead_storage(configure).await;
    set_mode(Path::new(&ts.app.config().file_storage_path), WRITABLE);

    let mut templates = listed(&ts).await;
    for _ in 0..100 {
        if templates == TEMPLATES {
            break;
        }
        assert_eq!(templates, 0, "none of them or all of them");
        tokio::time::sleep(Duration::from_millis(100)).await;
        templates = listed(&ts).await;
    }
    assert_eq!(
        templates, TEMPLATES,
        "stored within ten seconds of the list"
    );
    assert!(!ts.app.owes_templates(), "nothing is owed");
    ts.teardown().await;
}

/// A new account's first board is asked for while the templates are owed: the request
/// stores them itself, since it has nothing to copy otherwise.
#[tokio::test]
async fn a_first_board_asked_for_while_the_templates_are_owed_stores_them() {
    fn configure(config: &mut Configuration) {
        config.file_storage_path = dead_storage("onboard").display().to_string();
    }
    let ts = started_on_dead_storage(configure).await;
    ts.client
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .check_status(reqwest::StatusCode::INTERNAL_SERVER_ERROR);
    assert!(ts.app.owes_templates(), "still owed");

    set_mode(Path::new(&ts.app.config().file_storage_path), WRITABLE);
    ts.client
        .post_empty(&format!("{ROOT_WS}/onboard"))
        .await
        .check_ok();
    assert!(!ts.app.owes_templates(), "nothing is owed");
    assert_eq!(listed(&ts).await, TEMPLATES);
    ts.teardown().await;
}

/// Somebody a link lets in while the templates are owed is let in without a welcome
/// board, and makes the server try: the templates are there soon after.
#[tokio::test]
async fn a_member_let_in_while_the_templates_are_owed_has_them_stored_in_the_background() {
    fn configure(config: &mut Configuration) {
        config.file_storage_path = dead_storage("joined").display().to_string();
    }
    let ts = started_on_dead_storage(configure).await;
    set_mode(Path::new(&ts.app.config().file_storage_path), WRITABLE);

    let request = serde_json::json!({
        "token": ts.member_link().await,
        "account": {"username": "late", "email": "late@sample.com", "password": PASSWORD},
    });
    ts.anonymous()
        .post("/invitations/accept", &request)
        .await
        .check_ok();
    for _ in 0..100 {
        if !ts.app.owes_templates() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(!ts.app.owes_templates(), "stored soon after");
    let late = ts.login("late", PASSWORD).await;
    let boards = ts
        .app
        .store()
        .texts_unchecked(&format!(
            "SELECT title FROM containers WHERE CAST(created_by AS TEXT) = '{}'",
            late.me().await.id
        ))
        .await
        .expect("the boards are read");
    assert_eq!(boards, Vec::<String>::new(), "let in without one");
    ts.teardown().await;
}
