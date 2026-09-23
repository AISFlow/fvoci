use axum::extract::rejection::JsonRejection;
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
    pub source: Option<String>,
}

impl AppError {
    pub fn problem(status: StatusCode, code: ProblemCode) -> Self {
        Self {
            status,
            code,
            source: None,
        }
    }

    pub fn from_code(code: ProblemCode) -> Self {
        Self {
            status: code.status(),
            code,
            source: None,
        }
    }

    pub fn with_source(code: ProblemCode, pointer: impl Into<String>) -> Self {
        Self {
            status: code.status(),
            code,
            source: Some(pointer.into()),
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
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let body = ProblemBody {
            kind: "about:blank",
            title: self.code.title().to_string(),
            status: self.status.as_u16(),
            code: self.code.as_str().to_string(),
            source: self.source,
        };
        (
            self.status,
            [(axum::http::header::CONTENT_TYPE, "application/problem+json")],
            Json(body),
        )
            .into_response()
    }
}

impl From<JsonRejection> for AppError {
    fn from(rejection: JsonRejection) -> Self {
        match rejection {
            JsonRejection::JsonDataError(_) => {
                AppError::with_source(ProblemCode::InvalidInput, "/")
            }
            JsonRejection::JsonSyntaxError(_) => {
                AppError::with_source(ProblemCode::InvalidInput, "/")
            }
            JsonRejection::MissingJsonContentType(_) | JsonRejection::BytesRejection(_) => {
                AppError::from_code(ProblemCode::InvalidInput)
            }
            _ => AppError::from_code(ProblemCode::InvalidInput),
        }
    }
}
