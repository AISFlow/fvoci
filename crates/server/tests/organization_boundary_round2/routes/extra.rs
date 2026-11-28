//! Query-target hiding, invited-guest read boundaries, and removed pre-workspace paths.

use std::io::{Cursor, Read as _};

use fvoci_model::{
    Container, ContainerMember, ContainerVisibility, Workspace, WorkspaceRole, new_id,
};
use fvoci_store::DbType;
use reqwest::StatusCode;
use serde_json::{Value, json};

use super::super::common::{
    Client, HIDDEN_WORKSPACE_ID, ROOT_WORKSPACE_ID, ROOT_WS, Setup, UNKNOWN_ID,
};
use super::super::{answer, db_type_of_the_tests};
use super::{REACHED_BY_ITEM, WORKSPACE_ROUTES, fill_with_nothing};

pub(in super::super) fn archive_entries(bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).expect("an export is a ZIP");
    let mut entries = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).expect("an entry");
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).expect("the entry contents");
        if entry.name().expect("an entry name") == "version.json" {
            let mut header: Value = serde_json::from_slice(&bytes).expect("the archive header");
            assert!(header["date"].is_number(), "an export timestamp is present");
            header["date"] = json!(0);
            bytes = serde_json::to_vec(&header).expect("the normalized header");
        }
        entries.push((
            entry.name().expect("a UTF-8 entry name").into_owned(),
            bytes,
        ));
    }
    entries.sort();
    entries
}

pub(super) async fn compare_query_targets(client: &Client, own: &Container) {
    for (path, absent) in [(
        format!(
            "{ROOT_WS}/containers/{}/duplicate?toWorkspace={HIDDEN_WORKSPACE_ID}",
            own.id
        ),
        format!(
            "{ROOT_WS}/containers/{}/duplicate?toWorkspace={UNKNOWN_ID}",
            own.id
        ),
    )] {
        let body = json!({});
        let hidden = client.post(&path, &body).await;
        let missing = client.post(&absent, &body).await;
        assert_eq!(answer(&hidden), answer(&missing), "query target in {path}");
        hidden.check_not_found();
    }
    // For somebody of another organization the root workspace is not there either.
    let search = client
        .get(&format!("{ROOT_WS}/boards/search?q=Hidden%20query"))
        .await;
    if search.status != StatusCode::NOT_FOUND {
        let found: Vec<Container> = search.ok_json();
        assert!(
            found.is_empty(),
            "the search does not find hidden open boards"
        );
    }
}

#[tokio::test]
async fn query_targets_and_bulk_id_bodies_hide_unjoined_workspaces() {
    let ts = Setup::native().start().await.init_basic().await;
    let own = ts
        .create_board(ROOT_WORKSPACE_ID, ContainerVisibility::Private)
        .await;
    let owner = ts.stranger_in(HIDDEN_WORKSPACE_ID).await;
    let mut hidden = Container {
        id: new_id(),
        workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        title: "Hidden query board".to_owned(),
        ..Container::default()
    };
    ts.app
        .store()
        .insert_container_with_admin(&mut hidden, &owner)
        .await
        .expect("the hidden board");
    let guest = ts.add_user("query-guest").await;
    ts.app
        .store()
        .add_workspace_member(
            HIDDEN_WORKSPACE_ID,
            &guest.me().await.id,
            WorkspaceRole::Guest,
        )
        .await
        .expect("a guest actor");
    let mut actors = vec![ts.client.clone(), guest];
    if db_type_of_the_tests() == DbType::Postgres {
        let user = ts.client2.me().await.id;
        for table in ["workspace_members", "organization_members"] {
            ts.app
                .store()
                .execute_unchecked(&format!("DELETE FROM {table} WHERE user_id = '{user}'"))
                .await
                .expect("the actor leaves A");
        }
        let workspace = Workspace {
            id: new_id(),
            ..Workspace::default()
        };
        let org = ts
            .app
            .store()
            .create_organization("Query actor B", &workspace)
            .await
            .expect("B exists");
        let b = ts.app.store().for_org(&org).expect("B's scope");
        b.add_organization_member(&user, fvoci_model::OrgRole::Member)
            .await
            .expect("the actor belongs to B");
        b.add_workspace_member(&workspace.id, &user, WorkspaceRole::Member)
            .await
            .expect("the actor works in B");
        actors.push(ts.client2.clone());
    }
    for actor in actors {
        compare_query_targets(&actor, &own).await;
    }
    ts.teardown().await;
}

#[tokio::test]
async fn an_invited_workspace_guest_reads_its_board_without_gaining_editor_rights() {
    let ts = Setup::native().start().await.init_basic().await;
    let user = ts.client2.me().await.id;
    ts.app
        .store()
        .add_workspace_member(HIDDEN_WORKSPACE_ID, &user, WorkspaceRole::Guest)
        .await
        .expect("workspace guest");
    let mut board = Container {
        id: new_id(),
        workspace_id: HIDDEN_WORKSPACE_ID.to_owned(),
        visibility: ContainerVisibility::Open,
        ..Container::default()
    };
    ts.app
        .store()
        .insert_container_with_admin(&mut board, &ts.stranger_in(HIDDEN_WORKSPACE_ID).await)
        .await
        .expect("a hidden board");
    ts.client2
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/containers/{}", board.id))
        .await
        .check_not_found();
    ts.app
        .store()
        .save_member(&ContainerMember {
            container_id: board.id.clone(),
            user_id: user,
            role: fvoci_model::ContainerRole::Viewer,
            synthetic: false,
            inherited_role: None,
        })
        .await
        .expect("a viewer invitation");
    for (under, suffix) in [
        ("containers", ""),
        ("boards", "/views"),
        ("boards", "/cards"),
        ("containers", "/members"),
    ] {
        ts.client2
            .get(&format!(
                "/w/{HIDDEN_WORKSPACE_ID}/{under}/{}{suffix}",
                board.id
            ))
            .await
            .check_ok();
    }
    let boards: Vec<Container> = ts
        .client2
        .get(&format!("/w/{HIDDEN_WORKSPACE_ID}/boards"))
        .await
        .ok_json();
    assert_eq!(
        boards
            .iter()
            .map(|board| board.id.as_str())
            .collect::<Vec<_>>(),
        [board.id.as_str()],
        "the workspace list contains the invitation only"
    );
    ts.client2
        .patch(
            &format!("/w/{HIDDEN_WORKSPACE_ID}/containers/{}", board.id),
            &json!({"title": "not permitted"}),
        )
        .await
        .check_forbidden();
    ts.client2
        .post(
            &format!("/w/{HIDDEN_WORKSPACE_ID}/containers/{}/join", board.id),
            &json!({}),
        )
        .await
        .check_not_found();
    ts.teardown().await;
}

/// What was under `/api/v1` before it moved under `/w/{workspaceId}`, and what went
/// with the move (the search of every workspace, the users by identifiers of every
/// workspace): every one of them is a path that does not exist, whoever asks.
#[tokio::test]
async fn the_paths_from_before_the_workspace_are_not_there() {
    let ts = Setup::native().start().await.init_basic().await;
    // The wikis, the references and the stars came after the move, and were never
    // anywhere else.
    let gone = WORKSPACE_ROUTES
        .iter()
        .filter(|(_, suffix, _)| !suffix.starts_with("/wikis"))
        .filter(|(_, suffix, _)| !suffix.starts_with("/favorites"))
        .filter(|(_, suffix, _)| !suffix.starts_with("/courses"))
        .filter(|(_, suffix, _)| !suffix.starts_with("/references"))
        .filter(|(_, _, operation)| REACHED_BY_ITEM.contains(operation))
        .map(|(method, suffix, operation)| (*method, fill_with_nothing(suffix), *operation))
        .chain([
            ("POST", "/users".to_owned(), "getUsers"),
            ("GET", "/boards/search?q=secret".to_owned(), "searchBoards"),
        ]);
    let mut violations = Vec::new();
    let mut count = 0;
    for (method, path, operation) in gone {
        count += 1;
        for client in [&ts.client, &ts.anonymous()] {
            let response = Client::send(
                client
                    .request(method.parse().expect("an HTTP method"), &path)
                    .json(&json!({})),
            )
            .await;
            if response.status != StatusCode::NOT_FOUND
                || response.error_message() != "route not found"
            {
                violations.push(format!(
                    "{operation}: {method} {path}: {:?}",
                    answer(&response)
                ));
            }
        }
    }
    ts.teardown().await;
    // 77 since the reactions (addReaction, removeReaction) came, after the move, 78 with
    // the activity of cards (getCardActivity), and 79 with the export in Markdown
    // (exportContainerMarkdown).
    assert_eq!(
        count, 79,
        "the operations that moved or came after under the workspace, and the two that went"
    );
    assert!(violations.is_empty(), "{}", violations.join("\n"));
}
