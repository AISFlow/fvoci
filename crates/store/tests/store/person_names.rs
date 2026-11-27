//! The parts of a person's name: stored, read back, shown by, searched by, and carried
//! over from the name an account had before there were parts.

use fvoci_model::{
    Credential, NameShown, PersonName, PersonNameChange, Title, User, WorkspaceRole, new_id,
};
use fvoci_store::{Looking, Reach, Sight};

use crate::common;

fn change(name: &PersonName) -> PersonNameChange {
    PersonNameChange {
        first_name: Some(name.first_name.clone()),
        last_name: Some(name.last_name.clone()),
        latin_first_name: Some(name.latin_first_name.clone()),
        latin_last_name: Some(name.latin_last_name.clone()),
        custom_name: Some(name.custom_name.clone()),
        name_shown: Some(name.name_shown),
        title: Some(name.title),
        credentials: Some(name.credentials.clone()),
    }
}

fn kim() -> PersonName {
    PersonName {
        first_name: "영환".to_owned(),
        last_name: "김".to_owned(),
        latin_first_name: "Younghwan".to_owned(),
        latin_last_name: "Kim".to_owned(),
        custom_name: "YH".to_owned(),
        name_shown: NameShown::Name,
        title: Some(Title::Dr),
        credentials: vec![Credential::Phd, Credential::KrAttorney],
    }
}

/// Every part is stored and read back, and the account is shown by the name its
/// choice makes: on every read, and on every choice.
#[tokio::test]
async fn a_name_is_stored_whole_and_shown_by_its_choice() {
    let test = common::setup().await;
    let store = &test.store;
    let id = common::user(store, "kim").await;
    let other = common::user(store, "other").await;

    let stored = store
        .change_person_name(&id, &change(&kim()))
        .await
        .unwrap()
        .user;
    assert_eq!(stored.name, kim());
    assert_eq!(stored.display_name, "김영환");
    assert_eq!(store.get_user_by_id(&id).await.unwrap(), stored);
    // The same again changes nothing, and writes nothing.
    let again = store
        .change_person_name(&id, &change(&kim()))
        .await
        .unwrap();
    assert!(!again.changed, "the same name is no change");
    assert_eq!(again.user, stored, "not even the time");

    for (shown, want) in [
        (NameShown::LatinName, "Younghwan Kim"),
        (NameShown::CustomName, "YH"),
        (NameShown::Username, ""),
        (NameShown::Name, "김영환"),
    ] {
        let only_the_choice = PersonNameChange {
            name_shown: Some(shown),
            ..PersonNameChange::default()
        };
        let user = store
            .change_person_name(&id, &only_the_choice)
            .await
            .unwrap()
            .user;
        assert_eq!(user.display_name, want, "{shown:?}");
        assert_eq!(user.name.first_name, "영환", "the rest stays");
    }

    let emptied = PersonNameChange {
        first_name: Some(String::new()),
        last_name: Some(String::new()),
        title: Some(None),
        credentials: Some(Vec::new()),
        ..PersonNameChange::default()
    };
    let user = store.change_person_name(&id, &emptied).await.unwrap().user;
    assert_eq!(user.display_name, "Younghwan Kim", "down the list");
    assert_eq!((user.name.title, user.name.credentials.len()), (None, 0));

    let untouched = store.get_user_by_id(&other).await.unwrap();
    assert_eq!(
        (untouched.display_name.as_str(), untouched.name),
        ("", PersonName::default())
    );
    let gone = store
        .change_person_name(&new_id(), &change(&kim()))
        .await
        .expect_err("no such user");
    assert!(gone.is_not_found(), "{gone:?}");
    test.teardown().await;
}

/// A new account is shown by the name it is made with, whatever it says it is shown by.
#[tokio::test]
async fn a_new_account_is_shown_by_its_name() {
    let test = common::setup().await;
    let mut user = User {
        id: new_id(),
        username: "maker".to_owned(),
        email: "maker@example.com".to_owned(),
        display_name: "not taken over".to_owned(),
        name: PersonName {
            custom_name: "Written".to_owned(),
            ..PersonName::default()
        },
        ..User::default()
    };
    test.store.create_user(&mut user).await.unwrap();
    let read = test.store.get_user_by_id(&user.id).await.unwrap();
    assert_eq!(read.display_name, "Written");
    assert_eq!(read.name.name_shown, NameShown::Name);
    assert_eq!(read, user);
    test.teardown().await;
}

/// A person is found by the parts of their name, either way round, and by the name
/// they wrote, as by the login name.
#[tokio::test]
async fn people_are_found_by_every_part_of_their_name() {
    let test = common::setup().await;
    let store = &test.store;
    let seer = common::user(store, "seer").await;
    let workspace_id = common::workspace_of(store, &[&seer]).await;
    let kim_id = common::user(store, "kim").await;
    store
        .add_workspace_member(&workspace_id, &kim_id, WorkspaceRole::Member)
        .await
        .unwrap();
    store
        .change_person_name(&kim_id, &change(&kim()))
        .await
        .unwrap();
    let sight = Sight::InWorkspace {
        seer_id: &seer,
        workspace_id: &workspace_id,
        reach: Reach::Workspace,
        looking: Looking::AmongPeople,
    };
    for query in [
        "김영환",
        "영환",
        "younghwan KIM",
        "Kim Younghwan",
        "yh",
        "kim",
    ] {
        let found = store.search_users_in_sight(sight, query).await.unwrap();
        let ids: Vec<&str> = found.iter().map(|user| user.id.as_str()).collect();
        assert_eq!(ids, [kim_id.as_str()], "{query}");
    }
    let found = store.search_users_in_sight(sight, "lee").await.unwrap();
    assert!(found.is_empty(), "{found:?}");
    test.teardown().await;
}
