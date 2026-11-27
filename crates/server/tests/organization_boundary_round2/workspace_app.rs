//! The workspace app is usable by a person who starts the second organization.

use fvoci_model::{Category, Container, OrgKind, UserWorkspace};
use fvoci_store::DbType;
use serde_json::json;

use super::common::{Client, PASSWORD, Setup};
use super::{db_type_of_the_tests, open};

#[tokio::test]
async fn a_second_founder_can_use_the_workspace_app() {
    if db_type_of_the_tests() != DbType::Postgres {
        return;
    }
    let server = Setup::native()
        .configure(open)
        .on_an_empty_database()
        .with_templates()
        .start()
        .await;
    let mut workspaces = Vec::new();
    for name in ["first", "second"] {
        server
            .anonymous()
            .post(
                "/register",
                &json!({
                    "username": name, "email": format!("{name}@example.test"),
                    "password": PASSWORD, "organizationName": name,
                }),
            )
            .await
            .check_created();
        let client = server.login(name, PASSWORD).await;
        let listed: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
        let kinds: Vec<OrgKind> = listed.iter().map(|listed| listed.kind).collect();
        assert_eq!(
            kinds,
            [OrgKind::Shared, OrgKind::Personal],
            "a founder sees only the workspace of their organization, and their own space"
        );
        let workspace = &listed[0].id;
        exercise_workspace(&client, workspace, name).await;
        workspaces.push((client, workspace.clone()));
    }
    for (client, _) in &workspaces {
        let own: Vec<UserWorkspace> = client.get("/users/me/workspaces").await.ok_json();
        for (_, workspace) in &workspaces {
            if workspace != &own[0].id {
                for suffix in [
                    "",
                    "/categories",
                    "/templates",
                    "/users",
                    "/trash/containers",
                ] {
                    client
                        .get(&format!("/w/{workspace}{suffix}"))
                        .await
                        .check_not_found();
                }
            }
        }
    }
    server.teardown().await;
}

async fn exercise_workspace(client: &Client, workspace: &str, name: &str) {
    let path = format!("/w/{workspace}");
    client.get(&path).await.check_ok();
    let user = client.me().await;
    let category: Category = client
        .post(
            &format!("{path}/categories"),
            &json!({
                "name": name, "userId": user.id, "workspaceId": workspace,
            }),
        )
        .await
        .created_json();
    let templates: Vec<Container> = client.get(&format!("{path}/templates")).await.ok_json();
    assert!(
        !templates.is_empty(),
        "built-in templates are offered to both founders"
    );
    let copied = client
        .post(
            &format!("{path}/containers/{}/duplicate", templates[0].id),
            &json!({}),
        )
        .await;
    copied.check_created();
    let board = copied.value()["containers"][0]["id"]
        .as_str()
        .expect("the copied board")
        .to_owned();
    client
        .patch(
            &format!("{path}/containers/{board}"),
            &json!({"title": name}),
        )
        .await
        .check_ok();
    client
        .post(
            &format!("{path}/categories/{}/containers/{board}", category.id),
            &json!({}),
        )
        .await
        .check_no_content();
    for action in ["hide", "unhide"] {
        client
            .put(
                &format!(
                    "{path}/categories/{}/containers/{board}/{action}",
                    category.id
                ),
                &json!({}),
            )
            .await
            .check_no_content();
    }
    client
        .put(
            &format!("{path}/categories/{}/container-order", category.id),
            &json!([board]),
        )
        .await
        .check_ok();
    let categories = client.get(&format!("{path}/categories")).await;
    categories.check_ok();
    assert!(
        categories.text().contains(&board),
        "the board is in its category"
    );
    let found: Vec<Container> = client
        .get(&format!("{path}/boards/search?q={name}"))
        .await
        .ok_json();
    assert_eq!(found.len(), 1, "search finds this founder's board");
    assert_eq!(found[0].id, board, "search returns the copied board");
    assert_founder_memberships(client, &user.id, &board).await;
    exercise_people_and_trash(client, &path, &user.id, &board).await;
    client
        .post(&format!("{path}/onboard"), &json!({}))
        .await
        .check_ok();
    client
        .delete(&format!("{path}/categories/{}", category.id))
        .await
        .check_no_content();
}

async fn assert_founder_memberships(client: &Client, user_id: &str, board: &str) {
    let memberships = client.get("/users/me/memberships").await;
    memberships.check_ok();
    let member_items = memberships.value();
    let member_list = member_items.as_array().expect("memberships");
    assert_eq!(
        member_list.len(),
        3,
        "founder's copied board plus shared and personal welcome boards are listed"
    );
    assert!(
        member_list.iter().any(|m| m["containerId"] == board),
        "the global membership lookup reaches the copied board in the right organization"
    );
    assert!(
        member_list.iter().all(|m| m["userId"] == user_id),
        "all returned memberships belong to this founder"
    );
}

async fn exercise_people_and_trash(client: &Client, path: &str, user_id: &str, board: &str) {
    for suffix in ["/users".to_owned(), format!("/users/{user_id}")] {
        client.get(&format!("{path}{suffix}")).await.check_ok();
    }
    client
        .post(&format!("{path}/users"), &json!([user_id]))
        .await
        .check_ok();
    client
        .get(&format!("{path}/archive/export"))
        .await
        .check_ok();
    client
        .delete(&format!("{path}/containers/{board}"))
        .await
        .check_no_content();
    let trash = client.get(&format!("{path}/trash/containers")).await;
    trash.check_ok();
    assert!(
        trash.text().contains(board),
        "the deleted board is in the trash"
    );
    client
        .post(&format!("{path}/containers/{board}/restore"), &json!({}))
        .await
        .check_ok();
}

/// Starting an empty server makes no customer organization. Competing first signups
/// create exactly one organization and one administrator in a closed instance: the two
/// are held until both have found the instance without its first user, and go on to
/// store themselves together.
#[tokio::test]
async fn the_first_signup_creates_the_organization_atomically() {
    let server = Setup::native().on_an_empty_database().start().await;
    assert!(
        server
            .app
            .store()
            .instance_organization()
            .await
            .expect("the directory is read")
            .is_none(),
        "startup creates no customer organization"
    );
    let first = server.anonymous();
    let second = server.anonymous();
    let registration = |name: &str| {
        json!({
            "username": name, "email": format!("{name}@example.test"),
            "password": PASSWORD, "organizationName": name,
        })
    };
    let (alpha, beta) = (registration("alpha"), registration("beta"));
    let (a, b) = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::join!(
            first.post("/register", &alpha),
            second.post("/register", &beta)
        )
    })
    .await
    .expect("both sign-ups finish within the timeout");
    let successes = [&a, &b]
        .into_iter()
        .filter(|answer| answer.status == reqwest::StatusCode::CREATED)
        .count();
    assert_eq!(
        successes,
        1,
        "one first signup succeeds: {} / {}",
        a.text(),
        b.text()
    );
    for answer in [&a, &b] {
        if answer.status == reqwest::StatusCode::CREATED {
            assert_eq!(
                answer.value()["isAdmin"],
                true,
                "the first user is the administrator"
            );
        } else {
            assert_eq!(
                answer.status,
                reqwest::StatusCode::FORBIDDEN,
                "the late signup needs an invitation"
            );
        }
    }
    assert_eq!(
        server
            .app
            .store()
            .system()
            .count_rows("organizations")
            .await
            .expect("organizations are counted"),
        3,
        "the system's, the first user's organization and that user's own space: the \
         sign-up that was refused left neither an organization nor a space"
    );
    assert_eq!(
        server
            .app
            .store()
            .get_registered_user_count()
            .await
            .expect("accounts are counted"),
        1,
        "the rejected signup creates no account"
    );
    server.teardown().await;
}
