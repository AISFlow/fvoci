//! The search of a workspace's bodies in the database, for a server without a search
//! server (`search_url` empty).
//!
//! A card or a page is found if every word asked for (what was typed, split at white
//! space) is in its title or in its body as it is written there: "환불" finds
//! "환불규정을", and "배포하다" does not find "배포합니다". ASCII letters match in either
//! case, and no other letter does. Those whose title holds every word come first, then
//! the most recently changed. What the database answers is checked as what a search
//! server answers is ([`App::check_hits`]), and a search that takes longer than its
//! time is stopped in the database and answered with a 503.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use fvoci_model::{SearchHit, SearchKind};
use fvoci_store::{NumberSearch, Store, TextFound, TextSearch};

use super::Search;
use super::find::{Asked, Check, DEEPEST, Pages, found_hit, unavailable};
use super::query::Containers;
use super::snippet::snippet;
use crate::app::App;
use crate::error::AppError;

/// The most containers a statement names, one bound value each, well within what both
/// databases bind: a user who reads more is searched for in the whole workspace, a page
/// after another, and what comes back is checked the same.
pub(super) const MAX_BOUND_CONTAINERS: usize = 10_000;

/// Why a page of a search in the database was not answered in its time. The answer is
/// a 503 all the same; the log tells them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Late {
    /// Every search of the database that may run at once ran until the time.
    Turn,
    /// The pool gave no connection in time.
    Connection,
    /// The database did not finish the statement in time, and stopped it.
    Statement,
}

impl Late {
    /// What a page late in the database was late for: its statement once it was sent
    /// with time left ([`TextSearch::begun`]), and its connection before.
    fn after(begun: &AtomicBool) -> Self {
        if begun.load(Ordering::Acquire) {
            Self::Statement
        } else {
            Self::Connection
        }
    }

    /// What the log says of it.
    const fn said(self) -> &'static str {
        match self {
            Self::Turn => "a search of the database got no turn among those at once in its time",
            Self::Connection => "a search of the database got no connection in its time",
            Self::Statement => "a search of the database did not end in its time",
        }
    }
}

/// A page of what the database finds.
#[derive(Debug)]
enum Page {
    /// The identifiers found, the best first.
    Ids(Vec<String>),
    /// None by the time.
    Late(Late),
}

/// The words of what was typed, each once, in the order typed.
fn words(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        if !words.iter().any(|seen| seen == word) {
            words.push(word.to_owned());
        }
    }
    words
}

/// What a database search matches against: every word in title/body, or an exact card number.
#[derive(Clone)]
enum AskingFor {
    Words {
        words: Vec<String>,
        kind: Option<SearchKind>,
    },
    CardNumber(u32),
}

/// What a search asks the database, owned: each page is asked in a task of its own.
struct Asking {
    workspace_id: String,
    containers: Option<Vec<String>>,
    target: AskingFor,
}

impl Asking {
    /// A page of what the database finds, `size` past the `from` best, by `by`: once
    /// one of the searches of the database that may run at once is free, and late if
    /// none is by then, if the pool gives no connection by then, or if the statement
    /// does not end by then.
    ///
    /// Asked in a task of its own, which runs to its end however soon the request is
    /// given up (a browser drops the search of every word typed before the last):
    /// the statement is stopped by `by` all the same, and the connection goes back to
    /// the pool as it came.
    async fn page(
        &self,
        search: &Search,
        store: &Store,
        from: i64,
        size: i64,
        by: Instant,
    ) -> Result<Page, AppError> {
        let turn = Arc::clone(&search.database_searches);
        let Ok(Ok(turn)) = tokio::time::timeout_at(by.into(), turn.acquire_owned()).await else {
            return Ok(Page::Late(Late::Turn));
        };
        let begun = Arc::new(AtomicBool::new(false));
        let sent = Arc::clone(&begun);
        let store = store.clone();
        let workspace_id = self.workspace_id.clone();
        let containers = self.containers.clone();
        let target = self.target.clone();
        let page = tokio::spawn(async move {
            let _turn = turn;
            match target {
                AskingFor::Words { words, kind } => {
                    let search = TextSearch {
                        workspace_id: &workspace_id,
                        containers: containers.as_deref(),
                        kind,
                        words: &words,
                        begun: &sent,
                    };
                    store.search_text(&search, from, size, by).await
                }
                AskingFor::CardNumber(number) => {
                    let search = NumberSearch {
                        workspace_id: &workspace_id,
                        containers: containers.as_deref(),
                        number,
                        begun: &sent,
                    };
                    store.search_cards_by_number(&search, from, size, by).await
                }
            }
        });
        let late = || Late::after(&begun);
        // Getting a connection is waited for within the time too. The task keeps its turn
        // until it has one, and then asks for nothing but its transaction, which it ends.
        let Ok(page) = tokio::time::timeout_at(by.into(), page).await else {
            return Ok(Page::Late(late()));
        };
        match page
            .map_err(|error| AppError::internal(format_args!("the search task failed: {error}")))?
        {
            Ok(TextFound::Ids(ids)) => Ok(Page::Ids(ids)),
            Ok(TextFound::OutOfTime) => Ok(Page::Late(late())),
            // A pool that gave no connection in its own time, shorter than the search's.
            Err(error) if error.is_pool_timeout() => Ok(Page::Late(Late::Connection)),
            Err(error) => Err(AppError::from(error)),
        }
    }
}

impl App {
    /// The pages of a search in the database, checked, until there are enough.
    pub(super) async fn search_database(
        &self,
        check: &mut Check<'_>,
        asked: &Asked,
        pages: &Pages<'_>,
    ) -> Result<Vec<SearchHit>, AppError> {
        let target = match check.number {
            Some(number) => AskingFor::CardNumber(number),
            None => AskingFor::Words {
                words: words(&asked.text),
                kind: asked.kind,
            },
        };
        let asking = Asking {
            workspace_id: check.workspace_id.to_owned(),
            containers: match pages.containers {
                Containers::Listed(ids) => Some(ids.to_vec()),
                Containers::All => None,
            },
            target: target.clone(),
        };
        let size = i64::try_from(pages.size).unwrap_or(i64::MAX);
        let mut found = Vec::new();
        let mut from = 0_i64;
        loop {
            let ids = match asking
                .page(self.search(), self.store(), from, size, pages.by.into_std())
                .await?
            {
                Page::Ids(ids) => ids,
                Page::Late(late) => {
                    let word_count = match &target {
                        AskingFor::Words { words, .. } => words.len(),
                        AskingFor::CardNumber(_) => 1,
                    };
                    tracing::warn!(words = word_count, "{}", late.said());
                    return Err(unavailable());
                }
            };
            let more_may_match = ids.len() == pages.size;
            let kept = self.check_hits(check, ids, |id| id).await?;
            found.extend(kept.into_iter().map(|(_, document)| {
                let snippet = match &target {
                    AskingFor::Words { words, .. } => snippet(&document.text, words),
                    AskingFor::CardNumber(_) => Vec::new(),
                };
                found_hit(document, snippet)
            }));
            from = from.saturating_add(size);
            let deepest = i64::try_from(DEEPEST).unwrap_or(i64::MAX);
            if found.len() >= asked.limit || !more_may_match || from.saturating_add(size) > deepest
            {
                break;
            }
        }
        found.truncate(asked.limit);
        Ok(found)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_words_are_what_is_between_white_space_each_once() {
        assert_eq!(
            words("환불  규정\t환불\u{3000}API"),
            ["환불", "규정", "API"]
        );
        assert_eq!(words("   "), Vec::<String>::new());
    }

    #[test]
    fn a_page_is_late_for_its_statement_once_it_was_sent_and_for_its_connection_before() {
        assert_eq!(Late::after(&AtomicBool::new(true)), Late::Statement);
        assert_eq!(Late::after(&AtomicBool::new(false)), Late::Connection);
    }

    #[test]
    fn each_way_of_being_late_is_said_otherwise() {
        let said = [Late::Turn, Late::Connection, Late::Statement].map(Late::said);
        assert_eq!(
            said[1],
            "a search of the database got no connection in its time"
        );
        assert!(
            said[0] != said[1] && said[1] != said[2] && said[0] != said[2],
            "{said:?}"
        );
    }
}
