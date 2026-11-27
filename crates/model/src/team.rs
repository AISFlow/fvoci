//! A team is a group of the members of one workspace, excluding its guests.

use serde::{Deserialize, Serialize};

use crate::auth::AuthParamError;
use crate::name::{Named, required_name};

/// A named group of people within a workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    /// The group's identifier.
    pub id: String,
    /// The workspace it belongs to.
    pub workspace_id: String,
    /// Its name, unique within the workspace in any letter case.
    pub name: String,
    /// Its people, ordered by user identifier.
    pub members: Vec<TeamMember>,
}

/// A person in a team. Neither email addresses nor credentials are exposed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    /// The person's identifier.
    pub user_id: String,
    /// Their login name.
    pub username: String,
    /// Their displayed name, empty when none is set.
    pub display_name: String,
}

/// The name when creating or renaming a team.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, utoipa::ToSchema)]
pub struct TeamName {
    /// The name, by the rule of an organization's: surrounding whitespace is removed, and
    /// what is left is 1 to 64 characters (UTF-16 code units), none of them a control
    /// character or one that shows nothing, breaks a line or turns the text around.
    pub name: String,
}

/// Reads a team name by the rule of an organization's ([`required_name`]). Returns the
/// name to store.
///
/// # Errors
/// Returns an [`AuthParamError`] for `name` if the name is blank, too long or holds a
/// character it may not.
pub fn team_name(text: &str) -> Result<String, AuthParamError> {
    required_name(text, Named::Team)
}

/// The form of a team name that names differing only in letter case share. A name is
/// unique in its workspace by this form, and teams are listed in the order of its code
/// points; the server makes it, so both databases hold, compare and order the same text.
///
/// Upper case and then lower case fold what lower case alone keeps apart: `Straße` and
/// `STRASSE`, a final sigma and another, the long s and `s`. The dotless `ı` stays apart
/// from `i`, as Unicode's case folding keeps it (Turkish `Kırmızı` is not `Kirmizi`); its
/// capital `I` folds to `i` as everywhere else.
#[must_use]
pub fn team_name_key(name: &str) -> String {
    name.split(DOTLESS_I)
        .map(|part| part.to_uppercase().to_lowercase())
        .collect::<Vec<_>>()
        .join(DOTLESS_I_TEXT)
}

/// The one letter whose capital is another letter's capital: upper case would make it `I`.
const DOTLESS_I: char = '\u{0131}';
const DOTLESS_I_TEXT: &str = "\u{0131}";
