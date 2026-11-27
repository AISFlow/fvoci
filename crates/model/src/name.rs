//! The one rule of the names people give things together: an organization, a team.

use crate::auth::{AuthParamError, ORGANIZATION_NAME_MAX_LENGTH, display_name};
use crate::refusal::RefusalReason;

/// What is named by the rule of [`required_name`]: one rule, in the words of each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Named {
    /// The organization somebody starts by signing up, and so its first workspace.
    Organization,
    /// A workspace inside an organization.
    Workspace,
    /// A team of people of a workspace.
    Team,
}

impl Named {
    /// The request member the name is sent in.
    fn field(self) -> &'static str {
        match self {
            Self::Organization => "organizationName",
            Self::Workspace | Self::Team => "name",
        }
    }

    /// What the name is called in a refusal, without and with its article.
    fn words(self) -> (&'static str, &'static str) {
        match self {
            Self::Organization => ("organization name", "an organization name"),
            Self::Workspace => ("workspace name", "a workspace name"),
            Self::Team => ("team name", "a team name"),
        }
    }
}

/// Reads a name that people give something together (an organization, a team): without
/// the white space around it, not empty, and otherwise as [`display_name`] has a name.
/// Returns the name to store.
///
/// # Errors
///
/// Returns an [`AuthParamError`] for the member the name is sent in if the name is
/// blank, too long or holds a character it may not.
pub fn required_name(raw: &str, named: Named) -> Result<String, AuthParamError> {
    let (noun, with_article) = named.words();
    let name = display_name(raw).map_err(|error| {
        let message = match error.reason() {
            RefusalReason::TooLong => {
                format!("{with_article} has at most {ORGANIZATION_NAME_MAX_LENGTH} characters")
            }
            _ => format!(
                "{with_article} has no control characters, and none that show nothing or \
                 turn the text around"
            ),
        };
        AuthParamError::new(named.field(), error.reason(), message)
    })?;
    if name.is_empty() {
        return Err(AuthParamError::new(
            named.field(),
            RefusalReason::Required,
            format!("{noun} is required"),
        ));
    }
    Ok(name)
}
