//! OIDC / OAuth2 sign-in (source `packages/core/src/{oidc,oidc-providers}.ts`).

pub mod providers;

pub use providers::{OidcSettings, ProviderKey, ProviderKind, ResolvedProvider};
