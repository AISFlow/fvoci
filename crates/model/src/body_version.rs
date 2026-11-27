//! The versions kept of a card's body.

use std::str::FromStr;

use serde::Serialize;
use thiserror::Error;

use crate::body::Body;

/// How a version of a card's body came to be kept, as its readers are told: by somebody
/// who asked for it, by going back to a version, or by the server on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum BodyVersionCause {
    /// By the server: when its writers left it, while it was edited on, around a body
    /// written whole, before a version was gone back to, and the first of an import
    /// or a copy. Every version kept before the cause was kept is one of these.
    #[default]
    Auto,
    /// By an editor who asked for it then (`keepCardBodyVersion`, `keepPageBodyVersion`,
    /// `keepDiaryEntryBodyVersion`, `keepUnitBodyVersion`).
    Manual,
    /// The body a version was gone back to, by who went back to it
    /// (`restoreCardBodyVersion`, `restorePageBodyVersion`, `restoreDiaryEntryBodyVersion`,
    /// `restoreUnitBodyVersion`).
    Restored,
    /// The body after an agent wrote in it through a personal API token of an editor
    /// (`appendCardMarkdown`, or a body it wrote whole): named by the token
    /// (`agentTokenName`). The body before is kept first, as of who wrote it.
    Agent,
}

impl BodyVersionCause {
    /// Every cause there is.
    pub const ALL: [Self; 4] = [Self::Auto, Self::Manual, Self::Restored, Self::Agent];

    /// Returns the wire and database form.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Manual => "manual",
            Self::Restored => "restored",
            Self::Agent => "agent",
        }
    }
}

/// Error returned when a string names no cause of a version.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{0:?} is no cause of a version")]
pub struct ParseBodyVersionCauseError(String);

impl FromStr for BodyVersionCause {
    type Err = ParseBodyVersionCauseError;

    /// Reads the wire and database form ([`BodyVersionCause::as_str`]), exactly.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|cause| cause.as_str() == text)
            .ok_or_else(|| ParseBodyVersionCauseError(text.to_owned()))
    }
}

/// A version kept of a card's body: the body as it was at a time, and who wrote it up
/// to then.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BodyVersion {
    /// The place of the version among the card's, counted from 1. The server keeps the
    /// newest ones: the lowest numbers go as new versions come.
    pub version: i64,
    /// When the version was kept, in milliseconds since the Unix epoch. For a body
    /// nobody is known to have written (one from before versions were kept, or from an
    /// import) that the server kept, when the card was last changed.
    pub kept_at: i64,
    /// Who wrote the body up to this version since the one before, in the order of
    /// their identifiers: those who edited it and those who wrote it whole. Empty for
    /// a body nobody is known to have written; who asked for a version is not named
    /// for asking.
    pub editors: Vec<String>,
    /// Whether the body of this version is the one the card has now. Of versions
    /// with the same body, the newest; of none while the card's editors have typed
    /// what is not written to the card yet.
    pub is_current: bool,
    /// How it came to be kept.
    pub cause: BodyVersionCause,
    /// The name of the personal API token an agent wrote the body through, for a
    /// version of cause `agent`; left out otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub agent_token_name: Option<String>,
}

/// A version kept of a card's body, with the body.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct BodyVersionContent {
    /// The place of the version among the card's, counted from 1.
    pub version: i64,
    /// When the version was kept, in milliseconds since the Unix epoch.
    pub kept_at: i64,
    /// Who wrote the body up to this version since the one before, in the order of
    /// their identifiers. Empty for a body nobody is known to have written.
    pub editors: Vec<String>,
    /// Whether the body of this version is the one the card has now. Of versions
    /// with the same body, the newest; of none while the card's editors have typed
    /// what is not written to the card yet.
    pub is_current: bool,
    /// How it came to be kept.
    pub cause: BodyVersionCause,
    /// As `BodyVersion.agentTokenName`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub agent_token_name: Option<String>,
    /// The body of the card then; `null` if it had none.
    pub body: Option<Body>,
}

/// What became of a body an editor asked to keep as a version now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum KeptOutcome {
    /// It is kept: `version` is the new version, by hand.
    Kept,
    /// It is the card's newest version already, and nothing is kept: `version` is that
    /// one; `null` for a card that has no body and no version.
    Unchanged,
    /// A version was kept by hand within the last minute, and nothing is kept now:
    /// `version` is the newest such. What was typed since is kept as the server keeps
    /// it on its own, or by hand once the minute is over.
    Recent,
}

/// The answer to an editor who asked to keep a body as a version now.
#[derive(Debug, Clone, PartialEq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KeptBodyVersion {
    /// Whether a version was kept, and if not, why.
    pub outcome: KeptOutcome,
    /// The version the outcome names, as the list of versions has it.
    pub version: Option<BodyVersion>,
}
