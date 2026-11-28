//! Where a board that comes back from the trash is filed.

use fvoci_model::{Category, ContainerMember, ContainerVisibility, new_id};
use fvoci_store::Store;

use crate::boards::new_board;
use crate::common;

/// Stores a category of the user in the workspace and returns its identifier; a system one is
/// the default category, by its name.
async fn stored_category(
    test: &Store,
    user_id: &str,
    workspace_id: &str,
    category_type: fvoci_model::CategoryType,
) -> String {
    let name = match category_type {
        fvoci_model::CategoryType::System => fvoci_model::DEFAULT_CATEGORY_NAME,
        fvoci_model::CategoryType::Custom => "a category",
    };
    let category = Category {
        id: new_id(),
        name: name.to_owned(),
        user_id: user_id.to_owned(),
        workspace_id: workspace_id.to_owned(),
        category_type,
        ..Category::default()
    };
    test.create_category(&category)
        .await
        .expect("the category is stored");
    category.id
}

/// Files the boards under the category, in this order.
async fn filed_in_order(test: &Store, user_id: &str, category_id: &str, board_ids: &[String]) {
    test.add_update_category_container(user_id, category_id, board_ids)
        .await
        .expect("the boards are filed");
    test.reorder_category_containers(category_id, board_ids.to_vec())
        .await
        .expect("the boards are ordered");
}

/// The boards of the user's category, in their order.
async fn category_order(
    test: &Store,
    user_id: &str,
    workspace_id: &str,
    category_id: &str,
) -> Vec<String> {
    test.get_user_category_containers(user_id, workspace_id)
        .await
        .expect("the categories")
        .into_iter()
        .find(|entry| entry.category.id == category_id)
        .map_or_default(|entry| {
            entry
                .container_metadata
                .into_iter()
                .map(|board| board.container_id)
                .collect()
        })
}

/// Stores a board of the workspace made by the user and returns its identifier.
async fn stored_board_of(
    test: &Store,
    workspace_id: &str,
    user_id: &str,
    board_type: ContainerVisibility,
) -> String {
    let board_id = new_id();
    let mut board = new_board(&board_id, workspace_id, board_type);
    test.insert_container(&mut board, user_id)
        .await
        .expect("the board is inserted");
    board_id
}

/// A deleted board remembers where each user had filed it and comes back there: the same
/// category, between the boards it was between, hidden as it was; under the user's
/// default category, hidden as it was, if that category went meanwhile; nowhere for a
/// user who has no default category
/// yet. The places go with the board when the trash is emptied of it.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "three users' places, set up, deleted and restored with one board"
)]
async fn a_restored_board_is_filed_where_it_was() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let [kept, moved, unfiled] = [
        common::user(&test, "kept").await,
        common::user(&test, "moved").await,
        common::user(&test, "unfiled").await,
    ];
    // Made last, so that a tie of sort orders would put it after its neighbours.
    let before = stored_board_of(&test, &workspace_id, &kept, ContainerVisibility::Open).await;
    let after = stored_board_of(&test, &workspace_id, &kept, ContainerVisibility::Open).await;
    let board_id = stored_board_of(&test, &workspace_id, &kept, ContainerVisibility::Open).await;
    let custom = fvoci_model::CategoryType::Custom;
    let kept_category = stored_category(&test, &kept, &workspace_id, custom).await;
    let moved_category = stored_category(&test, &moved, &workspace_id, custom).await;
    let moved_default = stored_category(
        &test,
        &moved,
        &workspace_id,
        fvoci_model::CategoryType::System,
    )
    .await;
    let unfiled_category = stored_category(&test, &unfiled, &workspace_id, custom).await;
    let three = [before.clone(), board_id.clone(), after.clone()];
    filed_in_order(&test, &kept, &kept_category, &three).await;
    filed_in_order(
        &test,
        &moved,
        &moved_category,
        std::slice::from_ref(&board_id),
    )
    .await;
    filed_in_order(
        &test,
        &unfiled,
        &unfiled_category,
        std::slice::from_ref(&board_id),
    )
    .await;
    for (user_id, category_id) in [(&kept, &kept_category), (&moved, &moved_category)] {
        test.set_container_visibility(
            user_id,
            category_id,
            &board_id,
            fvoci_model::SidebarVisibility::Hidden,
        )
        .await
        .expect("the board is hidden");
    }

    test.delete_container(&board_id, &kept)
        .await
        .expect("the board is deleted");
    let gone = test
        .get_container_places(&board_id)
        .await
        .expect("the places");
    assert_eq!(gone, [], "not in any sidebar while in the trash");
    // Meanwhile the user orders what is left, and two categories go.
    filed_in_order(
        &test,
        &kept,
        &kept_category,
        &[before.clone(), after.clone()],
    )
    .await;
    for (user_id, category_id) in [(&moved, &moved_category), (&unfiled, &unfiled_category)] {
        test.delete_category(category_id, user_id, &workspace_id)
            .await
            .expect("the category is deleted");
    }
    test.undelete_container(&board_id, &kept)
        .await
        .expect("the board is restored");

    assert_eq!(
        category_order(&test, &kept, &workspace_id, &kept_category).await,
        three,
        "between the boards it was between"
    );
    let places = test
        .get_container_places(&board_id)
        .await
        .expect("the places");
    let place_of = |user_id: &str| places.iter().find(|place| place.user_id == user_id);
    assert!(
        place_of(&kept).is_some_and(|place| place.hidden),
        "{places:?}"
    );
    assert_eq!(
        place_of(&moved).map(|place| (place.category_id.as_str(), place.sort_order, place.hidden)),
        Some((moved_default.as_str(), 0, true)),
        "under the default category, hidden as it was"
    );
    assert!(
        place_of(&unfiled).is_none(),
        "nowhere without a default category"
    );

    test.delete_container(&board_id, &kept)
        .await
        .expect("deleted again");
    test.purge_trash(i64::MAX).await.expect("purged");
    assert!(
        test.get_deleted_container(&board_id).await.is_err(),
        "the board and the places kept with it are gone"
    );

    test.teardown().await;
}

/// A private board comes back in the sidebar only of those who are its members again: a
/// user who had filed it and was no member when it went keeps no place to it.
#[tokio::test]
async fn a_restored_private_board_is_filed_for_its_members_only() {
    let test = common::setup().await;
    let workspace_id = common::workspace(&test).await;
    let admin = common::user(&test, "admin").await;
    let former = common::user(&test, "former").await;
    let board_id =
        stored_board_of(&test, &workspace_id, &admin, ContainerVisibility::Private).await;
    test.save_member(&ContainerMember {
        container_id: board_id.clone(),
        user_id: admin.clone(),
        role: fvoci_model::ContainerRole::Admin,
        synthetic: false,
        inherited_role: None,
    })
    .await
    .expect("the admin is a member");
    for user_id in [&admin, &former] {
        let category_id = stored_category(
            &test,
            user_id,
            &workspace_id,
            fvoci_model::CategoryType::Custom,
        )
        .await;
        filed_in_order(
            &test,
            user_id,
            &category_id,
            std::slice::from_ref(&board_id),
        )
        .await;
    }

    test.delete_container(&board_id, &admin)
        .await
        .expect("the board is deleted");
    test.undelete_container(&board_id, &admin)
        .await
        .expect("the board is restored");

    let filed: Vec<String> = test
        .get_container_places(&board_id)
        .await
        .expect("the places")
        .into_iter()
        .map(|place| place.user_id)
        .collect();
    assert_eq!(filed, [admin], "only the member");

    test.teardown().await;
}
