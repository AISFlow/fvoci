//! A card, a page or a unit as a reference shows it, and a view as a board view does.
//!
//! A body names a block by its identifier alone (the node `reference`, and the view a
//! `boardView` shows), and what the block is called is read when the body is shown, for
//! whoever reads: [`LinkedBlock`] is that answer, for what a reference names, for what
//! names a block, for what is found to be named, and for the view a board view shows.

use serde::{Deserialize, Serialize};

use crate::block::{Block, BlockType};
use crate::card::CARD_ICON_FIELD;
use crate::container::{Container, ContainerKind};
use crate::page::PAGE_ICON_FIELD;
use crate::view_fields::ViewType;

/// What a reference may name: a card of a board, a page of a wiki or a unit of a course;
/// and the view of a board a board view shows, which no reference names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum LinkedKind {
    /// A card of a board.
    Card,
    /// A page of a wiki.
    Page,
    /// A unit of a course.
    Unit,
    /// A view of a board, as a board view of a body shows it
    /// ([`LinkedBlock::view_of`]). A reference names none: [`LinkedKind::of`] is never
    /// this.
    View,
}

impl LinkedKind {
    /// The kind of the block, if a reference may name a block of its type.
    #[must_use]
    pub fn of(block_type: &BlockType) -> Option<Self> {
        match block_type {
            BlockType::Card => Some(Self::Card),
            BlockType::Page => Some(Self::Page),
            BlockType::Unit => Some(Self::Unit),
            _ => None,
        }
    }
}

/// A card, a page or a unit as a reference shows it: what it is called, and where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkedBlock {
    /// The identifier of the card, the page or the unit.
    pub id: String,
    /// Whether it is a card, a page or a unit.
    pub kind: LinkedKind,
    /// Its title; empty for one without.
    pub title: String,
    /// A card's number on its board, as `Card.number` gives it; absent for a page, a
    /// unit, and a template of the board's cards.
    #[schema(minimum = 1)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u32>,
    /// Its icon, at most one grapheme; empty for none.
    pub icon: String,
    /// The identifier of the board, wiki or course it belongs to.
    pub container_id: String,
    /// Whether that is a board, a wiki or a course.
    pub container_kind: ContainerKind,
    /// The title of its board: told of a view alone, whose board a board view names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub container_title: Option<String>,
    /// How a view shows its cards: told of a view alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub view_type: Option<ViewType>,
}

impl LinkedBlock {
    /// The block as a reference shows it, on the container it is of; `None` for a block
    /// that is neither a card, a page nor a unit. The body of the block is not asked for.
    #[must_use]
    pub fn of(block: &Block, container: &Container) -> Option<Self> {
        let kind = LinkedKind::of(&block.block_type)?;
        let icon_field = match kind {
            LinkedKind::Card => Some(CARD_ICON_FIELD),
            LinkedKind::Page => Some(PAGE_ICON_FIELD),
            LinkedKind::Unit | LinkedKind::View => None,
        };
        let icon = icon_field
            .and_then(|field| block.fields.get(field))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Some(Self {
            id: block.id.clone(),
            kind,
            title: block.title.clone(),
            number: block.shown_number(),
            icon,
            container_id: container.id.clone(),
            container_kind: container.kind,
            container_title: None,
            view_type: None,
        })
    }

    /// A view of a board as a board view shows it, with its board's title and how it
    /// shows its cards; `None` for a block that is no view, or a container that is no
    /// board.
    #[must_use]
    pub fn view_of(view: &Block, board: &Container) -> Option<Self> {
        if view.block_type != BlockType::View
            || board.kind != ContainerKind::Board
            || view.container_id != board.id
        {
            return None;
        }
        let view_type = view
            .fields
            .get("viewType")
            .and_then(serde_json::Value::as_str)
            .and_then(|written| written.parse::<ViewType>().ok())
            .unwrap_or_default();
        Some(Self {
            id: view.id.clone(),
            kind: LinkedKind::View,
            title: view.title.clone(),
            number: None,
            icon: String::new(),
            container_id: board.id.clone(),
            container_kind: board.kind,
            container_title: Some(board.title.clone()),
            view_type: Some(view_type),
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn block(block_type: BlockType) -> Block {
        Block {
            id: "0197a5a0-0000-7000-8000-000000000001".to_owned(),
            container_id: "0197a5a0-0000-7000-8000-000000000002".to_owned(),
            block_type,
            title: "Plan".to_owned(),
            fields: json!({"icon": "📌", "order": "a"})
                .as_object()
                .expect("an object")
                .clone(),
            ..Block::default()
        }
    }

    #[test]
    fn a_card_a_page_and_a_unit_are_shown_and_nothing_else_is() {
        let container = Container {
            id: "0197a5a0-0000-7000-8000-000000000002".to_owned(),
            kind: ContainerKind::Wiki,
            ..Container::default()
        };
        let page = LinkedBlock::of(&block(BlockType::Page), &container).expect("a page is shown");
        assert_eq!(
            serde_json::to_value(&page).expect("serialized"),
            json!({"id": "0197a5a0-0000-7000-8000-000000000001", "kind": "page",
                "title": "Plan", "icon": "📌",
                "containerId": "0197a5a0-0000-7000-8000-000000000002", "containerKind": "wiki"}),
        );
        let course = Container {
            id: "0197a5a0-0000-7000-8000-000000000002".to_owned(),
            kind: ContainerKind::Course,
            ..Container::default()
        };
        let unit = LinkedBlock::of(&block(BlockType::Unit), &course).expect("a unit is shown");
        assert_eq!(
            serde_json::to_value(&unit).expect("serialized"),
            json!({"id": "0197a5a0-0000-7000-8000-000000000001", "kind": "unit",
                "title": "Plan", "icon": "",
                "containerId": "0197a5a0-0000-7000-8000-000000000002", "containerKind": "course"}),
        );
        let numbered = Block {
            number: Some(12),
            ..block(BlockType::Card)
        };
        let card = LinkedBlock::of(&numbered, &container).expect("a card is shown");
        assert_eq!(card.kind, LinkedKind::Card);
        assert_eq!(
            serde_json::to_value(&card).expect("serialized")["number"],
            json!(12),
            "with its number"
        );
        for other in [
            BlockType::Chapter,
            BlockType::Entry,
            BlockType::Comment,
            BlockType::View,
            BlockType::Attachment,
        ] {
            assert_eq!(
                LinkedBlock::of(&block(other), &container),
                None,
                "{other:?}"
            );
        }
    }

    #[test]
    fn a_view_is_shown_with_its_board_and_only_on_its_board() {
        let board = Container {
            id: "0197a5a0-0000-7000-8000-000000000002".to_owned(),
            kind: ContainerKind::Board,
            title: "Roadmap".to_owned(),
            ..Container::default()
        };
        let mut view = block(BlockType::View);
        view.fields.insert("viewType".to_owned(), json!("calendar"));
        let shown = LinkedBlock::view_of(&view, &board).expect("a view is shown");
        assert_eq!(
            serde_json::to_value(&shown).expect("serialized"),
            json!({"id": "0197a5a0-0000-7000-8000-000000000001", "kind": "view",
                "title": "Plan", "icon": "",
                "containerId": "0197a5a0-0000-7000-8000-000000000002", "containerKind": "board",
                "containerTitle": "Roadmap", "viewType": "calendar"}),
        );
        assert_eq!(
            LinkedBlock::of(&view, &board),
            None,
            "no reference names a view"
        );
        assert_eq!(
            LinkedBlock::view_of(&block(BlockType::Card), &board),
            None,
            "a card is no view"
        );
        let wiki = Container {
            kind: ContainerKind::Wiki,
            ..board.clone()
        };
        assert_eq!(
            LinkedBlock::view_of(&view, &wiki),
            None,
            "a view is of a board"
        );
        let other = Container {
            id: "0197a5a0-0000-7000-8000-000000000009".to_owned(),
            ..board
        };
        assert_eq!(
            LinkedBlock::view_of(&view, &other),
            None,
            "of its own board"
        );
    }
}
