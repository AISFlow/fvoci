//! The body of an error response: the problem details of RFC 9457.

use serde::{Deserialize, Serialize};

/// The media type of a [`Problem`].
pub const PROBLEM_MEDIA_TYPE: &str = "application/problem+json";

/// The JSON body the server sends with every error status of the API, as
/// `application/problem+json` (RFC 9457).
///
/// A client decides by `status` and, where there is one, `reason`; both are stable,
/// and so is `type`. `title` and `detail` are English text for logs and developers.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct Problem {
    /// What kind of problem this is: `/problems/<name>` under the path the server is
    /// served at, the address of a page of text where the server describes the kind.
    /// The same kind of problem has the same `type` every time. The name is that of the
    /// `reason` where there is one, else that of the status, written with hyphens:
    /// `/problems/last-admin`, `/problems/not-found`.
    #[serde(rename = "type")]
    pub problem_type: String,
    /// The name of the `type`, in English; the same for every problem of the type.
    pub title: String,
    /// The HTTP status of the answer.
    pub status: u16,
    /// What went wrong this time, in English. Not for showing to users, and without
    /// internals.
    pub detail: String,
    /// The request this answer is to: `urn:uuid:` and the `requestId`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub instance: Option<String>,
    /// Why, where a client must tell causes of the same status apart, such as
    /// `invalid_credentials` or `last_admin`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub reason: Option<String>,
    /// What of the request was refused, one entry for each. `register`,
    /// `changePassword`, `patchMe`, `patchUserSettings`, `patchClientState`,
    /// `createSubscription`, `createUpload`, `createImportUpload`, `setPublicLink`,
    /// `getCards`, `createCard` and `patchCard` list everything their check of the
    /// request's own shape refuses, the first 100; so does the check of a category's
    /// own members. Every other refusal names the one thing it is about: what is only
    /// known from what is stored, a block of a request with several, and the first
    /// node of a document (`body`) that may not be stored.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<InvalidInput>,
    /// The limit that was exceeded.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub limit: Option<u64>,
    /// How many seconds to wait before trying again, as in the `Retry-After` header.
    #[serde(rename = "retryAfter", skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub retry_after: Option<u64>,
    /// The identifier of the request, the same as in the `x-request-id` response header
    /// and in the server's request log.
    #[serde(rename = "requestId", skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub request_id: Option<String>,
    /// The organization whose provider the session must be made through, with the
    /// reason `sso_required`: what the sign-in is started for. With the reason
    /// `last_org_admin`, the organization that would be left without an administrator.
    #[serde(rename = "orgId", skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub org_id: Option<String>,
}

/// One thing of a request that was refused, and where in the request it is: exactly
/// one of `pointer`, `parameter` and `header` says.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(default)]
pub struct InvalidInput {
    /// A JSON Pointer (RFC 6901) to the member of the request body: `/username`,
    /// `/blockPatches/2/title`.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub pointer: Option<String>,
    /// The name of the parameter of the query or of the path, or of the part of a
    /// multipart form.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub parameter: Option<String>,
    /// The name of the request header.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub header: Option<String>,
    /// What is wrong with it, in English. Not for showing to users.
    pub detail: String,
    /// Why it was refused, stable: the `reason` of the problem where it has one, else
    /// what the check found: `required` (missing or blank), `too_short`, `too_long`,
    /// `invalid_format` (not the form the member has), `invalid_characters`, `unknown`
    /// (the request has no such member), or `invalid` where no finer cause is told.
    pub reason: String,
}

/// Writes the name of a member of a JSON object as a JSON Pointer to it (RFC 6901):
/// `a/b` is `/a~1b`.
#[must_use]
pub fn json_pointer(member: &str) -> String {
    format!("/{}", pointer_token(member))
}

/// Writes one reference token of a JSON Pointer: `~` is `~0` and `/` is `~1`.
#[must_use]
pub fn pointer_token(name: &str) -> String {
    name.replace('~', "~0").replace('/', "~1")
}

#[cfg(test)]
mod tests {
    use super::{json_pointer, pointer_token};

    #[test]
    fn a_member_name_is_escaped_in_a_pointer() {
        assert_eq!(json_pointer("username"), "/username");
        assert_eq!(json_pointer("a/b~c"), "/a~1b~0c");
        assert_eq!(json_pointer(""), "/", "the member with the empty name");
        assert_eq!(pointer_token("~1"), "~01", "the tilde first");
    }
}
