//! Integration tests for workspace creation, lookup, listing, and settings updates.

use fvoci_model::{JsonMap, Workspace, new_id};
use fvoci_store::StoreError;
use serde_json::json;

use crate::common;

fn settings(value: serde_json::Value) -> JsonMap {
    serde_json::from_value(value).expect("the settings are a JSON object")
}

/// Fetching a missing workspace returns not-found; fetching a stored workspace returns it.
#[tokio::test]
async fn get_workspace() {
    let test = common::setup().await;
    let store = &test.store;

    // Nonexistent workspace IDs.
    let error = store.get_workspace(&new_id()).await.unwrap_err();
    assert!(error.is_not_found(), "got {error:?}");
    let error = store.get_workspace("nonexistent-id").await.unwrap_err();
    assert!(error.is_not_found(), "got {error:?}");

    // Valid stored workspace.
    let workspace = Workspace {
        id: new_id(),
        ..Workspace::default()
    };

    store.create_workspace(&workspace).await.unwrap();

    let got = store.get_workspace(&workspace.id).await.unwrap();
    assert_eq!(got.id, workspace.id);

    test.teardown().await;
}

/// Listing workspaces returns the organization's workspaces oldest first.
#[tokio::test]
async fn get_all_workspaces() {
    let test = common::setup().await;
    let store = &test.store;

    // Initial state: an organization starts with its first workspace.
    let got = store.get_workspaces().await.unwrap();
    let ids: Vec<&str> = got.iter().map(|workspace| workspace.id.as_str()).collect();
    assert_eq!(ids, [test.workspace_id.as_str()], "got {got:?}");

    // Insert multiple workspaces and fetch them all in creation order.
    // insert
    let workspace_count = 10;
    let mut created = vec![test.workspace_id.clone()];
    for _ in 0..workspace_count {
        let workspace = Workspace {
            id: new_id(),
            ..Workspace::default()
        };

        store.create_workspace(&workspace).await.unwrap();
        created.push(workspace.id);
    }

    let got = store.get_workspaces().await.unwrap();
    assert_eq!(got.len(), workspace_count + 1);
    let ids: Vec<String> = got.into_iter().map(|workspace| workspace.id).collect();
    assert_eq!(
        ids, created,
        "oldest first: the first is the root workspace"
    );

    test.teardown().await;
}

/// A workspace is stored whole and once, and reads back as it was stored.
#[tokio::test]
async fn a_workspace_is_stored_whole_and_once() {
    let test = common::setup().await;
    let store = &test.store;
    let creator = common::user(store, "workspace.creator").await;

    let workspace = Workspace {
        id: new_id(),
        title: "the name".to_owned(),
        settings: settings(json!({ "list": [1, 2], "nested": { "on": true } })),
        modified_by: creator.clone(),
        updated_at: 5,
        guest_invitations: false,
    };
    store.create_workspace(&workspace).await.unwrap();
    let created = store.get_workspace(&workspace.id).await.unwrap();
    assert_eq!(created.settings, workspace.settings);
    assert_eq!(created.modified_by, creator);
    assert!(created.updated_at > 5, "the store stamps the time");
    assert_eq!(created.title, "the name", "the title is the stored name");
    assert!(!created.guest_invitations, "the switch is stored as given");

    // A workspace stored without settings and on behalf of nobody reads back as such.
    let bare = Workspace {
        id: new_id(),
        ..Workspace::default()
    };
    store.create_workspace(&bare).await.unwrap();
    let got = store.get_workspace(&bare.id).await.unwrap();
    assert!(got.settings.is_empty(), "got {got:?}");
    assert_eq!(got.modified_by, "", "no user is stored as none");
    assert!(got.guest_invitations, "guests may be invited unless said");

    let all = store.get_workspaces().await.unwrap();
    let ids: Vec<&str> = all.iter().map(|workspace| workspace.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            test.workspace_id.as_str(),
            workspace.id.as_str(),
            bare.id.as_str()
        ],
        "oldest first, after the organization's first workspace"
    );

    // A workspace is created once.
    let again = Workspace {
        title: "another name".to_owned(),
        ..workspace.clone()
    };
    let error = store.create_workspace(&again).await.unwrap_err();
    assert!(matches!(error, StoreError::Database(_)), "got {error:?}");
    let got = store.get_workspace(&workspace.id).await.unwrap();
    assert_eq!(got, created, "the refused workspace changed nothing");

    // A workspace that does not exist: not found.
    let missing = store.get_workspace(&new_id()).await.unwrap_err();
    assert_eq!(missing.to_string(), "{workspace} not found");
    assert_eq!(
        store.get_workspaces().await.unwrap().len(),
        3,
        "no workspace was made"
    );

    // Non-UUID text such as "0" is rejected as a workspace identifier.
    let zero = Workspace {
        id: "0".to_owned(),
        ..Workspace::default()
    };
    let error = store.create_workspace(&zero).await.unwrap_err();
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == "0"),
        "got {error:?}"
    );

    test.teardown().await;
}
