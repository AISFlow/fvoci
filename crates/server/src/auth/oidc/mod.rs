//! Signing in through an organization's `OpenID Connect` provider: the requests to the
//! provider and the checks of what it answers. What a sign-in does with the answer is
//! the application's (`crate::app`).

pub mod fetch;
pub mod provider;
