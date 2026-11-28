//! A person's stars in a workspace (`favorites`): their order, the most a workspace
//! keeps, and what takes them away.

use fvoci_model::{FavoriteKind, MAX_FAVORITES, new_id};
use fvoci_store::{FavoriteAdded, Store, StoredFavorite};

use crate::common;

fn on(kind: FavoriteKind) -> StoredFavorite {
    StoredFavorite {
        target_id: new_id(),
        kind,
    }
}

async fn starred(store: &Store, user_id: &str, workspace_id: &str) -> Vec<String> {
    store
        .favorites_of(user_id, workspace_id)
        .await
        .expect("the stars are read")
        .into_iter()
        .map(|favorite| favorite.target_id)
        .collect()
}

#[tokio::test]
async fn stars_keep_their_order_and_a_workspace_holds_a_hundred() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "starrer").await;
    let workspace_id = test.workspace_id.clone();
    common::works_in(store, &workspace_id, &user_id).await;

    let first = on(FavoriteKind::Container);
    let second = on(FavoriteKind::Card);
    for favorite in [&first, &second] {
        assert_eq!(
            store
                .add_favorite(&workspace_id, &user_id, favorite, 1)
                .await
                .expect("put on"),
            FavoriteAdded::Added
        );
    }
    assert_eq!(
        store
            .add_favorite(&workspace_id, &user_id, &first, 2)
            .await
            .expect("asked again"),
        FavoriteAdded::AlreadyThere
    );
    assert_eq!(
        starred(store, &user_id, &workspace_id).await,
        [first.target_id.clone(), second.target_id.clone()]
    );
    store
        .reorder_favorites(
            &workspace_id,
            &user_id,
            &[second.target_id.clone(), new_id()],
        )
        .await
        .expect("put in order");
    assert_eq!(
        starred(store, &user_id, &workspace_id).await,
        [second.target_id.clone(), first.target_id.clone()]
    );

    for _ in 2..MAX_FAVORITES {
        let added = store
            .add_favorite(&workspace_id, &user_id, &on(FavoriteKind::Page), 3)
            .await
            .expect("put on");
        assert_eq!(added, FavoriteAdded::Added);
    }
    assert_eq!(
        store
            .add_favorite(&workspace_id, &user_id, &on(FavoriteKind::Unit), 4)
            .await
            .expect("asked"),
        FavoriteAdded::Full
    );
    assert_eq!(
        starred(store, &user_id, &workspace_id).await.len(),
        MAX_FAVORITES
    );

    assert!(
        store
            .remove_favorite(&workspace_id, &user_id, &first.target_id)
            .await
            .expect("taken off")
    );
    assert!(
        !store
            .remove_favorite(&workspace_id, &user_id, &first.target_id)
            .await
            .expect("asked again")
    );
    test.teardown().await;
}

/// Two stars put at once are put one after the other: the same one once, and none past
/// the most a workspace keeps. Without that, on PostgreSQL, both would find no star
/// there, and the second would fail on the first's key.
#[tokio::test]
async fn stars_put_at_once_are_put_once_and_never_past_the_most() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "hurried").await;
    let workspace_id = test.workspace_id.clone();
    common::works_in(store, &workspace_id, &user_id).await;

    let one = on(FavoriteKind::Card);
    let both = tokio::join!(
        store.add_favorite(&workspace_id, &user_id, &one, 1),
        store.add_favorite(&workspace_id, &user_id, &one, 1),
    );
    let both = (both.0.expect("put on"), both.1.expect("put on"));
    assert!(
        matches!(
            both,
            (FavoriteAdded::Added, FavoriteAdded::AlreadyThere)
                | (FavoriteAdded::AlreadyThere, FavoriteAdded::Added)
        ),
        "{both:?}"
    );
    for at in 1..MAX_FAVORITES - 1 {
        let at = i64::try_from(at).expect("a small number");
        store
            .add_favorite(&workspace_id, &user_id, &on(FavoriteKind::Page), at)
            .await
            .expect("put on");
    }
    let (last, past) = (on(FavoriteKind::Unit), on(FavoriteKind::Unit));
    let both = tokio::join!(
        store.add_favorite(&workspace_id, &user_id, &last, 200),
        store.add_favorite(&workspace_id, &user_id, &past, 200),
    );
    let both = (both.0.expect("put on"), both.1.expect("asked"));
    assert!(
        matches!(
            both,
            (FavoriteAdded::Added, FavoriteAdded::Full)
                | (FavoriteAdded::Full, FavoriteAdded::Added)
        ),
        "{both:?}"
    );
    assert_eq!(
        starred(store, &user_id, &workspace_id).await.len(),
        MAX_FAVORITES
    );
}

#[tokio::test]
async fn the_stars_go_with_the_users_leaving_the_workspace() {
    let test = common::setup().await;
    let store = &test.store;
    let admin = common::user(store, "admin").await;
    let user_id = common::user(store, "leaver").await;
    let workspace_id = test.workspace_id.clone();
    common::works_in(store, &workspace_id, &admin).await;
    common::works_in(store, &workspace_id, &user_id).await;
    store
        .add_favorite(&workspace_id, &user_id, &on(FavoriteKind::Card), 1)
        .await
        .expect("put on");
    store
        .remove_workspace_member(&workspace_id, &user_id, &admin)
        .await
        .expect("taken out");
    assert_eq!(
        starred(store, &user_id, &workspace_id).await,
        Vec::<String>::new()
    );
    test.teardown().await;
}
