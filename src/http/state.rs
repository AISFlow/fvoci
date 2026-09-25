use std::sync::Arc;

use crate::attachments::{ObjectStorage, UploadLimits};
use crate::auth::AuthService;
use crate::collab::CollabHub;
use crate::http::rate_limit::RateLimiter;
use crate::search::meili::MeiliConfig;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub rate_limiter: RateLimiter,
    pub storage: ObjectStorage,
    pub upload: UploadLimits,
    pub collab: Option<Arc<CollabHub>>,
    pub meili: Option<MeiliConfig>,
}
