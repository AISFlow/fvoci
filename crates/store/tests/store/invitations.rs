//! Invitations in the store: a link is found by the digest of its token while it is
//! live, and whoever accepts it is let in with the count of the use, or not at all.

use std::sync::Arc;

use fvoci_model::{ContainerMember, ContainerRole, Invitation, User, WorkspaceRole, new_id};
use fvoci_store::{
    Accepted, Acceptor, Inviter, InviterOfContainer, MailedTo, NewLink, Standing, Store, StoreError,
};
use tokio::sync::Barrier;
use tokio::task::JoinSet;

use crate::common::{self, TestStore};

pub(crate) const NOW: i64 = 1_000_000;
pub(crate) const LATER: i64 = NOW + 1_000;

/// Somebody who is not stored yet.
pub(crate) fn newcomer(username: &str) -> User {
    User {
        id: new_id(),
        username: username.to_owned(),
        email: format!("{username}@example.com"),
        ..User::default()
    }
}

/// Stores a link into the workspace of the test that `maker` makes.
pub(crate) async fn link(
    test: &TestStore,
    maker: &str,
    token_hash: &str,
    guest_of: Option<(&str, ContainerRole)>,
    max_uses: Option<i64>,
) -> Invitation {
    test.create_invitation(&NewLink {
        workspace_id: &test.workspace_id,
        guest_of,
        token_hash,
        created_by: maker,
        now: NOW,
        expires_at: LATER,
        max_uses,
        mailed: None,
    })
    .await
    .expect("the link is stored")
}

/// What the store says of the maker of a live link, as the server asks it before an
/// acceptance: the acceptance is stored only if they are that still.
pub(crate) struct MakerFound {
    maker: String,
    workspace_role: Option<WorkspaceRole>,
    member: Option<ContainerMember>,
    team_role: Option<ContainerRole>,
}

impl MakerFound {
    /// Asks the store about the maker of the live link whose token has the digest
    /// `token_hash` at `now`; of nobody, if there is no such link.
    pub(crate) async fn of(store: &Store, token_hash: &str, now: i64) -> Result<Self, StoreError> {
        let link = store.live_invitation(token_hash, now, None).await?;
        let (maker, workspace_id, board_id) = match &link {
            Some(link) => (
                link.created_by.as_str(),
                link.workspace_id.as_str(),
                link.container_id.as_deref(),
            ),
            None => ("", "", None),
        };
        let workspace_role = store
            .resolve_workspace(maker, workspace_id)
            .await?
            .map(|access| access.workspace_role);
        let member = match board_id {
            Some(board_id) => match store.get_member_for_container(board_id, maker).await {
                Ok(member) => Some(member),
                Err(error) if error.is_not_found() => None,
                Err(error) => return Err(error),
            },
            None => None,
        };
        let team_role = match board_id {
            Some(id) => store.container_team_role(id, maker).await?,
            None => None,
        };
        Ok(Self {
            maker: maker.to_owned(),
            workspace_role,
            member,
            team_role,
        })
    }

    /// The maker as found, for the acceptance.
    pub(crate) fn inviter(&self) -> Inviter<'_> {
        Inviter {
            standing: Standing {
                user_id: &self.maker,
                workspace_role: self.workspace_role,
                member: self.member.as_ref(),
                team_role: self.team_role,
            },
            of_container: InviterOfContainer::Member,
        }
    }
}

/// Accepts the link as the server does: its maker is held to what the store says of
/// them when the acceptance is asked for.
pub(crate) async fn accept(
    store: &Store,
    token_hash: &str,
    acceptor: Acceptor<'_>,
    now: i64,
) -> Result<Option<Accepted>, StoreError> {
    let maker = MakerFound::of(store, token_hash, now).await?;
    Ok(store
        .accept_invitation(token_hash, acceptor, now, maker.inviter())
        .await?
        .accepted())
}

/// What `user_id` is in the workspace of the test; `None` for nobody there.
async fn role_in_the_workspace(test: &TestStore, user_id: &str) -> Option<WorkspaceRole> {
    test.resolve_workspace(user_id, &test.workspace_id)
        .await
        .expect("the membership is read")
        .map(|access| access.workspace_role)
}

async fn uses_of(test: &TestStore, token_hash: &str) -> Option<i64> {
    test.live_invitation(token_hash, NOW, None)
        .await
        .expect("the link is read")
        .map(|link| link.uses)
}

/// A link is live until it ends, is used as often as it may be, or is revoked; it is
/// found in its own organization only, by the digest of its token.
#[tokio::test]
async fn a_link_is_live_until_it_ends_is_used_up_or_is_revoked() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let for_members = link(&test, &admin, "members", None, None).await;
    let for_a_guest = link(
        &test,
        &admin,
        "guest",
        Some((&boards[0].id, ContainerRole::Viewer)),
        Some(1),
    )
    .await;
    assert_eq!(
        (
            for_members.role,
            for_members.max_uses,
            &for_members.container_id
        ),
        (WorkspaceRole::Member, None, &None)
    );
    assert_eq!(
        (
            for_a_guest.role,
            for_a_guest.max_uses,
            for_a_guest.container_role
        ),
        (WorkspaceRole::Guest, Some(1), Some(ContainerRole::Viewer))
    );
    assert_eq!(for_members.token, None, "the store never has a token");

    // The organization of a token is found past the wall, for a live link only.
    let found = async |token_hash: &'static str, now: i64| {
        test.org_of_live_invitation(token_hash, now, None)
            .await
            .expect("the organization is asked")
    };
    assert_eq!(
        found("members", NOW).await.as_deref(),
        Some(test.org_id.as_str())
    );
    assert_eq!(found("members", LATER).await, None, "ended");
    assert_eq!(found("nothing", NOW).await, None, "no such link");

    // A link for a guest names a board of its workspace that is no template.
    for board_id in [new_id(), "no identifier".to_owned()] {
        let refused = test
            .create_invitation(&NewLink {
                workspace_id: &test.workspace_id,
                guest_of: Some((&board_id, ContainerRole::Editor)),
                token_hash: "elsewhere",
                created_by: &admin,
                now: NOW,
                expires_at: LATER,
                max_uses: Some(1),
                mailed: None,
            })
            .await
            .expect_err("no such board");
        assert!(
            refused.is_not_found() || matches!(refused, StoreError::InvalidId(_)),
            "{refused:?}"
        );
    }

    // Listed newest first, all of them or one maker's; revoked, a link is gone.
    let listed = test
        .live_invitations(&test.workspace_id, None, NOW)
        .await
        .expect("the links are listed");
    let mut ids: Vec<&str> = listed.iter().map(|link| link.id.as_str()).collect();
    ids.sort_unstable();
    let mut both = [for_members.id.as_str(), for_a_guest.id.as_str()];
    both.sort_unstable();
    assert_eq!(ids, both);
    let somebody_else = new_id();
    for (maker, revoked) in [
        (Some(somebody_else.as_str()), false),
        (None, true),
        (None, false),
    ] {
        assert_eq!(
            test.revoke_invitation(&test.workspace_id, &for_members.id, maker, NOW)
                .await
                .expect("the link is revoked, or not"),
            revoked,
            "{maker:?}"
        );
    }
    assert_eq!(found("members", NOW).await, None, "revoked");
    assert_eq!(
        uses_of(&test, "guest").await,
        Some(0),
        "the other is as it was"
    );

    test.teardown().await;
}

/// Of many who accept at once a link that lets one in, one gets in and the others are
/// told that there is no such link; nothing of them is stored, not the account of who
/// came with one to make.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn of_many_who_accept_a_link_for_one_at_once_one_gets_in() {
    const ROUNDS: usize = 6;
    const RACERS: usize = 6;

    for round in 0..ROUNDS {
        let test = common::setup().await;
        let admin = common::user(&test, "admin").await;
        let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
        let board_id = boards[0].id.clone();
        link(
            &test,
            &admin,
            "one",
            Some((&board_id, ContainerRole::Editor)),
            Some(1),
        )
        .await;
        // Half come with an account they have, somewhere else; half with one to make.
        let unscoped = test.scoped(fvoci_store::Tenant::Unset);
        let mut racers = JoinSet::new();
        let barrier = Arc::new(Barrier::new(RACERS));
        for index in 0..RACERS {
            let store: Store = test.store.clone();
            let barrier = Arc::clone(&barrier);
            let mut user = newcomer(&format!("racer{index}"));
            if index % 2 == 0 {
                unscoped
                    .create_user(&mut user)
                    .await
                    .expect("an account of no organization");
            }
            racers.spawn(async move {
                barrier.wait().await;
                let acceptor = if index % 2 == 0 {
                    Acceptor::Account(&user.id)
                } else {
                    Acceptor::NewAccount(&mut user)
                };
                let accepted = accept(&store, "one", acceptor, NOW).await;
                (user.id.clone(), index % 2 == 1, accepted)
            });
        }
        let outcomes = racers.join_all().await;

        let mut let_in = Vec::new();
        for (user_id, made, outcome) in &outcomes {
            match outcome {
                Ok(Some(accepted)) => {
                    assert!(accepted.used, "round {round}: {accepted:?}");
                    assert_eq!(accepted.container_id.as_deref(), Some(board_id.as_str()));
                    let_in.push(user_id.clone());
                }
                Ok(None) => {
                    assert_eq!(
                        role_in_the_workspace(&test, user_id).await,
                        None,
                        "round {round}: who was refused is not in the workspace"
                    );
                    if *made {
                        let stored = test.get_user_by_id(user_id).await;
                        assert!(
                            stored.is_err_and(|error| error.is_not_found()),
                            "round {round}: no account is left of a refused acceptance"
                        );
                    }
                }
                Err(error) => panic!("round {round}: {error:?}"),
            }
        }
        assert_eq!(let_in.len(), 1, "round {round}: one gets in");
        assert_eq!(
            role_in_the_workspace(&test, &let_in[0]).await,
            Some(WorkspaceRole::Guest),
            "round {round}"
        );
        assert_eq!(uses_of(&test, "one").await, None, "round {round}: used up");
        let members = test
            .get_members_for_container(&board_id)
            .await
            .expect("the members of the board");
        assert_eq!(members.len(), 1, "round {round}: {members:?}");
        assert_eq!(members[0].user_id, let_in[0], "round {round}");
        assert_eq!(
            members[0].role,
            ContainerRole::Editor,
            "round {round}: {:?}",
            members[0]
        );

        test.teardown().await;
    }
}

/// A link for members lets in as many as accept it, at once too, and counts each once;
/// who accepts again uses nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_link_for_members_counts_everyone_it_lets_in_once() {
    const RACERS: usize = 6;
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    link(&test, &admin, "many", None, None).await;

    let mut racers = JoinSet::new();
    for index in 0..RACERS {
        let store: Store = test.store.clone();
        racers.spawn(async move {
            let mut user = newcomer(&format!("member{index}"));
            let accepted = accept(&store, "many", Acceptor::NewAccount(&mut user), NOW)
                .await
                .expect("the acceptance is stored")
                .expect("the link is live");
            assert!(accepted.used, "{accepted:?}");
            user.id
        });
    }
    let members = racers.join_all().await;
    assert_eq!(uses_of(&test, "many").await, i64::try_from(RACERS).ok());
    for user_id in &members {
        assert_eq!(
            role_in_the_workspace(&test, user_id).await,
            Some(WorkspaceRole::Member)
        );
    }
    // Again, and by who administers the workspace: nothing changes, nothing is used.
    for user_id in [&members[0], &admin] {
        let again = accept(&test, "many", Acceptor::Account(user_id), NOW)
            .await
            .expect("the acceptance is read")
            .expect("the link is live");
        assert!(!again.used, "{again:?}");
    }
    assert_eq!(uses_of(&test, "many").await, i64::try_from(RACERS).ok());
    assert_eq!(
        role_in_the_workspace(&test, &admin).await,
        Some(WorkspaceRole::Admin),
        "a link for members lowers nobody"
    );
    // An ended link lets nobody in, and stores no account for who came with one.
    let mut late = newcomer("late");
    let refused = accept(&test, "many", Acceptor::NewAccount(&mut late), LATER)
        .await
        .expect("the acceptance is read");
    assert_eq!(refused, None);
    assert!(
        test.get_user_by_id(&late.id)
            .await
            .is_err_and(|error| error.is_not_found())
    );

    test.teardown().await;
}

/// A guest's link makes somebody of the workspace a member of its board and leaves
/// what they are in the workspace; a member's link makes a guest a member.
#[tokio::test]
async fn a_link_raises_what_somebody_is_and_never_lowers_it() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let member = common::user(&test, "member").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 2).await;
    link(
        &test,
        &admin,
        "guest",
        Some((&boards[0].id, ContainerRole::Viewer)),
        Some(1),
    )
    .await;
    link(&test, &admin, "members", None, None).await;

    // The administrator of the board accepts the link to it: nothing changes, and the
    // link is not used.
    let as_admin = ContainerMember::new(boards[0].id.clone(), admin.clone(), ContainerRole::Admin);
    test.add_member(&as_admin)
        .await
        .expect("the administrator of the board");
    let unchanged = accept(&test, "guest", Acceptor::Account(&admin), NOW)
        .await
        .expect("the acceptance is read")
        .expect("the link is live");
    assert!(!unchanged.used, "{unchanged:?}");
    let role = async |board: usize, user_id: &str| {
        test.get_member_for_container(&boards[board].id, user_id)
            .await
            .ok()
            .map(|member| member.role)
    };
    assert_eq!(role(0, &admin).await, Some(ContainerRole::Admin));
    assert_eq!(uses_of(&test, "guest").await, Some(0));

    // A member of the workspace accepts it: on the board, and still a member.
    let joined = accept(&test, "guest", Acceptor::Account(&member), NOW)
        .await
        .expect("the acceptance is stored")
        .expect("the link is live");
    assert!(joined.used, "{joined:?}");
    assert_eq!(role(0, &member).await, Some(ContainerRole::Viewer));
    assert_eq!(
        role_in_the_workspace(&test, &member).await,
        Some(WorkspaceRole::Member)
    );

    // Somebody from outside comes in as a guest of the other board, and then accepts
    // the link for members: a member, with the board they were on.
    link(
        &test,
        &admin,
        "second",
        Some((&boards[1].id, ContainerRole::Editor)),
        Some(1),
    )
    .await;
    let mut outsider = newcomer("outsider");
    accept(&test, "second", Acceptor::NewAccount(&mut outsider), NOW)
        .await
        .expect("the acceptance is stored")
        .expect("the link is live");
    assert_eq!(
        role_in_the_workspace(&test, &outsider.id).await,
        Some(WorkspaceRole::Guest)
    );
    assert_eq!(role(1, &outsider.id).await, Some(ContainerRole::Editor));
    assert_eq!(
        role(0, &outsider.id).await,
        None,
        "on the board of the link only"
    );
    let raised = accept(&test, "members", Acceptor::Account(&outsider.id), NOW)
        .await
        .expect("the acceptance is stored")
        .expect("the link is live");
    assert!(raised.used, "{raised:?}");
    assert_eq!(
        role_in_the_workspace(&test, &outsider.id).await,
        Some(WorkspaceRole::Member)
    );
    assert_eq!(role(1, &outsider.id).await, Some(ContainerRole::Editor));

    test.teardown().await;
}

/// Somebody who comes into a workspace anew as the guest of one board holds no role
/// from an earlier time there on another: not on a board that is there, and not on one
/// that comes back from the trash.
#[tokio::test]
async fn a_guest_who_comes_back_holds_no_role_from_before() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let returning = common::user(&test, "returning").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 3).await;
    let (stays, goes, invited_to) = (&boards[0].id, &boards[1].id, &boards[2].id);
    for (board_id, user_id) in [(stays, &returning), (goes, &returning), (goes, &admin)] {
        let member = ContainerMember::new(board_id.clone(), user_id.clone(), ContainerRole::Admin);
        test.add_member(&member)
            .await
            .expect("a member of the board");
    }
    // They leave the workspace, with their roles left on its boards, and one of the
    // boards goes to the trash.
    test.execute_unchecked(&format!(
        "DELETE FROM workspace_members WHERE user_id = '{returning}'"
    ))
    .await
    .expect("the member leaves");
    test.delete_container(goes, &admin)
        .await
        .expect("the board goes to the trash");

    link(
        &test,
        &admin,
        "back",
        Some((invited_to, ContainerRole::Viewer)),
        Some(1),
    )
    .await;
    accept(&test, "back", Acceptor::Account(&returning), NOW)
        .await
        .expect("the acceptance is stored")
        .expect("the link is live");
    assert_eq!(
        role_in_the_workspace(&test, &returning).await,
        Some(WorkspaceRole::Guest)
    );
    test.undelete_container(goes, &admin)
        .await
        .expect("the board comes back");

    let role = async |board_id: &str, user_id: &str| {
        test.get_member_for_container(board_id, user_id)
            .await
            .ok()
            .map(|member| member.role)
    };
    assert_eq!(
        role(invited_to, &returning).await,
        Some(ContainerRole::Viewer)
    );
    assert_eq!(
        role(stays, &returning).await,
        None,
        "a board that was there"
    );
    assert_eq!(role(goes, &returning).await, None, "a board that came back");
    assert_eq!(
        role(goes, &admin).await,
        Some(ContainerRole::Admin),
        "who stayed comes back with the board"
    );

    test.teardown().await;
}

/// While the invitations of guests are turned off no link for a guest is stored, and
/// those there were stay revoked when they are turned on again; a link for members is
/// made either way.
#[tokio::test]
async fn no_link_for_a_guest_is_made_while_guests_are_not_invited() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let guest_of = Some((boards[0].id.as_str(), ContainerRole::Editor));
    link(&test, &admin, "before", guest_of, Some(1)).await;
    let switch = async |enabled: bool| {
        test.set_guest_invitations(
            &test.workspace_id,
            fvoci_model::GuestInvitationsChange { enabled },
            &admin,
        )
        .await
        .expect("the switch is stored");
    };

    switch(false).await;
    let refused = test
        .create_invitation(&NewLink {
            workspace_id: &test.workspace_id,
            guest_of,
            token_hash: "while-off",
            created_by: &admin,
            now: NOW,
            expires_at: LATER,
            max_uses: Some(1),
            mailed: None,
        })
        .await;
    assert!(
        matches!(refused, Err(StoreError::NotFound(_))),
        "{refused:?}"
    );
    link(&test, &admin, "members", None, None).await;

    switch(true).await;
    assert_eq!(uses_of(&test, "before").await, None, "revoked for good");
    assert_eq!(uses_of(&test, "while-off").await, None);
    link(&test, &admin, "after", guest_of, Some(1)).await;
    assert_eq!(uses_of(&test, "after").await, Some(0));

    test.teardown().await;
}

/// The links made for the recipients of a mailed guest link name it and their
/// address; revoking it revokes those nobody used, and it is what they are made like
/// even once it was used itself.
#[tokio::test]
async fn a_mailed_links_children_name_it_and_go_with_it_while_unused() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let sender = common::user(&test, "sender").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let guest_of = Some((boards[0].id.as_str(), ContainerRole::Viewer));
    let parent = link(&test, &admin, "parent", guest_of, Some(1)).await;
    let child = async |token_hash: &str, recipient: &str| {
        test.create_invitation(&NewLink {
            workspace_id: &test.workspace_id,
            guest_of,
            token_hash,
            created_by: &sender,
            now: NOW,
            expires_at: LATER,
            max_uses: Some(1),
            mailed: Some(MailedTo {
                parent_id: &parent.id,
                recipient,
            }),
        })
        .await
        .expect("the child is stored")
    };
    let used = child("used", "used@example.test").await;
    let unused = child("unused", "unused@example.test").await;
    assert_eq!(unused.parent_id.as_deref(), Some(parent.id.as_str()));
    assert_eq!(unused.recipient.as_deref(), Some("unused@example.test"));
    assert_eq!(parent.parent_id, None);
    assert_eq!(parent.recipient, None);
    let listed = test
        .live_invitations(&test.workspace_id, None, NOW)
        .await
        .expect("listed");
    let recipients: Vec<Option<&str>> = listed
        .iter()
        .map(|link| link.recipient.as_deref())
        .collect();
    assert_eq!(
        recipients.len(),
        3,
        "the parent and its children: {recipients:?}"
    );
    assert!(recipients.contains(&Some("unused@example.test")));
    assert!(
        accept(
            &test,
            "used",
            Acceptor::NewAccount(&mut newcomer("taker")),
            NOW
        )
        .await
        .expect("accepted")
        .is_some()
    );
    assert_eq!(
        test.unended_invitation("used", NOW)
            .await
            .expect("asked")
            .map(|link| link.uses),
        Some(1)
    );

    assert!(
        test.revoke_invitation(&test.workspace_id, &parent.id, Some(&admin), NOW)
            .await
            .expect("revoked"),
        "the parent, by its maker"
    );
    assert_eq!(
        test.live_invitation("unused", NOW, None)
            .await
            .expect("asked"),
        None,
        "an unused child goes with its parent, whoever made it"
    );
    assert_eq!(
        test.unended_invitation("parent", NOW).await.expect("asked"),
        None,
        "a revoked link is nothing to make links like"
    );
    assert_eq!(
        test.unended_invitation("used", NOW)
            .await
            .expect("asked")
            .map(|link| link.id),
        Some(used.id),
        "who used theirs keeps it: it is not revoked"
    );
    test.teardown().await;
}

/// A used link is still what its recipients' links are made like, until it ends.
#[tokio::test]
async fn a_used_link_is_unended_until_it_ends() {
    let test = common::setup().await;
    let admin = common::user(&test, "admin").await;
    let boards = common::create_test_boards(&test, &test.workspace_id, &admin, 1).await;
    let guest_of = Some((boards[0].id.as_str(), ContainerRole::Viewer));
    let used_up = link(&test, &admin, "used-up", guest_of, Some(1)).await;
    assert!(
        accept(
            &test,
            "used-up",
            Acceptor::NewAccount(&mut newcomer("first")),
            NOW
        )
        .await
        .expect("accepted")
        .is_some()
    );
    assert_eq!(
        test.live_invitation("used-up", NOW, None)
            .await
            .expect("asked"),
        None
    );
    assert_eq!(
        test.unended_invitation("used-up", NOW)
            .await
            .expect("asked")
            .map(|link| link.id),
        Some(used_up.id)
    );
    assert_eq!(
        test.unended_invitation("used-up", LATER)
            .await
            .expect("asked"),
        None,
        "not once it ended"
    );
    test.teardown().await;
}
