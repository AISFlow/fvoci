//! The limit on reading shared boards with tokens that open nothing.
//!
//! A read token is unguessable, but a client that asks for many boards with many tokens
//! still tries. A read whose token opens nothing counts against the address of the
//! client (an IPv4 address, or the /64 of an IPv6 address, as the login limiter counts
//! them); an address with [`MAX_SHARED_READ_FAILURES`] such reads within
//! [`SHARED_READ_WINDOW`] is refused (429) until the window that has them is over. A
//! reader with a good link is never held back by its own reads, however many come at
//! once or are given up on the way. Reads with bad tokens that come at once may pass
//! the limit together before they are counted: with tokens of 256 random bits, a few
//! more guesses change nothing.
//!
//! The counts are kept in memory and forgotten when the process ends; the oldest go
//! when there are too many.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::auth::limiter::AddressKey;
use crate::error::AppError;

/// How many reads that open nothing an address may have within [`SHARED_READ_WINDOW`].
pub const MAX_SHARED_READ_FAILURES: u32 = 30;

/// The window the failures of an address are counted in.
pub const SHARED_READ_WINDOW: Duration = Duration::from_secs(60);

/// How many addresses are kept at most.
const MAX_ADDRESSES: usize = 100_000;

/// The failures of one address within its current window.
#[derive(Debug, Clone, Copy)]
struct Count {
    /// When the window began: at the first read after the last window ended.
    since: Instant,
    /// The reads since then that opened nothing.
    failures: u32,
}

/// The counts of the addresses, and the order they were first counted in.
#[derive(Debug, Default)]
struct Counts {
    by_address: HashMap<AddressKey, Count>,
    order: VecDeque<AddressKey>,
}

/// The limit on reads of shared boards that open nothing, by the client's address.
#[derive(Debug, Default)]
pub struct SharedReads {
    counts: Mutex<Counts>,
}

impl SharedReads {
    /// Lets a read in, or refuses it with 429 if its address has had too many reads
    /// that opened nothing within the window. A request without an address (the
    /// administration socket) is not limited.
    ///
    /// # Errors
    ///
    /// Returns the 429 of an address over the limit, with the wait until its window ends.
    ///
    /// # Panics
    ///
    /// If a thread panicked while it held the counts.
    pub fn admit(&self, address: Option<IpAddr>) -> Result<(), AppError> {
        self.admit_at(address, Instant::now())
    }

    fn admit_at(&self, address: Option<IpAddr>, now: Instant) -> Result<(), AppError> {
        let Some(address) = address.map(AddressKey::from) else {
            return Ok(());
        };
        let mut counts = self.counts.lock().expect("the counts of shared reads");
        let Some(count) = counts.by_address.get_mut(&address) else {
            return Ok(());
        };
        let ends = count.since + SHARED_READ_WINDOW;
        if ends <= now {
            *count = Count {
                since: now,
                failures: 0,
            };
        } else if count.failures >= MAX_SHARED_READ_FAILURES {
            return Err(AppError::rate_limited(ends - now));
        }
        Ok(())
    }

    /// Counts a read whose token opened nothing against its address.
    ///
    /// # Panics
    ///
    /// If a thread panicked while it held the counts.
    pub fn failed(&self, address: Option<IpAddr>) {
        self.failed_at(address, Instant::now());
    }

    fn failed_at(&self, address: Option<IpAddr>, now: Instant) {
        let Some(address) = address.map(AddressKey::from) else {
            return;
        };
        let mut counts = self.counts.lock().expect("the counts of shared reads");
        let Counts { by_address, order } = &mut *counts;
        if let Some(count) = by_address.get_mut(&address) {
            if count.since + SHARED_READ_WINDOW <= now {
                *count = Count {
                    since: now,
                    failures: 0,
                };
            }
            count.failures = count.failures.saturating_add(1);
            return;
        }
        if by_address.len() >= MAX_ADDRESSES
            && let Some(oldest) = order.pop_front()
        {
            by_address.remove(&oldest);
        }
        by_address.insert(
            address,
            Count {
                since: now,
                failures: 1,
            },
        );
        order.push_back(address);
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};

    use super::*;

    const ADDRESS: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 7)));
    const OTHER: Option<IpAddr> = Some(IpAddr::V4(Ipv4Addr::new(203, 0, 113, 8)));

    #[test]
    fn an_address_is_refused_after_its_failures_until_the_window_ends() {
        let limit = SharedReads::default();
        let start = Instant::now();
        for _ in 0..MAX_SHARED_READ_FAILURES {
            assert!(limit.admit_at(ADDRESS, start).is_ok(), "within the limit");
            limit.failed_at(ADDRESS, start);
        }
        let refused = limit
            .admit_at(ADDRESS, start + Duration::from_secs(1))
            .expect_err("over the limit");
        assert_eq!(refused.status(), 429, "too many");
        assert!(limit.admit_at(OTHER, start).is_ok(), "another address");
        assert!(limit.admit_at(None, start).is_ok(), "no address");
        assert!(
            limit.admit_at(ADDRESS, start + SHARED_READ_WINDOW).is_ok(),
            "the window is over, and a new one counts anew"
        );
    }

    /// Reads that open the board, also many at once or given up on the way (a browser
    /// that drops the images of a view it leaves), count for nothing.
    #[test]
    fn reads_that_fail_nothing_are_never_held_back() {
        let limit = SharedReads::default();
        let now = Instant::now();
        for _ in 0..MAX_SHARED_READ_FAILURES * 3 {
            limit
                .admit_at(ADDRESS, now)
                .expect("a good link is never held back");
        }
        limit.failed_at(ADDRESS, now);
        assert!(limit.admit_at(ADDRESS, now).is_ok(), "one failure is one");
    }

    #[test]
    fn the_addresses_of_one_ipv6_network_count_together() {
        let limit = SharedReads::default();
        let now = Instant::now();
        let in_network = |host: u16| {
            Some(IpAddr::V6(Ipv6Addr::new(
                0x2001, 0xdb8, 0, 1, 0, 0, 0, host,
            )))
        };
        for host in 0..u16::try_from(MAX_SHARED_READ_FAILURES).expect("a small number") {
            assert!(
                limit.admit_at(in_network(host), now).is_ok(),
                "within the limit"
            );
            limit.failed_at(in_network(host), now);
        }
        assert!(
            limit.admit_at(in_network(999), now).is_err(),
            "another host of the same /64"
        );
        let elsewhere = Some(IpAddr::V6(Ipv6Addr::new(0x2001, 0xdb8, 0, 2, 0, 0, 0, 1)));
        assert!(limit.admit_at(elsewhere, now).is_ok(), "another /64");
    }

    #[test]
    fn the_oldest_address_goes_when_there_are_too_many() {
        let limit = SharedReads::default();
        let now = Instant::now();
        for _ in 0..MAX_SHARED_READ_FAILURES {
            limit.failed_at(ADDRESS, now);
        }
        assert!(limit.admit_at(ADDRESS, now).is_err(), "limited");
        for n in 0..MAX_ADDRESSES {
            let n = u32::try_from(n).expect("a small number");
            limit.failed_at(Some(IpAddr::V4(Ipv4Addr::from(n))), now);
        }
        assert!(limit.admit_at(ADDRESS, now).is_ok(), "forgotten");
    }
}
