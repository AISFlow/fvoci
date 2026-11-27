use super::*;

fn add_listener(listeners: &mut Listeners, user_id: &str) -> ListenerId {
    add_listener_reading(listeners, user_id).0
}

/// A listener whose connection comes from `address`.
pub(super) fn add_listener_from(
    listeners: &mut Listeners,
    user_id: &str,
    address: Option<AddressKey>,
) -> ListenerId {
    let (id, _) = add_listener_reading(listeners, user_id);
    listeners
        .by_id
        .get_mut(&id)
        .expect("the listener was just added")
        .address = address;
    id
}

/// A listener, and what it is sent.
fn add_listener_reading(
    listeners: &mut Listeners,
    user_id: &str,
) -> (ListenerId, mpsc::Receiver<Bytes>) {
    let (outbox, inbox) = mpsc::channel(8);
    listeners.next_id += 1;
    let id = listeners.next_id;
    listeners.by_id.insert(
        id,
        Listener {
            user_id: user_id.to_owned(),
            session_id: String::new(),
            login: None,
            making: (!user_id.is_empty()).then(SessionKept::with_password),
            token: None,
            address: None,
            outbox,
            closed: CancellationToken::new(),
            closed_because: Arc::default(),
            workspaces: Vec::new(),
            workspace_roles: HashMap::new(),
            blocks: Vec::new(),
            shared_containers: Vec::new(),
            presence: presence::State::default(),
        },
    );
    (id, inbox)
}

fn unsubscribe_from_workspace(listeners: &mut Listeners, id: ListenerId, workspace_id: &str) {
    if let Some(listener) = listeners.by_id.get_mut(&id) {
        listener
            .workspaces
            .retain(|workspace| workspace != workspace_id);
    }
    remove_from(&mut listeners.by_workspace, workspace_id, id);
}

fn workspaces_of(listeners: &Listeners, id: ListenerId) -> &[String] {
    &listeners.by_id[&id].workspaces
}

fn blocks_of(listeners: &Listeners, id: ListenerId) -> &[String] {
    &listeners.by_id[&id].blocks
}

fn workspace_listeners<'a>(listeners: &'a Listeners, workspace_id: &str) -> &'a [ListenerId] {
    listeners
        .by_workspace
        .get(workspace_id)
        .map_or(&[], Vec::as_slice)
}

fn block_listeners<'a>(listeners: &'a Listeners, block_id: &str) -> &'a [ListenerId] {
    listeners.by_block.get(block_id).map_or(&[], Vec::as_slice)
}

/// Subscribing, unsubscribing, and disconnecting listeners on workspaces.
#[test]
fn workspace_subscription() {
    let mut listeners = Listeners::default();
    let workspace_id = "fake-workspace-id";

    // Adding a listener registers it with no workspace subscriptions.
    let session = add_listener(&mut listeners, "");
    assert_eq!(listeners.by_id.len(), 1, "one listener");
    assert!(
        listeners.by_workspace.is_empty(),
        "no workspace subscriptions yet"
    );
    assert!(
        workspaces_of(&listeners, session).is_empty(),
        "the session has no workspaces"
    );

    // Subscribing to a workspace is idempotent.
    for round in ["first", "again"] {
        subscribe_to_workspace(&mut listeners, session, workspace_id);
        assert_eq!(
            workspace_listeners(&listeners, workspace_id),
            [session],
            "{round}"
        );
        assert_eq!(
            workspaces_of(&listeners, session),
            [workspace_id],
            "{round}"
        );
    }

    // Unsubscribing from a workspace is idempotent.
    for round in ["first", "again"] {
        unsubscribe_from_workspace(&mut listeners, session, workspace_id);
        assert!(
            workspace_listeners(&listeners, workspace_id).is_empty(),
            "{round}"
        );
        assert!(workspaces_of(&listeners, session).is_empty(), "{round}");
    }

    // Removing the listener clears it from the table.
    listeners.remove(session);
    assert!(listeners.by_id.is_empty(), "no listener is left");

    // Removing a listener clears all of its workspace subscriptions.
    let workspace_id2 = "other-fake-workspace-id";
    let session = add_listener(&mut listeners, "");
    subscribe_to_workspace(&mut listeners, session, workspace_id);
    subscribe_to_workspace(&mut listeners, session, workspace_id2);
    assert_eq!(listeners.by_id.len(), 1, "one listener");
    assert_eq!(
        workspace_listeners(&listeners, workspace_id),
        [session],
        "subscribed to the first"
    );
    assert_eq!(
        workspace_listeners(&listeners, workspace_id2),
        [session],
        "and to the second"
    );
    listeners.remove(session);
    assert!(listeners.by_id.is_empty(), "no listener is left");
    assert!(
        workspace_listeners(&listeners, workspace_id).is_empty(),
        "nor in the first workspace"
    );
    assert!(
        workspace_listeners(&listeners, workspace_id2).is_empty(),
        "nor in the second"
    );
    assert!(
        listeners.by_workspace.is_empty(),
        "empty workspaces are dropped from the index"
    );
}

/// A workspace that goes to the trash is followed by nobody: each of its listeners stops
/// following it and is told so, and keeps its connection and what else it follows.
#[test]
fn a_workspace_in_the_trash_is_followed_by_nobody() {
    let mut listeners = Listeners::default();
    let (one, mut one_hears) = add_listener_reading(&mut listeners, "a");
    let (other, mut other_hears) = add_listener_reading(&mut listeners, "b");
    for id in [one, other] {
        subscribe_to_workspace(&mut listeners, id, "trashed");
    }
    subscribe_to_workspace(&mut listeners, one, "kept");

    assert_eq!(
        listeners.stop_following_workspace("trashed"),
        2,
        "both are told"
    );
    assert!(
        !listeners.by_workspace.contains_key("trashed"),
        "nobody follows it"
    );
    assert_eq!(listeners.by_id.len(), 2, "no connection is closed");
    assert_eq!(workspaces_of(&listeners, one), ["kept"], "the other stays");
    assert_eq!(workspace_listeners(&listeners, "kept"), [one]);
    for hears in [&mut one_hears, &mut other_hears] {
        let told: serde_json::Value =
            serde_json::from_slice(&hears.try_recv().expect("told")).expect("JSON");
        assert_eq!(
            told,
            serde_json::json!({
                "type": "workspaceUnsubscribed",
                "workspaceId": "trashed",
                "reason": "workspace access ended",
            })
        );
    }
}

/// Looking up listeners by user in a workspace returns every tab of that user and none of
/// any other user.
#[test]
fn workspace_subscription_subscribe_users_to_workspace_retrieve_by_user() {
    let mut listeners = Listeners::default();
    let workspace_id = "fake-workspace-id";
    let anonymous = add_listener(&mut listeners, "");
    let user1 = add_listener(&mut listeners, "fake-user-id");
    let user2 = add_listener(&mut listeners, "fake-user-id2");
    for id in [anonymous, user1, user2] {
        subscribe_to_workspace(&mut listeners, id, workspace_id);
    }
    assert_eq!(listeners.by_id.len(), 3, "three listeners");
    assert_eq!(
        workspace_listeners(&listeners, workspace_id).len(),
        3,
        "all in the workspace"
    );

    let found = listeners.of_user(workspace_id, "fake-user-id");
    assert_eq!(found, [user1], "the user's listener is found");

    let other_tab = add_listener(&mut listeners, "fake-user-id");
    subscribe_to_workspace(&mut listeners, other_tab, workspace_id);
    let mut found = listeners.of_user(workspace_id, "fake-user-id");
    found.sort_unstable();
    let mut both = vec![user1, other_tab];
    both.sort_unstable();
    assert_eq!(found, both, "both tabs of the user, not the other user's");
    listeners.remove(other_tab);

    for id in [anonymous, user1, user2] {
        listeners.remove(id);
    }
    assert!(listeners.by_id.is_empty(), "no listener is left");
    assert!(
        workspace_listeners(&listeners, workspace_id).is_empty(),
        "nor in the workspace"
    );
    assert_eq!(
        listeners.of_user(workspace_id, "fake-user-id"),
        Vec::<ListenerId>::new(),
        "nobody to find"
    );
}

/// Subscribing, unsubscribing, and disconnecting listeners on blocks.
#[test]
fn blocks_subscription() {
    let mut listeners = Listeners::default();
    let block_ids = [
        "block1".to_owned(),
        "block2".to_owned(),
        "block3".to_owned(),
    ];

    // "Should correctly add a session"
    let session = add_listener(&mut listeners, "");
    assert_eq!(listeners.by_id.len(), 1, "one listener");
    assert!(listeners.by_block.is_empty(), "no block subscriptions yet");

    // "Should correctly subscribe to a set of blocks" and "Subscribing again to a
    // subscribed block would have no effect"
    for round in ["first", "again"] {
        subscribe_to_blocks(&mut listeners, session, &block_ids);
        for block_id in &block_ids {
            assert_eq!(
                block_listeners(&listeners, block_id),
                [session],
                "{round} {block_id}"
            );
        }
        assert_eq!(blocks_of(&listeners, session), block_ids, "{round}");
    }

    // "Should correctly unsubscribe to a set of blocks"
    unsubscribe_from_blocks(&mut listeners, session, &block_ids);
    for block_id in &block_ids {
        assert!(
            block_listeners(&listeners, block_id).is_empty(),
            "{block_id}"
        );
    }
    assert!(
        blocks_of(&listeners, session).is_empty(),
        "the session has no blocks"
    );

    // "Unsubscribing again to an unsubscribed block would have no effect"
    unsubscribe_from_blocks(&mut listeners, session, &block_ids[..1]);
    assert!(
        block_listeners(&listeners, "block1").is_empty(),
        "still not subscribed"
    );
    assert!(blocks_of(&listeners, session).is_empty(), "still no blocks");

    // "Should correctly be removed from the server"
    listeners.remove(session);
    assert!(listeners.by_id.is_empty(), "no listener is left");

    // "If subscribed to blocks and removed, should be removed from the blocks
    // subscription list"
    let session = add_listener(&mut listeners, "");
    subscribe_to_blocks(&mut listeners, session, &block_ids);
    assert_eq!(
        blocks_of(&listeners, session),
        block_ids,
        "subscribed to all three"
    );
    listeners.remove(session);
    assert!(listeners.by_id.is_empty(), "no listener is left");
    for block_id in &block_ids {
        assert!(
            block_listeners(&listeners, block_id).is_empty(),
            "{block_id}"
        );
    }
}

/// The wall of an organization shuts out the connections that follow one of its
/// workspaces with a session made otherwise, and no other: also one that confirmed
/// the account's password, which is not the organization's to take.
#[test]
fn a_wall_shuts_out_who_follows_the_organization_with_another_session() {
    let mut listeners = Listeners::default();
    let ours = vec!["ours-1".to_owned(), "ours-2".to_owned()];
    let follows = |listeners: &mut Listeners, session: Session, workspace: Option<&str>| {
        let id = add_listener(listeners, "user");
        if let Some(listener) = listeners.by_id.get_mut(&id) {
            listener.making = Some(SessionKept::of(&session));
            listener.login = session.login;
        }
        if let Some(workspace) = workspace {
            subscribe_to_workspace(listeners, id, workspace);
        }
        id
    };
    let through = |provider_id: &str| Session {
        login: Some(LoginOrigin {
            provider_id: provider_id.to_owned(),
            org_id: "org".to_owned(),
            issuer: "https://issuer.example".to_owned(),
            subject: Some("subject".to_owned()),
            revision: Some(1),
        }),
        ..Session::default()
    };
    let confirmed = |session: Session| Session {
        password_confirmed_at: Some(1),
        ..session
    };
    let with_password = follows(&mut listeners, Session::default(), Some("ours-2"));
    let through_another = follows(&mut listeners, through("theirs"), Some("ours-1"));
    let another_confirmed = follows(&mut listeners, confirmed(through("theirs")), Some("ours-1"));
    let through_ours = follows(&mut listeners, through("ours"), Some("ours-1"));
    let ours_confirmed = follows(&mut listeners, confirmed(through("ours")), Some("ours-2"));
    let elsewhere = follows(&mut listeners, Session::default(), Some("theirs-1"));
    let following_nothing = follows(&mut listeners, Session::default(), None);
    let shut_out = |id: ListenerId| {
        let listener = &listeners.by_id[&id];
        listener
            .workspaces
            .iter()
            .any(|workspace| ending::shuts_out(listener, workspace, &ours, "ours"))
    };
    assert!(shut_out(with_password), "a password");
    assert!(shut_out(through_another), "another provider");
    assert!(
        shut_out(another_confirmed),
        "another provider, the password confirmed"
    );
    assert!(!shut_out(through_ours), "the organization's provider");
    assert!(
        !shut_out(ours_confirmed),
        "the organization's provider, the password confirmed"
    );
    assert!(!shut_out(elsewhere), "follows another organization");
    assert!(!shut_out(following_nothing), "follows nothing of it");
}

/// An ending found by a check of what the connection followed with one role ends only
/// that: a subscription made since under another role stays, and is told nothing.
#[test]
fn an_ending_checked_against_one_role_leaves_a_later_subscription() {
    use ending::{Ended, Of};
    use fvoci_model::WorkspaceRole;

    let mut listeners = Listeners::default();
    let (id, mut hears) = add_listener_reading(&mut listeners, "u");
    subscribe_to_workspace(&mut listeners, id, "w");
    listeners
        .by_id
        .get_mut(&id)
        .expect("the listener")
        .workspace_roles
        .insert("w".to_owned(), WorkspaceRole::Member);
    assert!(
        !listeners.end_workspace(id, "w", Ended::Access, Of::Role(WorkspaceRole::Admin)),
        "checked against the role it followed with before"
    );
    assert_eq!(workspaces_of(&listeners, id), ["w"], "it still follows it");
    assert!(hears.try_recv().is_err(), "and is told nothing");
    assert!(
        listeners.end_workspace(id, "w", Ended::Access, Of::Role(WorkspaceRole::Member)),
        "checked against the role it follows with"
    );
    assert_eq!(
        workspaces_of(&listeners, id).len(),
        0,
        "it follows it no more"
    );
    assert!(hears.try_recv().is_ok(), "told it ended");
}

/// `Command.blockIds` defaults to an empty list when omitted, accepts a list of strings,
/// and rejects an explicit `null`.
#[test]
fn command_block_ids_defaults_when_omitted_and_rejects_null() {
    let omitted: Command =
        serde_json::from_str(r#"{"type":"subscribeBlocks"}"#).expect("omitted blockIds");
    assert_eq!(
        omitted.block_ids,
        Vec::<String>::new(),
        "omitted blockIds defaults to empty"
    );

    let listed: Command =
        serde_json::from_str(r#"{"type":"subscribeBlocks","blockIds":["b1","b2"]}"#)
            .expect("listed blockIds");
    assert_eq!(
        listed.block_ids,
        ["b1", "b2"],
        "listed blockIds are preserved"
    );

    let null_err = serde_json::from_str::<Command>(r#"{"type":"subscribeBlocks","blockIds":null}"#);
    assert!(null_err.is_err(), "explicit null blockIds must be rejected");
}
