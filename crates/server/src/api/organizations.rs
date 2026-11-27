//! Starting an organization from an account that is there already, the one route about
//! organizations that is under none, and leaving one. What an organization's
//! administrators do to it is under `/org/{orgId}` too (`org`, `org_admin`).

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use fvoci_model::{NewOrganization, Problem, UserWorkspace};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::auth::address_of;
use super::contract::params::{AcceptLanguage, MemberOrgId};
use super::contract::tag;
use super::{ApiResult, RequiredSession, accept_language, created, decode_object, no_content};
use crate::app::App;
use crate::client_address::ClientAddress;

pub(super) fn routes() -> OpenApiRouter<App> {
    OpenApiRouter::new()
        .routes(routes!(handle_create_organization))
        .routes(routes!(handle_leave_organization))
}

/// Start an organization
///
/// Makes an organization that people work in together, and its first workspace, both
/// under `name`, for the account of the session, which administers both: what
/// `register` does with `organizationName`, for an account that is there. Answers the
/// workspace as `getMyWorkspaces` lists it from then on.
///
/// An account makes a few organizations and no more: the ones in use that it started
/// are counted, by `register` or here, and not the ones it was invited into, nor its
/// own space. One more than the limit is 400 with the reason `too_many_organizations`,
/// and `limit` tells how many that is. On a server that takes one organization
/// (`org_creation` is `first_user_only`; `registrationOpen` of `getLoginOptions` is
/// false once it has its first user) nobody starts another: 403 with the reason
/// `invitation_required`. The name is read as `organizationName` of `register` is:
/// blank, too long or with a character it may not have is 400, and `errors` names
/// `name`. Counted by where it comes from, with the registrations: too many are 429.
#[utoipa::path(
    post,
    path = "/org",
    operation_id = "createOrganization",
    tag = tag::ORGANIZATION,
    params(AcceptLanguage),
    request_body = NewOrganization,
    responses(
        (status = 201, description = "The first workspace of the organization that was made.", body = UserWorkspace),
        (
            status = 429,
            description = "Too many registrations and organizations from where this one comes from.",
            body = Problem,
            content_type = "application/problem+json",
            headers(("Retry-After" = u64, description = "How many seconds to wait before trying again.")),
        ),
    ),
)]
async fn handle_create_organization(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    client: Option<Extension<ClientAddress>>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult {
    let request: NewOrganization = decode_object(&body)?;
    let name = request.stored_name()?;
    let workspace = app
        .start_organization_with_language(
            &session,
            &name,
            address_of(client),
            accept_language(&headers),
        )
        .await?;
    Ok(created(&workspace))
}

/// Leave an organization
///
/// Takes the account of the session out of an organization that people work in
/// together, at its own asking, all at once or not at all. It leaves each workspace of
/// the organization, in use or in the trash, as `removeWorkspaceMember` takes a member
/// out: a board it was the only admin of passes to the workspace's oldest other
/// administrator, and a workspace it joined as the organization's administrator
/// (`joinOrgWorkspace`) records that it left. Its roles on boards and wikis, its teams,
/// what it follows (`subscriptions`) in the organization and the links of invitations it
/// made there that nobody used go with it, and so does its membership; who leaves the
/// server's own first organization administers the server no more. Its sessions,
/// its identities and what it wrote stay, and the organization stays one it made. Its
/// open connections stop following the organization's workspaces
/// (`workspaceUnsubscribed`). It comes back only as anybody new does: by an invitation,
/// or as the organization lets in who signs in through its provider. Asked of any
/// session: leaving is the account's own, whatever provider the organization takes.
///
/// 404 for an organization that is not there or the account is no member of, and for
/// an own space, which nobody leaves. A guest of a workspace is no member of its
/// organization: 404 here, and it leaves a workspace or board by `leaveContainer`.
///
/// What stands in the way is 409, and nothing changes: an account without a password
/// (`hasPassword` of `getMe` is false) must set one (`setPassword`) before leaving
/// (reason `password_required`); the server's only administrator stays in the server's
/// own first organization (reason `last_instance_admin`); the only administrator of an
/// organization that has other people in it stays until somebody else is one (reason
/// `sole_org_admin`); and nobody leaves an organization they are the only person in
/// (reason `alone_in_organization`). Asked in that order (`password_required`,
/// `last_instance_admin`, `sole_org_admin`, `alone_in_organization`). Of two
/// administrators who leave at once, the second is asked once the first is out.
#[utoipa::path(
    post,
    path = "/org/{orgId}/leave",
    operation_id = "leaveOrganization",
    tag = tag::ORGANIZATION,
    params(MemberOrgId),
    responses((status = 204, description = "The account is out of the organization.")),
)]
async fn handle_leave_organization(
    State(app): State<App>,
    RequiredSession(session): RequiredSession,
    Path(org_id): Path<String>,
) -> ApiResult {
    app.leave_organization(&session, &org_id).await?;
    Ok(no_content())
}
