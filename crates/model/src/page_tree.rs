//! The tree of a wiki's pages as a whole: where a page may go, how deep the pages are
//! and how many, and the place a page takes among its siblings, with its siblings
//! numbered anew where there is no room left between them.
//!
//! Every write of the tree (making and moving pages) decides here,
//! over the pages of the wiki read under its lock: the same rules on every database,
//! and none that a cycle of parents could send round for ever.

use std::collections::{HashMap, HashSet};

use crate::page::{DIGITS, PageNode, is_order, order_between, sort_pages};
use crate::unit::UnitNode;
use crate::{BlockError, OrderError};

/// How deep the pages of a wiki may go when nothing else says: a page at the top is at
/// depth 1.
pub const DEFAULT_PAGE_DEPTH: u32 = 10;

/// How many pages a wiki may have when nothing else says, those in the trash not
/// counted.
pub const DEFAULT_WIKI_PAGES: u32 = 5_000;

/// The longest order a page is given: past it, the pages of the parent are numbered
/// anew. One place asked for again and again grows an order by one character every five
/// or six pages, so this is some 150 pages put at one place before it happens.
pub(crate) const ORDER_MAX_CHARS: usize = 32;

/// How large the tree of a wiki may grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageLimits {
    /// The deepest a page may be: a page at the top of the wiki is at depth 1.
    pub depth: u32,
    /// The most pages the wiki may have, those in the trash not counted.
    pub pages: u32,
}

impl Default for PageLimits {
    fn default() -> Self {
        Self {
            depth: DEFAULT_PAGE_DEPTH,
            pages: DEFAULT_WIKI_PAGES,
        }
    }
}

/// A page of the tree as its decisions need it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    parent_id: String,
    order: String,
}

/// The pages of a wiki: each with its parent (empty at the top) and its order.
#[derive(Debug, Clone, Default)]
pub struct PageTree {
    pages: HashMap<String, Node>,
    children: HashMap<String, Vec<String>>,
}

/// Where a page goes among its siblings: its order, and the siblings whose orders
/// change to leave it room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    /// The order of the page that is placed.
    pub order: String,
    /// The siblings numbered anew, with their new orders, in the order they are listed
    /// in: empty where there was room between the two the page goes between.
    pub renumbered: Vec<(String, String)>,
}

impl FromIterator<PageNode> for PageTree {
    fn from_iter<I: IntoIterator<Item = PageNode>>(pages: I) -> Self {
        let mut tree = Self::default();
        for page in pages {
            tree.insert(page.id, page.parent_id, page.order);
        }
        tree
    }
}

impl FromIterator<UnitNode> for PageTree {
    fn from_iter<I: IntoIterator<Item = UnitNode>>(units: I) -> Self {
        let mut tree = Self::default();
        for unit in units {
            tree.insert(unit.id, unit.parent_id, unit.order);
        }
        tree
    }
}

impl PageTree {
    /// Adds one node to the tree.
    fn insert(&mut self, id: String, parent_id: String, order: String) {
        self.children
            .entry(parent_id.clone())
            .or_default()
            .push(id.clone());
        self.pages.insert(id, Node { parent_id, order });
    }
    /// How many pages the tree has.
    #[must_use]
    pub fn len(&self) -> usize {
        self.pages.len()
    }

    /// Whether the tree has no page.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pages.is_empty()
    }

    /// Whether `page_id` is a page of the tree.
    #[must_use]
    pub fn contains(&self, page_id: &str) -> bool {
        self.pages.contains_key(page_id)
    }

    /// The parent of a page of the tree: empty at the top, `None` for no page of it.
    #[must_use]
    pub fn parent_of(&self, page_id: &str) -> Option<&str> {
        self.pages.get(page_id).map(|node| node.parent_id.as_str())
    }

    /// How many pages lead from the top of the wiki to `page_id`, the page itself
    /// counted: 1 at the top, and 0 for the top itself (the empty identifier). A cycle
    /// of parents, which no write makes, counts as deep as the tree is large.
    #[must_use]
    pub fn depth(&self, page_id: &str) -> usize {
        let mut depth = 0;
        let mut at = page_id;
        while let Some(node) = self.pages.get(at) {
            depth += 1;
            if depth > self.pages.len() {
                break;
            }
            at = &node.parent_id;
        }
        depth
    }

    /// The page and every page below it, each once, a parent before what is below it.
    #[must_use]
    pub fn below(&self, page_id: &str) -> Vec<&str> {
        let Some((id, _)) = self.pages.get_key_value(page_id) else {
            return Vec::new();
        };
        let mut found: Vec<&str> = vec![id.as_str()];
        let mut seen: HashSet<&str> = HashSet::from([id.as_str()]);
        let mut next = 0;
        while let Some(at) = found.get(next).copied() {
            next += 1;
            for child in self.children.get(at).into_iter().flatten() {
                if seen.insert(child.as_str()) {
                    found.push(child.as_str());
                }
            }
        }
        found
    }

    /// How many levels the page and the pages below it take: 1 for a page with none
    /// below it, 0 for no page of the tree.
    #[must_use]
    pub fn height(&self, page_id: &str) -> usize {
        let below = self.below(page_id);
        let mut levels: HashMap<&str, usize> = HashMap::with_capacity(below.len());
        // Parents come first: a page's level is its parent's and one.
        let mut height = 0;
        for id in below {
            let parent = self.parent_of(id).unwrap_or_default();
            let level = if id == page_id {
                1
            } else {
                levels.get(parent).copied().unwrap_or(0) + 1
            };
            levels.insert(id, level);
            height = height.max(level);
        }
        height
    }

    /// Checks that pages may come below `parent_id` (a page of the tree, or the empty
    /// identifier for the top): `height` levels of them, `adding` pages that the tree
    /// does not have yet.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::ParentNotOnContainer`] for a parent that is no page of the
    /// tree, [`BlockError::PageTooDeep`] if the lowest of the pages would be deeper than
    /// the limit, and [`BlockError::TooManyPages`] if the tree would have more pages
    /// than it may.
    pub fn check_room(
        &self,
        parent_id: &str,
        height: usize,
        adding: usize,
        limits: PageLimits,
    ) -> Result<(), BlockError> {
        if !parent_id.is_empty() && !self.contains(parent_id) {
            return Err(BlockError::ParentNotOnContainer);
        }
        if self.depth(parent_id) + height > depth_limit(limits) {
            return Err(BlockError::PageTooDeep {
                limit: limits.depth,
            });
        }
        if adding > 0 && self.len() + adding > usize::try_from(limits.pages).unwrap_or(usize::MAX) {
            return Err(BlockError::TooManyPages {
                limit: limits.pages,
            });
        }
        Ok(())
    }

    /// Checks that the tree as a whole is within `limits`, as a copy of it would be made
    /// anew: no page deeper, and no more pages, than they let a wiki have.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::PageTooDeep`] or [`BlockError::TooManyPages`], as
    /// [`PageTree::check_room`] does.
    pub fn check_whole(&self, limits: PageLimits) -> Result<(), BlockError> {
        let height = self
            .siblings("", "")
            .iter()
            .map(|top| self.height(&top.id))
            .max()
            .unwrap_or(0);
        Self::default().check_room("", height, self.len(), limits)
    }

    /// Checks that the page `page_id` may move below `parent_id`, with every page below
    /// it: neither the page itself nor a page below it, and no deeper than the limit.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::ParentBelowBlock`] for the page itself or a page below it,
    /// and what [`PageTree::check_room`] returns for the rest.
    pub fn check_move(
        &self,
        page_id: &str,
        parent_id: &str,
        limits: PageLimits,
    ) -> Result<(), BlockError> {
        if !parent_id.is_empty() && self.below(page_id).contains(&parent_id) {
            return Err(BlockError::ParentBelowBlock);
        }
        if !parent_id.is_empty() && !self.contains(parent_id) {
            return Err(BlockError::ParentNotOnContainer);
        }
        let height = self.height(page_id);
        // Pages deeper than a limit lowered since may still move, as long as no deeper:
        // among their siblings too, however far past the limit they are.
        let deepest_now = (self.depth(page_id) + height).saturating_sub(1);
        let deepest = self.depth(parent_id) + height;
        if deepest > depth_limit(limits) && deepest > deepest_now {
            return Err(BlockError::PageTooDeep {
                limit: limits.depth,
            });
        }
        Ok(())
    }

    /// Whether the page is already below `parent_id` at the place `after` says: a move
    /// there changes nothing.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::AfterNotSibling`] if `after` names no other page below the
    /// parent.
    pub fn is_at(
        &self,
        page_id: &str,
        parent_id: &str,
        after: Option<&str>,
    ) -> Result<bool, BlockError> {
        let at = place_index(&self.siblings(parent_id, page_id), after)?;
        let now = self
            .siblings(parent_id, "")
            .iter()
            .position(|page| page.id == page_id);
        Ok(now == Some(at))
    }

    /// The pages below `parent_id`, the empty identifier for the top, but `except`, in
    /// the order they are listed in: their identifiers and their orders.
    #[must_use]
    pub fn siblings(&self, parent_id: &str, except: &str) -> Vec<PageNode> {
        let mut siblings: Vec<PageNode> = self
            .children
            .get(parent_id)
            .into_iter()
            .flatten()
            .filter(|id| id.as_str() != except)
            .filter_map(|id| {
                self.pages.get(id).map(|node| PageNode {
                    id: id.clone(),
                    parent_id: parent_id.to_owned(),
                    order: node.order.clone(),
                    ..PageNode::default()
                })
            })
            .collect();
        sort_pages(&mut siblings);
        siblings
    }

    /// Where a page goes below `parent_id` (the empty identifier for the top) at the
    /// place `after` says, among the pages there but `except` (the page itself, when it
    /// is moved there): right after the page of that identifier, first for the empty
    /// identifier, and last for `None`.
    ///
    /// # Errors
    ///
    /// Returns [`BlockError::AfterNotSibling`] if `after` names no other page below the
    /// parent.
    pub fn place(
        &self,
        parent_id: &str,
        after: Option<&str>,
        except: &str,
    ) -> Result<Place, BlockError> {
        let siblings = self.siblings(parent_id, except);
        let at = place_index(&siblings, after)?;
        Ok(place_at(&siblings, at))
    }
}

/// The deepest a page may be, as an index.
fn depth_limit(limits: PageLimits) -> usize {
    usize::try_from(limits.depth).unwrap_or(usize::MAX)
}

/// The index among `siblings` that `after` names: right after the page of that
/// identifier, first for the empty identifier, and last for `None`.
fn place_index(siblings: &[PageNode], after: Option<&str>) -> Result<usize, BlockError> {
    Ok(match after {
        None => siblings.len(),
        Some("") => 0,
        Some(after) => {
            siblings
                .iter()
                .position(|page| page.id == after)
                .ok_or(BlockError::AfterNotSibling)?
                + 1
        }
    })
}

/// The place at index `at` of `siblings`, listed in their order: between the orders of
/// the two it goes between where there is a short one, and otherwise, or where two of
/// the siblings have one order or one has none, with every sibling numbered anew.
fn place_at(siblings: &[PageNode], at: usize) -> Place {
    let before = at.checked_sub(1).and_then(|index| siblings.get(index));
    let behind = siblings.get(at);
    let between: Result<String, OrderError> = order_between(
        before.map(|page| page.order.as_str()),
        behind.map(|page| page.order.as_str()),
    );
    let in_order = siblings.iter().all(|page| is_order(&page.order))
        && siblings
            .windows(2)
            .all(|pair| pair[0].order < pair[1].order);
    if in_order
        && let Ok(order) = between
        && order.len() <= ORDER_MAX_CHARS
    {
        return Place {
            order,
            renumbered: Vec::new(),
        };
    }
    // Two of the same order, one that is no order, or no short one left between them.
    let mut orders = spread_orders(siblings.len() + 1);
    let order = orders.remove(at);
    let renumbered = siblings
        .iter()
        .zip(orders)
        .filter(|(page, order)| page.order != *order)
        .map(|(page, order)| (page.id.clone(), order))
        .collect();
    Place { order, renumbered }
}

/// How many digits an order has, for the arithmetic of [`spread_orders`].
const BASE: u128 = 62;
const _: () = assert!(DIGITS.len() == 62, "BASE is the number of digits");

/// `count` orders, each after the one before, spread evenly over the orders of as few
/// digits as leave room between every two: short, and with room at both ends.
#[must_use]
pub(crate) fn spread_orders(count: usize) -> Vec<String> {
    let base = BASE;
    // A count is less than any `u128`.
    let slots = u128::try_from(count).unwrap_or(u128::MAX).saturating_add(1);
    // Room for at least one order between every two and at the ends.
    let mut width = 1_u32;
    while base.pow(width) < slots.saturating_mul(2) {
        width += 1;
    }
    let span = base.pow(width);
    (1..slots)
        .map(|index| {
            let mut value = index * span / slots;
            let mut digits: Vec<char> = Vec::new();
            for _ in 0..width {
                let digit = usize::try_from(value % base).unwrap_or_default();
                digits.push(char::from(DIGITS[digit]));
                value /= base;
            }
            // Least first: the order is the other way, and does not end in the least
            // digit, which leaves the same fraction.
            let order: String = digits.into_iter().rev().collect();
            order.trim_end_matches(char::from(DIGITS[0])).to_owned()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parent_id: &str, order: &str) -> PageNode {
        PageNode {
            id: id.to_owned(),
            parent_id: parent_id.to_owned(),
            order: order.to_owned(),
            ..PageNode::default()
        }
    }

    /// a ─ b ─ c, and d at the top.
    fn tree() -> PageTree {
        [
            node("a", "", "V"),
            node("b", "a", "V"),
            node("c", "b", "V"),
            node("d", "", "W"),
        ]
        .into_iter()
        .collect()
    }

    fn limits(depth: u32, pages: u32) -> PageLimits {
        PageLimits { depth, pages }
    }

    #[test]
    fn spread_orders_are_short_valid_and_leave_room_between_every_two() {
        for count in [1, 2, 30, 61, 62, 63, 500, 5_000] {
            let orders = spread_orders(count);
            assert_eq!(orders.len(), count);
            assert!(
                orders.iter().all(|order| order.len() <= 3),
                "{count}: {:?}",
                orders.iter().map(String::len).max()
            );
            let mut bounds: Vec<Option<&str>> = vec![None];
            bounds.extend(orders.iter().map(|order| Some(order.as_str())));
            bounds.push(None);
            for pair in bounds.windows(2) {
                let between = order_between(pair[0], pair[1])
                    .unwrap_or_else(|error| panic!("{count}: {pair:?}: {error}"));
                assert!(between.len() <= 4, "{count}: {pair:?} gives {between:?}");
            }
        }
    }

    #[test]
    fn depth_and_height_count_the_pages_on_the_way() {
        let tree = tree();
        assert_eq!(
            ["", "a", "b", "c", "d", "none"].map(|id| tree.depth(id)),
            [0, 1, 2, 3, 1, 0]
        );
        assert_eq!(
            ["a", "b", "c", "d", "none"].map(|id| tree.height(id)),
            [3, 2, 1, 1, 0]
        );
        assert_eq!(tree.below("a"), ["a", "b", "c"]);
    }

    #[test]
    fn a_page_moves_neither_below_itself_nor_deeper_than_the_limit() {
        let tree = tree();
        let roomy = limits(10, 100);
        for parent in ["a", "b", "c"] {
            assert_eq!(
                tree.check_move("a", parent, roomy),
                Err(BlockError::ParentBelowBlock),
                "{parent}"
            );
        }
        assert_eq!(tree.check_move("b", "d", roomy), Ok(()));
        assert_eq!(tree.check_move("b", "", roomy), Ok(()));
        assert_eq!(
            tree.check_move("b", "none", roomy),
            Err(BlockError::ParentNotOnContainer)
        );
        // b and c below d are at depths 2 and 3; below c's place, deeper.
        assert_eq!(tree.check_move("b", "d", limits(3, 100)), Ok(()));
        assert_eq!(
            tree.check_move("d", "c", limits(3, 100)),
            Err(BlockError::PageTooDeep { limit: 3 })
        );
        assert_eq!(
            tree.check_room("c", 1, 1, limits(3, 100)),
            Err(BlockError::PageTooDeep { limit: 3 })
        );
        assert_eq!(tree.check_room("b", 1, 1, limits(3, 5)), Ok(()));
        assert_eq!(
            tree.check_room("b", 1, 2, limits(3, 5)),
            Err(BlockError::TooManyPages { limit: 5 })
        );
    }

    #[test]
    fn a_whole_tree_is_held_to_its_depth_and_its_count() {
        let tree = tree();
        assert_eq!(tree.check_whole(limits(3, 4)), Ok(()));
        assert_eq!(
            tree.check_whole(limits(2, 4)),
            Err(BlockError::PageTooDeep { limit: 2 })
        );
        assert_eq!(
            tree.check_whole(limits(3, 3)),
            Err(BlockError::TooManyPages { limit: 3 })
        );
        assert_eq!(PageTree::default().check_whole(limits(1, 0)), Ok(()));
    }

    #[test]
    fn pages_deeper_than_a_lowered_limit_move_as_long_as_no_deeper() {
        let tree = tree();
        // c is at depth 3, below a limit of 2 lowered since.
        assert_eq!(tree.check_move("c", "b", limits(2, 100)), Ok(()));
        assert_eq!(tree.check_move("c", "a", limits(2, 100)), Ok(()));
        assert_eq!(tree.check_move("b", "", limits(2, 100)), Ok(()));
        assert_eq!(
            tree.check_move("d", "b", limits(2, 100)),
            Err(BlockError::PageTooDeep { limit: 2 })
        );
    }

    #[test]
    fn a_page_far_past_a_lowered_limit_moves_among_its_siblings() {
        let tree = tree();
        // c is at depth 3, two levels below a limit of 1.
        assert_eq!(tree.check_move("c", "b", limits(1, 100)), Ok(()));
        assert_eq!(
            tree.check_move("c", "nowhere", limits(1, 100)),
            Err(BlockError::ParentNotOnContainer)
        );
        // A page the tree does not have goes nowhere deeper than nothing.
        assert_eq!(tree.check_move("unknown", "", limits(1, 100)), Ok(()));
    }

    #[test]
    fn a_page_is_at_the_place_it_already_has() {
        let tree = tree();
        assert_eq!(tree.is_at("d", "", None), Ok(true));
        assert_eq!(tree.is_at("d", "", Some("a")), Ok(true));
        assert_eq!(tree.is_at("d", "", Some("")), Ok(false));
        assert_eq!(tree.is_at("a", "", Some("")), Ok(true));
        assert_eq!(tree.is_at("b", "", None), Ok(false));
        assert_eq!(
            tree.is_at("d", "", Some("d")),
            Err(BlockError::AfterNotSibling)
        );
    }

    #[test]
    fn a_cycle_of_parents_ends_every_walk() {
        let tree: PageTree = [node("a", "b", "V"), node("b", "a", "V")]
            .into_iter()
            .collect();
        assert_eq!(tree.depth("a"), 3);
        assert_eq!(tree.below("a"), ["a", "b"]);
        assert!(tree.height("a") <= 2);
    }

    #[test]
    fn a_place_between_two_leaves_the_siblings_as_they_are() {
        let tree = tree();
        let place = tree.place("", Some("a"), "").expect("a place");
        assert!("V" < place.order.as_str() && place.order.as_str() < "W");
        assert_eq!(place.renumbered, []);
        assert_eq!(tree.place("", Some(""), "").expect("first").order, "U");
        assert_eq!(tree.place("", None, "").expect("last").order, "X");
        // Moved within its parent, a page is not its own neighbour.
        assert_eq!(tree.place("", None, "d").expect("last").order, "W");
        assert_eq!(
            tree.place("", Some("d"), "d"),
            Err(BlockError::AfterNotSibling)
        );
        assert_eq!(
            tree.place("", Some("b"), ""),
            Err(BlockError::AfterNotSibling)
        );
    }

    #[test]
    fn siblings_of_one_order_or_none_are_numbered_anew() {
        for orders in [["V", "V", "V"], ["", "", ""], ["V0", "V", "W"]] {
            let tree: PageTree = [
                node("p", "", orders[0]),
                node("q", "", orders[1]),
                node("r", "", orders[2]),
            ]
            .into_iter()
            .collect();
            let listed: Vec<String> = tree.siblings("", "").into_iter().map(|p| p.id).collect();
            for after in [Some(""), Some(listed[0].as_str()), Some("r"), None] {
                let place = tree.place("", after, "").expect("a place");
                let mut ordered: Vec<(String, String)> = listed
                    .iter()
                    .map(|id| {
                        let renumbered = place.renumbered.iter().find(|(one, _)| one == id);
                        let order = renumbered.map_or_else(
                            || tree.pages[id].order.clone(),
                            |(_, order)| order.clone(),
                        );
                        (id.clone(), order)
                    })
                    .collect();
                ordered.push(("new".to_owned(), place.order.clone()));
                ordered.sort_by(|one, other| one.1.cmp(&other.1));
                let orders: Vec<&str> = ordered.iter().map(|(_, order)| order.as_str()).collect();
                assert!(
                    orders.windows(2).all(|pair| pair[0] < pair[1]),
                    "{orders:?} after {after:?}"
                );
                let at = ordered.iter().position(|(id, _)| id == "new");
                let expected = match after {
                    None => Some(3),
                    Some("") => Some(0),
                    Some(after) => listed.iter().position(|id| id == after).map(|at| at + 1),
                };
                assert_eq!(at, expected, "{orders:?} after {after:?}");
            }
        }
    }

    #[test]
    fn an_order_grown_too_long_has_its_siblings_numbered_anew() {
        let long = format!("V{}", "1".repeat(ORDER_MAX_CHARS));
        let tree: PageTree = [node("p", "", &long), node("q", "", &format!("{long}2"))]
            .into_iter()
            .collect();
        let place = tree.place("", Some("p"), "").expect("a place");
        assert!(place.order.len() <= ORDER_MAX_CHARS);
        assert_eq!(place.renumbered.len(), 2, "{place:?}");
    }
}
