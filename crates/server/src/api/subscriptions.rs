//! Subscriptions to blocks.

use axum::body::Bytes;
use axum::extract::Path;
use fvoci_model::{BlockType, RefusalReason, Refusals, Subscription};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use super::contract::bodies::NewSubscription;
use super::contract::params::MemberWorkspaceId;
use super::contract::params::{BlockId, SubscribedUserId};
use super::contract::tag;
use super::workspace::{InLinked, InWorkspace};

use super::require_permission_through;
use super::{ApiResult, created, decode_json, json_object, json_response, no_content};
use crate::app::App;
use crate::error::AppError;
use crate::permissions::ContainerPermission;

/// The refusal of this area, whatever is refused.
const ACCESS_DENIED: &str = "access denied";

/// What a request to end a subscription misses.
const SUBSCRIPTION: &str = "subscription";

pub(super) fn routes() -> OpenApiRouter<App> {
    // The first position after `/subscriptions` is a block in one route and a user in
    // the other. A router position has one name, so it gets a neutral one, and each
    // handler reads it as what it is. The contract names each as what it is: see
    // `contract::RENAMED_PATHS`.
    OpenApiRouter::new()
        .routes(routes!(handle_create_subscription))
        .routes(routes!(handle_delete_subscription))
        .routes(routes!(handle_get_subscriptions))
}

/// Decodes `blockType` of [`SubscriptionRequest`]: `null` and non-strings fail JSON
/// decoding, `""` is missing, and any other string must be a known [`BlockType`].
fn deserialize_requested_block_type<'de, D>(deserializer: D) -> Result<Option<BlockType>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = String::deserialize(deserializer)?;
    if raw.is_empty() {
        return Ok(None);
    }
    raw.parse::<BlockType>()
        .map(Some)
        .map_err(serde::de::Error::custom)
}

/// A subscription as a request body carries it. Unknown members are refused with 400.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
struct SubscriptionRequest {
    #[serde(deserialize_with = "deserialize_requested_block_type")]
    block_type: Option<BlockType>,
    block_id: String,
    user_id: String,
}

impl SubscriptionRequest {
    /// Decodes the body: a member that is missing leaves the default. A body that is no
    /// JSON object, `null` included, has unknown members, or whose members have the wrong
    /// types (`null` among them) is a bad request.
    fn decode(body: &[u8]) -> Result<Self, AppError> {
        let members = json_object(decode_json(body)?)?;
        serde_json::from_value(Value::Object(members)).map_err(AppError::bad_request)
    }

    /// Validates every member and refuses all that are wrong at once: block identifier,
    /// block type, user identifier.
    fn into_subscription(self) -> Result<Subscription, AppError> {
        let mut refused = Refusals::new();
        if self.block_id.is_empty() {
            refused.refuse("blockId", RefusalReason::Required, "missing block id");
        }
        if self.block_type.is_none() {
            refused.refuse("blockType", RefusalReason::Required, "missing block type");
        }
        if self.user_id.is_empty() {
            refused.refuse("userId", RefusalReason::Required, "missing user id");
        }
        refused.into_result()?;
        let subscription = Subscription {
            block_type: self.block_type.unwrap_or_default(),
            block_id: self.block_id,
            user_id: self.user_id,
            notified_at: 0,
            created_at: 0,
            deleted_at: None,
        };
        if let Err(error) = subscription.validate() {
            error.into_refusals().into_result()?;
        }
        Ok(subscription)
    }
}

/// Subscribe to a block
///
/// Subscribes the user of the session to a block of a board, a page of a wiki, or a unit
/// of a course they may read: whoever writes a card, a page or a unit watches it, and
/// may stop and start again. A chapter of a course takes no subscriptions (404). A
/// `userId` that is not the user of the session is 403. The `blockType` must be a known
/// `BlockType` (400 otherwise), and is stored without comparing it with the block's own
/// type. Subscribing again is no error: the
/// subscription stays as it was made, with its `blockType` and `createdAt`, and only its
/// `notifiedAt` is set anew.
#[utoipa::path(
    post,
    path = "/w/{workspaceId}/subscriptions",
    params(MemberWorkspaceId),
    operation_id = "createSubscription",
    tag = tag::SUBSCRIPTIONS,
    request_body = NewSubscription,
    responses((status = 201, description = "The subscription.", body = Subscription)),
)]
async fn handle_create_subscription(InLinked(ws): InLinked, body: Bytes) -> ApiResult {
    let subscription = SubscriptionRequest::decode(&body)?.into_subscription()?;
    if ws.kind == fvoci_model::OrgKind::Shared
        && !matches!(
            subscription.block_type,
            BlockType::Card | BlockType::Page | BlockType::Unit
        )
    {
        return Err(AppError::bad_request(
            "subscriptions in a shared organization apply only to cards, pages, and units",
        )
        .with_field("blockType"));
    }
    if ws.caller.user_id != subscription.user_id {
        return Err(
            AppError::forbidden("a user subscribes for themselves only").with_field("userId")
        );
    }
    // The request names a block and no container: check authorization before inspecting
    // the block type so an unauthorized caller cannot distinguish a chapter from a unit.
    let block = ws.app.block(&subscription.block_id).await?;
    require_permission_through(
        &ws.app,
        &ws.caller.user_id,
        &block.container_id,
        ContainerPermission::View,
        ACCESS_DENIED,
        "block",
    )
    .await?;
    if block.block_type == BlockType::Chapter {
        return Err(AppError::not_found("block"));
    }

    let subscription = ws.app.create_subscription(subscription).await?;
    tracing::debug!(
        user_id = %subscription.user_id,
        block_id = %subscription.block_id,
        "CREATE subscription"
    );
    Ok(created(&subscription))
}

/// End a subscription
///
/// Ends the subscription of the user of the session to a block. Nothing about the
/// block is checked: a subscription to a block the user may no longer see can still be
/// ended. A subscription that is not there is 404, and so are those of another user.
#[utoipa::path(
    delete,
    path = "/w/{workspaceId}/subscriptions/{blockOrUserId}/{userId}",
    operation_id = "deleteSubscription",
    tag = tag::SUBSCRIPTIONS,
    params(MemberWorkspaceId, BlockId, SubscribedUserId),
    responses((status = 204, description = "The subscription is ended.")),
)]
async fn handle_delete_subscription(
    ws: InWorkspace,
    Path((_, block_id, user_id)): Path<(String, String, String)>,
) -> ApiResult {
    // The subscriptions of another user are nobody's business: not even that there is
    // one is said.
    if ws.caller.user_id != user_id {
        return Err(AppError::not_found(SUBSCRIPTION));
    }
    ws.app
        .delete_subscription(&block_id, &user_id)
        .await
        .map_err(|error| {
            if error.is_not_found() {
                AppError::not_found(SUBSCRIPTION)
            } else {
                error
            }
        })?;
    tracing::debug!(block_id, user_id, "DELETE subscription");
    Ok(no_content())
}

/// The subscriptions of the user
///
/// The subscriptions of the user of the session; those of another user are not there.
/// Oldest first, and by the identifier of the block among those made in the same
/// millisecond.
#[utoipa::path(
    get,
    path = "/w/{workspaceId}/subscriptions/{blockOrUserId}",
    operation_id = "getSubscriptions",
    tag = tag::SUBSCRIPTIONS,
    params(MemberWorkspaceId, SubscribedUserId),
    responses((status = 200, description = "The subscriptions.", body = Vec<Subscription>)),
)]
async fn handle_get_subscriptions(
    InLinked(ws): InLinked,
    Path((_, user_id)): Path<(String, String)>,
) -> ApiResult {
    if ws.caller.user_id != user_id {
        return Err(AppError::not_found("user"));
    }
    let subscriptions = ws.app.subscriptions(&user_id).await?;
    tracing::debug!(user_id, count = subscriptions.len(), "GET subscriptions");
    Ok(json_response(&subscriptions))
}
