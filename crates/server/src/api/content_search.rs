//! Searching what the cards and the pages of a workspace say.

use fvoci_model::{SearchHit, SearchKind};
use serde::Deserialize;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::params::{ContentKind, ContentLimit, ContentText, MemberWorkspaceId};
use super::contract::tag;
use super::query::{QueryParams, int_param};
use super::workspace::{InContainer, InWorkspace};
use super::{ApiResult, json_response};
use super::{Delegated, Read};
use crate::app::App;
use crate::error::AppError;
use crate::search::{Asked, AskedError};

#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ContentSearchQuery {
    pub(super) q: String,
    pub(super) kind: String,
    pub(super) limit: String,
}

/// How many hits a search answers with unless it asks for another number.
const DEFAULT_LIMIT: i64 = 20;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new().routes(routes!(handle_search_content))
}

/// Search what the cards, pages, entries and units of a workspace say
///
/// The cards of the boards, the pages of the wikis and the units of the courses of the
/// workspace that the user has a role on, their own or a team's, the pages of its open
/// wikis and the units of its open courses for who works there (an open board they have
/// not joined is not searched), and, in the workspace of the user's own space, the
/// entries of its diaries, whose title and body together hold every word of `q`. When
/// `q` is `#` followed only by ASCII digits (such as `#12`, or `#00012` normalized to
/// `12`) and `kind` is omitted or `card`, it matches non-template cards across every
/// readable board of the workspace whose `number` equals that positive integer; any
/// other text (including non-digits mixed in) or other `kind` runs ordinary word search. The
/// title of an entry is its day (`2026-11-21`). Chapters, comments, property values and
/// attached files are not searched, and neither are templates. How words match depends
/// on the server: with a search server, Korean is matched by its words, whatever
/// particles and endings they carry, and a compound by its parts too ("환불 규정" finds
/// "환불규정을"); without one, each word (what is between white space) is in the title
/// or the body as it is written there, ASCII letters in either case ("환불" finds
/// "환불규정을", "배포하다" does not find "배포합니다"). Each hit is the card, the page,
/// the entry or the unit as it is now: its `kind`, its `title`, the container it is in
/// (`containerId`, `containerKind`, and `containerTitle`, by which a client tells their
/// addresses and names apart),
/// and `snippet`, a piece of its body in stretches of plain text, those that matched
/// marked; the snippet is empty for a body that changed a moment ago, or has nothing to
/// show. The best match first (without a search server: those whose title holds every
/// word, then the most recently changed). Nothing says how many match in all: fewer than
/// `limit` means that no more of what the user reads matches (for a user with a role on
/// more containers than a search names, 1,000 with a search server and 10,000 without:
/// among the 10,000 best matches of the workspace).
///
/// A `q` of more than 200 characters, a `kind` that is none and a `limit` outside 1
/// to 50 are 400; a `q` that is empty finds nothing. A user who searches more often
/// than the server lets one is told to wait (429, `Retry-After`). While the search
/// server does not answer, where it cannot hold its index, or where a search takes more
/// than three seconds, the answer is 503 with the reason `search_unavailable`: nothing
/// else of the server waits for it, and the database does not stand in for a search
/// server.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/search",
    operation_id = "searchWorkspaceContent",
    tag = tag::WORKSPACES,
    params(MemberWorkspaceId, ContentText, ContentKind, ContentLimit),
    responses((status = 200, description = "What was found.", body = Vec<SearchHit>)),
)]
async fn handle_search_content(
    Delegated(InContainer(ws), ..): Delegated<Read, InContainer>,
    QueryParams(query): QueryParams<ContentSearchQuery>,
) -> ApiResult {
    Ok(json_response(&search_content(&ws, &query, None).await?))
}

/// What `query` finds in the workspace `ws` was entered for, as
/// `searchWorkspaceContent` finds it: in the container `within` alone if one is given.
///
/// # Errors
///
/// As `searchWorkspaceContent` answers.
pub(super) async fn search_content(
    ws: &InWorkspace,
    query: &ContentSearchQuery,
    within: Option<&str>,
) -> Result<Vec<SearchHit>, AppError> {
    let kind = match query.kind.as_str() {
        "" => None,
        word => Some(
            word.parse::<SearchKind>()
                .map_err(|error| AppError::bad_request(error).with_parameter("kind"))?,
        ),
    };
    let limit = int_param(&query.limit, "limit", DEFAULT_LIMIT)?;
    let limit = usize::try_from(limit).unwrap_or(0);
    let asked = Asked::new(&query.q, kind, limit).map_err(|error| {
        AppError::bad_request(error).with_parameter(match error {
            AskedError::TextTooLong => "q",
            AskedError::LimitOutOfRange => "limit",
        })
    })?;
    let asked = match within {
        Some(container_id) => asked.within(container_id),
        None => asked,
    };
    let found = ws
        .app
        .search_workspace(&ws.caller.user_id, &ws.workspace_id, &asked)
        .await?;
    tracing::debug!(found = found.len(), "SearchContent");
    Ok(found)
}
