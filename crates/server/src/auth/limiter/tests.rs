use super::*;

#[test]
fn each_refusal_is_counted_under_its_limit() {
    let metrics = Metrics::kept().expect("the metrics register");
    let limiter = LoginLimiter::new(Limits {
        pair_free_failures: 1,
        account_max_failures: 3,
        address_attempts_per_minute: Some(5),
        ..LIMITS
    })
    .with_metrics(metrics.clone());
    let now = Instant::now();
    let count = |limit| metrics.login_refusals(limit);

    // The second failure of a pair makes it wait.
    fail(&limiter, "alice", "203.0.113.7", now);
    fail(&limiter, "alice", "203.0.113.7", now);
    assert!(wait_of(&limiter, "alice", "203.0.113.7", now).is_some());
    assert_eq!(count(LoginLimit::PairDelay), 1);

    // A third failure, from elsewhere, locks the account for every address.
    fail(&limiter, "alice", "198.51.100.1", now);
    assert!(wait_of(&limiter, "alice", "192.0.2.1", now).is_some());
    assert_eq!(count(LoginLimit::AccountLock), 1);

    // An attempt in progress holds its address up.
    let attempt = limiter
        .admit_login(&["bob"], Some(address("192.0.2.9")), now)
        .expect("admitted");
    assert!(wait_of(&limiter, "carol", "192.0.2.9", now).is_some());
    assert_eq!(count(LoginLimit::AddressBusy), 1);
    attempt.failed(now);

    // Five attempts a minute from an address, then no more.
    for name in ["d1", "d2", "d3", "d4"] {
        fail(&limiter, name, "192.0.2.9", now);
    }
    assert!(wait_of(&limiter, "d5", "192.0.2.9", now).is_some());
    assert_eq!(count(LoginLimit::AddressRate), 1);
    assert_eq!(
        count(LoginLimit::PairDelay),
        1,
        "nothing else counted twice"
    );
    assert_eq!(count(LoginLimit::AccountLock), 1);
}

const LIMITS: Limits = Limits {
    pair_free_failures: 5,
    pair_base_delay: Duration::from_secs(30),
    pair_max_delay: Duration::from_secs(3600),
    account_max_failures: 100,
    account_lock: Duration::from_secs(3600),
    address_attempts_per_minute: None,
};

fn address(text: &str) -> IpAddr {
    text.parse().expect("an address")
}

fn secs(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// One failed attempt; panics if it is not admitted.
#[track_caller]
fn fail(limiter: &LoginLimiter, account: &str, from: &str, now: Instant) {
    limiter
        .admit_login(&[account], Some(address(from)), now)
        .expect("the attempt is admitted")
        .failed(now);
}

#[track_caller]
fn wait_of(limiter: &LoginLimiter, account: &str, from: &str, now: Instant) -> Option<u64> {
    limiter
        .admit_login(&[account], Some(address(from)), now)
        .err()
        .map(|RetryAfter(wait)| wait.as_secs())
}

#[test]
fn a_pair_waits_longer_with_each_failure_past_the_free_ones() {
    let limiter = LoginLimiter::new(LIMITS);
    let mut now = Instant::now();
    for _ in 0..5 {
        fail(&limiter, "alice", "203.0.113.7", now);
    }
    assert_eq!(
        wait_of(&limiter, "alice", "203.0.113.7", now),
        None,
        "five are free"
    );

    // Each further failure: 30 s, 1 min, 2 min, ... up to an hour.
    for expected in [30, 60, 120, 240, 480, 960, 1920, 3600, 3600] {
        fail(&limiter, "alice", "203.0.113.7", now);
        assert_eq!(
            wait_of(&limiter, "alice", "203.0.113.7", now),
            Some(expected),
            "the wait after this failure"
        );
        assert_eq!(
            wait_of(&limiter, "alice", "203.0.113.7", now + secs(expected - 1)),
            Some(1),
            "still a second to go"
        );
        now += secs(expected);
        assert_eq!(wait_of(&limiter, "alice", "203.0.113.7", now), None, "over");
    }
}

#[test]
fn only_who_fails_waits() {
    let limiter = LoginLimiter::new(LIMITS);
    let now = Instant::now();
    for _ in 0..6 {
        fail(&limiter, "alice", "203.0.113.7", now);
    }
    assert_eq!(wait_of(&limiter, "alice", "203.0.113.7", now), Some(30));
    assert_eq!(
        wait_of(&limiter, "alice", "198.51.100.1", now),
        None,
        "alice signs in from her own address"
    );
    assert_eq!(
        wait_of(&limiter, "bob", "203.0.113.7", now),
        None,
        "another account from the same address"
    );
    assert_eq!(
        wait_of(&limiter, "  ALICE ", "203.0.113.7", now),
        Some(30),
        "the name in other letters is the same account"
    );
}

#[test]
fn a_success_ends_the_failures_of_the_pair_and_of_the_account_only() {
    let limiter = LoginLimiter::new(LIMITS);
    let now = Instant::now();
    for _ in 0..6 {
        fail(&limiter, "alice", "203.0.113.7", now);
    }
    for _ in 0..4 {
        fail(&limiter, "alice", "198.51.100.1", now);
    }
    limiter
        .admit_login(&["alice"], Some(address("198.51.100.1")), now)
        .expect("alice is admitted from her address")
        .succeeded("alice");

    assert_eq!(
        wait_of(&limiter, "alice", "203.0.113.7", now),
        Some(30),
        "the guesser still waits"
    );
    for _ in 0..5 {
        fail(&limiter, "alice", "198.51.100.1", now);
    }
    assert_eq!(
        wait_of(&limiter, "alice", "198.51.100.1", now),
        None,
        "her own failures started over: five are free again"
    );
}

#[test]
fn an_account_is_locked_after_too_many_failures_from_anywhere() {
    let limiter = LoginLimiter::new(Limits {
        account_max_failures: 10,
        ..LIMITS
    });
    let now = Instant::now();
    // Ten addresses, one failure each: no pair has anything to wait for.
    for index in 0..9 {
        fail(&limiter, "alice", &format!("203.0.113.{index}"), now);
    }
    assert_eq!(
        wait_of(&limiter, "alice", "198.51.100.1", now),
        None,
        "nine"
    );
    fail(&limiter, "alice", "203.0.113.200", now);

    assert_eq!(
        wait_of(&limiter, "alice", "198.51.100.1", now),
        Some(3600),
        "locked for everybody, the owner's address too"
    );
    assert_eq!(
        wait_of(&limiter, "bob", "203.0.113.1", now),
        None,
        "not bob"
    );
    let later = now + secs(3600);
    assert_eq!(
        wait_of(&limiter, "alice", "198.51.100.1", later),
        None,
        "over"
    );
    // And the count started over: nine more are possible again.
    for index in 0..9 {
        fail(&limiter, "alice", &format!("203.0.113.{index}"), later);
    }
    assert_eq!(wait_of(&limiter, "alice", "198.51.100.1", later), None);
}

#[test]
fn an_address_makes_so_many_attempts_a_minute_and_one_at_a_time() {
    let limiter = LoginLimiter::new(Limits {
        address_attempts_per_minute: Some(3),
        ..LIMITS
    });
    let start = Instant::now();
    let from = Some(address("203.0.113.7"));

    let first = limiter
        .admit_login(&["a"], from, start)
        .expect("the first attempt");
    assert_eq!(
        limiter.admit_login(&["b"], from, start).err(),
        Some(RetryAfter(BUSY_RETRY)),
        "one at a time"
    );
    assert!(
        limiter
            .admit_registration(Some(address("198.51.100.1")), start)
            .is_ok(),
        "another address is not held up"
    );
    drop(first);
    // The refused attempt was not counted; successes are.
    limiter
        .admit_login(&["b"], from, start + secs(10))
        .expect("the second attempt")
        .succeeded("b");
    drop(
        limiter
            .admit_registration(from, start + secs(20))
            .expect("a registration counts as the third"),
    );
    assert_eq!(
        limiter.admit_login(&["c"], from, start + secs(30)).err(),
        Some(RetryAfter(secs(30))),
        "three a minute: room when the first is a minute old"
    );
    assert!(limiter.admit_login(&["c"], from, start + secs(60)).is_ok());
}

#[test]
fn ipv6_addresses_of_one_network_are_one_address() {
    let limiter = LoginLimiter::new(Limits {
        address_attempts_per_minute: Some(1),
        ..LIMITS
    });
    let now = Instant::now();
    drop(limiter.admit_registration(Some(address("2001:db8:1:2::1")), now));
    assert!(
        limiter
            .admit_registration(Some(address("2001:db8:1:2:ffff::9")), now)
            .is_err(),
        "the same /64"
    );
    assert!(
        limiter
            .admit_registration(Some(address("2001:db8:1:3::1")), now)
            .is_ok(),
        "another /64"
    );
    // A pair is counted by the network too.
    let limiter = LoginLimiter::new(LIMITS);
    for host in 1..=6 {
        fail(&limiter, "alice", &format!("2001:db8:1:2::{host}"), now);
    }
    assert_eq!(
        wait_of(&limiter, "alice", "2001:db8:1:2::99", now),
        Some(30)
    );
}

#[test]
fn an_attempt_without_an_address_is_counted_by_its_account_only() {
    let limiter = LoginLimiter::new(Limits {
        account_max_failures: 3,
        address_attempts_per_minute: Some(1),
        ..LIMITS
    });
    let now = Instant::now();
    for _ in 0..3 {
        limiter
            .admit_login(&["alice"], None, now)
            .expect("no address, no limit by address")
            .failed(now);
    }
    assert!(
        limiter.admit_login(&["alice"], None, now).is_err(),
        "locked"
    );
}

#[test]
fn room_is_made_by_dropping_what_says_nothing_first_and_then_the_oldest() {
    let start = Instant::now();
    let count = u32::try_from(MAX_RECORDS).expect("fits");
    // The value: whether the record says nothing, and when it was touched.
    let touched = |key: u32| start + Duration::from_millis(u64::from(key));
    let mut records: HashMap<u32, (bool, Instant)> = (0..count)
        .map(|key| (key, (key % 2 == 0, touched(key))))
        .collect();
    make_room(&mut records, |record| record.0, |_| true, |record| record.1);
    assert_eq!(records.len(), MAX_RECORDS / 2, "half said nothing");
    assert!(records.values().all(|record| !record.0));

    // The records were touched in the order of their keys, the highest key last.
    // Key `count - 1` is the oldest, but actively locked (`!evictable`), so it must survive (`H-4`).
    let locked_oldest = count - 1;
    let mut full: HashMap<u32, (bool, bool, Instant)> = (0..count)
        .map(|key| (key, (false, key != locked_oldest, touched(count - key))))
        .collect();
    make_room(
        &mut full,
        |record| record.0,
        |record| record.1,
        |record| record.2,
    );
    let tenth = u32::try_from(MAX_RECORDS / 10).expect("fits");
    assert_eq!(
        full.len(),
        MAX_RECORDS - MAX_RECORDS / 10,
        "all say something: a tenth of evictable records goes"
    );
    assert!(
        full.contains_key(&locked_oldest),
        "locked record is never evicted even when oldest"
    );
    assert!(
        (count - tenth - 1..count - 1).all(|key| !full.contains_key(&key)),
        "the oldest evictable tenth is the one that went"
    );
    assert!(full.contains_key(&0), "the youngest stays");
}

#[test]
fn what_is_counted_does_not_depend_on_whether_there_is_an_account() {
    // The names are counted as the attempt gave them, each for itself: a second
    // spelling of a name is the same name, and an email address is another.
    let limiter = LoginLimiter::new(LIMITS);
    let now = Instant::now();
    let from = Some(address("203.0.113.7"));
    for _ in 0..6 {
        limiter
            .admit_login(&["Alice ", "alice@example.com"], from, now)
            .expect("the attempt is admitted")
            .failed(now);
    }
    assert_eq!(
        wait_of(&limiter, "alice", "203.0.113.7", now),
        Some(30),
        "the name, however it is spelled"
    );
    assert_eq!(
        wait_of(&limiter, "ALICE@example.com", "203.0.113.7", now),
        Some(30),
        "the email address the same attempts gave"
    );
    assert_eq!(
        wait_of(&limiter, "bob", "203.0.113.7", now),
        None,
        "another name"
    );
    assert!(
        limiter
            .admit_login(&["bob", "alice@example.com"], from, now)
            .is_err(),
        "one name that waits is enough"
    );
}

#[test]
fn a_success_ends_the_failures_at_the_name_that_signed_in_only() {
    let limiter = LoginLimiter::new(LIMITS);
    let now = Instant::now();
    let from = Some(address("203.0.113.7"));
    for _ in 0..6 {
        fail(&limiter, "victim@example.com", "203.0.113.7", now);
    }
    for _ in 0..3 {
        fail(&limiter, "mallory", "203.0.113.7", now);
    }
    // Mallory signs in to her own account and names the victim's address beside it.
    let later = now + secs(31);
    limiter
        .admit_login(&["mallory", "victim@example.com"], from, later)
        .expect("the wait is over")
        .succeeded("mallory");
    for _ in 0..5 {
        fail(&limiter, "mallory", "203.0.113.7", later);
    }
    assert_eq!(
        wait_of(&limiter, "mallory", "203.0.113.7", later),
        None,
        "her own failures in a row ended with the success"
    );
    fail(&limiter, "victim@example.com", "203.0.113.7", later);
    assert_eq!(
        wait_of(&limiter, "victim@example.com", "203.0.113.7", later),
        Some(60),
        "those at the other name went on: the seventh doubles the wait"
    );
}

#[test]
fn a_wait_that_cannot_be_added_to_the_clock_is_the_longest_wait() {
    let limiter = LoginLimiter::new(Limits {
        pair_free_failures: 0,
        pair_base_delay: Duration::MAX,
        pair_max_delay: Duration::MAX,
        account_max_failures: 1,
        account_lock: Duration::MAX,
        ..LIMITS
    });
    let now = Instant::now();
    fail(&limiter, "alice", "203.0.113.7", now);
    assert_eq!(
        wait_of(&limiter, "alice", "203.0.113.7", now),
        Some(LONGEST_WAIT.as_secs()),
        "no panic, and the longest wait there is"
    );
}

#[test]
fn forgetting_an_account_ends_its_lock_and_the_wait_of_one_address_only() {
    let limiter = LoginLimiter::new(Limits {
        pair_free_failures: 2,
        account_max_failures: 12,
        ..LIMITS
    });
    let now = Instant::now();
    for _ in 0..3 {
        fail(&limiter, "alice", "203.0.113.200", now);
        fail(&limiter, "alice@example.com", "203.0.113.9", now);
        fail(&limiter, "bob", "203.0.113.200", now);
    }
    for last in 1..=9 {
        fail(&limiter, "alice", &format!("203.0.113.{last}"), now);
    }
    assert!(
        wait_of(&limiter, "alice", "198.51.100.1", now).is_some(),
        "locked"
    );
    limiter.forget_account(
        &["Alice", "alice@example.com"],
        Some(address("203.0.113.200")),
    );
    assert_eq!(wait_of(&limiter, "alice", "198.51.100.1", now), None);
    assert_eq!(wait_of(&limiter, "alice", "203.0.113.200", now), None);
    assert!(
        wait_of(&limiter, "alice@example.com", "203.0.113.9", now).is_some(),
        "a guesser elsewhere still waits"
    );
    assert!(
        wait_of(&limiter, "bob", "203.0.113.200", now).is_some(),
        "another account keeps what was counted"
    );
}

#[test]
fn dummy_account_flood_does_not_evict_an_actively_locked_account() {
    let limiter = LoginLimiter::new(Limits {
        account_max_failures: 2,
        account_lock: Duration::from_secs(3600),
        ..LIMITS
    });
    let start = Instant::now();
    fail(&limiter, "victim", "198.51.100.1", start);
    fail(&limiter, "victim", "198.51.100.2", start);
    assert_eq!(wait_of(&limiter, "victim", "192.0.2.1", start), Some(3600));

    let later = start + secs(10);
    {
        let mut records = limiter.records();
        for index in 0..(MAX_RECORDS + 5_000) {
            let mut key = [0_u8; 32];
            key[..8].copy_from_slice(&u64::try_from(index).expect("index").to_le_bytes());
            count_account_failure(
                &mut records.accounts,
                key,
                Limits {
                    account_max_failures: 100,
                    account_lock: Duration::from_secs(3600),
                    ..LIMITS
                },
                later,
            );
        }
    }

    assert_eq!(
        wait_of(&limiter, "victim", "192.0.2.1", later),
        Some(3590),
        "locked account must never be evicted by unlocked dummy traffic"
    );
}
