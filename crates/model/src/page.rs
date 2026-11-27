//! Pages of a wiki: a page as its operations give and take it, where it stands among
//! its siblings, and a page as its wiki's tree lists it.
//!
//! A page is a block of the type `page` ([`crate::BlockType::Page`]) of a wiki
//! ([`crate::ContainerKind::Wiki`]): its title is the block's, its text the block's body,
//! and its icon and its place among the pages of the same parent are among the block's
//! fields ([`PAGE_ICON_FIELD`], [`PAGE_ORDER_FIELD`]).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::body::{Body, BodyChange};
use crate::card::{ICON_OF_SEVERAL, is_icon};
use crate::refusal::{RefusalReason, Refusals};
use crate::{BLOCK_TITLE_MAX_CHARS, Block, BlockPatch, JsonMap};

/// The field of a page that holds its icon, usually one emoji: a string.
pub const PAGE_ICON_FIELD: &str = "icon";

/// The field of a page that holds its place among the pages of the same parent
/// ([`order_between`]): a string. Siblings are listed by it, and by identifier where
/// two have the same.
pub const PAGE_ORDER_FIELD: &str = "order";

/// The digits of an order, least first. They are in the order of their bytes, so that
/// the server's comparison and a client's comparison of strings agree.
pub(crate) const DIGITS: &[u8; 62] =
    b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// How many digits there are: one more than the index of the greatest.
const BASE: usize = DIGITS.len();

/// Why two orders have no order between them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum OrderError {
    /// An order is empty, holds a character that is no digit of an order, or ends in
    /// the least digit: [`order_between`] made none like it.
    #[error("an order is not one the server made")]
    Malformed,
    /// The first order is not before the second.
    #[error("the first order is not before the second")]
    NotInOrder,
}

/// An order between `before` and `after`: after `before` (`None` for nothing before
/// it, the first place) and before `after` (`None` for nothing after it, the last
/// place). Orders compare as strings.
///
/// An order is a fraction written in the digits `0-9A-Za-z` without its last zeros, so
/// there is always one between two that differ. At an end it steps by one digit, which
/// keeps the orders of pages added one after another at the end, or at the start,
/// short: one more character about every sixty pages. Between two it goes half way.
///
/// # Errors
///
/// Returns [`OrderError::Malformed`] for a bound that is no order this function makes,
/// and [`OrderError::NotInOrder`] unless `before` is less than `after`.
pub fn order_between(before: Option<&str>, after: Option<&str>) -> Result<String, OrderError> {
    let low = before.map(digits_of).transpose()?;
    let high = after.map(digits_of).transpose()?;
    let made = match (low, high) {
        (None, None) => vec![BASE / 2],
        (Some(low), None) => step_above(&low),
        (None, Some(high)) => step_below(&high),
        (Some(low), Some(high)) if low < high => halfway(&low, &high),
        (Some(_), Some(_)) => return Err(OrderError::NotInOrder),
    };
    Ok(made
        .into_iter()
        .map(|digit| char::from(DIGITS[digit]))
        .collect())
}

/// The greatest digit.
const TOP: usize = BASE - 1;

/// An order above `low` with nothing above it: `low` up to its first digit that is
/// not the greatest, and that digit one up.
fn step_above(low: &[usize]) -> Vec<usize> {
    let at_top = low.iter().take_while(|digit| **digit == TOP).count();
    let mut made = low[..at_top].to_vec();
    made.push(low.get(at_top).copied().unwrap_or(0) + 1);
    made
}

/// An order below `high` with nothing below it: `high` up to its first digit that is
/// not zero, and that digit one down; below a one, a zero and then the greatest digit,
/// which leaves the next step down its room.
fn step_below(high: &[usize]) -> Vec<usize> {
    let zeros = high.iter().take_while(|digit| **digit == 0).count();
    let mut made = high[..zeros].to_vec();
    // An order does not end in a zero, so it has a digit after them.
    match high.get(zeros).copied().unwrap_or(1) {
        1 => made.extend([0, TOP]),
        digit => made.push(digit - 1),
    }
    made
}

/// An order between `low` and `high`, where `low` is less than `high`.
fn halfway(low: &[usize], high: &[usize]) -> Vec<usize> {
    let digit_of = |digits: &[usize], place: usize| digits.get(place).copied().unwrap_or(0);
    // Where the two first differ, `low` taken as going on in zeros: `high` has a digit
    // there, being the greater and not ending in a zero.
    let mut place = 0;
    while high.get(place) == Some(&digit_of(low, place)) {
        place += 1;
    }
    let (below, above) = (digit_of(low, place), digit_of(high, place));
    let mut made = high[..place].to_vec();
    if above - below > 1 {
        made.push(usize::midpoint(below, above));
    } else if high.len() > place + 1 {
        // Digits that follow each other: `high` cut here is between the two.
        made.push(above);
    } else {
        // And `high` ends here: the order goes on from `low`, half way to the top.
        made.push(below);
        let rest = low.get(place + 1..).unwrap_or_default();
        let at_top = rest.iter().take_while(|digit| **digit == TOP).count();
        made.extend(&rest[..at_top]);
        made.push(usize::midpoint(digit_of(rest, at_top) + 1, BASE));
    }
    made
}

/// Whether a value is an order: a string [`order_between`] takes as a bound.
pub(crate) fn is_order_value(value: &Value) -> bool {
    value.as_str().is_some_and(|order| digits_of(order).is_ok())
}

/// Whether `order` is an order [`order_between`] takes as a bound.
pub(crate) fn is_order(order: &str) -> bool {
    digits_of(order).is_ok()
}

/// The digits of an order as their indices in [`DIGITS`].
fn digits_of(order: &str) -> Result<Vec<usize>, OrderError> {
    let digits: Vec<usize> = order
        .bytes()
        .map(|byte| DIGITS.iter().position(|digit| *digit == byte))
        .collect::<Option<_>>()
        .ok_or(OrderError::Malformed)?;
    match digits.last() {
        Some(last) if *last != 0 => Ok(digits),
        _ => Err(OrderError::Malformed),
    }
}

/// The text of a field of a block; empty if the block has none, or one that is no text.
pub(crate) fn text_field(fields: &JsonMap, field: &str) -> String {
    fields
        .get(field)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

/// A page as the tree of its wiki lists it: without its body.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct PageNode {
    /// The page's identifier.
    pub id: String,
    /// The identifier of the page it is below; empty for a page at the top of its wiki.
    pub parent_id: String,
    /// The page's title.
    pub title: String,
    /// The page's icon, at most one grapheme; empty for none.
    pub icon: String,
    /// The page's place among the pages of the same parent: they are listed by it as
    /// strings compare (by code unit, not by a locale), and by `id` where two have the
    /// same. The server makes it; a client reads it only to sort.
    pub order: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch.
    pub updated_at: i64,
}

/// What the tree lists of the page that the block is.
impl From<Block> for PageNode {
    fn from(block: Block) -> Self {
        Self {
            icon: text_field(&block.fields, PAGE_ICON_FIELD),
            order: text_field(&block.fields, PAGE_ORDER_FIELD),
            id: block.id,
            parent_id: block.parent_id,
            title: block.title,
            created_at: block.created_at,
            updated_at: block.updated_at,
        }
    }
}

/// Puts pages in the order their tree lists them in: by [`PageNode::order`], and by
/// identifier where two have the same.
pub fn sort_pages(pages: &mut [PageNode]) {
    pages.sort_by(|one, other| (&one.order, &one.id).cmp(&(&other.order, &other.id)));
}

/// A page of a wiki.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct Page {
    /// The page's identifier.
    pub id: String,
    /// The identifier of the wiki the page is of.
    pub wiki_id: String,
    /// The identifier of the page it is below; empty for a page at the top of its wiki.
    pub parent_id: String,
    /// The page's title.
    pub title: String,
    /// The page's icon, at most one grapheme; empty for none.
    pub icon: String,
    /// The page's place among the pages of the same parent, as in the tree
    /// (`getPageTree`).
    pub order: String,
    /// The page's body; `null` for a page without one.
    pub body: Option<Body>,
    /// The identifier of the user who made the page.
    pub created_by: String,
    /// The identifier of the user who last changed the page.
    pub modified_by: String,
    /// Creation time in milliseconds since the Unix epoch.
    pub created_at: i64,
    /// Last modification time in milliseconds since the Unix epoch: what `readAt` of a
    /// change of the page names.
    pub updated_at: i64,
}

impl From<Block> for Page {
    /// The page that the block of a page is.
    fn from(block: Block) -> Self {
        Self {
            icon: text_field(&block.fields, PAGE_ICON_FIELD),
            order: text_field(&block.fields, PAGE_ORDER_FIELD),
            id: block.id,
            wiki_id: block.container_id,
            parent_id: block.parent_id,
            title: block.title,
            body: block.body,
            created_by: block.created_by,
            modified_by: block.modified_by,
            created_at: block.created_at,
            updated_at: block.updated_at,
        }
    }
}

/// A page in the trash, at the top of what went with it, as the trash of a wiki lists
/// it: `restorePage` brings it back with the pages below it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct DeletedPage {
    /// The page's identifier.
    pub id: String,
    /// The page's title when it was deleted.
    pub title: String,
    /// The page's icon when it was deleted; empty for none.
    pub icon: String,
    /// The page it was below; empty for one at the top of the wiki. It may be gone: the
    /// page then comes back at the top.
    pub parent_id: String,
    /// When it was deleted, in milliseconds since the Unix epoch.
    pub deleted_at: i64,
    /// Who deleted it, who may no longer exist; empty if not known.
    pub deleted_by: String,
    /// How many pages below it went with it, and come back with it.
    pub below: u32,
}

/// A page to make.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct NewPage {
    /// The page's title. Left out, empty.
    pub title: String,
    /// The page's icon, at most one grapheme. Left out, none.
    pub icon: String,
    /// The identifier of the page it goes below: a page of the same wiki. Left out or
    /// empty, the page is at the top of the wiki.
    pub parent_id: String,
    /// The identifier of the page it goes right after: a page of the same parent. Empty
    /// for the first place; left out or `null`, the last place.
    pub after: Option<String>,
}

impl NewPage {
    /// Checks the shape of the page as a request sends it.
    ///
    /// # Errors
    ///
    /// Returns the title if it is longer than a block's may be, and the icon if it is
    /// more than one grapheme.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        refuse_title(&mut refused, Some(&self.title));
        refuse_icon(&mut refused, Some(&self.icon));
        refused.into_result()
    }
}

/// Where a page moves to, with every page below it: within its wiki.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PageMove {
    /// The identifier of the page it goes below: a page of the same wiki, neither the
    /// page itself nor one below it. Left out or empty, the page goes to the top of the
    /// wiki.
    pub parent_id: String,
    /// The identifier of the page it goes right after: another page of that parent.
    /// Empty for the first place; left out or `null`, the last place.
    pub after: Option<String>,
}

/// What a copy of a page, with every page below it, is given: its title, for the copy
/// of the page asked for alone (the pages below it keep theirs).
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PageDuplicate {
    /// The copy's title; left out or `null`, the page's own. A client names the copy
    /// in its reader's language (" (copy)"): the server does not.
    pub title: Option<String>,
}

impl PageDuplicate {
    /// Refuses, at `title`, a title longer than [`BLOCK_TITLE_MAX_CHARS`] characters.
    ///
    /// # Errors
    ///
    /// Returns the refusal if the title may not be stored.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        refuse_title(&mut refused, self.title.as_deref());
        refused.into_result()
    }
}

/// What a title that is too long is refused with.
const TITLE_TOO_LONG: &str = "the title is longer than 16383 characters";
const _: () = assert!(
    BLOCK_TITLE_MAX_CHARS == 16_383,
    "TITLE_TOO_LONG names the limit"
);

/// What a new body without `readAt` is refused with.
const READ_AT_MISSING: &str =
    "a new body is sent with readAt, the updatedAt of the page as it was read";

/// What a page is changed to. A member that is left out stays as it is. Where the page
/// is in the tree is none of them: a member the patch does not have is refused, so
/// that a request to move the page does not pass for one that did.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct PagePatch {
    /// The new title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The new icon, at most one grapheme; empty for none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    /// The new body; `null` takes the body away. Sent only with `readAt`.
    #[serde(skip_serializing_if = "BodyChange::is_keep")]
    #[schema(value_type = Option<Body>)]
    pub body: BodyChange,
    /// The `updatedAt` of the page as the client read it: if the page has changed since,
    /// the patch is refused with 409 and changes nothing. Needed with `body`, and
    /// checked whenever it is sent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_at: Option<i64>,
}

impl PagePatch {
    /// Checks the shape of the patch as a request sends it, all of it.
    ///
    /// # Errors
    ///
    /// Returns every member that is refused: a title longer than a block's may be, an
    /// icon of more than one grapheme, and a new body without `readAt`.
    pub fn check_request(&self) -> Result<(), Refusals> {
        let mut refused = Refusals::new();
        if !self.body.is_keep() && self.read_at.is_none() {
            refused.refuse("readAt", RefusalReason::Required, READ_AT_MISSING);
        }
        refuse_title(&mut refused, self.title.as_deref());
        refuse_icon(&mut refused, self.icon.as_deref());
        refused.into_result()
    }

    /// The patch of the page's block that does what this patch says.
    #[must_use]
    pub fn into_block_patch(self) -> BlockPatch {
        let mut updated_fields = JsonMap::new();
        if let Some(icon) = self.icon {
            updated_fields.insert(PAGE_ICON_FIELD.to_owned(), Value::from(icon));
        }
        BlockPatch {
            title: self.title,
            updated_fields,
            body: self.body,
            read_at: self.read_at,
            ..BlockPatch::default()
        }
    }
}

/// Refuses a title longer than a block's may be ([`BLOCK_TITLE_MAX_CHARS`]), where a
/// request sends one: named here, which the refusal of the block that is stored does
/// not do.
pub(crate) fn refuse_title(refused: &mut Refusals, title: Option<&str>) {
    if title.is_some_and(|title| title.chars().count() > BLOCK_TITLE_MAX_CHARS) {
        refused.refuse("title", RefusalReason::TooLong, TITLE_TOO_LONG);
    }
}

/// Refuses an icon of more than one grapheme.
fn refuse_icon(refused: &mut Refusals, icon: Option<&str>) {
    if icon.is_some_and(|icon| !is_icon(icon)) {
        refused.refuse("icon", RefusalReason::Invalid, ICON_OF_SEVERAL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn between(before: Option<&str>, after: Option<&str>) -> String {
        order_between(before, after).expect("an order between the two")
    }

    /// A sequence of numbers that is the same in every run.
    struct Sequence(u64);

    impl Sequence {
        fn below(&mut self, limit: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 % u64::try_from(limit).expect("a small limit"))
                .expect("less than the limit")
        }
    }

    #[test]
    fn the_first_order_leaves_room_on_both_sides() {
        let first = between(None, None);
        assert_eq!(first, "V");
        assert!(between(None, Some(&first)) < first);
        assert!(between(Some(&first), None) > first);
    }

    #[test]
    fn orders_between_two_are_between_them_wherever_they_are_asked_for() {
        let mut places = Sequence(0x2545_F491_4F6C_DD1D);
        let mut orders: Vec<String> = Vec::new();
        for _ in 0..3000 {
            let at = places.below(orders.len() + 1);
            let before = at.checked_sub(1).map(|index| orders[index].as_str());
            let after = orders.get(at).map(String::as_str);
            let made = between(before, after);
            assert!(
                before.is_none_or(|before| before < made.as_str())
                    && after.is_none_or(|after| made.as_str() < after),
                "{made:?} is between {before:?} and {after:?}"
            );
            assert!(
                !made.ends_with('0') && made.bytes().all(|byte| DIGITS.contains(&byte)),
                "{made:?} is an order"
            );
            orders.insert(at, made);
        }
        assert!(orders.is_sorted_by(|one, other| one < other));
    }

    #[test]
    fn orders_next_to_each_other_still_have_one_between() {
        for (before, after) in [
            ("1", "2"),
            ("V", "V1"),
            ("Vz", "W"),
            ("01", "1"),
            ("z", "z1"),
        ] {
            let made = between(Some(before), Some(after));
            assert!(
                before < made.as_str() && made.as_str() < after,
                "{made:?} is between {before:?} and {after:?}"
            );
        }
        assert_eq!(between(None, Some("1")), "0z");
        assert_eq!(between(None, Some("01")), "00z");
        assert_eq!(between(Some("V"), Some("W")), "VV");
        assert_eq!(between(Some("z"), None), "z1");
    }

    #[test]
    fn pages_added_at_one_end_keep_short_orders() {
        let (mut last, mut first) = (between(None, None), between(None, None));
        for _ in 0..1000 {
            last = between(Some(&last), None);
            first = between(None, Some(&first));
        }
        assert!(last.len() <= 20, "{last:?} after a thousand at the end");
        assert!(first.len() <= 20, "{first:?} after a thousand at the start");
    }

    #[test]
    fn what_is_no_order_or_not_in_order_is_refused() {
        for malformed in ["", "V0", "a-b", "ö"] {
            assert_eq!(
                order_between(Some(malformed), None),
                Err(OrderError::Malformed),
                "{malformed:?}"
            );
            assert_eq!(
                order_between(None, Some(malformed)),
                Err(OrderError::Malformed),
                "{malformed:?}"
            );
        }
        assert_eq!(
            order_between(Some("V"), Some("V")),
            Err(OrderError::NotInOrder)
        );
        assert_eq!(
            order_between(Some("W"), Some("V")),
            Err(OrderError::NotInOrder)
        );
    }

    #[test]
    fn a_page_is_a_block_no_write_of_blocks_makes() {
        use crate::{BlockError, BlockType};

        let page: BlockType = "page".parse().expect("known type");
        assert_eq!(page, BlockType::Page);
        assert_eq!(page.as_str(), "page");
        assert!(page.has_body() && !page.is_current());
        assert_eq!(
            BlockError::type_not_current(&page),
            BlockError::PageByBlocks
        );
        // Its body is edited together as a card's, an entry's and a unit's are.
        let with_room: Vec<&str> = ["card", "page", "comment", "view", "attachment", "chapter"]
            .into_iter()
            .filter(|name| name.parse::<BlockType>().expect("known type").has_room())
            .collect();
        assert_eq!(with_room, ["card", "page"]);
    }

    #[test]
    fn the_fields_of_a_page_are_an_icon_and_an_order_the_server_made() {
        use serde_json::json;

        use crate::{BlockError, BlockType};

        let with = |field: &str, value: Value| {
            let mut block = Block {
                container_id: "w".to_owned(),
                block_type: BlockType::Page,
                ..Block::default()
            };
            block.fields.insert(field.to_owned(), value);
            block.validate()
        };
        for (field, value) in [
            (PAGE_ICON_FIELD, json!("📄")),
            (PAGE_ICON_FIELD, json!(null)),
            (PAGE_ORDER_FIELD, json!("V1")),
            ("anything", json!(5)),
        ] {
            assert_eq!(with(field, value.clone()), Ok(()), "{field}: {value}");
        }
        for (field, value) in [
            (PAGE_ICON_FIELD, json!(5)),
            (PAGE_ORDER_FIELD, json!(5)),
            (PAGE_ORDER_FIELD, json!("")),
            (PAGE_ORDER_FIELD, json!("V0")),
            (PAGE_ORDER_FIELD, json!("a b")),
        ] {
            assert!(
                matches!(
                    with(field, value.clone()),
                    Err(BlockError::InvalidPageField { field: refused, .. }) if refused == field
                ),
                "{field}: {value}"
            );
        }
    }

    #[test]
    fn a_page_is_read_from_its_block_and_changed_through_it() {
        use serde_json::json;

        use crate::BlockType;

        let mut block = Block {
            id: "p".to_owned(),
            container_id: "w".to_owned(),
            parent_id: "above".to_owned(),
            block_type: BlockType::Page,
            title: "Welcome".to_owned(),
            created_by: "maker".to_owned(),
            modified_by: "editor".to_owned(),
            created_at: 1,
            updated_at: 2,
            ..Block::default()
        };
        block.fields.insert(PAGE_ICON_FIELD.to_owned(), json!("👋"));
        block.fields.insert(PAGE_ORDER_FIELD.to_owned(), json!("V"));
        let node = PageNode::from(block.clone());
        let page = Page::from(block);
        assert_eq!(
            (
                page.wiki_id.as_str(),
                page.parent_id.as_str(),
                page.icon.as_str(),
                page.order.as_str()
            ),
            ("w", "above", "👋", "V")
        );
        assert_eq!(
            (
                node.id,
                node.parent_id,
                node.title,
                node.icon,
                node.order,
                node.updated_at
            ),
            (
                page.id,
                page.parent_id,
                page.title,
                page.icon,
                page.order,
                2
            )
        );

        let patch = PagePatch {
            title: Some("Start".to_owned()),
            icon: Some(String::new()),
            ..PagePatch::default()
        };
        assert_eq!(patch.check_request(), Ok(()));
        let of_block = patch.into_block_patch();
        assert_eq!(of_block.title.as_deref(), Some("Start"));
        assert_eq!(
            of_block.updated_fields.get(PAGE_ICON_FIELD),
            Some(&json!(""))
        );
        assert!(of_block.parent_id.is_none());

        let long = "t".repeat(BLOCK_TITLE_MAX_CHARS + 1);
        assert!(
            TITLE_TOO_LONG.contains(&BLOCK_TITLE_MAX_CHARS.to_string()),
            "the refusal says the limit there is"
        );
        let refused = PagePatch {
            title: Some(long.clone()),
            icon: Some("ab".to_owned()),
            body: BodyChange::Clear,
            ..PagePatch::default()
        };
        let refused = refused.check_request().expect_err("refused");
        assert_eq!(refused.len(), 3, "{refused}");
        let new = NewPage {
            title: long,
            icon: "ab".to_owned(),
            ..NewPage::default()
        };
        let refused = new.check_request().expect_err("refused");
        assert_eq!(refused.len(), 2, "{refused}");
        let moved: Result<PagePatch, _> = serde_json::from_value(json!({"parentId": "p"}));
        assert!(moved.is_err(), "a patch has no place to move the page to");
    }

    #[test]
    fn the_tree_lists_pages_by_order_and_then_by_identifier() {
        let page = |id: &str, order: &str| PageNode {
            id: id.to_owned(),
            order: order.to_owned(),
            ..PageNode::default()
        };
        let mut pages = [page("b", "V"), page("c", "1"), page("a", "V")];
        sort_pages(&mut pages);
        let ids: Vec<&str> = pages.iter().map(|page| page.id.as_str()).collect();
        assert_eq!(ids, ["c", "a", "b"]);
    }
}
