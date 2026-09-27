//! Web Push: VAPID keys, subscription storage, the `push` outbox fan-out
//! and the delivery sender.
//! The HTTP route lives in `http::routes::push`.

pub mod consumer;
pub mod db;
pub mod message;
pub mod send;
pub mod sender;
pub mod vapid;

pub use consumer::{push_consumer, PUSH_CONSUMER};
pub use db::{
    disconnect_browser, list_for_user, normalize_subscription_key, register_subscription,
    remove_by_endpoint, upsert_subscription, validate_subscription_keys, PushSubscriptionRow,
    PUSH_SUBSCRIPTIONS_PER_USER,
};
pub use send::{endpoint_origin_for_log, PushPayload, PushSendOutcome};
pub use sender::{spawn_push_sender, PushSenderHandle, PushSenderSettings};
pub use vapid::{
    ensure_vapid_keys, load_vapid_key_pair, load_vapid_public_key, rotate_vapid_keys,
    vapid_context, RotateVapidOutcome, VapidKeysError,
};
