//! A person's stars: the boards, wikis, courses, cards, pages and units of a workspace
//! they keep at hand, at the head of their sidebar and of their search.
//!
//! A star is its person's alone. It names what it is on by its identifier, and what
//! that is called is read whenever the stars are listed, for whoever lists them: what
//! they do not read then is left out, without a word.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::container::{Container, ContainerKind};
use crate::linked::{LinkedBlock, LinkedKind};

/// The most stars a person keeps in one workspace.
pub const MAX_FAVORITES: usize = 100;

/// What a star is on.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum FavoriteKind {
    /// A board, a wiki or a course.
    Container,
    /// A card of a board.
    Card,
    /// A page of a wiki.
    Page,
    /// A unit of a course.
    Unit,
}

impl FavoriteKind {
    /// The word of the kind, as the API and the database write it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Container => "container",
            Self::Card => "card",
            Self::Page => "page",
            Self::Unit => "unit",
        }
    }

    /// The kind of star of what a reference resolves to: a card, a page or a unit. None
    /// for a view of a board, which is not starred apart from its board.
    #[must_use]
    pub const fn of_linked(kind: LinkedKind) -> Option<Self> {
        match kind {
            LinkedKind::Card => Some(Self::Card),
            LinkedKind::Page => Some(Self::Page),
            LinkedKind::Unit => Some(Self::Unit),
            LinkedKind::View => None,
        }
    }
}

impl fmt::Display for FavoriteKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that is no kind of star.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown kind of favorite {0:?}")]
pub struct UnknownFavoriteKind(pub String);

impl FromStr for FavoriteKind {
    type Err = UnknownFavoriteKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "container" => Ok(Self::Container),
            "card" => Ok(Self::Card),
            "page" => Ok(Self::Page),
            "unit" => Ok(Self::Unit),
            other => Err(UnknownFavoriteKind(other.to_owned())),
        }
    }
}

/// A star as its person is shown it now: what it is on, what that is called, and where
/// it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Favorite {
    /// The identifier of what the star is on.
    pub target_id: String,
    /// What that is.
    pub kind: FavoriteKind,
    /// Its title as it is now; empty for one without.
    pub title: String,
    /// Its icon, at most one grapheme; empty for none.
    pub icon: String,
    /// A card's number on its board, as `Card.number` gives it; absent for anything
    /// else.
    #[schema(minimum = 1)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// The board, wiki or course it is, or is of.
    pub container_id: String,
    /// What that container is.
    pub container_kind: ContainerKind,
}

impl Favorite {
    /// The star on a container, as the container is now.
    #[must_use]
    pub fn of_container(container: &Container) -> Self {
        Self {
            target_id: container.id.clone(),
            kind: FavoriteKind::Container,
            title: container.title.clone(),
            icon: container.icon.clone(),
            number: None,
            container_id: container.id.clone(),
            container_kind: container.kind,
        }
    }

    /// The star on a card, a page or a unit, as a reference shows it now; none on a view
    /// ([`FavoriteKind::of_linked`]).
    #[must_use]
    pub fn of_block(linked: LinkedBlock) -> Option<Self> {
        Some(Self {
            target_id: linked.id,
            kind: FavoriteKind::of_linked(linked.kind)?,
            title: linked.title,
            icon: linked.icon,
            number: linked.number,
            container_id: linked.container_id,
            container_kind: linked.container_kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_reads_back_from_its_word() {
        for kind in [
            FavoriteKind::Container,
            FavoriteKind::Card,
            FavoriteKind::Page,
            FavoriteKind::Unit,
        ] {
            assert_eq!(kind.as_str().parse(), Ok(kind));
            assert_eq!(
                serde_json::to_value(kind).expect("JSON"),
                serde_json::json!(kind.as_str())
            );
        }
        assert!("entry".parse::<FavoriteKind>().is_err());
    }
}
