use std::sync::Arc;

use crate::attachments::{LocalStorage, UploadLimits};
use crate::auth::AuthService;
use crate::collab::CollabHub;
use crate::documents::convert::ConvertClient;
use crate::http::rate_limit::RateLimiter;
use crate::import_job::{ImportJobSettings, ImportQueue};
use crate::search::meili::MeiliConfig;

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
    pub meili: Option<MeiliConfig>,
    pub document_convert: Option<ConvertClient>,
    pub import_settings: Option<ImportJobSettings>,
    pub import_queue: ImportQueue,
}
