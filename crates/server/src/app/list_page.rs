//! One page of a list, as a request asks for it: every list the API answers in pages
//! takes the same `page` and `perPage`.

use std::num::NonZeroU32;

use fvoci_store::Paging;

/// How many items a page has when the request does not say.
pub const DEFAULT_PER_PAGE: NonZeroU32 = NonZeroU32::new(50).expect("more than zero");

/// The most items a page has. A list longer than that is read page after page.
pub const MAX_PER_PAGE: NonZeroU32 = NonZeroU32::new(1000).expect("more than zero");

/// One page of a list: the page, counted from 0, of pages of `per_page` items, from 1 to
/// [`MAX_PER_PAGE`] (the API refuses any other).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListPage {
    /// The page, counted from 0.
    pub page: u32,
    /// How many items a page has.
    pub per_page: NonZeroU32,
}

impl ListPage {
    /// The page as the store reads it.
    pub(super) fn paging(self) -> Paging {
        Paging {
            page: self.page,
            per_page: self.per_page,
        }
    }
}
