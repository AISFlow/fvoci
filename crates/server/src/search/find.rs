//! A search of a workspace's bodies, and the check of what the search server answers.

use std::collections::{HashMap, HashSet};

use fvoci_model::{PublicContainers, SearchHit, SearchKind, SnippetPart};
use fvoci_store::SearchDocument;
use tokio::time::Instant;

use super::client::ClientError;
use super::database::MAX_BOUND_CONTAINERS;
use super::query::{Containers, MatchBy, Scope, card_number_query};
use super::{Engine, Way, document};
use crate::app::App;
use crate::error::{AppError, Reason};
use crate::metrics::SearchAsked;

/// The most hits a search answers with.
pub const MAX_LIMIT: usize = 50;
/// The most characters a search takes.
pub const MAX_TEXT_CHARS: usize = 200;

/// The most containers a request to the search server lists. A user who reads more
/// is searched for in the whole workspace, and what comes back is checked the same.
const MAX_LISTED_CONTAINERS: usize = 1000;
/// How many times the documents a search answers with a page asks for, where the
/// request lists the user's containers and where it cannot: the check below drops few
/// of the first, and may drop most of the second.
const ASKED_FOR_LISTED: usize = 2;
/// See [`ASKED_FOR_LISTED`].
const ASKED_FOR_ALL: usize = 5;
/// How far into what matches a search reads, a page after another: what both search
/// servers page to by default.
pub(super) const DEEPEST: usize = 10_000;

/// What a search asks for: words, and at most so many of a kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asked {
    pub(super) text: String,
    pub(super) kind: Option<SearchKind>,
    pub(super) limit: usize,
    /// The one container searched, of those the user reads; all of them if none.
    pub(super) within: Option<String>,
}

/// What of a search's parameters is out of bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AskedError {
    /// The words are more than [`MAX_TEXT_CHARS`] characters.
    #[error("the words are more than {MAX_TEXT_CHARS} characters")]
    TextTooLong,
    /// The number of hits is none, or more than [`MAX_LIMIT`].
    #[error("the number of hits is from 1 to {MAX_LIMIT}")]
    LimitOutOfRange,
}

impl Asked {
    /// A search for `text`, of `kind` if one is given, answering at most `limit` hits.
    ///
    /// # Errors
    ///
    /// Returns which of the two is out of bounds: more than [`MAX_TEXT_CHARS`]
    /// characters, or a limit that is not from 1 to [`MAX_LIMIT`].
    pub fn new(text: &str, kind: Option<SearchKind>, limit: usize) -> Result<Self, AskedError> {
        let text = text.trim();
        if text.chars().count() > MAX_TEXT_CHARS {
            return Err(AskedError::TextTooLong);
        }
        if !(1..=MAX_LIMIT).contains(&limit) {
            return Err(AskedError::LimitOutOfRange);
        }
        Ok(Self {
            text: text.to_owned(),
            kind,
            limit,
            within: None,
        })
    }

    /// The same search of the container `container_id` alone, if the user reads it:
    /// the `limit` is of its hits, and a container the user does not read has none.
    #[must_use]
    pub fn within(self, container_id: &str) -> Self {
        Self {
            within: Some(container_id.to_owned()),
            ..self
        }
    }

    /// If the query is `#12` (or `#0012` normalized to `12`) and the requested kind
    /// admits cards (`None` or `Some(SearchKind::Card)`), returns that positive number.
    pub(super) fn card_number(&self) -> Option<u32> {
        if self.kind.is_some_and(|kind| kind != SearchKind::Card) {
            return None;
        }
        card_number_query(&self.text)
    }
}

/// The 503 of a search that was not answered in time, or that the search server did
/// not answer.
pub(super) fn unavailable() -> AppError {
    AppError::service_unavailable("the search is not available for now")
        .with_reason(Reason::SearchUnavailable)
}

/// The 503 of a search server that cannot hold the index (why was said in the log when
/// it was found), until it is put right and the server started again.
pub(super) fn index_refused() -> AppError {
    AppError::service_unavailable("the search server cannot hold the index")
        .with_reason(Reason::SearchUnavailable)
}

impl App {
    /// Finds the cards and the pages of a workspace whose title or body holds the
    /// words asked for, among those the user reads, the best match first. For a handle
    /// of the workspace ([`App::in_workspace`]) that reaches the kinds of container
    /// to search ([`App::reaching`]): one that reaches boards alone finds no page. The
    /// user reads what is on the boards and in the wikis they have a role on, their
    /// own or a team's, and in the open boards and open wikis of a workspace they work
    /// in, joined or not.
    ///
    /// The search server is asked where one is configured, and the database
    /// otherwise (`search::database`); every card and page either answers with is then
    /// asked of the database (`check_hits`): what the user does not read now is left
    /// out. Where that leaves fewer than were asked for, what comes next is asked for,
    /// until there are enough, or nothing more matches: how many an answer holds says
    /// nothing of what was left out. Words that are empty find nothing.
    ///
    /// # Errors
    ///
    /// Returns a 429 for a user who searches too often, a 503 with the reason
    /// `search_unavailable` if the search server cannot hold the index, or if it or the
    /// database does not
    /// answer in time (three seconds), and the store's error if a fact cannot be read.
    pub async fn search_workspace(
        &self,
        user_id: &str,
        workspace_id: &str,
        asked: &Asked,
    ) -> Result<Vec<SearchHit>, AppError> {
        let search = self.search();
        let Some(way) = search.way() else {
            self.metrics().search_asked(SearchAsked::Unavailable);
            return Err(index_refused());
        };
        search
            .limiter
            .take(user_id, Instant::now())
            .map_err(|wait| {
                self.metrics().search_asked(SearchAsked::Limited);
                AppError::rate_limited(wait)
            })?;
        if asked.text.is_empty() {
            return Ok(Vec::new());
        }
        let theirs: HashSet<String> = self
            .containers_for_user_and_workspace(user_id, workspace_id, PublicContainers::OpenToRead)
            .await?
            .into_iter()
            .map(|container| container.id)
            .filter(|id| asked.within.as_ref().is_none_or(|within| within == id))
            .collect();
        if theirs.is_empty() {
            return Ok(Vec::new());
        }
        // In one order, whatever order the set has them in.
        let mut listed: Vec<String> = theirs.iter().cloned().collect();
        listed.sort_unstable();
        let most_listed = match way {
            Way::Engine(_) => MAX_LISTED_CONTAINERS,
            Way::Database => MAX_BOUND_CONTAINERS,
        };
        let (containers, asked_for) = if listed.len() > most_listed {
            (Containers::All, ASKED_FOR_ALL)
        } else {
            (Containers::Listed(&listed), ASKED_FOR_LISTED)
        };
        let mut check = Check {
            user_id,
            workspace_id,
            kind: asked.kind,
            number: asked.card_number(),
            theirs: &theirs,
            reads: HashMap::new(),
            answered: HashSet::new(),
        };
        let pages = Pages {
            containers,
            size: asked.limit.saturating_mul(asked_for),
            by: Instant::now() + search.timeout,
        };
        let found = match way {
            Way::Engine(engine) => self.search_engine(engine, &mut check, asked, &pages).await,
            Way::Database => self.search_database(&mut check, asked, &pages).await,
        };
        let found = found.inspect_err(|error| {
            if error.reason() == Some(Reason::SearchUnavailable) {
                self.metrics().search_asked(SearchAsked::Unavailable);
            }
        })?;
        self.metrics().search_asked(SearchAsked::Answered);
        Ok(found)
    }

    /// The pages of a search on the search server, checked, until there are enough.
    async fn search_engine(
        &self,
        engine: &Engine,
        check: &mut Check<'_>,
        asked: &Asked,
        pages: &Pages<'_>,
    ) -> Result<Vec<SearchHit>, AppError> {
        let org_id = self
            .store()
            .tenant()
            .org_id()
            .ok_or_else(|| AppError::internal("a search outside an organization"))?;
        let scope = Scope::Workspace {
            org_id: &org_id,
            workspace_id: check.workspace_id,
            containers: pages.containers,
            kind: asked.kind,
        };
        let match_by = match check.number {
            Some(number) => MatchBy::CardNumber(number),
            None => MatchBy::Text(&asked.text),
        };
        let size = pages.size;
        let mut found = Vec::new();
        let mut from = 0;
        loop {
            let left = pages.by.saturating_duration_since(Instant::now());
            let hits = engine
                .client
                .search(&scope, match_by, from, size, left)
                .await
                .map_err(|error| search_failed(engine, &error))?;
            let more_may_match = hits.len() == size;
            let kept = self.check_hits(check, hits, |hit| &hit.id).await?;
            found.extend(kept.into_iter().map(|(hit, document)| {
                let snippet = if matches!(match_by, MatchBy::CardNumber(_)) {
                    Vec::new()
                } else if hit.digest == document::digest(&document.title, &document.text) {
                    hit.snippet
                } else {
                    Vec::new()
                };
                found_hit(document, snippet)
            }));
            from += size;
            if found.len() >= asked.limit || !more_may_match || from + size > DEEPEST {
                break;
            }
        }
        found.truncate(asked.limit);
        Ok(found)
    }

    /// What the user reads now of what a search answered, in its order, with the
    /// document of each.
    ///
    /// The search server's index may be behind what is stored, and the database may
    /// have changed since it was searched, so a hit is no more than an identifier to
    /// ask about. A hit is kept if the database, as this handle reaches it, has a
    /// document of that identifier now ([`SearchDocument`]: a card or a page whose
    /// `isTemplate` is not `true`, that is no block of the trash, in a container that
    /// is no template), in this workspace, of the kind asked for, in a container the user
    /// has a role on or that is an open wiki, and reads ([`App::container_access`], asked
    /// once a container). What is answered of it is what the database says: the
    /// container and its kind, the kind and the title.
    pub(super) async fn check_hits<T>(
        &self,
        check: &mut Check<'_>,
        hits: Vec<T>,
        id_of: impl Fn(&T) -> &String,
    ) -> Result<Vec<(T, SearchDocument)>, AppError> {
        let ids: Vec<&str> = hits.iter().map(|hit| id_of(hit).as_str()).collect();
        let mut documents: HashMap<String, SearchDocument> = self
            .store()
            .search_documents(&ids)
            .await?
            .into_iter()
            .map(|document| (document.id.clone(), document))
            .collect();
        let mut kept = Vec::new();
        for hit in hits {
            // Taken out: an identifier answered twice has a document once.
            let Some(document) = documents.remove(id_of(&hit)) else {
                continue;
            };
            // A container the user neither has a role on nor reads as an open wiki is
            // asked nothing about: what the index answers of the containers of others
            // costs the same as what it does not.
            if document.workspace_id != check.workspace_id
                || check.kind.is_some_and(|kind| kind != document.kind)
                || check.number.is_some_and(|number| {
                    document.kind != SearchKind::Card || document.number != Some(number)
                })
                || !check.theirs.contains(&document.container_id)
                || !check.answered.insert(document.id.clone())
            {
                continue;
            }
            let reads = if let Some(reads) = check.reads.get(&document.container_id) {
                *reads
            } else {
                let reads = self
                    .container_access(check.user_id, &document.container_id)
                    .await?
                    .is_some_and(|access| {
                        access.grant.reads()
                            && !access.container.is_template
                            && access.container.workspace_id == check.workspace_id
                    });
                check.reads.insert(document.container_id.clone(), reads);
                reads
            };
            if reads {
                kept.push((hit, document));
            }
        }
        Ok(kept)
    }
}

/// What a search answers of a card or a page it kept: what the database says, and a
/// piece of its body.
pub(super) fn found_hit(document: SearchDocument, snippet: Vec<SnippetPart>) -> SearchHit {
    SearchHit {
        id: document.id,
        kind: document.kind,
        container_id: document.container_id,
        container_kind: document.container_kind,
        container_title: document.container_title,
        title: document.title,
        number: document.number,
        snippet,
    }
}

/// How a search is asked for a page after another.
pub(super) struct Pages<'a> {
    /// Which containers of the workspace are asked about.
    pub(super) containers: Containers<'a>,
    /// How many are asked for at a time.
    pub(super) size: usize,
    /// When the search is to be answered by.
    pub(super) by: Instant,
}

/// What a search checks its hits against, and what it has learned so far: kept over
/// the pages of one search.
pub(super) struct Check<'a> {
    user_id: &'a str,
    pub(super) workspace_id: &'a str,
    kind: Option<SearchKind>,
    /// The exact card number being matched, if any (`#12`).
    pub(super) number: Option<u32>,
    /// The boards and wikis of the workspace the user has a role on, and its open
    /// wikis for who works there.
    theirs: &'a HashSet<String>,
    /// Whether the user reads a board, for the boards asked about.
    reads: HashMap<String, bool>,
    /// The identifiers answered already: a document comes back once.
    answered: HashSet<String>,
}

/// The 503 of a search that failed so, noted. An index that is gone is looked for
/// again by the upkeep.
fn search_failed(engine: &Engine, error: &ClientError) -> AppError {
    // Said now and then: while the search server is away, every search fails so.
    if engine.search_failure_is_news(Instant::now(), error) {
        tracing::warn!(%error, "a search was not answered by the search server");
    }
    if error.is_missing_index() {
        engine.lose_index();
    }
    unavailable()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_asked_for_is_within_bounds() {
        let asked = Asked::new("  환불 규정 ", Some(SearchKind::Card), 50).expect("in bounds");
        assert_eq!(asked.text, "환불 규정");
        assert_eq!(Asked::new("", None, 1).expect("empty words").text, "");
        Asked::new(&"가".repeat(MAX_TEXT_CHARS), None, 20).expect("the longest");
        assert_eq!(
            Asked::new(&"가".repeat(MAX_TEXT_CHARS + 1), None, 20),
            Err(AskedError::TextTooLong)
        );
        for limit in [0, MAX_LIMIT + 1, usize::MAX] {
            assert_eq!(
                Asked::new("x", None, limit),
                Err(AskedError::LimitOutOfRange),
                "{limit}"
            );
        }
    }
}
