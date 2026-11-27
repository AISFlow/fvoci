//! The limits of a wiki's tree: how deep its pages go and how many it has.

use anyhow::bail;
use fvoci_model::{DEFAULT_PAGE_DEPTH, DEFAULT_WIKI_PAGES, PageLimits};
use serde::Deserialize;

pub(super) const ENV_KEYS: [(&str, super::EnvKind); 2] = [
    ("wiki_page_depth", super::EnvKind::Number),
    ("wiki_pages", super::EnvKind::Number),
];

/// The keys `wiki_*`, flattened into the main configuration file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[cfg_attr(test, derive(serde::Serialize))]
#[serde(default)]
pub struct WikiSettings {
    /// The deepest a page may be in its wiki: a page at the top is at depth 1.
    pub wiki_page_depth: u32,
    /// The most pages one wiki may have, those in the trash not counted.
    pub wiki_pages: u32,
}

impl Default for WikiSettings {
    fn default() -> Self {
        Self {
            wiki_page_depth: DEFAULT_PAGE_DEPTH,
            wiki_pages: DEFAULT_WIKI_PAGES,
        }
    }
}

impl WikiSettings {
    /// The limits as the tree of a wiki is held to them.
    #[must_use]
    pub fn page_limits(self) -> PageLimits {
        PageLimits {
            depth: self.wiki_page_depth,
            pages: self.wiki_pages,
        }
    }

    /// Checks that a wiki may have a page at all.
    pub(super) fn check(self) -> anyhow::Result<()> {
        if self.wiki_page_depth == 0 || self.wiki_pages == 0 {
            bail!("wiki_page_depth and wiki_pages must be at least 1");
        }
        Ok(())
    }
}
