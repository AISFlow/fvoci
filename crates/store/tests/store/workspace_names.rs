//! The migration that makes a workspace's name its own among the workspaces its
//! organization has in use (23): names stored twice before keep the oldest workspace's,
//! and the others are told apart, whatever names are there already. And the name the
//! first user gives the organization, which is its first workspace's.

use fvoci_model::new_id;

use crate::common;

/// The organization's name is its first workspace's too, and a name is one workspace's
/// among those an organization has in use: where the organization there is was given
/// another workspace of that name before anybody signed up, the name is refused and
/// nothing is stored. No other way in makes a workspace: an invitation leads into one
/// that is there, and who starts an organization of their own starts it with its one.
#[tokio::test]
async fn the_first_user_names_the_organization_as_no_other_workspace_of_it_is_named() {
    let test = common::setup().await;
    // Stored past the organization's own way of making a workspace, which is its
    // administrators' and so nobody's before the first user.
    test.system()
        .execute_unchecked(&format!(
            "INSERT INTO workspaces (id, settings, created_at, updated_at, org_id, name) \
             SELECT '{}', '{{}}', 1, 1, org_id, 'Design' FROM workspaces WHERE id = '{}'",
            new_id(),
            test.workspace_id
        ))
        .await
        .expect("another workspace of the organization");

    let mut first = fvoci_model::User {
        id: new_id(),
        username: "first".to_owned(),
        email: "first@example.com".to_owned(),
        ..fvoci_model::User::default()
    };
    let error = test
        .sign_up(&mut first, "Design", fvoci_store::Founding::First)
        .await
        .expect_err("the name is another workspace's");
    assert!(
        matches!(
            error,
            fvoci_store::StoreError::Conflict(fvoci_store::Taken::WorkspaceName)
        ),
        "{error:?}"
    );
    assert!(
        !test.is_bootstrapped().await.expect("the flag is read"),
        "nothing was stored, nothing is decided"
    );
    let workspace_id = test
        .sign_up(&mut first, "Design studio", fvoci_store::Founding::First)
        .await
        .expect("under another name the first user starts the organization");
    assert_eq!(workspace_id, test.workspace_id);
    assert!(first.is_admin);

    test.teardown().await;
}
