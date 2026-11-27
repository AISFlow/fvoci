//! The queue in front of a fake way of sending: what it does with each outcome, the
//! pace of all mail, and recovery passing while invitations wait.

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use tokio::time::Instant;

use super::*;
use crate::app::language::Language;

/// A link to choose a new password, of a server at `https://example.test`.
fn reset_link() -> SecretLink {
    let server: crate::config::ServerUrl = "https://example.test".parse().expect("a URL");
    SecretLink::new(&server, SecretPage::ResetPassword, "secret")
}

/// A way of sending that answers from a script, and the last answer once it runs out.
struct Fake {
    start: Instant,
    script: Mutex<VecDeque<Outcome>>,
    /// Each attempt: when, in whole seconds from the start, and to whom.
    attempts: Mutex<Vec<(u64, String)>>,
    /// While true, an attempt to a recipient at `held.test` is answered only once it
    /// is false again.
    held: tokio::sync::watch::Sender<bool>,
}

impl Fake {
    fn new(script: &[Outcome]) -> Arc<Self> {
        Arc::new(Self {
            start: Instant::now(),
            script: Mutex::new(script.iter().copied().collect()),
            attempts: Mutex::new(Vec::new()),
            held: tokio::sync::watch::Sender::new(false),
        })
    }

    fn attempts(&self) -> Vec<(u64, String)> {
        self.attempts.lock().expect("attempts").clone()
    }
}

impl Outbound for Fake {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn send<'a>(
        &'a self,
        message: &'a Outgoing,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + 'a>> {
        self.attempts.lock().expect("attempts").push((
            self.start.elapsed().as_secs(),
            message.to.as_str().to_owned(),
        ));
        let mut script = self.script.lock().expect("script");
        let outcome = if script.len() > 1 {
            script.pop_front()
        } else {
            script.front().copied()
        };
        let outcome = outcome.unwrap_or(Outcome::Sent);
        let mut held = (message.to.as_str().ends_with("@held.test")).then(|| self.held.subscribe());
        Box::pin(async move {
            if let Some(held) = held.as_mut() {
                // The sender lives as long as the fake; it is never dropped first.
                let _ = held.wait_for(|held| !*held).await;
            }
            outcome
        })
    }
}

const LATER: Outcome = Outcome::Later {
    after: None,
    code: Some(451),
};

fn mailer_of(fake: &Arc<Fake>, metrics: Metrics) -> Mailer {
    mailer_with(fake, metrics, SmtpSettings::default())
}

fn mailer_with(fake: &Arc<Fake>, metrics: Metrics, settings: SmtpSettings) -> Mailer {
    let settings = SmtpSettings {
        smtp_host: "smtp.example.test".to_owned(),
        mail_from: "sender@example.test".to_owned(),
        ..settings
    };
    let outbound: Arc<dyn Outbound> = Arc::<Fake>::clone(fake);
    Mailer::with_outbound(&settings, Some(outbound), metrics).expect("mailer")
}

/// Delivers one message to `to` in a task of its own.
fn send(mailer: &Mailer, queued: Queued, to: &str) -> tokio::task::JoinHandle<()> {
    let mailer = mailer.clone();
    let to = to.to_owned();
    tokio::spawn(async move {
        let written = queued
            .deliver(
                &mailer,
                || async {
                    Ok::<_, ()>(Letter {
                        to,
                        content: Content::reset(&reset_link(), Language::English),
                    })
                },
                || async { Ok(true) },
            )
            .await;
        assert!(written.is_ok());
    })
}

fn reset(mailer: &Mailer, account: &str) -> Queued {
    mailer.queue_reset(account).expect("a place")
}

fn invitation(mailer: &Mailer) -> Queued {
    mailer
        .queue_invitations("workspace", "org", 300, Organizations::Several, 1)
        .expect("a place")
        .pop()
        .expect("one")
}

/// The count of `fvoci_mail_failed_total` of `kind` and `class`.
fn failed(metrics: &Metrics, kind: MailKind, class: MailFailure) -> u64 {
    let text = String::from_utf8(metrics.render().expect("kept").expect("rendered")).expect("text");
    let kind = format!("kind=\"{}\"", kind.as_str());
    let class = format!("class=\"{}\"", class.as_str());
    text.lines()
        .find(|line| {
            line.starts_with("fvoci_mail_failed_total{")
                && line.contains(&kind)
                && line.contains(&class)
        })
        .and_then(|line| line.rsplit(' ').next())
        .and_then(|value| value.parse().ok())
        .expect("a counter of each kind and class")
}

#[tokio::test(start_paused = true)]
async fn a_deferred_message_is_tried_again_and_a_refused_one_is_not() {
    let metrics = Metrics::kept().expect("metrics");
    let fake = Fake::new(&[LATER, Outcome::Sent]);
    let mailer = mailer_of(&fake, metrics.clone());
    send(&mailer, reset(&mailer, "a"), "a@example.test")
        .await
        .expect("sent");
    assert_eq!(
        fake.attempts(),
        [
            (0, "a@example.test".to_owned()),
            (1, "a@example.test".to_owned())
        ]
    );

    let fake = Fake::new(&[Outcome::Refused { code: Some(550) }]);
    let mailer = mailer_of(&fake, metrics.clone());
    send(&mailer, reset(&mailer, "b"), "b@example.test")
        .await
        .expect("given up");
    assert_eq!(fake.attempts().len(), 1, "a refusal is final");
    assert_eq!(
        failed(&metrics, MailKind::PasswordReset, MailFailure::Permanent),
        1
    );
    assert_eq!(
        failed(&metrics, MailKind::PasswordReset, MailFailure::Temporary),
        0
    );
}

#[tokio::test(start_paused = true)]
async fn a_message_deferred_at_every_attempt_is_given_up_after_the_third() {
    let metrics = Metrics::kept().expect("metrics");
    let fake = Fake::new(&[LATER]);
    let mailer = mailer_of(&fake, metrics.clone());
    send(&mailer, invitation(&mailer), "a@example.test")
        .await
        .expect("given up");
    let times: Vec<u64> = fake.attempts().into_iter().map(|(at, _)| at).collect();
    assert_eq!(times, [0, 1, 5], "after one and four seconds");
    assert_eq!(
        failed(&metrics, MailKind::Invitation, MailFailure::Temporary),
        1
    );

    let fake = Fake::new(&[
        Outcome::Later {
            after: Some(Duration::from_secs(30)),
            code: Some(421),
        },
        Outcome::Later {
            after: Some(Duration::from_hours(5)),
            code: Some(421),
        },
        Outcome::Sent,
    ]);
    let mailer = mailer_of(&fake, Metrics::none());
    send(&mailer, reset(&mailer, "a"), "a@example.test")
        .await
        .expect("sent");
    let times: Vec<u64> = fake.attempts().into_iter().map(|(at, _)| at).collect();
    assert_eq!(
        times,
        [0, 30],
        "as long as the way asks; given up when it asks for more than a minute"
    );
}

#[tokio::test(start_paused = true)]
async fn all_mail_keeps_the_pace_of_mail_per_second() {
    let fake = Fake::new(&[Outcome::Sent]);
    let mailer = mailer_of(&fake, Metrics::none());
    let sending: Vec<_> = (0..3)
        .map(|n| send(&mailer, reset(&mailer, &format!("{n}")), "a@example.test"))
        .collect();
    for task in sending {
        task.await.expect("sent");
    }
    let times: Vec<u64> = fake.attempts().into_iter().map(|(at, _)| at).collect();
    assert_eq!(times, [0, 1, 2], "the default is one a second");
}

#[tokio::test(start_paused = true)]
async fn while_the_way_keeps_deferring_invitations_wait_and_recovery_is_tried() {
    let fake = Fake::new(&[LATER]);
    let mailer = mailer_of(&fake, Metrics::none());
    send(&mailer, invitation(&mailer), "first@example.test")
        .await
        .expect("given up");
    // Three deferrals in a row: invitations wait for a minute from the third.
    let waiting = send(&mailer, invitation(&mailer), "second@example.test");
    let recovery = send(&mailer, reset(&mailer, "a"), "owner@example.test");
    recovery.await.expect("tried and given up");
    waiting.await.expect("tried and given up");
    let attempts = fake.attempts();
    let of = |to: &str| -> Vec<u64> {
        attempts
            .iter()
            .filter(|(_, recipient)| recipient == to)
            .map(|(at, _)| *at)
            .collect()
    };
    assert_eq!(of("owner@example.test"), [6, 7, 11], "{attempts:?}");
    let second = of("second@example.test");
    assert!(
        second[0] >= 11 + 60,
        "the waiting invitation goes after the pause: {attempts:?}"
    );
}

#[test]
fn a_root_on_this_machine_only_is_told_apart() {
    for root in [
        "http://localhost:8000",
        "http://127.0.0.1/boards",
        "http://127.0.0.2",
        "https://[::1]:8443",
        "http://LOCALHOST",
        "http://boards.localhost",
        "http://0.0.0.0:8000",
        "http://[::]:8000",
        "http://[::ffff:127.0.0.1]:8000",
        "http://127.1",
        "http://localhost.:8000",
        "HTTP://LocalHost",
    ] {
        assert!(is_loopback_root(root), "{root}");
    }
    for root in [
        "https://boards.example.test",
        "http://10.0.0.5:8000",
        "https://[2001:db8::1]",
        "http://127.0.0.1.example.test",
        "not a url",
    ] {
        assert!(!is_loopback_root(root), "{root}");
    }
}

#[tokio::test(start_paused = true)]
async fn recovery_keeps_the_pace_while_invitations_wait_and_another_backs_off() {
    let fake = Fake::new(&[
        LATER,
        LATER,
        LATER,
        Outcome::Later {
            after: Some(Duration::from_secs(30)),
            code: Some(421),
        },
        Outcome::Sent,
    ]);
    let mailer = mailer_of(&fake, Metrics::none());
    send(&mailer, invitation(&mailer), "first@example.test")
        .await
        .expect("given up");
    // Invitations wait now; more of them than they may send at once.
    let waiting: Vec<_> = (0..4)
        .map(|n| send(&mailer, invitation(&mailer), &format!("i{n}@example.test")))
        .collect();
    let backing_off = send(&mailer, reset(&mailer, "a"), "a@example.test");
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let next = send(&mailer, reset(&mailer, "b"), "b@example.test");
    for task in waiting.into_iter().chain([backing_off, next]) {
        task.await.expect("sent");
    }
    let attempts = fake.attempts();
    let of = |to: &str| -> Vec<u64> {
        attempts
            .iter()
            .filter(|(_, recipient)| recipient == to)
            .map(|(at, _)| *at)
            .collect()
    };
    assert_eq!(of("a@example.test"), [6, 36], "{attempts:?}");
    assert_eq!(
        of("b@example.test"),
        [7],
        "the next recovery message has the next turn while another waits: {attempts:?}"
    );
    assert!(
        of("i0@example.test")[0] < 36,
        "a message sent lets the invitations go: {attempts:?}"
    );
}

#[test]
fn an_organization_holds_its_share_of_the_invitation_queue_within_its_own_hour() {
    let fake = Fake::new(&[Outcome::Sent]);
    let mailer = mailer_with(
        &fake,
        Metrics::none(),
        SmtpSettings {
            mail_invitation_queue: 6,
            invitation_mail_recipients: 3,
            ..SmtpSettings::default()
        },
    );
    let held = mailer
        .queue_invitations("w1", "a", 300, Organizations::Several, 3)
        .expect("half of the queue");
    assert_eq!(held.len(), 3);
    assert_eq!(
        mailer
            .queue_invitations("w2", "a", 300, Organizations::Several, 1)
            .err(),
        Some(InvitationRefusal::OrgShareFull),
        "another workspace of the same organization counts in its share"
    );
    let other = mailer
        .queue_invitations("w3", "b", 300, Organizations::Several, 3)
        .expect("another organization has its own half");
    assert_eq!(
        mailer
            .queue_invitations("w4", "c", 1, Organizations::Several, 1)
            .err(),
        Some(InvitationRefusal::QueueFull)
    );
    assert_eq!(
        mailer.org_shares.messages_of("c"),
        None,
        "the share taken before the queue refused is given back"
    );
    drop(other);
    mailer
        .queue_invitations("w4", "c", 1, Organizations::Several, 1)
        .expect("the refusal took nothing of c's hour");
    drop(held);
    assert!(matches!(
        mailer
            .queue_invitations("w5", "d", 2, Organizations::Several, 3)
            .err(),
        Some(InvitationRefusal::OrgHourlyLimit(_))
    ));
    mailer
        .queue_invitations("w5", "d", 3, Organizations::Several, 3)
        .expect("the organization's own limit");
    assert!(
        matches!(
            mailer
                .queue_invitations("w6", "d", 3, Organizations::Several, 1)
                .err(),
            Some(InvitationRefusal::OrgHourlyLimit(_))
        ),
        "the hour counts every workspace of the organization"
    );
    assert!(
        mailer
            .queue_invitations("w6", "e", 300, Organizations::Several, 1)
            .is_ok(),
        "nothing of the refusals was kept"
    );
}

#[tokio::test(start_paused = true)]
async fn invitations_never_take_the_last_sending_and_one_organization_takes_two() {
    let fake = Fake::new(&[Outcome::Sent]);
    fake.held.send_replace(true);
    let mailer = mailer_with(
        &fake,
        Metrics::none(),
        SmtpSettings {
            mail_per_second: 100,
            ..SmtpSettings::default()
        },
    );
    let mut sending = Vec::new();
    for (org, count) in [("a", 3), ("b", 1), ("c", 1)] {
        for (n, queued) in mailer
            .queue_invitations(org, org, 300, Organizations::Several, count)
            .expect("places")
            .into_iter()
            .enumerate()
        {
            sending.push(send(&mailer, queued, &format!("{org}{n}@held.test")));
        }
    }
    tokio::time::sleep(Duration::from_millis(500)).await;
    let attempts = fake.attempts();
    assert_eq!(attempts.len(), 3, "three of four sendings: {attempts:?}");
    let of_a = attempts
        .iter()
        .filter(|(_, to)| to.starts_with('a'))
        .count();
    assert_eq!(of_a, 2, "two of them one organization's: {attempts:?}");
    sending.push(send(&mailer, reset(&mailer, "owner"), "owner@example.test"));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        fake.attempts()
            .iter()
            .any(|(_, to)| to == "owner@example.test"),
        "recovery has the fourth"
    );
    fake.held.send_replace(false);
    for task in sending {
        task.await.expect("sent");
    }
    assert_eq!(fake.attempts().len(), 6);
}

#[tokio::test(start_paused = true)]
async fn an_invitation_waiting_for_a_sending_takes_no_turn_it_could_use_in_a_pause() {
    let fake = Fake::new(&[LATER]);
    fake.held.send_replace(true);
    let mailer = mailer_of(&fake, Metrics::none());
    let mut sending = Vec::new();
    let mut waiting = Vec::new();
    for org in ["a", "b", "c"] {
        let mut queued = mailer
            .queue_invitations(org, org, 300, Organizations::Several, 2)
            .expect("places")
            .into_iter();
        let first = queued.next().expect("one");
        sending.push(send(&mailer, first, &format!("{org}0@held.test")));
        waiting.push((org, queued.next().expect("two")));
    }
    for (org, queued) in waiting {
        sending.push(send(&mailer, queued, &format!("{org}1@example.test")));
    }
    tokio::time::sleep(Duration::from_secs(6)).await;
    for _ in 0..3 {
        mailer.pace.observe(LATER);
    }
    fake.held.send_replace(false);
    for task in sending {
        task.await.expect("given up");
    }
    let attempts = fake.attempts();
    let early: Vec<_> = attempts
        .iter()
        .filter(|(at, to)| to.ends_with("1@example.test") && *at < 66)
        .collect();
    assert!(
        early.is_empty(),
        "the invitations that waited for a sending went after the pause: {attempts:?}"
    );
}

const DOWN: Outcome = Outcome::PathDown { code: Some(535) };

/// The value of `fvoci_mail_path_down`.
fn path_down(metrics: &Metrics) -> i64 {
    let text = String::from_utf8(metrics.render().expect("kept").expect("rendered")).expect("text");
    text.lines()
        .find_map(|line| line.strip_prefix("fvoci_mail_path_down "))
        .and_then(|value| value.parse().ok())
        .expect("the gauge of the closed way")
}

#[tokio::test(start_paused = true)]
async fn a_closed_way_holds_invitations_and_tries_one_message_a_minute_recovery_first() {
    let metrics = Metrics::kept().expect("metrics");
    let fake = Fake::new(&[DOWN, DOWN, Outcome::Sent]);
    let mailer = mailer_of(&fake, metrics.clone());
    assert_eq!(path_down(&metrics), 0);
    let invitation = send(&mailer, invitation(&mailer), "i@example.test");
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(path_down(&metrics), 1, "the first refusal closes the way");
    let recovery = send(&mailer, reset(&mailer, "a"), "r@example.test");
    recovery.await.expect("sent");
    invitation.await.expect("sent");
    assert_eq!(
        fake.attempts(),
        [
            (0, "i@example.test".to_owned()),
            (60, "r@example.test".to_owned()),
            (120, "r@example.test".to_owned()),
            (121, "i@example.test".to_owned()),
        ],
        "one attempt a minute, the recovery message's while it waits; the invitation \
         goes once the way took a message"
    );
    assert_eq!(path_down(&metrics), 0, "open again");
    for kind in [MailKind::PasswordReset, MailKind::Invitation] {
        for class in [MailFailure::Permanent, MailFailure::Temporary] {
            assert_eq!(
                failed(&metrics, kind, class),
                0,
                "a closed way costs no message its attempts"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn a_message_waits_for_a_closed_way_a_quarter_of_an_hour_at_most() {
    let metrics = Metrics::kept().expect("metrics");
    let fake = Fake::new(&[DOWN]);
    let mailer = mailer_of(&fake, metrics.clone());
    send(&mailer, reset(&mailer, "a"), "r@example.test")
        .await
        .expect("given up");
    let times: Vec<u64> = fake.attempts().into_iter().map(|(at, _)| at).collect();
    let expected: Vec<u64> = (0..=LONGEST_PATH_WAIT.as_secs()).step_by(60).collect();
    assert_eq!(times, expected, "once a minute, more than three attempts");
    assert_eq!(
        failed(&metrics, MailKind::PasswordReset, MailFailure::Temporary),
        1
    );
    assert_eq!(
        failed(&metrics, MailKind::PasswordReset, MailFailure::Permanent),
        0
    );
    assert_eq!(path_down(&metrics), 1, "nothing opened it");
}

#[tokio::test(start_paused = true)]
async fn a_message_the_way_refuses_opens_a_closed_way_again() {
    let fake = Fake::new(&[DOWN, Outcome::Refused { code: Some(550) }, Outcome::Sent]);
    let metrics = Metrics::kept().expect("metrics");
    let mailer = mailer_of(&fake, metrics.clone());
    send(&mailer, reset(&mailer, "a"), "a@example.test")
        .await
        .expect("refused");
    assert_eq!(
        path_down(&metrics),
        0,
        "the way read the keys before it refused"
    );
    send(&mailer, invitation(&mailer), "i@example.test")
        .await
        .expect("sent");
    let times: Vec<u64> = fake.attempts().into_iter().map(|(at, _)| at).collect();
    assert_eq!(times, [0, 60, 61]);
}

#[tokio::test(start_paused = true)]
async fn an_invitation_is_held_while_the_way_stays_closed_and_never_given_up_for_it() {
    let metrics = Metrics::kept().expect("metrics");
    let fake = Fake::new(&[DOWN]);
    let mailer = mailer_of(&fake, metrics.clone());
    let invitation = send(&mailer, invitation(&mailer), "i@example.test");
    tokio::time::sleep(Duration::from_mins(40)).await;
    assert!(!invitation.is_finished(), "held, not given up");
    assert_eq!(
        failed(&metrics, MailKind::Invitation, MailFailure::Temporary),
        0
    );
    let times: Vec<u64> = fake.attempts().into_iter().map(|(at, _)| at).collect();
    assert_eq!(
        times,
        (0..40 * 60).step_by(60).collect::<Vec<u64>>(),
        "with no recovery message waiting, it tries the way once a minute"
    );
    invitation.abort();
}

#[tokio::test(start_paused = true)]
async fn a_recovery_message_waiting_for_a_closed_way_gives_up_without_its_turn() {
    let mailer = mailer_of(&Fake::new(&[]), Metrics::none());
    let _ = mailer.pace.observe(DOWN);
    let now = tokio::time::Instant::now();
    assert!(
        mailer.turn(MailKind::PasswordReset, Some(now)).await,
        "the next attempt comes within its quarter of an hour"
    );
    let _ = mailer.pace.observe(DOWN);
    let waited_long = now
        .checked_sub(LONGEST_PATH_WAIT)
        .expect("the paused clock starts later than a quarter of an hour")
        + Duration::from_secs(30);
    let started = tokio::time::Instant::now();
    assert!(
        !mailer
            .turn(MailKind::PasswordReset, Some(waited_long))
            .await,
        "its quarter of an hour ends before the next attempt"
    );
    assert!(started.elapsed() <= Duration::from_secs(30));
    assert!(
        mailer.turn(MailKind::Invitation, Some(waited_long)).await,
        "an invitation waits for as long as it takes"
    );
}

#[tokio::test(start_paused = true)]
async fn a_refusal_without_a_reply_of_the_way_leaves_it_closed() {
    let metrics = Metrics::kept().expect("metrics");
    let fake = Fake::new(&[DOWN, Outcome::Refused { code: None }, Outcome::Sent]);
    let mailer = mailer_of(&fake, metrics.clone());
    send(&mailer, reset(&mailer, "a"), "a@example.test")
        .await
        .expect("refused");
    assert_eq!(path_down(&metrics), 1, "nothing was read of the way");
}

#[test]
fn three_busy_organizations_leave_a_fourth_room_and_one_alone_takes_all() {
    let fake = Fake::new(&[Outcome::Sent]);
    let mailer = mailer_of(&fake, Metrics::none());
    // A workspace a request, so that only the shares and the queue say no.
    let request = std::sync::atomic::AtomicU32::new(0);
    let queue = |org: &str, organizations, messages| {
        let n = request.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        mailer.queue_invitations(&format!("w{n}"), org, 1000, organizations, messages)
    };
    let fill = |org: &str| {
        let mut held = Vec::new();
        loop {
            match queue(org, Organizations::Several, 20) {
                Ok(places) => held.extend(places),
                Err(refusal) => {
                    assert_eq!(refusal, InvitationRefusal::OrgShareFull, "{org}");
                    return held;
                }
            }
        }
    };
    // The default queue of 200 and requests of 20: half of what the others leave.
    let first = fill("a");
    let second = fill("b");
    let third = fill("c");
    assert_eq!((first.len(), second.len(), third.len()), (100, 40, 20));
    let fourth = queue("d", Organizations::Several, 20).expect("a fourth organization finds room");
    let fifth = queue("e", Organizations::Several, 20).expect("and a fifth, the last room");
    assert_eq!(
        queue("f", Organizations::Several, 1).err(),
        Some(InvitationRefusal::QueueFull)
    );
    drop((first, second, third, fourth, fifth));

    let alone: Vec<Queued> = (0..10)
        .flat_map(|_| queue("a", Organizations::One, 20).expect("the whole queue"))
        .collect();
    assert_eq!(alone.len(), 200);
    assert_eq!(
        queue("a", Organizations::One, 1).err(),
        Some(InvitationRefusal::QueueFull),
        "the queue, not a share of it"
    );
}

#[tokio::test(start_paused = true)]
async fn a_message_that_no_longer_stands_is_withdrawn_before_its_next_attempt() {
    let fake = Fake::new(&[DOWN, Outcome::Sent]);
    let mailer = mailer_of(&fake, Metrics::none());
    let stands = std::sync::atomic::AtomicBool::new(true);
    let queued = invitation(&mailer);
    let delivering = queued.deliver(
        &mailer,
        || async {
            Ok::<_, ()>(Letter {
                to: "i@example.test".to_owned(),
                content: Content::reset(&reset_link(), Language::English),
            })
        },
        || async { Ok(stands.load(std::sync::atomic::Ordering::SeqCst)) },
    );
    let mut delivering = std::pin::pin!(delivering);
    // The first attempt finds the way closed; meanwhile its link is revoked.
    tokio::select! {
        _ = &mut delivering => panic!("held while the way is closed"),
        () = tokio::time::sleep(Duration::from_secs(30)) => {}
    }
    stands.store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(delivering.await, Ok(Delivered::Withdrawn));
    assert_eq!(fake.attempts().len(), 1, "nothing more was sent");
}

/// The notice that carries the link back has its place only where the recovery queue
/// has one, and no hour is counted for it: an account told more than the hour's
/// number of notices still has each one queued.
#[test]
fn the_notice_with_a_link_back_takes_a_place_and_counts_no_hour() {
    let fake = Fake::new(&[Outcome::Sent]);
    let mailer = mailer_with(
        &fake,
        Metrics::none(),
        SmtpSettings {
            mail_reset_queue: 1,
            mail_reset_per_account_hour: 1,
            ..SmtpSettings::default()
        },
    );
    let held = reset(&mailer, "account");
    assert_eq!(
        mailer.queue_email_changed().err(),
        Some(ResetMailDrop::QueueFull),
        "no place while the queue is full"
    );
    drop(held);
    for told in 0..3 {
        let place = mailer.queue_email_changed();
        assert!(place.is_ok(), "notice {told} of the hour has a place");
    }
    assert_eq!(
        mailer.queue_email_notice("account").err(),
        None,
        "the notices with a link counted nothing"
    );
}

#[test]
fn invitation_address_quota_admits_up_to_hourly_limit_case_insensitively() {
    let fake = Fake::new(&[Outcome::Sent]);
    let mailer = mailer_with(
        &fake,
        Metrics::none(),
        SmtpSettings {
            invitation_mail_per_address_hour: 2,
            ..SmtpSettings::default()
        },
    );
    let first = mailer.admit_invitation_addresses(&[
        "Target@Example.test".to_owned(),
        "other@example.test".to_owned(),
    ]);
    assert_eq!(first, [true, true]);
    let second = mailer.admit_invitation_addresses(&[
        "target@example.test".to_owned(),
        "other@example.test".to_owned(),
    ]);
    assert_eq!(second, [true, true], "second message in the hour fits");
    let third = mailer.admit_invitation_addresses(&[
        "TARGET@EXAMPLE.TEST".to_owned(),
        "fresh@example.test".to_owned(),
    ]);
    assert_eq!(
        third,
        [false, true],
        "third to target in the same hour is silently skipped while fresh address passes"
    );
}
