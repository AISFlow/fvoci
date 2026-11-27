//! The routes a personal API token is sent to: those that say so by their extractor,
//! [`Delegated`], and the MCP server, which takes a token and nothing else
//! ([`crate::api::mcp::TokenCaller`]) and enters a workspace for each tool as
//! [`Delegated`] does for a route; no other.
//!
//! Every extractor of a workspace takes a session alone ([`Admits::Sessions`]): a token
//! is refused where a session is looked up (403, reason `token_forbidden_endpoint`).
//! `Delegated<Read, W>` and `Delegated<Write, W>` enter the same workspace as `W` does,
//! for a session or for a token whose scope covers what the route does. What it does is
//! the route's to say, by which of the two it takes: never read of the HTTP method, as
//! a `POST` that only reads (`resolveReferences`) and a `GET` that writes are both
//! there.

use std::future::Future;
use std::marker::PhantomData;

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use fvoci_model::ApiTokenScope;

use crate::app::App;
use crate::error::AppError;

/// The operations that take a personal API token, by their `operationId`, with the
/// scope a token needs for each: those whose handler takes [`Delegated`], and the MCP
/// server (`postMcp`), which a token of either scope is sent to and whose tools each ask
/// for their own; as a test of the sources holds it. The contract says it of each of
/// them.
pub(crate) const TOKEN_OPERATIONS: [(&str, ApiTokenScope); 74] = [
    ("appendCardMarkdown", ApiTokenScope::Write),
    ("completeUpload", ApiTokenScope::Write),
    ("createCard", ApiTokenScope::Write),
    ("createCardAttachment", ApiTokenScope::Write),
    ("createCardComment", ApiTokenScope::Write),
    ("createPage", ApiTokenScope::Write),
    ("createPageComment", ApiTokenScope::Write),
    ("createUnit", ApiTokenScope::Write),
    ("createUnitComment", ApiTokenScope::Write),
    ("createUpload", ApiTokenScope::Write),
    ("deleteCardComment", ApiTokenScope::Write),
    ("deletePageComment", ApiTokenScope::Write),
    ("deleteUnitComment", ApiTokenScope::Write),
    ("deleteUpload", ApiTokenScope::Write),
    ("getBlocks", ApiTokenScope::Read),
    ("getCard", ApiTokenScope::Read),
    ("getCardBodyVersion", ApiTokenScope::Read),
    ("getCardBodyVersions", ApiTokenScope::Read),
    ("getCardByNumber", ApiTokenScope::Read),
    ("getCardCommentVersions", ApiTokenScope::Read),
    ("getCardComments", ApiTokenScope::Read),
    ("getCards", ApiTokenScope::Read),
    ("getCommentVersions", ApiTokenScope::Read),
    ("getContainer", ApiTokenScope::Read),
    ("getCourseSchedule", ApiTokenScope::Read),
    ("getCourses", ApiTokenScope::Read),
    ("getFile", ApiTokenScope::Read),
    ("getFileRecord", ApiTokenScope::Read),
    ("getFileVersion", ApiTokenScope::Read),
    ("getFileVersions", ApiTokenScope::Read),
    ("getMyContainerPermissions", ApiTokenScope::Read),
    ("getMyCourseProgress", ApiTokenScope::Read),
    ("getPage", ApiTokenScope::Read),
    ("getPageBodyVersion", ApiTokenScope::Read),
    ("getPageBodyVersions", ApiTokenScope::Read),
    ("getPageCommentVersions", ApiTokenScope::Read),
    ("getPageComments", ApiTokenScope::Read),
    ("getPageTree", ApiTokenScope::Read),
    ("getReferencesTo", ApiTokenScope::Read),
    ("getUnit", ApiTokenScope::Read),
    ("getUnitBodyVersion", ApiTokenScope::Read),
    ("getUnitBodyVersions", ApiTokenScope::Read),
    ("getUnitCommentVersions", ApiTokenScope::Read),
    ("getUnitComments", ApiTokenScope::Read),
    ("getUnitTree", ApiTokenScope::Read),
    ("getUpload", ApiTokenScope::Write),
    ("getWikis", ApiTokenScope::Read),
    ("getWorkspace", ApiTokenScope::Read),
    ("getWorkspaceBoards", ApiTokenScope::Read),
    ("getWorkspaceLearningToday", ApiTokenScope::Read),
    ("getWorkspaceOverview", ApiTokenScope::Read),
    ("getWorkspaceToday", ApiTokenScope::Read),
    ("getWorkspaceUser", ApiTokenScope::Read),
    ("getWorkspaceUsers", ApiTokenScope::Read),
    ("getWorkspaceUsersById", ApiTokenScope::Read),
    ("listCardFiles", ApiTokenScope::Read),
    ("listContainerFiles", ApiTokenScope::Read),
    ("listCourseExams", ApiTokenScope::Read),
    ("listCourseMeetings", ApiTokenScope::Read),
    ("listWorkspaceContainers", ApiTokenScope::Read),
    ("patchCard", ApiTokenScope::Write),
    ("patchCardComment", ApiTokenScope::Write),
    ("patchPage", ApiTokenScope::Write),
    ("patchPageComment", ApiTokenScope::Write),
    ("patchUnit", ApiTokenScope::Write),
    ("patchUnitComment", ApiTokenScope::Write),
    ("postMcp", ApiTokenScope::Read),
    ("putUnitProgress", ApiTokenScope::Write),
    ("putUploadPart", ApiTokenScope::Write),
    ("resolveReferences", ApiTokenScope::Read),
    ("searchReferences", ApiTokenScope::Read),
    ("searchWorkspaceBoards", ApiTokenScope::Read),
    ("searchWorkspaceContent", ApiTokenScope::Read),
    ("uploadFile", ApiTokenScope::Write),
];

/// The scope a personal API token needs for the operation `operation_id`; `None` for
/// one that takes no token.
pub(crate) fn token_scope_of(operation_id: &str) -> Option<ApiTokenScope> {
    TOKEN_OPERATIONS
        .iter()
        .find(|(operation, _)| *operation == operation_id)
        .map(|(_, scope)| *scope)
}

/// Which credentials an extractor of a workspace takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Admits {
    /// A session alone.
    Sessions,
    /// A session, or a personal API token whose scope covers this one.
    Tokens(ApiTokenScope),
}

/// An extractor of a workspace, entered for whom `admits` lets in: what [`Delegated`]
/// wraps. Its own `FromRequestParts` enters it for [`Admits::Sessions`].
pub(crate) trait Wall: Sized {
    /// Enters the workspace of the request for its session, or for a token if
    /// `admits` takes one.
    fn enter(
        parts: &mut Parts,
        app: &App,
        admits: Admits,
    ) -> impl Future<Output = Result<Self, AppError>> + Send;
}

/// What a route does with what a token reaches, as [`Delegated`] says it.
pub(crate) trait Needs: Send + Sync + 'static {
    /// The scope a token needs for it.
    const SCOPE: ApiTokenScope;
}

/// A route that reads.
#[derive(Debug)]
pub(crate) struct Read;

/// A route that writes the content of what it reaches.
#[derive(Debug)]
pub(crate) struct Write;

impl Needs for Read {
    const SCOPE: ApiTokenScope = ApiTokenScope::Read;
}

impl Needs for Write {
    const SCOPE: ApiTokenScope = ApiTokenScope::Write;
}

/// The workspace extractor `W`, for a session or a personal API token whose scope
/// covers `N` ([`Read`] or [`Write`]).
#[derive(Debug)]
pub(crate) struct Delegated<N, W>(pub(crate) W, pub(crate) PhantomData<N>);

impl<N: Needs, W: Wall + Send> FromRequestParts<App> for Delegated<N, W> {
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, app: &App) -> Result<Self, Self::Rejection> {
        let wall = W::enter(parts, app, Admits::Tokens(N::SCOPE)).await?;
        Ok(Self(wall, PhantomData))
    }
}

/// Implements `FromRequestParts` of each wall as entered for a session alone.
macro_rules! sessions_only {
    ($($wall:ty),+ $(,)?) => {$(
        impl axum::extract::FromRequestParts<crate::app::App> for $wall {
            type Rejection = crate::error::AppError;

            async fn from_request_parts(
                parts: &mut axum::http::request::Parts,
                app: &crate::app::App,
            ) -> Result<Self, Self::Rejection> {
                <Self as crate::api::delegated::Wall>::enter(
                    parts,
                    app,
                    crate::api::delegated::Admits::Sessions,
                )
                .await
            }
        }
    )+};
}

pub(crate) use sessions_only;

#[cfg(test)]
mod tests;
