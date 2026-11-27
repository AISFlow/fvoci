use super::*;
use crate::realtime::{Listener, Listeners};
use std::io::Write as _;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn visit(board: &str, card: &str, token: &str) -> Visit {
    Visit {
        workspace_id: "workspace".to_owned(),
        container_id: board.to_owned(),
        card_id: card.to_owned(),
        read_token: token.to_owned(),
        wall: Wall::for_tests(),
    }
}

fn add(listeners: &mut Listeners, user: &str, visit: Visit, age: Duration) {
    let (outbox, _) = mpsc::channel(1);
    listeners.next_id += 1;
    listeners.by_id.insert(
        listeners.next_id,
        Listener {
            user_id: user.to_owned(),
            session_id: String::new(),
            login: None,
            making: (!user.is_empty()).then(crate::app::SessionKept::with_password),
            token: None,
            address: None,
            outbox,
            closed: CancellationToken::new(),
            closed_because: Arc::default(),
            workspaces: Vec::new(),
            workspace_roles: std::collections::HashMap::new(),
            blocks: Vec::new(),
            shared_containers: Vec::new(),
            presence: State {
                renewed: Some(Instant::now() - age),
                ..State::default()
            },
        },
    );
    listeners.place(listeners.next_id, Some(visit));
}

#[test]
fn a_stalled_connection_expires_without_a_socket_close_and_tabs_union_their_cards() {
    let mut listeners = Listeners::default();
    let here = visit("board", "", "");
    for (user, card, token, age) in [
        ("one", "a", "", Duration::ZERO),
        ("one", "b", "", Duration::ZERO),
        ("two", "c", "", LEASE),
        ("signed-in", "secret-card", "token", Duration::ZERO),
    ] {
        add(&mut listeners, user, visit("board", card, token), age);
    }
    add(
        &mut listeners,
        "elsewhere",
        visit("other-board", "d", ""),
        Duration::ZERO,
    );
    let roster = snapshot(&here, &listeners);
    assert_eq!(
        roster.people,
        vec![Person {
            user_id: "one".to_owned(),
            card_ids: BTreeSet::from(["a".to_owned(), "b".to_owned()])
        }],
        "stalled and other-board connections have no place; live tabs union their cards"
    );
    assert_eq!(
        roster.anonymous_count, 1,
        "a token never identifies its signed-in user"
    );
}

#[tokio::test(start_paused = true)]
async fn commands_have_a_bounded_burst_and_refill_at_the_configured_rate() {
    let mut rate = Rate {
        left: 2000,
        counted: Instant::now(),
    };
    assert!(rate.admits(2), "first command of the burst");
    assert!(rate.admits(2), "second command of the burst");
    assert!(!rate.admits(2), "the burst is exhausted");
    tokio::time::advance(Duration::from_millis(499)).await;
    assert!(!rate.admits(2), "no whole command has refilled yet");
    tokio::time::advance(Duration::from_millis(1)).await;
    assert!(
        rate.admits(2),
        "half a second buys one command at two per second"
    );
    assert!(!rate.admits(2), "only one refilled");
}

#[test]
fn fifty_visits_hold_only_bounded_identifiers_in_the_connection_table() {
    let id = "01930000-0000-7000-8000-000000000001";
    let mut listeners = Listeners::default();
    for _ in 0..50 {
        let mut place = visit(id, id, "");
        place.workspace_id = id.to_owned();
        add(&mut listeners, id, place, Duration::ZERO);
    }
    let bytes: usize = listeners
        .by_id
        .values()
        .map(|listener| {
            let place = listener.presence.visit.as_ref().expect("a visit");
            std::mem::size_of::<State>()
                + place.workspace_id.capacity()
                + place.container_id.capacity()
                + place.card_id.capacity()
                + place.read_token.capacity()
        })
        .sum::<usize>()
        + listeners
            .visits
            .by_container
            .iter()
            .map(|((_, workspace, board), on)| {
                std::mem::size_of::<(ContainerKey, OnContainer)>()
                    + workspace.capacity()
                    + board.capacity()
                    + on.visitors.len() * std::mem::size_of::<ListenerId>()
            })
            .sum::<usize>();
    assert!(
        bytes < 20_000,
        "fifty active visit states hold {bytes} bytes before allocator overhead"
    );
    writeln!(std::io::stdout(), "presence table: 50 visits, {bytes} bytes (State, owned identifier capacities and the boards' index; excludes existing sockets, allocator overhead and outgoing snapshot caches)").expect("the measurement is written");
}

/// Which users' visits `flag_reached` flagged, sorted.
fn flagged(listeners: &Listeners) -> Vec<&str> {
    let mut users: Vec<&str> = listeners
        .by_id
        .values()
        .filter(|listener| listener.presence.recheck)
        .map(|listener| listener.user_id.as_str())
        .collect();
    users.sort_unstable();
    users
}

/// Two people on `board`, one on `other-board`, all unflagged.
fn two_boards() -> Listeners {
    let mut listeners = Listeners::default();
    for (user, board) in [("one", "board"), ("two", "board"), ("three", "other-board")] {
        add(&mut listeners, user, visit(board, "", ""), Duration::ZERO);
    }
    listeners
}

#[test]
fn a_change_flags_the_rosters_of_the_people_it_concerns_and_nothing_else() {
    use crate::collab::AccessScope;

    let here = Wall::for_tests();
    let cases = [
        (
            Some(AccessScope::container("board").of_user("two")),
            here,
            vec!["one", "two"],
            "a present person's change: the whole roster asks again",
        ),
        (
            Some(AccessScope::container("board").of_user("absent")),
            here,
            vec![],
            "somebody not present changes no roster",
        ),
        (
            Some(AccessScope::workspace("workspace")),
            here,
            vec!["one", "three", "two"],
            "everybody's change in the workspace reaches every board of it",
        ),
        (
            Some(AccessScope::container("board")),
            crate::collab::another_wall_for_tests(),
            vec![],
            "another organization's change reaches nobody here",
        ),
        (
            Some(AccessScope::organization().of_user("three")),
            here,
            vec!["three"],
            "a user's change in the organization reaches only the boards they are on",
        ),
        (
            None,
            here,
            vec!["one", "three", "two"],
            "missed changes may have reached anybody",
        ),
    ];
    for (scope, wall, want, why) in cases {
        let mut listeners = two_boards();
        let change = scope.map(|scope| AccessChange::for_tests(scope, wall));
        flag_reached(&mut listeners, change.as_ref());
        assert_eq!(flagged(&listeners), want, "{why}");
    }
}

#[test]
fn an_admission_read_before_a_change_that_reaches_it_is_asked_again() {
    use crate::collab::AccessScope;

    let here = visit("board", "", "");
    let (sender, mut changes) = broadcast::channel(2);
    assert!(!changed_meanwhile(&mut changes, &here), "nothing changed");
    for (scope, wall) in [
        (AccessScope::container("other-board"), Wall::for_tests()),
        (
            AccessScope::container("board"),
            crate::collab::another_wall_for_tests(),
        ),
    ] {
        sender
            .send(AccessChange::for_tests(scope, wall))
            .expect("the receiver listens");
    }
    assert!(
        !changed_meanwhile(&mut changes, &here),
        "another board's or organization's change leaves the admission be"
    );
    sender
        .send(AccessChange::for_tests(
            AccessScope::container("board"),
            Wall::for_tests(),
        ))
        .expect("the receiver listens");
    assert!(changed_meanwhile(&mut changes, &here), "this board changed");
    for _ in 0..3 {
        sender
            .send(AccessChange::for_tests(
                AccessScope::container("other-board"),
                Wall::for_tests(),
            ))
            .expect("the receiver listens");
    }
    assert!(
        changed_meanwhile(&mut changes, &here),
        "changes missed by a lagging listener may have reached the admission"
    );
}

#[test]
fn the_boards_index_follows_every_visit_and_forgets_a_connection_that_went_away() {
    let mut listeners = two_boards();
    let board = visit("board", "", "").container();
    let before = listeners.visits.changed(&board);
    let first = *listeners.by_id.keys().min().expect("a connection");
    listeners.place(first, Some(visit("other-board", "", "")));
    assert_ne!(
        listeners.visits.changed(&board),
        before,
        "leaving a board changes it"
    );
    assert_eq!(
        snapshot(&visit("board", "", ""), &listeners).people.len(),
        1,
        "the roster reads the index"
    );
    let unchanged = listeners.visits.changed(&board);
    listeners.place(first, Some(visit("other-board", "", "")));
    assert_eq!(
        listeners.visits.changed(&board),
        unchanged,
        "the same visit again changes nothing"
    );
    let ids: Vec<ListenerId> = listeners.by_id.keys().copied().collect();
    for id in ids {
        listeners.remove(id);
    }
    assert!(
        listeners.visits.by_container.is_empty(),
        "no board keeps a connection that went away"
    );
}

#[test]
fn a_lapsed_visit_renewed_moves_its_board_so_the_rosters_built_without_it_are_built_again() {
    let mut listeners = two_boards();
    let board = visit("board", "", "").container();
    let before = listeners.visits.changed(&board);
    listeners.visits.touch(&board);
    assert_ne!(
        listeners.visits.changed(&board),
        before,
        "a reader that saw the board before builds its roster again"
    );
    let gone = visit("gone", "", "").container();
    listeners.visits.touch(&gone);
    assert_eq!(
        listeners.visits.changed(&gone),
        None,
        "a board nobody is on is not made by touching it"
    );
}
