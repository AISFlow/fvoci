//! Deserialization wire types for Trello board JSON exports.
//!
//! Uses streaming [`Bounded`] and [`CommentActions`] visitors so an oversized
//! JSON array is rejected immediately at element `N + 1` instead of allocating unbounded
//! vectors before post-parse count checks run.

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{self, Deserializer, SeqAccess, Visitor};

use super::summary::BundleLimits;

/// Prefix embedded in custom Serde errors when a streaming element ceiling is exceeded,
/// allowing `convert_trello_bytes` to surface `413` (`Reason::ArchiveTooLarge`).
pub(super) const TRELLO_LIMIT_PREFIX: &str = "trello_limit_exceeded:";

/// Hard streaming ceiling on total raw `actions` entries scanned in a Trello export
/// (most of which are non-comment activity records discarded on the fly).
pub(super) const MAX_ACTIONS_CEILING: usize = 100_000;

/// Maximum pre-allocated capacity trusted from a `SeqAccess::size_hint`.
const INITIAL_SEQ_CAP: usize = 256;

/// A vector deserialized with a hard streaming element ceiling `N`.
#[derive(Debug, Clone)]
pub(super) struct Bounded<T, const N: usize>(pub(super) Vec<T>);

impl<T, const N: usize> Default for Bounded<T, N> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<'de, T, const N: usize> Deserialize<'de> for Bounded<T, N>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct BoundedVisitor<T, const N: usize>(PhantomData<T>);

        impl<'de, T, const N: usize> Visitor<'de> for BoundedVisitor<T, N>
        where
            T: Deserialize<'de>,
        {
            type Value = Bounded<T, N>;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "a sequence of at most {N} elements")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let cap = seq.size_hint().unwrap_or(0).min(N).min(INITIAL_SEQ_CAP);
                let mut items = Vec::with_capacity(cap);
                while let Some(item) = seq.next_element()? {
                    if items.len() == N {
                        return Err(de::Error::custom(format!("{TRELLO_LIMIT_PREFIX}{N}")));
                    }
                    items.push(item);
                }
                Ok(Bounded(items))
            }
        }

        deserializer.deserialize_seq(BoundedVisitor(PhantomData))
    }
}

/// Streaming collector for Trello `actions` that retains only `"commentCard"` entries
/// (up to [`BundleLimits::DEFAULT_MAX_TRELLO_COMMENTS`]) and caps total scanned actions
/// at [`MAX_ACTIONS_CEILING`].
#[derive(Debug, Default)]
pub(super) struct CommentActions(pub(super) Vec<TrelloAction>);

impl<'de> Deserialize<'de> for CommentActions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ActionsVisitor;

        impl<'de> Visitor<'de> for ActionsVisitor {
            type Value = CommentActions;

            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a sequence of Trello actions")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut comments = Vec::new();
                let mut seen: usize = 0;
                while let Some(action) = seq.next_element::<TrelloAction>()? {
                    if seen == MAX_ACTIONS_CEILING {
                        return Err(de::Error::custom(format!(
                            "{TRELLO_LIMIT_PREFIX}{MAX_ACTIONS_CEILING}"
                        )));
                    }
                    seen += 1;
                    if action.action_type == "commentCard" {
                        if comments.len() == BundleLimits::DEFAULT_MAX_TRELLO_COMMENTS {
                            return Err(de::Error::custom(format!(
                                "{TRELLO_LIMIT_PREFIX}{}",
                                BundleLimits::DEFAULT_MAX_TRELLO_COMMENTS
                            )));
                        }
                        comments.push(action);
                    }
                }
                Ok(CommentActions(comments))
            }
        }

        deserializer.deserialize_seq(ActionsVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloBoard {
    pub(super) name: String,
    #[serde(default)]
    pub(super) desc: String,
    pub(super) lists: Option<Bounded<TrelloList, { BundleLimits::DEFAULT_MAX_TRELLO_LISTS }>>,
    pub(super) cards: Option<Bounded<TrelloCard, { BundleLimits::DEFAULT_MAX_TRELLO_CARDS }>>,
    #[serde(default)]
    pub(super) labels: Bounded<TrelloLabel, { BundleLimits::DEFAULT_MAX_TRELLO_LABELS }>,
    #[serde(default)]
    pub(super) checklists:
        Bounded<TrelloChecklist, { BundleLimits::DEFAULT_MAX_TRELLO_CHECKLISTS }>,
    #[serde(default)]
    pub(super) actions: CommentActions,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloList {
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) closed: bool,
    #[serde(default)]
    pub(super) pos: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloLabel {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) color: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloCard {
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) desc: String,
    #[serde(default)]
    pub(super) closed: bool,
    #[serde(default)]
    pub(super) id_list: String,
    #[serde(default)]
    pub(super) pos: f64,
    #[serde(default)]
    pub(super) due: Option<String>,
    #[serde(default)]
    pub(super) start: Option<String>,
    #[serde(default)]
    pub(super) id_labels: Bounded<String, { BundleLimits::DEFAULT_MAX_TRELLO_LABELS }>,
    #[serde(default)]
    pub(super) labels: Bounded<TrelloLabel, { BundleLimits::DEFAULT_MAX_TRELLO_LABELS }>,
    #[serde(default)]
    pub(super) id_checklists: Bounded<String, { BundleLimits::DEFAULT_MAX_TRELLO_CHECKLISTS }>,
    #[serde(default)]
    pub(super) attachments:
        Bounded<TrelloAttachment, { BundleLimits::DEFAULT_MAX_TRELLO_ATTACHMENTS }>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloChecklist {
    pub(super) id: String,
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) id_card: String,
    #[serde(default)]
    pub(super) pos: f64,
    #[serde(default)]
    pub(super) check_items:
        Bounded<TrelloCheckItem, { BundleLimits::DEFAULT_MAX_TRELLO_CHECK_ITEMS }>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloCheckItem {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) state: String,
    #[serde(default)]
    pub(super) pos: f64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloAttachment {
    #[serde(default)]
    pub(super) name: String,
    #[serde(default)]
    pub(super) url: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloAction {
    #[serde(default, rename = "type")]
    pub(super) action_type: String,
    #[serde(default)]
    pub(super) date: Option<String>,
    #[serde(default)]
    pub(super) data: TrelloActionData,
    #[serde(default)]
    pub(super) member_creator: Option<TrelloMember>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloActionData {
    #[serde(default)]
    pub(super) text: Option<String>,
    #[serde(default)]
    pub(super) card: Option<TrelloActionCard>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloActionCard {
    #[serde(default)]
    pub(super) id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TrelloMember {
    #[serde(default)]
    pub(super) full_name: Option<String>,
    #[serde(default)]
    pub(super) username: Option<String>,
}
