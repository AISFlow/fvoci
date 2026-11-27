//! Password hashing: Argon2id, stored as PHC strings.
//!
//! The parameters are the ones the OWASP Password Storage Cheat Sheet summarizes as its
//! minimum for Argon2id: 19 MiB of memory, 2 iterations, 1 degree of parallelism. A hash
//! carries the parameters it was made with, so they can be raised later: a password that
//! verifies against a hash made with other parameters is reported as
//! [`Verdict::MatchWithOutdatedHash`], and the caller stores a new hash while it still
//! has the password.
//!
//! One hash takes that memory and tens of milliseconds of one processor. Callers run
//! these functions off the async runtime and bound how many run at once.

use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher as _, PasswordVerifier as _};
use argon2::{Algorithm, Argon2, Params, Version};
use thiserror::Error;

/// The memory one hash uses, in KiB.
pub const MEMORY_KIB: u32 = 19_456;

/// The number of passes over that memory.
pub const ITERATIONS: u32 = 2;

/// The degree of parallelism.
pub const PARALLELISM: u32 = 1;

/// A password could not be hashed. This is the server's failure, not the password's.
#[derive(Debug, Error)]
#[error("hashing a password failed: {0}")]
pub struct HashPasswordError(argon2::password_hash::Error);

/// What checking a password against a stored hash found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// The password is not the one the hash was made from, or the hash is none.
    Mismatch,
    /// The password is right.
    Match,
    /// The password is right, and the hash was made with other parameters than the
    /// current ones: it is time to store a new hash.
    MatchWithOutdatedHash,
}

/// The current parameters.
fn current_params() -> Params {
    Params::new(MEMORY_KIB, ITERATIONS, PARALLELISM, None)
        .expect("the constants are within the bounds of Argon2")
}

fn hasher(params: Params) -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

/// Hashes a password with the current parameters and a new random salt, into a PHC
/// string (`$argon2id$v=19$m=19456,t=2,p=1$<salt>$<hash>`).
///
/// # Errors
///
/// Returns an error if the operating system supplies no salt or the hash cannot be
/// computed.
pub fn hash_password(password: &str) -> Result<String, HashPasswordError> {
    hash_password_with(password, current_params())
}

/// Hashes a password with the given parameters. For telling how the server treats a
/// hash that was made before the parameters changed; everything else uses
/// [`hash_password`].
///
/// # Errors
///
/// Returns an error if the operating system supplies no salt or the hash cannot be
/// computed.
pub fn hash_password_with(password: &str, params: Params) -> Result<String, HashPasswordError> {
    hasher(params)
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(HashPasswordError)
}

/// Checks a password against a stored PHC string. The hash is computed with the
/// parameters the string names.
///
/// Anything that is no Argon2 PHC string matches no password, the empty password
/// included.
pub fn verify_password(hash: &str, password: &str) -> Verdict {
    let Ok(parsed) = PasswordHash::new(hash) else {
        return Verdict::Mismatch;
    };
    if hasher(current_params())
        .verify_password(password.as_bytes(), &parsed)
        .is_err()
    {
        return Verdict::Mismatch;
    }
    if is_current(&parsed) {
        Verdict::Match
    } else {
        Verdict::MatchWithOutdatedHash
    }
}

/// Does the work of checking `password` against a stored hash, for a login to an account
/// that does not exist: the answer is always "no", and it takes as long as the answer
/// for an account that does. Without it, how fast a login is refused would tell which
/// names are accounts.
pub fn verify_against_no_account(password: &str) {
    /// A hash of something nobody knows, made once, with the current parameters.
    static NOBODYS_HASH: std::sync::LazyLock<Option<String>> = std::sync::LazyLock::new(|| {
        hash_password(&crate::auth::secret::new_secret())
            .inspect_err(|error| {
                tracing::error!(%error, "unable to make the hash that stands for no account");
            })
            .ok()
    });
    match NOBODYS_HASH.as_deref() {
        Some(hash) => {
            let _mismatch = verify_password(hash, password);
        }
        // Without that hash the same work is done another way: the answer must not come
        // faster for a name that is no account.
        None => {
            let _unused = hash_password(password);
        }
    }
}

/// Whether a hash was made with the current algorithm, version and parameters.
fn is_current(hash: &PasswordHash) -> bool {
    let current = current_params();
    hash.algorithm == Algorithm::Argon2id.ident()
        && hash.version == Some(Version::V0x13.into())
        && Params::try_from(hash).is_ok_and(|params| {
            params.m_cost() == current.m_cost()
                && params.t_cost() == current.t_cost()
                && params.p_cost() == current.p_cost()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parameters at the lower bounds of Argon2, for hashes that are cheap to make.
    fn cheap_params() -> Params {
        Params::new(Params::MIN_M_COST, 1, 1, None).expect("the lower bounds are valid")
    }

    #[test]
    fn a_hash_is_a_phc_string_with_the_current_parameters() {
        let hash = hash_password("correct horse").expect("hashing succeeds");
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "the hash names its algorithm and parameters: {hash}"
        );
        assert!(!hash.contains("correct horse"), "and not the password");
        assert_ne!(
            hash,
            hash_password("correct horse").expect("hashing succeeds"),
            "every hash has a salt of its own"
        );
    }

    #[test]
    fn the_right_password_matches_and_no_other() {
        let hash = hash_password("correct horse").expect("hashing succeeds");
        assert_eq!(verify_password(&hash, "correct horse"), Verdict::Match);
        assert_eq!(verify_password(&hash, "correct horsf"), Verdict::Mismatch);
        assert_eq!(verify_password(&hash, ""), Verdict::Mismatch);
        assert_eq!(
            verify_password(&hash, "correct horse "),
            Verdict::Mismatch,
            "nothing is trimmed"
        );
    }

    #[test]
    fn a_password_of_any_length_and_any_characters_is_hashed_whole() {
        // bcrypt, which this replaces, looked at the first 72 bytes only.
        let long = "가".repeat(300);
        let hash = hash_password(&long).expect("hashing succeeds");
        assert_eq!(verify_password(&hash, &long), Verdict::Match);
        let mut differs_at_the_end = "가".repeat(299);
        differs_at_the_end.push('나');
        assert_eq!(
            verify_password(&hash, &differs_at_the_end),
            Verdict::Mismatch
        );
    }

    #[test]
    fn a_hash_made_with_other_parameters_matches_and_is_reported_as_outdated() {
        let hash = hash_password_with("correct horse", cheap_params()).expect("hashing succeeds");
        assert_eq!(
            verify_password(&hash, "correct horse"),
            Verdict::MatchWithOutdatedHash,
            "the password is right, the hash is not the current kind: {hash}"
        );
        assert_eq!(verify_password(&hash, "wrong"), Verdict::Mismatch);
    }

    #[test]
    fn what_is_no_argon2_hash_matches_nothing() {
        for hash in [
            "",
            "not-a-hash",
            "correct horse",
            // A hash of the algorithm this server used to have.
            "$2a$10$DZVQxQn1EROpkOn480U6r.1kNvclgpYT9dEdvM7Xx3MdlV83Fn90G",
            "$argon2id$v=19$m=19456,t=2,p=1$",
        ] {
            for password in ["", "correct horse", "test-password"] {
                assert_eq!(
                    verify_password(hash, password),
                    Verdict::Mismatch,
                    "hash {hash:?}, password {password:?}"
                );
            }
        }
    }
}
