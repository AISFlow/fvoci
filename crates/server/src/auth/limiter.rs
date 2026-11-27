//! Limits on login attempts.
//!
//! Three things are counted, all in memory:
//!
//! * **A pair of account and address.** After a few failures in a row, each further
//!   failure makes that address wait longer before it may try that account again. Only
//!   whoever fails waits: nobody locks another user out by guessing at their account.
//! * **An account.** Failures in a row from anywhere, up to a number after which
//!   nobody signs in to the account with a password for a while. This is what stops
//!   guessing from many addresses.
//! * **An address.** How many attempts it makes per minute, whatever the account, and
//!   one attempt at a time: this is what stops one password tried on many accounts,
//!   and one address from filling the queue of password hashes.
//!
//! An account is whatever name the attempt gives: the user name, the email address, or
//! both, each counted for itself. Whether such an account exists makes no difference to
//! what is counted or answered, and nothing here is asked after the account was looked
//! up: a limit that knew the account from the name would tell, by who has to wait,
//! which names are accounts. (The one exception ends a lock: after a reset link proved
//! the account, `LoginLimiter::forget_account`; its token tells as much already.) The
//! price is that an account that is tried by its name and by its email address has the
//! failures of each for itself.
//!
//! The records are forgotten when the process ends, and the oldest go when there are
//! too many.

use std::collections::{HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sha2::{Digest as _, Sha256};

use crate::config::Configuration;
use crate::metrics::{LoginLimit, Metrics};

/// How many records of each kind are kept at most.
const MAX_RECORDS: usize = 100_000;

/// The window of the count of attempts per address.
const ADDRESS_WINDOW: Duration = Duration::from_secs(60);

/// How long an address that has an attempt in progress is told to wait before the next.
const BUSY_RETRY: Duration = Duration::from_secs(1);

/// The longest wait and the longest lock: what is configured as more is taken as this.
/// A time this far ahead can always be added to the clock.
pub const LONGEST_WAIT: Duration = Duration::from_hours(10 * 365 * 24);

/// The numbers of the limits; see the configuration keys of the same names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// Failures in a row that a pair of account and address has for free.
    pub pair_free_failures: u32,
    /// The wait after the first failure that is not free; it doubles with each further.
    pub pair_base_delay: Duration,
    /// The longest wait of a pair.
    pub pair_max_delay: Duration,
    /// Failures in a row, from anywhere, after which an account is locked.
    pub account_max_failures: u32,
    /// How long an account stays locked.
    pub account_lock: Duration,
    /// Attempts an address may make per minute; `None` for no limit by address.
    pub address_attempts_per_minute: Option<u32>,
}

impl Limits {
    /// The limits of a configuration.
    pub fn from_config(config: &Configuration) -> Self {
        Self {
            pair_free_failures: config.login_pair_free_failures,
            pair_base_delay: Duration::from_secs(config.login_pair_base_delay_seconds),
            pair_max_delay: Duration::from_secs(config.login_pair_max_delay_seconds)
                .min(LONGEST_WAIT),
            account_max_failures: config.login_account_max_failures,
            account_lock: Duration::from_secs(config.login_account_lock_seconds).min(LONGEST_WAIT),
            address_attempts_per_minute: (config.login_address_attempts_per_minute > 0)
                .then_some(config.login_address_attempts_per_minute),
        }
    }
}

/// An attempt was refused before it was looked at: try again after this long.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryAfter(pub Duration);

/// An account, by the name an attempt gave for it, in any letter case. Kept as a digest:
/// the name is of the attempt's choosing and may be long.
type AccountKey = [u8; 32];

/// An address as it is counted: an IPv4 address, or the /64 an IPv6 address is in,
/// which is what one subscriber usually has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum AddressKey {
    V4([u8; 4]),
    V6([u8; 8]),
}

impl From<IpAddr> for AddressKey {
    fn from(address: IpAddr) -> Self {
        match address.to_canonical() {
            IpAddr::V4(address) => Self::V4(address.octets()),
            IpAddr::V6(address) => {
                let octets = address.octets();
                let mut network = [0; 8];
                network.copy_from_slice(&octets[..8]);
                Self::V6(network)
            }
        }
    }
}

fn account_key(name: &str) -> AccountKey {
    Sha256::digest(name.trim().to_lowercase().as_bytes()).into()
}

#[derive(Debug, Clone, Copy)]
struct PairRecord {
    failures: u32,
    blocked_until: Instant,
    last_failure: Instant,
}

#[derive(Debug, Clone, Copy)]
struct AccountRecord {
    failures: u32,
    locked_until: Option<Instant>,
    last_failure: Instant,
}

#[derive(Debug, Default)]
struct AddressRecord {
    /// When the attempts of the last minute were made.
    attempts: VecDeque<Instant>,
    /// Whether an attempt of this address is in progress.
    busy: bool,
}

#[derive(Debug, Default)]
struct Records {
    pairs: HashMap<(AccountKey, AddressKey), PairRecord>,
    accounts: HashMap<AccountKey, AccountRecord>,
    addresses: HashMap<AddressKey, AddressRecord>,
}

/// Counts login attempts and says which may go ahead.
#[derive(Debug, Clone)]
pub struct LoginLimiter {
    limits: Limits,
    records: Arc<Mutex<Records>>,
    /// Where the refusals are counted, by the limit that refused.
    metrics: Metrics,
}

/// A request that was counted against its address and may go ahead; dropping it frees
/// the address for the next.
#[derive(Debug)]
pub struct AddressTurn {
    _attempt: Attempt,
}

/// An attempt that may go ahead. Whoever holds it says how it ended; an attempt that is
/// dropped without that counts as neither a failure nor a success, and frees its
/// address for the next attempt either way.
#[derive(Debug)]
pub struct Attempt {
    limiter: LoginLimiter,
    /// The names the attempt gave for the account; none for a registration.
    accounts: Vec<AccountKey>,
    address: Option<AddressKey>,
}

impl LoginLimiter {
    /// A limiter with these limits and no records. A wait or a lock longer than
    /// [`LONGEST_WAIT`] is taken as that.
    pub fn new(mut limits: Limits) -> Self {
        limits.pair_max_delay = limits.pair_max_delay.min(LONGEST_WAIT);
        limits.account_lock = limits.account_lock.min(LONGEST_WAIT);
        Self {
            limits,
            records: Arc::default(),
            metrics: Metrics::none(),
        }
    }

    /// This limiter, counting its refusals in `metrics`.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Metrics) -> Self {
        self.metrics = metrics;
        self
    }

    fn records(&self) -> std::sync::MutexGuard<'_, Records> {
        self.records
            .lock()
            .expect("the login records are never held across a panic")
    }

    /// Asks whether an attempt to sign in from `address` may go ahead at `now`, and
    /// counts it against the address if it may. `names` are the names the attempt gives
    /// for the account, as it gives them: its user name, its email address, or both.
    /// This is asked before the account is looked up, so that the answer cannot depend
    /// on whether there is one.
    ///
    /// # Errors
    ///
    /// Returns how long to wait if the address has made too many attempts or has one in
    /// progress, if the address is made to wait at one of the names, or if one of the
    /// names is locked. Of several waits it is the longest.
    pub fn admit_login(
        &self,
        names: &[&str],
        address: Option<IpAddr>,
        now: Instant,
    ) -> Result<Attempt, RetryAfter> {
        let mut accounts: Vec<AccountKey> = names.iter().map(|name| account_key(name)).collect();
        accounts.sort_unstable();
        accounts.dedup();
        let address = address.map(AddressKey::from);
        let mut records = self.records();
        self.wait_at(&records, &accounts, address, now)?;
        self.count_against_address(&mut records, address, now)?;
        Ok(Attempt {
            limiter: self.clone(),
            accounts,
            address,
        })
    }

    /// Refuses, with the longest of the waits and counting its limit, if one of
    /// `accounts` is locked at `now` or `address` is made to wait at one of them.
    fn wait_at(
        &self,
        records: &Records,
        accounts: &[AccountKey],
        address: Option<AddressKey>,
        now: Instant,
    ) -> Result<(), RetryAfter> {
        let waits = accounts.iter().flat_map(|account| {
            let locked = records
                .accounts
                .get(account)
                .and_then(|record| record.locked_until)
                .map(|until| (until, LoginLimit::AccountLock));
            let blocked = address
                .and_then(|address| records.pairs.get(&(*account, address)))
                .map(|record| (record.blocked_until, LoginLimit::PairDelay));
            [locked, blocked]
        });
        // The longest wait is the one answered, and the limit counted; of two as long,
        // the lock of the account.
        let Some((until, limit)) = waits
            .flatten()
            .filter(|(until, _)| *until > now)
            .max_by_key(|(until, limit)| (*until, *limit == LoginLimit::AccountLock))
        else {
            return Ok(());
        };
        self.metrics.login_refused(limit);
        Err(RetryAfter(until - now))
    }

    /// Asks whether a registration from `address` may go ahead at `now`: a registration
    /// costs a password hash like a login, and is counted against its address like one.
    ///
    /// # Errors
    ///
    /// Returns how long to wait if the address has made too many attempts or has one in
    /// progress.
    pub fn admit_registration(
        &self,
        address: Option<IpAddr>,
        now: Instant,
    ) -> Result<Attempt, RetryAfter> {
        let address = address.map(AddressKey::from);
        self.count_against_address(&mut self.records(), address, now)?;
        Ok(Attempt {
            limiter: self.clone(),
            accounts: Vec::new(),
            address,
        })
    }

    /// Asks whether a request from `address` that names no account to count may go
    /// ahead at `now`, and counts it against the address as it is admitted. The turn it
    /// returns has no ending to tell: dropping it frees the address for the next.
    ///
    /// # Errors
    ///
    /// Returns how long to wait if the address has made too many attempts or has one in
    /// progress.
    pub fn admit_from_address(
        &self,
        address: Option<IpAddr>,
        now: Instant,
    ) -> Result<AddressTurn, RetryAfter> {
        self.admit_registration(address, now)
            .map(|attempt| AddressTurn { _attempt: attempt })
    }

    /// Ends the lock of each of `names`, and the wait of `address` at them. For the
    /// owner who proved the account another way (a reset link) and chose the only
    /// password that opens it now. What other addresses wait for at those names stays,
    /// as it does when the owner signs in: no reason to let a guesser go on.
    pub fn forget_account(&self, names: &[&str], address: Option<IpAddr>) {
        let address = address.map(AddressKey::from);
        let mut records = self.records();
        for account in names.iter().map(|name| account_key(name)) {
            records.accounts.remove(&account);
            if let Some(address) = address {
                records.pairs.remove(&(account, address));
            }
        }
    }

    /// Counts an attempt against its address and marks the address as busy.
    fn count_against_address(
        &self,
        records: &mut Records,
        address: Option<AddressKey>,
        now: Instant,
    ) -> Result<(), RetryAfter> {
        let (Some(address), Some(per_minute)) = (address, self.limits.address_attempts_per_minute)
        else {
            return Ok(());
        };
        if !records.addresses.contains_key(&address) {
            make_room(
                &mut records.addresses,
                |record| {
                    !record.busy
                        && record
                            .attempts
                            .back()
                            .is_none_or(|last| now.duration_since(*last) >= ADDRESS_WINDOW)
                },
                |record| !record.busy,
                // An address with an attempt in progress is as young as can be.
                |record| match record.attempts.back() {
                    Some(last) if !record.busy => *last,
                    _ => now,
                },
            );
        }
        let record = records.addresses.entry(address).or_default();
        while record
            .attempts
            .front()
            .is_some_and(|oldest| now.duration_since(*oldest) >= ADDRESS_WINDOW)
        {
            record.attempts.pop_front();
        }
        if record.busy {
            self.metrics.login_refused(LoginLimit::AddressBusy);
            return Err(RetryAfter(BUSY_RETRY));
        }
        let allowed = usize::try_from(per_minute).unwrap_or(usize::MAX);
        if record.attempts.len() >= allowed {
            // Room again when the oldest attempt of the window is a minute old.
            let oldest = record.attempts.front().copied().unwrap_or(now);
            self.metrics.login_refused(LoginLimit::AddressRate);
            return Err(RetryAfter(
                ADDRESS_WINDOW.saturating_sub(now.duration_since(oldest)),
            ));
        }
        record.attempts.push_back(now);
        record.busy = true;
        Ok(())
    }
}

impl Attempt {
    /// Makes an attempt that named no account one at the password of `name`, where a
    /// request admitted by its address alone turns out to try one: the same turn of
    /// the address, counted once.
    ///
    /// # Errors
    ///
    /// Returns how long to wait if the name is locked at `now` or the address is made
    /// to wait at it, as [`LoginLimiter::admit_login`] does; the turn is given up.
    pub fn at_account(mut self, name: &str, now: Instant) -> Result<Self, RetryAfter> {
        let accounts = vec![account_key(name)];
        let limiter = self.limiter.clone();
        limiter.wait_at(&limiter.records(), &accounts, self.address, now)?;
        self.accounts = accounts;
        Ok(self)
    }

    /// The attempt failed: the password was wrong, or there is no such account. It
    /// counts at every name the attempt gave.
    pub fn failed(self, now: Instant) {
        let limits = self.limiter.limits;
        let mut records = self.limiter.records();
        for account in &self.accounts {
            if let Some(address) = self.address {
                count_pair_failure(&mut records.pairs, (*account, address), limits, now);
            }
            count_account_failure(&mut records.accounts, *account, limits, now);
        }
    }

    /// The attempt succeeded with `name` as the name of the account: the failures in a
    /// row at that name end here, and so do those of this address at it. What other
    /// addresses have to wait for at that name stays: the owner signing in is no reason
    /// to let a guesser go on. So does what was counted at another name the attempt
    /// gave: a right password for one account says nothing about that one.
    pub fn succeeded(self, name: &str) {
        let account = account_key(name);
        let mut records = self.limiter.records();
        records.accounts.remove(&account);
        if let Some(address) = self.address {
            records.pairs.remove(&(account, address));
        }
    }
}

/// Counts one more failure of an address at a name, and makes the address wait once the
/// free ones are used up.
fn count_pair_failure(
    pairs: &mut HashMap<(AccountKey, AddressKey), PairRecord>,
    pair: (AccountKey, AddressKey),
    limits: Limits,
    now: Instant,
) {
    if !pairs.contains_key(&pair) {
        make_room(
            pairs,
            |record| {
                record.blocked_until <= now
                    && now.duration_since(record.last_failure) >= limits.pair_max_delay
            },
            |record| record.blocked_until <= now,
            |record| record.last_failure,
        );
        if pairs.len() >= MAX_RECORDS {
            return;
        }
    }
    let record = pairs.entry(pair).or_insert(PairRecord {
        failures: 0,
        blocked_until: now,
        last_failure: now,
    });
    record.failures = record.failures.saturating_add(1);
    record.last_failure = now;
    if record.failures > limits.pair_free_failures {
        let doublings = record.failures - limits.pair_free_failures - 1;
        let delay = limits
            .pair_base_delay
            .checked_mul(2_u32.saturating_pow(doublings))
            .unwrap_or(limits.pair_max_delay)
            .min(limits.pair_max_delay);
        // No more than `LONGEST_WAIT` ahead: see `LoginLimiter::new`.
        record.blocked_until = now + delay;
    }
}

/// Counts one more failure at a name from anywhere, and locks the name at the number
/// that is too many.
fn count_account_failure(
    accounts: &mut HashMap<AccountKey, AccountRecord>,
    account: AccountKey,
    limits: Limits,
    now: Instant,
) {
    if !accounts.contains_key(&account) {
        make_room(
            accounts,
            |record| {
                record.locked_until.is_none_or(|until| until <= now)
                    && now.duration_since(record.last_failure) >= limits.account_lock
            },
            // Never evict an actively locked account when dummy names flood the table (`H-4`).
            |record| record.locked_until.is_none_or(|until| until <= now),
            |record| record.last_failure,
        );
        if accounts.len() >= MAX_RECORDS {
            return;
        }
    }
    let record = accounts.entry(account).or_insert(AccountRecord {
        failures: 0,
        locked_until: None,
        last_failure: now,
    });
    record.failures = record.failures.saturating_add(1);
    record.last_failure = now;
    if record.failures >= limits.account_max_failures {
        // No more than `LONGEST_WAIT` ahead: see `LoginLimiter::new`.
        record.locked_until = Some(now + limits.account_lock);
        // The count starts over when the lock ends.
        record.failures = 0;
    }
}

impl Drop for Attempt {
    fn drop(&mut self) {
        if let Some(address) = self.address
            && let Ok(mut records) = self.limiter.records.lock()
            && let Some(record) = records.addresses.get_mut(&address)
        {
            record.busy = false;
        }
    }
}

/// Makes room for one more record where there are [`MAX_RECORDS`]: the records that say
/// nothing any more go, and if all still say something, up to the tenth of the
/// `evictable` ones that were touched longest ago, by `last_touched`. Active locks
/// (`!evictable(record)`) are never evicted (`H-4`).
fn make_room<K: std::hash::Hash + Eq + Copy, V>(
    records: &mut HashMap<K, V>,
    says_nothing: impl Fn(&V) -> bool,
    evictable: impl Fn(&V) -> bool,
    last_touched: impl Fn(&V) -> Instant,
) {
    if records.len() < MAX_RECORDS {
        return;
    }
    records.retain(|_, record| !says_nothing(record));
    if records.len() >= MAX_RECORDS {
        for key in oldest(records, MAX_RECORDS / 10, evictable, last_touched) {
            records.remove(&key);
        }
    }
}

/// The keys of up to `count` evictable records that were touched longest ago.
fn oldest<K: Copy, V>(
    records: &HashMap<K, V>,
    count: usize,
    evictable: impl Fn(&V) -> bool,
    last_touched: impl Fn(&V) -> Instant,
) -> Vec<K> {
    let mut by_age: Vec<(Instant, K)> = records
        .iter()
        .filter(|(_, record)| evictable(record))
        .map(|(key, record)| (last_touched(record), *key))
        .collect();
    if count < by_age.len() {
        by_age.select_nth_unstable_by_key(count, |(touched, _)| *touched);
        by_age.truncate(count);
    }
    by_age.into_iter().map(|(_, key)| key).collect()
}

#[cfg(test)]
mod tests;
