use std::sync::Arc;

use crate::auth::AuthService;
use crate::http::rate_limit::RateLimiter;

#[derive(Clone)]
pub struct AppState {
    pub auth: Arc<AuthService>,
    pub branding_name: String,
    pub public_origin: String,
    pub cookie_secure: bool,
    pub rate_limiter: RateLimiter,
}
