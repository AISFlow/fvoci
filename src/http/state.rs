use std::sync::Arc;

use crate::attachments::{LocalStorage, UploadLimits};
use crate::auth::AuthService;
use crate::collab::CollabHub;
use crate::http::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub rate_limiter: RateLimiter,
    pub storage: LocalStorage,
    pub upload: UploadLimits,
    pub collab: Option<Arc<CollabHub>>,
}
