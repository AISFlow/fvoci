//! What a search of the bodies answers with.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::{BlockType, ContainerKind};

/// The kind of thing a search finds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum SearchKind {
    /// A card of a board.
    Card,
    /// A page of a wiki.
    Page,
    /// An entry of a diary: found by its writer alone, whose own space the diary is in.
    Entry,
    /// A unit of a course.
    Unit,
}

impl SearchKind {
    /// Every kind there is.
    pub const ALL: [Self; 4] = [Self::Card, Self::Page, Self::Entry, Self::Unit];

    /// The kind of thing a block of this type is to a search; `None` for a block a
    /// search never finds: a view, a comment, an attachment, a chapter.
    #[must_use]
    pub fn of_block(block_type: &BlockType) -> Option<Self> {
        match block_type {
            BlockType::Card => Some(Self::Card),
            BlockType::Page => Some(Self::Page),
            BlockType::Entry => Some(Self::Entry),
            BlockType::Unit => Some(Self::Unit),
            _ => None,
        }
    }

    /// The type of the blocks that are of this kind.
    #[must_use]
    pub fn block_type(self) -> BlockType {
        match self {
            Self::Card => BlockType::Card,
            Self::Page => BlockType::Page,
            Self::Entry => BlockType::Entry,
            Self::Unit => BlockType::Unit,
        }
    }

    /// The word of the kind, as the API and the index write it.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Card => "card",
            Self::Page => "page",
            Self::Entry => "entry",
            Self::Unit => "unit",
        }
    }
}

impl fmt::Display for SearchKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A word that names no kind of thing a search finds.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is no kind a search finds")]
pub struct UnknownSearchKind(String);

impl FromStr for SearchKind {
    type Err = UnknownSearchKind;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text)
            .ok_or_else(|| UnknownSearchKind(text.to_owned()))
    }
}

/// A stretch of a snippet: text as it is in the body, never markup.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SnippetPart {
    /// The text of the stretch.
    pub text: String,
    /// Whether the stretch is what the search matched.
    pub matched: bool,
}

/// One thing a search found: the user reads it now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    /// The identifier of the card, the page, the entry or the unit.
    pub id: String,
    /// What it is.
    pub kind: SearchKind,
    /// The identifier of the board, wiki, diary or course the match is in.
    pub container_id: String,
    /// What its container is: a board, a wiki, a diary or a course.
    pub container_kind: ContainerKind,
    /// The title of the board, wiki, diary or course the match is in; empty if untitled.
    #[serde(default)]
    pub container_title: String,
    /// Its title as it is now; of an entry, its day (`2026-11-21`).
    pub title: String,
    /// A card's number on its board, as `Card.number` gives it; absent for a page.
    #[schema(minimum = 1)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// A piece of its body around what matched, in stretches that follow one
    /// another; empty where the body has changed since it was indexed, or has nothing
    /// to show.
    pub snippet: Vec<SnippetPart>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_is_listed_and_reads_back() {
        for kind in SearchKind::ALL {
            // A new kind is added to `ALL` too: the store reads the blocks of its kinds.
            match kind {
                SearchKind::Card | SearchKind::Page | SearchKind::Entry | SearchKind::Unit => {}
            }
            assert_eq!(SearchKind::of_block(&kind.block_type()), Some(kind));
            assert_eq!(kind.as_str().parse(), Ok(kind));
        }
        assert_eq!(SearchKind::of_block(&BlockType::Comment), None);
        assert!("comment".parse::<SearchKind>().is_err());
    }
}
