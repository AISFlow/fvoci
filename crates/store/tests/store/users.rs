//! Integration tests for user storage, lookup, password updates, and user search.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use fvoci_model::InstanceRole;
use fvoci_model::{
    ClientStatePatch, Density, Session, Theme, User, UserSettings, UserSettingsPatch,
    WorkspaceRole, new_id,
};
use fvoci_store::{AdminChange, DbType, Founding, Looking, Reach, Sight, Store, StoreError, Taken};
use serde_json::{Value, json};

use crate::common;

/// A small clock-derived count (`0..10`) for varying test user batch sizes.
fn clock_count() -> usize {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_secs();
    usize::try_from(seconds % 10).expect("a value below ten fits")
}

fn now_millis() -> i64 {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is past the epoch")
        .as_millis();
    i64::try_from(millis).expect("the current time fits in 64 bits")
}

async fn create_user(store: &Store, mut user: User) -> User {
    store
        .create_user(&mut user)
        .await
        .expect("the user is created");
    user
}

fn value_of<'p>(entries: &'p BTreeMap<String, String>, name: &str) -> Option<&'p str> {
    entries.get(name).map(String::as_str)
}

/// Listing users returns all stored users, growing as new users are created.
#[tokio::test]
async fn get_users_by_workspace() {
    let test = common::setup().await;
    let store = &test.store;

    let users = store.get_users_by_workspace().await.unwrap();
    assert_eq!(users.len(), 0);

    let user_id = new_id();

    let mut user = User {
        id: user_id.clone(),
        username: "darth.vader".to_owned(),
        ..User::default()
    };
    store.create_user(&mut user).await.unwrap();
    assert_eq!(user.id, user_id);
    assert_eq!(user.username, "darth.vader");

    let users = store.get_users_by_workspace().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].username, "darth.vader");

    test.teardown().await;
}

/// A user is created and read back.
#[tokio::test]
async fn create_and_get_user() {
    let test = common::setup().await;
    let store = &test.store;

    let mut user = User {
        id: new_id(),
        username: "damao".to_owned(),
        email: "mock@email.com".to_owned(),
        ..User::default()
    };

    // "CreateUser"
    store.create_user(&mut user).await.unwrap();

    // "GetUserByID"
    let got = store.get_user_by_id(&user.id).await.unwrap();
    assert_eq!(got.id, user.id);
    assert_eq!(got.username, user.username);
    assert_eq!(got.email, user.email);

    // "GetUserByID nonexistent"
    let error = store.get_user_by_id(&new_id()).await.unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");

    // "GetUserByUsername"
    let got = store.get_user_by_username(&user.username).await.unwrap();
    assert_eq!(got.id, user.id);
    assert_eq!(got.username, user.username);
    assert_eq!(got.email, user.email);

    // Non-UUID text passed to lookup-by-ID returns not-found, followed by lookup-by-username.
    let error = store
        .get_user_by_id("nonexistent-username")
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
    let error = store
        .get_user_by_username("nonexistent-username")
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");

    // "GetUserByEmail"
    let got = store.get_user_by_email(&user.email).await.unwrap();
    assert_eq!(got.id, user.id);
    assert_eq!(got.username, user.username);
    assert_eq!(got.email, user.email);

    // Non-UUID text passed to lookup-by-ID returns not-found, followed by lookup-by-email.
    let error = store.get_user_by_id("nonexistent-email").await.unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");
    let error = store
        .get_user_by_email("nonexistent-email")
        .await
        .unwrap_err();
    assert!(matches!(error, StoreError::NotFound(_)), "got {error:?}");

    test.teardown().await;
}

/// One table-driven case for `get_users_list`.
struct UsersListCase<'a> {
    name: &'static str,
    user_ids: [&'a str; 2],
    expected_error: bool,
    expected_ids: &'a [&'a str],
}

/// Looking up users by a list of IDs, including missing and non-UUID entries.
#[tokio::test]
async fn get_users_list() {
    let test = common::setup().await;
    let store = &test.store;

    let user1 = common::user(store, "user1-username").await;
    let user2 = common::user(store, "user2-username").await;
    let (nobody1, nobody2) = (new_id(), new_id());

    let test_cases = [
        UsersListCase {
            name: "all of the IDs are found",
            user_ids: [&user1, &user2],
            expected_error: false,
            expected_ids: &[&user1, &user2],
        },
        UsersListCase {
            name: "some of the IDs are found",
            user_ids: [&user2, &nobody1],
            expected_error: true,
            expected_ids: &[&user2],
        },
        UsersListCase {
            name: "none of the IDs are found",
            user_ids: [&nobody1, &nobody2],
            expected_error: true,
            expected_ids: &[],
        },
        // Text that is not a UUID identifier matches nobody.
        UsersListCase {
            name: "text that is not an identifier is not found",
            user_ids: [&user1, "non-existent"],
            expected_error: true,
            expected_ids: &[&user1],
        },
    ];

    for tc in test_cases {
        // The subtest named `tc.name`.
        let name = tc.name;
        let user_ids = tc.user_ids.map(str::to_owned);
        let result = store.get_users_list(&user_ids).await;
        if tc.expected_error {
            let error = result.as_ref().expect_err(name);
            assert!(error.is_not_found(), "{name}: got {error:?}");
        } else {
            assert!(result.is_ok(), "{name}: got {result:?}");
        }

        let mut found: Vec<String> = if let Ok(users) = result {
            users.into_iter().map(|user| user.id).collect()
        } else {
            // Verify individually that exactly the expected IDs exist in the store.
            let mut found = Vec::new();
            for id in &user_ids {
                if let Ok(users) = store.get_users_list(std::slice::from_ref(id)).await {
                    found.extend(users.into_iter().map(|user| user.id));
                }
            }
            found
        };
        found.sort_unstable();
        let mut expected = tc.expected_ids.to_vec();
        expected.sort_unstable();
        assert_eq!(found, expected, "{name}");
    }

    test.teardown().await;
}

/// A user is created and updated.
#[tokio::test]
async fn create_and_update_user() {
    let test = common::setup().await;
    let store = &test.store;

    // Create a user with username and email, then verify password updates.
    let mut user = User {
        id: new_id(),
        username: "damao".to_owned(),
        email: "mock@email.com".to_owned(),
        ..User::default()
    };
    store.create_user(&mut user).await.unwrap();

    let got = store.get_user_by_id(&user.id).await.unwrap();
    assert_eq!(got.id, user.id);
    assert_eq!(got.username, user.username);
    assert_eq!(got.email, user.email);

    // "UpdateUserPassword"
    let new_password = new_id();
    let _ = store
        .reset_user_password(&user.username, &new_password)
        .await
        .unwrap();

    let got = store.get_user_by_username(&user.username).await.unwrap();
    assert_eq!(got.username, user.username);
    assert_eq!(got.password_hash, new_password);

    // "UpdateUserPasswordByID": the password a user changes themselves, from the one
    // that was just set.
    let old_password = new_password;
    let new_password = new_id();
    let changed = store
        .change_user_password(
            &user.id,
            &old_password,
            &new_password,
            &new_id(),
            "unused-digest",
        )
        .await
        .unwrap();
    assert!(changed.is_some(), "the hash was still the one seen");

    let got = store.get_user_by_id(&user.id).await.unwrap();
    assert_eq!(got.id, user.id);
    assert_eq!(got.password_hash, new_password);

    test.teardown().await;
}

/// The count of registered users follows the users made.
#[tokio::test]
async fn create_and_get_registered_user_count() {
    let test = common::setup().await;
    let store = &test.store;

    // Each user needs a unique username and email.
    let random_n = clock_count();
    for index in 0..random_n {
        let mut user = User {
            id: new_id(),
            username: format!("user-{index}"),
            email: format!("user-{index}@example.com"),
            ..User::default()
        };
        store.create_user(&mut user).await.unwrap();
    }

    let got = store.get_registered_user_count().await.unwrap();
    assert_eq!(usize::try_from(got).ok(), Some(random_n));

    test.teardown().await;
}

/// Patching the properties of a user.
#[tokio::test]
async fn patch_user_props() {
    let test = common::setup().await;
    let store = &test.store;

    let mut user = User {
        id: new_id(),
        ..User::default()
    };
    store.create_user(&mut user).await.unwrap();

    let key1 = "new_key_1";
    let key2 = "new_key_2";
    let key3 = "new_key_3";
    let fields = |pairs: &[(&str, &str)]| -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect()
    };

    // Only update props
    let patch = ClientStatePatch {
        updated_fields: fields(&[
            (key1, "new_value_1"),
            (key2, "new_value_2"),
            (key3, "new_value_3"),
        ]),
        ..ClientStatePatch::default()
    };

    let client_state = store.patch_client_state(&user.id, &patch).await.unwrap();
    assert_eq!(client_state.len(), 3);
    assert_eq!(value_of(&client_state, key1), Some("new_value_1"));
    assert_eq!(value_of(&client_state, key2), Some("new_value_2"));
    assert_eq!(value_of(&client_state, key3), Some("new_value_3"));

    // Delete a prop
    let patch = ClientStatePatch {
        deleted_fields: vec![key1.to_owned()],
        ..ClientStatePatch::default()
    };

    let client_state = store.patch_client_state(&user.id, &patch).await.unwrap();
    assert_eq!(value_of(&client_state, key1), None);
    assert_eq!(value_of(&client_state, key2), Some("new_value_2"));
    assert_eq!(value_of(&client_state, key3), Some("new_value_3"));

    // update and delete together
    let patch = ClientStatePatch {
        updated_fields: fields(&[(key3, "new_value_3_new_again")]),
        deleted_fields: vec![key2.to_owned()],
    };
    let client_state = store.patch_client_state(&user.id, &patch).await.unwrap();
    assert_eq!(value_of(&client_state, key1), None);
    assert_eq!(value_of(&client_state, key2), None);
    assert_eq!(value_of(&client_state, key3), Some("new_value_3_new_again"));

    test.teardown().await;
}

// Additional user store invariants: timestamps, uniqueness, deactivation, and search.

/// What a created user gets stamped with, which members a read fills in, and what is
/// one user's alone.
#[tokio::test]
async fn create_user_stamps_times_and_reads_back_the_stored_members() {
    let test = common::setup().await;
    let store = &test.store;

    let before = now_millis();
    let mut user = User {
        id: new_id(),
        username: "leia".to_owned(),
        email: "leia@example.com".to_owned(),
        password_hash: "hash".to_owned(),
        created_at: 1,
        updated_at: 2,
        deleted_at: Some(3),
        ..User::default()
    };
    store.create_user(&mut user).await.unwrap();
    assert!(user.created_at >= before, "created_at is the current time");
    assert_eq!(user.updated_at, user.created_at);
    assert_eq!(user.deleted_at, None, "a new user is never deleted");

    let got = store.get_user_by_id(&user.id).await.unwrap();
    assert_eq!(got, user, "everything stored reads back");

    // The identifier, the name and the address are each one user's.
    let mut same_id = User {
        username: "luke".to_owned(),
        email: "luke@example.com".to_owned(),
        ..user.clone()
    };
    let duplicate = store.create_user(&mut same_id).await.unwrap_err();
    assert!(
        matches!(duplicate, StoreError::Database(_)),
        "got {duplicate:?}"
    );
    let mut namesake = User {
        id: new_id(),
        email: "namesake@example.com".to_owned(),
        ..user.clone()
    };
    let taken = store.create_user(&mut namesake).await.unwrap_err();
    assert!(
        matches!(taken, StoreError::Conflict(Taken::Username)),
        "got {taken:?}"
    );
    assert_eq!(taken.to_string(), "the user name is taken");
    let mut same_address = User {
        id: new_id(),
        username: "organa".to_owned(),
        ..user.clone()
    };
    let taken = store.create_user(&mut same_address).await.unwrap_err();
    assert!(
        matches!(taken, StoreError::Conflict(Taken::Email)),
        "got {taken:?}"
    );
    assert_eq!(taken.to_string(), "the email address is taken");
    assert_eq!(
        store.get_registered_user_count().await.unwrap(),
        1,
        "none of the refused users was stored"
    );
    let got = store.get_user_by_id(&user.id).await.unwrap();
    assert_eq!(got, user, "the refused users changed nothing");

    // Text that is not an identifier is no user's identifier.
    let mut unnamed = User {
        id: "not-an-id".to_owned(),
        username: "han".to_owned(),
        email: "han@example.com".to_owned(),
        ..User::default()
    };
    let error = store.create_user(&mut unnamed).await.unwrap_err();
    assert!(
        matches!(&error, StoreError::InvalidId(refused) if refused == "not-an-id"),
        "got {error:?}"
    );

    test.teardown().await;
}

/// A name and an address are one user's whatever their letter case: a second user with
/// either, spelled in another case, is refused as a conflict that says which is taken.
#[tokio::test]
async fn a_name_or_an_address_in_another_letter_case_is_taken() {
    let test = common::setup().await;
    let store = &test.store;

    let han = create_user(
        store,
        User {
            id: new_id(),
            username: "Han.Solo".to_owned(),
            email: "Han@Example.com".to_owned(),
            ..User::default()
        },
    )
    .await;

    let cases = [
        ("han.solo", "smuggler@example.com", Taken::Username),
        ("HAN.SOLO", "smuggler@example.com", Taken::Username),
        ("hAN.sOLO", "smuggler@example.com", Taken::Username),
        ("smuggler", "han@example.com", Taken::Email),
        ("smuggler", "HAN@EXAMPLE.COM", Taken::Email),
        ("smuggler", "hAN@eXAMPLE.COM", Taken::Email),
    ];
    for (username, email, taken) in cases {
        let mut second = User {
            id: new_id(),
            username: username.to_owned(),
            email: email.to_owned(),
            ..User::default()
        };
        let error = store.create_user(&mut second).await.unwrap_err();
        assert!(
            matches!(error, StoreError::Conflict(of) if of == taken),
            "{username} <{email}>: {taken:?} is taken: got {error:?}"
        );
        let said = match taken {
            Taken::Username => "the user name is taken",
            Taken::Email => "the email address is taken",
            Taken::Identity => "the identity at the provider is taken",
            Taken::WorkspaceName => unreachable!("a user takes no workspace's name"),
            Taken::TeamName => unreachable!("a user takes no group's name"),
            Taken::LinkUse => unreachable!("a user takes no use of a link"),
        };
        assert_eq!(error.to_string(), said, "{username} <{email}>");
        assert!(
            !error.is_not_found(),
            "{username} <{email}>: a conflict is not a missing record"
        );
    }

    // Both taken at once: a conflict, of whichever the database looks at first.
    let mut both = User {
        id: new_id(),
        username: "han.solo".to_owned(),
        email: "han@example.com".to_owned(),
        ..User::default()
    };
    let error = store.create_user(&mut both).await.unwrap_err();
    assert!(matches!(error, StoreError::Conflict(_)), "got {error:?}");

    assert_eq!(
        store.get_registered_user_count().await.unwrap(),
        1,
        "none of the refused users was stored"
    );
    let stored = store.get_user_by_id(&han.id).await.unwrap();
    assert_eq!(
        (stored.username.as_str(), stored.email.as_str()),
        ("Han.Solo", "Han@Example.com"),
        "the first user keeps the name and the address as given"
    );

    // A name that only resembles the taken one is free: the comparison is of whole
    // names, and `_` is no wildcard.
    for (username, email) in [
        ("han_solo", "han_solo@example.com"),
        ("han.sol", "han@example.co"),
    ] {
        let mut other = User {
            id: new_id(),
            username: username.to_owned(),
            email: email.to_owned(),
            ..User::default()
        };
        store
            .create_user(&mut other)
            .await
            .unwrap_or_else(|error| panic!("{username} <{email}> is free: {error:?}"));
    }
    assert_eq!(store.get_registered_user_count().await.unwrap(), 3);

    test.teardown().await;
}

/// Lookups by name and by email address ignore the case of ASCII letters on both
/// databases, and find the user as stored.
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "one store, looked up by every name and address in every letter case"
)]
async fn user_lookups_ignore_ascii_case() {
    let test = common::setup().await;
    let store = &test.store;

    let han = create_user(
        store,
        User {
            id: new_id(),
            username: "Han.Solo".to_owned(),
            email: "Han@Example.com".to_owned(),
            ..User::default()
        },
    )
    .await;
    let leia = create_user(
        store,
        User {
            id: new_id(),
            username: "leia".to_owned(),
            email: "leia@example.com".to_owned(),
            password_hash: "leia-hash".to_owned(),
            ..User::default()
        },
    )
    .await;

    for username in ["Han.Solo", "han.solo", "HAN.SOLO", "hAN.sOLO"] {
        let found = store
            .get_user_by_username(username)
            .await
            .unwrap_or_else(|error| panic!("{username} names the user: {error:?}"));
        assert_eq!(found.id, han.id, "{username}");
        assert_eq!(found.username, "Han.Solo", "{username}: read as stored");
    }
    for email in [
        "Han@Example.com",
        "han@example.com",
        "HAN@EXAMPLE.COM",
        "hAN@eXAMPLE.COM",
    ] {
        let found = store
            .get_user_by_email(email)
            .await
            .unwrap_or_else(|error| panic!("{email} is the user's address: {error:?}"));
        assert_eq!(found.id, han.id, "{email}");
        assert_eq!(found.email, "Han@Example.com", "{email}: read as stored");
    }
    let found = store.get_user_by_username("LEIA").await.unwrap();
    assert_eq!(found.id, leia.id, "each name finds its own user");

    // Only the case is ignored: the whole name is compared, a name is not an address,
    // and `%` and `_` are not wildcards.
    for username in ["han", "han.solo ", "han_solo", "%", "Han@Example.com", ""] {
        let error = store.get_user_by_username(username).await.unwrap_err();
        assert!(error.is_not_found(), "{username:?}: got {error:?}");
        assert_eq!(error.to_string(), "{user} not found", "{username:?}");
    }
    for email in ["han@example", "han_example.com", "%", "Han.Solo", ""] {
        let error = store.get_user_by_email(email).await.unwrap_err();
        assert!(error.is_not_found(), "{email:?}: got {error:?}");
    }

    // A password is set by name in any letter case too, for that user alone.
    let fvoci_store::PasswordSet { user_id: reset, .. } = store
        .reset_user_password("hAN.sOLO", "new-hash")
        .await
        .unwrap();
    assert_eq!(reset, han.id, "the user whose password was set");
    let found = store.get_user_by_id(&han.id).await.unwrap();
    assert_eq!(found.password_hash, "new-hash");
    let found = store.get_user_by_id(&leia.id).await.unwrap();
    assert_eq!(
        found.password_hash, "leia-hash",
        "another user's password stays"
    );

    // SQLite folds the ASCII letters only, so there a name that differs in the case of
    // another letter is another name. What PostgreSQL makes of it follows the locale of
    // the database, and is not pinned here.
    if common::db_type() == DbType::Sqlite {
        let zoe = create_user(
            store,
            User {
                id: new_id(),
                username: "ZOË".to_owned(),
                email: "ZOË@example.com".to_owned(),
                ..User::default()
            },
        )
        .await;
        let found = store.get_user_by_username("zoË").await.unwrap();
        assert_eq!(found.id, zoe.id, "the ASCII letters are folded");
        let found = store.get_user_by_email("zoË@EXAMPLE.COM").await.unwrap();
        assert_eq!(found.id, zoe.id, "the ASCII letters are folded");
        let error = store.get_user_by_username("zoë").await.unwrap_err();
        assert!(error.is_not_found(), "Ë is not folded: got {error:?}");
        let error = store
            .get_user_by_email("zoë@example.com")
            .await
            .unwrap_err();
        assert!(error.is_not_found(), "Ë is not folded: got {error:?}");
        create_user(
            store,
            User {
                id: new_id(),
                username: "zoë".to_owned(),
                email: "zoë@example.com".to_owned(),
                ..User::default()
            },
        )
        .await;
    }

    test.teardown().await;
}

/// The search is a substring match on the user name of at most ten users, ignoring the
/// case of every letter on both databases, in which `%` and `_` are characters.
#[tokio::test]
async fn a_search_of_users_matches_substrings_of_the_user_name() {
    let test = common::setup().await;
    let store = &test.store;
    // The search is of the users of a workspace, as one of them sees them.
    let seer = common::user(store, "seer").await;
    let workspace_id = common::workspace_of(store, &[&seer]).await;
    let sight = Sight::InWorkspace {
        seer_id: &seer,
        workspace_id: &workspace_id,
        reach: Reach::Workspace,
        looking: Looking::AmongPeople,
    };
    let let_in = async |user: User| {
        store
            .add_workspace_member(&workspace_id, &user.id, WorkspaceRole::Member)
            .await
            .expect("the user is let into the workspace");
    };

    let found = store
        .search_users_in_sight(sight, "anything")
        .await
        .unwrap();
    assert!(found.is_empty(), "nobody else, no error: {found:?}");

    let names = (0..12).map(|index| format!("Trooper.{index}"));
    for (index, username) in names.chain(["yoda".to_owned()]).enumerate() {
        let email = format!("search.{index}@example.com");
        let user = User {
            id: new_id(),
            username,
            email,
            ..User::default()
        };
        let_in(create_user(store, user).await).await;
    }

    let found = store.search_users_in_sight(sight, "oop").await.unwrap();
    assert_eq!(found.len(), 10, "at most ten of the twelve matches");
    assert!(
        found
            .iter()
            .all(|user| user.username.starts_with("Trooper.")),
        "got {found:?}"
    );

    let found = store
        .search_users_in_sight(sight, "Trooper.11")
        .await
        .unwrap();
    assert_eq!(found.len(), 1);

    let found = store.search_users_in_sight(sight, "").await.unwrap();
    assert_eq!(
        found.len(),
        10,
        "an empty query matches every user, the seer too"
    );

    let found = store.search_users_in_sight(sight, "y_d").await.unwrap();
    assert!(found.is_empty(), "an underscore is no wildcard: {found:?}");
    let found = store.search_users_in_sight(sight, "o%").await.unwrap();
    assert!(found.is_empty(), "a percent sign is no wildcard: {found:?}");

    let found = store
        .search_users_in_sight(sight, "example.com")
        .await
        .unwrap();
    assert!(found.is_empty(), "email addresses are not searched");

    let found = store
        .search_users_in_sight(sight, "trooper.11")
        .await
        .unwrap();
    assert_eq!(found.len(), 1, "the case is ignored");
    let_in(
        create_user(
            store,
            User {
                id: new_id(),
                username: "Ödön".to_owned(),
                email: "odon@example.com".to_owned(),
                ..User::default()
            },
        )
        .await,
    )
    .await;
    let found = store.search_users_in_sight(sight, "öDÖN").await.unwrap();
    assert_eq!(found.len(), 1, "on every letter, on both databases");

    // A query is data, whatever it contains.
    let found = store
        .search_users_in_sight(sight, "' OR '1'='1")
        .await
        .expect("a quote in the query is not SQL");
    assert!(found.is_empty(), "got {found:?}");

    test.teardown().await;
}

/// Updates of users that do not exist change nothing. A password set by name fails with
/// an error that is not a "not found"; a password changed by identifier answers that
/// nothing was changed, as it does when the password is another one by now.
#[tokio::test]
async fn updates_of_missing_users_fail() {
    let test = common::setup().await;
    let store = &test.store;

    let by_name = store
        .reset_user_password("ghost", "hash")
        .await
        .unwrap_err();
    assert_eq!(by_name.to_string(), "user not found (ghost)");
    assert!(!by_name.is_not_found(), "reported as a server error");

    let ghost = new_id();
    let by_id = store
        .change_user_password(&ghost, "", "hash", &new_id(), "unused-digest")
        .await
        .expect("a change for no user is answered");
    assert!(
        by_id.is_none(),
        "nobody has a hash to replace: nothing was changed"
    );
    let error = store.get_user_by_id(&ghost).await.unwrap_err();
    assert!(error.is_not_found(), "and no user was made: got {error:?}");

    // Text that is not an identifier is refused as such before any user is looked for.
    let no_id = store
        .change_user_password("ghost", "", "hash", &new_id(), "unused-digest")
        .await
        .unwrap_err();
    assert!(
        matches!(&no_id, StoreError::InvalidId(refused) if refused == "ghost"),
        "got {no_id:?}"
    );

    test.teardown().await;
}

/// The bag of client state is kept per user, and a patch returns the bag as it is
/// afterwards, by name.
#[tokio::test]
async fn user_preferences_are_per_user_and_ordered() {
    let test = common::setup().await;
    let store = &test.store;
    let user_a = common::user(store, "user-a").await;
    let user_b = common::user(store, "user-b").await;

    let none = store.get_client_state(&user_a).await.unwrap();
    assert!(none.is_empty(), "got {none:?}");

    let patch = |updated: &[(&str, &str)], deleted: &[&str]| ClientStatePatch {
        updated_fields: updated
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        deleted_fields: deleted.iter().map(|name| (*name).to_owned()).collect(),
    };

    store
        .patch_client_state(&user_a, &patch(&[("b", "1"), ("a", "2"), ("c", "3")], &[]))
        .await
        .unwrap();
    store
        .patch_client_state(&user_b, &patch(&[("a", "other")], &[]))
        .await
        .unwrap();

    let patched = store
        .patch_client_state(
            &user_a,
            &patch(&[("a", "4"), ("d", "5")], &["b", "missing"]),
        )
        .await
        .unwrap();
    let names: Vec<&str> = patched.keys().map(String::as_str).collect();
    assert_eq!(names, ["a", "c", "d"], "by name");
    assert_eq!(value_of(&patched, "a"), Some("4"));

    let stored = store.get_client_state(&user_a).await.unwrap();
    assert_eq!(
        stored, patched,
        "the patch returns what is stored, in the same order"
    );

    let other = store.get_client_state(&user_b).await.unwrap();
    assert_eq!(value_of(&other, "a"), Some("other"));
    assert_eq!(other.len(), 1);

    // A name that is both updated and deleted ends up deleted.
    let both = store
        .patch_client_state(&user_b, &patch(&[("a", "again")], &["a"]))
        .await
        .unwrap();
    assert!(both.is_empty(), "got {both:?}");
    let other = store.get_client_state(&user_b).await.unwrap();
    assert!(other.is_empty(), "got {other:?}");

    // Text that is not an identifier names no user: it has no client state, and none
    // is stored for it.
    let none = store.get_client_state("not-a-user").await.unwrap();
    assert!(none.is_empty(), "got {none:?}");
    let error = store
        .patch_client_state("not-a-user", &patch(&[("a", "1")], &[]))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, StoreError::UserNotFound(user) if user == "not-a-user"),
        "got {error:?}"
    );
    let none = store.get_client_state("not-a-user").await.unwrap();
    assert!(none.is_empty(), "got {none:?}");

    test.teardown().await;
}

/// A time in milliseconds for the sessions of the tests below, and one at which they
/// are valid.
const SESSION_MADE: i64 = 1_800_000_000_000;
const SESSION_VALID: i64 = SESSION_MADE + 1;

/// Stores a session of the user under the digest, opened with the password hash that
/// was `seen`, which must be the user's. Returns the identifier of the session.
pub(crate) async fn open_session_under(
    store: &Store,
    user_id: &str,
    digest: &str,
    seen: &str,
) -> String {
    let session = Session {
        id: new_id(),
        user_id: user_id.to_owned(),
        created_at: SESSION_MADE,
        last_used_at: SESSION_MADE,
        expires_at: SESSION_MADE + 1000,
        user_agent: "a browser".to_owned(),
        ..Session::default()
    };
    let stored = store
        .create_session(&session, digest, seen)
        .await
        .expect("the session is stored");
    assert!(
        stored,
        "{digest}: the user has the password hash that was seen"
    );
    session.id
}

/// Whether a session is found under the digest; `user_row_deletion::open_session` stores
/// a user's under the user's identifier.
pub(crate) async fn has_session(store: &Store, digest: &str) -> bool {
    match store
        .get_session(digest, SESSION_VALID, SESSION_MADE - 1)
        .await
    {
        Ok(_) => true,
        Err(StoreError::NoRows) => false,
        Err(other) => panic!("looking a session up failed: {other:?}"),
    }
}

// -- administrators of the instance -------------------------------------------------

/// A user who is not stored yet.
fn new_user(username: &str) -> User {
    User {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@example.com"),
        ..User::default()
    }
}

/// The first user stored is the administrator, whatever the caller says about it, and
/// no user after that is. The instance counts as bootstrapped from then on.
#[tokio::test]
async fn the_first_user_is_the_administrator() {
    let test = common::setup().await;
    assert!(
        !test.is_bootstrapped().await.expect("the flag is read"),
        "a fresh instance has had no user"
    );

    let mut first = new_user("first");
    test.create_user(&mut first)
        .await
        .expect("the first user is stored");
    let mut second = User {
        is_admin: true,
        ..new_user("second")
    };
    test.create_user(&mut second)
        .await
        .expect("the second user is stored");

    assert!(first.is_admin, "the first user comes back as administrator");
    assert!(
        !second.is_admin,
        "the claim of the second is not taken over"
    );
    for (user, expected) in [(&first, true), (&second, false)] {
        let stored = test
            .get_user_by_id(&user.id)
            .await
            .expect("the user is stored");
        assert_eq!(stored.is_admin, expected, "{}", user.username);
    }
    assert!(
        test.is_bootstrapped().await.expect("the flag is read"),
        "the instance has had its first user"
    );

    test.teardown().await;
}

/// A user whose registration fails is not the first user: the next one is.
#[tokio::test]
async fn a_user_who_was_not_stored_was_not_the_first() {
    let test = common::setup().await;

    let mut invalid = User {
        id: "no identifier".to_owned(),
        ..new_user("invalid")
    };
    let error = test
        .create_user(&mut invalid)
        .await
        .expect_err("text that is no identifier is refused");
    assert!(matches!(error, StoreError::InvalidId(_)), "{error:?}");
    assert!(
        !test.is_bootstrapped().await.expect("the flag is read"),
        "nothing was stored, nothing is decided"
    );

    let mut first = new_user("first");
    let workspace_id = test
        .sign_up(&mut first, "the first organization", Founding::First)
        .await
        .expect("the first user starts the organization");
    assert!(first.is_admin, "and is the administrator");
    assert_eq!(workspace_id, test.workspace_id, "of the workspace there is");

    let mut late = new_user("late");
    let error = test
        .sign_up(&mut late, "a late organization", Founding::First)
        .await
        .expect_err("nobody else gets in as the first");
    assert!(matches!(error, StoreError::NotFirstUser), "{error:?}");
    let error = test
        .get_user_by_id(&late.id)
        .await
        .expect_err("the late user is not stored");
    assert!(error.is_not_found(), "{error:?}");

    test.teardown().await;
}

/// Who is first is decided with the insertion. Of users stored at once into an empty
/// instance, exactly one is the administrator; of those who come as "the first", exactly
/// one gets in.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_users_stored_at_once_one_is_the_first() {
    const ROUNDS: usize = 12;
    const RACERS: usize = 6;

    for round in 0..ROUNDS {
        let test = common::setup().await;
        let as_first = round % 2 == 0;
        let mut racers = tokio::task::JoinSet::new();
        for index in 0..RACERS {
            let store = test.store.clone();
            racers.spawn(async move {
                let mut user = new_user(&format!("racer{index}"));
                let outcome = if as_first {
                    store
                        .sign_up(&mut user, "the organization", Founding::First)
                        .await
                        .map(drop)
                } else {
                    store.create_user(&mut user).await
                };
                (user, outcome)
            });
        }
        let outcomes = racers.join_all().await;

        let stored = outcomes
            .iter()
            .filter(|(_, outcome)| outcome.is_ok())
            .count();
        let admins = outcomes
            .iter()
            .filter(|(user, outcome)| outcome.is_ok() && user.is_admin)
            .count();
        assert_eq!(admins, 1, "round {round}: one administrator");
        if as_first {
            assert_eq!(stored, 1, "round {round}: one gets in as the first");
            for (_, outcome) in &outcomes {
                assert!(
                    matches!(outcome, Ok(()) | Err(StoreError::NotFirstUser)),
                    "round {round}: {outcome:?}"
                );
            }
        } else {
            assert_eq!(stored, RACERS, "round {round}: all are stored");
        }
        let listed = test
            .get_users_by_workspace()
            .await
            .expect("the users are read");
        assert_eq!(
            listed.iter().filter(|user| user.is_admin).count(),
            1,
            "round {round}: and one is stored as administrator"
        );

        test.teardown().await;
    }
}

/// `set_user_admin` makes and unmakes administrators, says what it did, and refuses to
/// take the role from the only one. A deactivated administrator does not count.
#[tokio::test]
async fn set_user_admin_keeps_one_administrator() {
    let test = common::setup().await;
    let admin_id = common::user(&test, "admin").await;
    let other_id = common::user(&test, "other").await;
    let is_admin = async |user_id: &str| {
        test.get_user_by_id(user_id)
            .await
            .expect("the user is stored")
            .is_admin
    };

    assert_eq!(
        test.set_user_admin(&admin_id, InstanceRole::User, &admin_id)
            .await
            .expect("no failure"),
        AdminChange::LastAdmin,
        "the only administrator stays one"
    );
    assert!(is_admin(&admin_id).await, "nothing changed");
    for nobody in [new_id(), "no identifier".to_owned()] {
        assert_eq!(
            test.set_user_admin(&nobody, InstanceRole::Administrator, &nobody)
                .await
                .expect("no failure"),
            AdminChange::NoSuchUser,
            "{nobody}"
        );
    }

    assert_eq!(
        test.set_user_admin(&other_id, InstanceRole::Administrator, &other_id)
            .await
            .expect("no failure"),
        AdminChange::NotAnAdministrator,
        "who is none makes nobody one"
    );
    assert!(!is_admin(&other_id).await, "nothing changed");
    let AdminChange::Done(made) = test
        .set_user_admin(&other_id, InstanceRole::Administrator, &admin_id)
        .await
        .expect("no failure")
    else {
        panic!("the other user is made an administrator");
    };
    assert_eq!(made.id, other_id, "the user is returned");
    assert!(made.is_admin, "as stored: an administrator");
    let AdminChange::Done(unmade) = test
        .set_user_admin(&admin_id, InstanceRole::User, &admin_id)
        .await
        .expect("no failure")
    else {
        panic!("with another administrator, the first may step down");
    };
    assert!(!unmade.is_admin, "the first is none any more");

    // The other one is the only administrator now. A deactivated administrator next to
    // them would not change that.
    test.set_user_admin(&admin_id, InstanceRole::Administrator, &other_id)
        .await
        .expect("no failure");
    common::execute_raw(
        &test.database_connection(),
        &format!("UPDATE users SET deleted_at = 1 WHERE id = '{admin_id}'"),
    )
    .await;
    assert_eq!(
        test.set_user_admin(&other_id, InstanceRole::User, &other_id)
            .await
            .expect("no failure"),
        AdminChange::LastAdmin,
        "a deactivated administrator is none to count on"
    );
    assert_eq!(
        test.set_user_admin(&admin_id, InstanceRole::User, &admin_id)
            .await
            .expect("no failure"),
        AdminChange::NoSuchUser,
        "and is not there to change"
    );

    test.teardown().await;
}

/// The check for another administrator and the write are one step: of two
/// administrators who step down at once, one is refused and stays.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_two_administrators_who_step_down_at_once_one_stays() {
    const ROUNDS: usize = 40;
    let test = common::setup().await;
    let first_id = common::user(&test, "first").await;
    let second_id = common::user(&test, "second").await;

    for round in 0..ROUNDS {
        for user_id in [&first_id, &second_id] {
            test.make_admin_by_operator(user_id)
                .await
                .expect("both are administrators before the round");
        }

        let mut racers = tokio::task::JoinSet::new();
        for user_id in [&first_id, &second_id] {
            let store = test.store.clone();
            let user_id = user_id.clone();
            racers.spawn(async move {
                store
                    .set_user_admin(&user_id, InstanceRole::User, &user_id)
                    .await
            });
        }
        let outcomes: Vec<AdminChange> = racers
            .join_all()
            .await
            .into_iter()
            .map(|outcome| outcome.expect("no failure"))
            .collect();

        let refused = outcomes
            .iter()
            .filter(|outcome| **outcome == AdminChange::LastAdmin)
            .count();
        assert_eq!(refused, 1, "round {round}: one is refused: {outcomes:?}");
        let admins = test
            .get_users_by_workspace()
            .await
            .expect("the users are read")
            .iter()
            .filter(|user| user.is_admin)
            .count();
        assert_eq!(admins, 1, "round {round}: and one administrator is left");
    }

    test.teardown().await;
}

// -- settings and the bag of client state ----------------------------------------------

/// The change to the settings that the members of a request make.
fn settings_patch(members: Value) -> UserSettingsPatch {
    let Value::Object(members) = members else {
        panic!("a change to the settings is an object, not {members}");
    };
    UserSettingsPatch::from_members(&members).expect("a valid change to the settings")
}

/// Settings are stored per user, apart from the bag of client state: a setting and an
/// entry of the bag may have the same name. Setting replaces, resetting puts the default
/// back, and every setting as it is afterwards is returned.
#[tokio::test]
async fn settings_are_stored_per_user_and_apart_from_client_state() {
    let test = common::setup().await;
    let store = &test.store;
    let user_a = common::user(store, "user-a").await;
    let user_b = common::user(store, "user-b").await;

    assert_eq!(
        store.get_user_settings(&user_a).await.unwrap(),
        UserSettings::default(),
        "nothing is set to begin with"
    );
    let after = store
        .patch_user_settings(
            &user_a,
            &settings_patch(json!({ "theme": "dark", "density": "compact" })),
        )
        .await
        .unwrap();
    assert_eq!(
        after,
        UserSettings {
            theme: Theme::Dark,
            density: Density::Compact,
            ..UserSettings::default()
        },
        "what was set, and the defaults for the rest"
    );
    let after = store
        .patch_user_settings(
            &user_a,
            &settings_patch(json!({ "theme": "light", "density": null, "haptics": null })),
        )
        .await
        .unwrap();
    let light = UserSettings {
        theme: Theme::Light,
        ..UserSettings::default()
    };
    assert_eq!(
        after, light,
        "set replaces, reset puts the default back, and resetting what is not stored does nothing"
    );
    assert_eq!(
        store.get_user_settings(&user_a).await.unwrap(),
        after,
        "the patch returns what is stored"
    );
    assert_eq!(
        store.get_user_settings(&user_b).await.unwrap(),
        UserSettings::default(),
        "another user's settings are their own"
    );

    // An entry of the bag with the name of a setting is another thing.
    let bag = ClientStatePatch {
        updated_fields: BTreeMap::from([("theme".to_owned(), "of-the-client".to_owned())]),
        ..ClientStatePatch::default()
    };
    let entries = store.patch_client_state(&user_a, &bag).await.unwrap();
    assert_eq!(value_of(&entries, "theme"), Some("of-the-client"));
    assert_eq!(
        store.get_user_settings(&user_a).await.unwrap(),
        light,
        "the setting is untouched by the bag"
    );
    let after = store
        .patch_user_settings(&user_a, &settings_patch(json!({ "theme": null })))
        .await
        .unwrap();
    assert_eq!(after, UserSettings::default(), "the setting is reset");
    let entries = store.get_client_state(&user_a).await.unwrap();
    assert_eq!(
        value_of(&entries, "theme"),
        Some("of-the-client"),
        "and the bag is untouched by the setting"
    );

    // Neither is stored for a user that does not exist, and the refusal says so.
    let nobody = new_id();
    let error = store
        .patch_user_settings(&nobody, &settings_patch(json!({ "theme": "dark" })))
        .await
        .unwrap_err();
    assert!(
        matches!(&error, StoreError::UserNotFound(user) if *user == nobody),
        "got {error:?}"
    );
    let error = store.patch_client_state(&nobody, &bag).await.unwrap_err();
    assert!(
        matches!(&error, StoreError::UserNotFound(user) if *user == nobody),
        "got {error:?}"
    );
    assert_eq!(
        store.get_user_settings(&nobody).await.unwrap(),
        UserSettings::default(),
        "nothing was stored for nobody"
    );
    let entries = store.get_client_state(&nobody).await.unwrap();
    assert!(entries.is_empty(), "got {entries:?}");

    test.teardown().await;
}

/// The bag of client state holds a bounded number of entries. A change that would leave
/// more is refused whole; replacing and removing entries of a full bag still works.
#[tokio::test]
async fn the_bag_of_client_state_is_bounded() {
    let test = common::setup().await;
    let store = &test.store;
    let user_id = common::user(store, "collector").await;
    let limit = fvoci_model::CLIENT_STATE_MAX_ENTRIES;
    let entries = |range: std::ops::Range<usize>| ClientStatePatch {
        updated_fields: range
            .map(|index| (format!("entry-{index:03}"), "v".to_owned()))
            .collect(),
        ..ClientStatePatch::default()
    };

    let full = store
        .patch_client_state(&user_id, &entries(0..limit))
        .await
        .expect("as many entries as the bag holds");
    assert_eq!(full.len(), limit, "the bag is full");

    let error = store
        .patch_client_state(&user_id, &entries(limit - 1..limit + 1))
        .await
        .expect_err("one more than the bag holds");
    assert!(
        matches!(error, StoreError::TooManyClientStateEntries { limit: said } if said == limit),
        "got {error:?}"
    );
    let stored = store.get_client_state(&user_id).await.unwrap();
    assert_eq!(stored, full, "nothing of the refused change was stored");

    // Replacing an entry, and adding one while removing another, fit.
    let mut swap = entries(limit..limit + 1);
    swap.updated_fields
        .insert("entry-000".to_owned(), "replaced".to_owned());
    swap.deleted_fields.push("entry-001".to_owned());
    let swapped = store
        .patch_client_state(&user_id, &swap)
        .await
        .expect("as many entries as before");
    assert_eq!(swapped.len(), limit, "still full");
    assert_eq!(value_of(&swapped, "entry-000"), Some("replaced"));
    assert_eq!(value_of(&swapped, "entry-001"), None);

    // Settings are not entries of the bag: a full bag does not keep them out.
    let settings = store
        .patch_user_settings(&user_id, &settings_patch(json!({ "theme": "dark" })))
        .await
        .expect("a setting is stored next to a full bag");
    assert_eq!(settings.theme, Theme::Dark, "as it was set");

    test.teardown().await;
}

// -- accounts without an address of their own --------------------------------------------

/// An account without an address of its own (one made through a provider) holds no
/// place among the addresses: there are as many as there are, and no lookup by address
/// finds one, not even by the empty address. The others stay one to an address.
#[tokio::test]
async fn an_empty_address_is_nobody_s_and_taken_by_none() {
    let test = common::setup().await;
    let store = &test.store;
    for name in ["no-address-1", "no-address-2"] {
        create_user(
            store,
            User {
                id: new_id(),
                username: name.to_owned(),
                email: String::new(),
                ..User::default()
            },
        )
        .await;
    }
    let error = store.get_user_by_email("").await.unwrap_err();
    assert!(error.is_not_found(), "got {error:?}");
    create_user(
        store,
        User {
            id: new_id(),
            username: "with-address".to_owned(),
            email: "kept@example.com".to_owned(),
            ..User::default()
        },
    )
    .await;
    let error = store
        .create_user(&mut User {
            id: new_id(),
            username: "same-address".to_owned(),
            email: "KEPT@example.com".to_owned(),
            ..User::default()
        })
        .await
        .unwrap_err();
    assert!(
        matches!(error, StoreError::Conflict(Taken::Email)),
        "got {error:?}"
    );
    test.teardown().await;
}

// -- a password that changed in the meantime ---------------------------------------------
