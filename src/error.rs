use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;

pub const SESSION_COOKIE: &str = "fvoci_session";
pub const API_PREFIX: &str = "/api/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProblemCode {
    AuthenticationRequired,
    InvalidEmailOrPassword,
    InvalidInput,
    InstanceSetupAlreadyCompleted,
    PasswordInvalid,
    SlugTaken,
    OriginMismatch,
    NotFound,
    RateLimited,
}

impl ProblemCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "authentication_required",
            Self::InvalidEmailOrPassword => "invalid_email_or_password",
            Self::InvalidInput => "invalid_input",
            Self::InstanceSetupAlreadyCompleted => "instance_setup_already_completed",
            Self::PasswordInvalid => "password_invalid",
            Self::SlugTaken => "slug_taken",
            Self::OriginMismatch => "origin_mismatch",
            Self::NotFound => "not_found",
            Self::RateLimited => "rate_limited",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::AuthenticationRequired => "authentication required",
            Self::InvalidEmailOrPassword => "invalid email or password",
            Self::InvalidInput => "invalid input",
            Self::InstanceSetupAlreadyCompleted => "instance setup already completed",
            Self::PasswordInvalid => "password_invalid",
            Self::SlugTaken => "slug taken",
            Self::OriginMismatch => "origin mismatch",
            Self::NotFound => "not found",
            Self::RateLimited => "rate limited",
        }
    }

    pub fn status(self) -> StatusCode {
        match self {
            Self::AuthenticationRequired | Self::InvalidEmailOrPassword => StatusCode::UNAUTHORIZED,
            Self::InvalidInput | Self::PasswordInvalid => StatusCode::BAD_REQUEST,
            Self::InstanceSetupAlreadyCompleted | Self::NotFound => StatusCode::NOT_FOUND,
            Self::SlugTaken => StatusCode::CONFLICT,
            Self::OriginMismatch => StatusCode::FORBIDDEN,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
        }
    }
}

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: ProblemCode,
}

impl AppError {
    pub fn problem(status: StatusCode, code: ProblemCode) -> Self {
        Self { status, code }
    }

    pub fn from_code(code: ProblemCode) -> Self {
        Self {
            status: code.status(),
            code,
        }
    }
}

#[derive(Serialize)]
struct ProblemBody {
    #[serde(rename = "type")]
    kind: &'static str,
    title: String,
    status: u16,
    code: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ProblemBody {
            kind: "about:blank",
            title: self.code.title().to_string(),
            status: self.status.as_u16(),
            code: self.code.as_str().to_string(),
        };
        (
            self.status,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            Json(body),
        )
            .into_response()
    }
}
