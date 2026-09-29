//! Attachment transfer modes (#149).
//!
//! * `proxy` (A): part and original bytes pass through this server, which
//!   authorizes every request and streams to or from storage.
//! * `presigned` (B): after the same authorization the server signs short-lived
//!   S3 URLs, and the browser PUTs parts to and GETs originals from the storage
//!   origin directly. Only S3 with `S3_PUBLIC_ENDPOINT` can do this.
//!
//! Exactly one mode is in effect at a time. `FVOCI_ATTACHMENT_TRANSFER_MODE`
//! wins over the admin-stored `attachmentTransfer.mode`, which wins over the
//! `proxy` default (see [`crate::settings::attachment_transfer`]). An upload
//! session keeps the mode it was created with for its whole life, so a switch
//! never moves a half-sent upload to the other path and no byte is sent twice.
//! A failed transfer is never retried through the other mode.

use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub enum TransferMode {
    #[default]
    Proxy,
    Presigned,
}

impl TransferMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proxy => "proxy",
            Self::Presigned => "presigned",
        }
    }

    /// Exact value only: the settings store applies the environment value
    /// verbatim, so startup must accept exactly what the store accepts.
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "proxy" => Some(Self::Proxy),
            "presigned" => Some(Self::Presigned),
            _ => None,
        }
    }
}

/// Why this process cannot hand out presigned URLs. Fixed for the life of the
/// process: it follows `STORAGE_DRIVER` and `S3_PUBLIC_ENDPOINT`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub enum TransferUnavailable {
    /// Local storage has no URL a browser could use.
    StorageLocal,
    /// S3 without `S3_PUBLIC_ENDPOINT`: nothing to sign browser URLs against.
    PublicEndpointMissing,
}

impl TransferUnavailable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::StorageLocal => "storage_local",
            Self::PublicEndpointMissing => "public_endpoint_missing",
        }
    }
}

pub const DEFAULT_PRESIGN_PART_TTL: Duration = Duration::from_secs(900);
pub const DEFAULT_PRESIGN_DOWNLOAD_TTL: Duration = Duration::from_secs(60);

/// Lifetimes of browser-facing signed URLs. S3 checks expiry when a request
/// starts, so a part PUT that began in time finishes; an issued URL cannot be
/// revoked before it expires (only rotating the S3 access key does that).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PresignTtls {
    pub part: Duration,
    pub download: Duration,
}

impl Default for PresignTtls {
    fn default() -> Self {
        Self {
            part: DEFAULT_PRESIGN_PART_TTL,
            download: DEFAULT_PRESIGN_DOWNLOAD_TTL,
        }
    }
}

/// A browser-facing signed URL. It carries a credential scope and signature,
/// so it goes only into the response that needs it and is never logged or
/// written to audit and event payloads.
#[derive(Clone)]
pub struct PresignedUrl {
    pub url: String,
    pub expires_at: DateTime<Utc>,
}

impl std::fmt::Debug for PresignedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PresignedUrl")
            .field("url", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_exact_values_only() {
        assert_eq!(TransferMode::parse("proxy"), Some(TransferMode::Proxy));
        assert_eq!(
            TransferMode::parse("presigned"),
            Some(TransferMode::Presigned)
        );
        for bad in ["", " presigned", "Presigned", "direct", "proxy\n"] {
            assert_eq!(TransferMode::parse(bad), None, "{bad:?}");
        }
        assert_eq!(
            serde_json::to_value(TransferMode::Presigned).unwrap(),
            serde_json::json!("presigned")
        );
        assert_eq!(TransferMode::default(), TransferMode::Proxy);
    }

    #[test]
    fn presigned_url_debug_hides_the_signature() {
        let url = PresignedUrl {
            url: "https://files.example/k?X-Amz-Signature=abc".into(),
            expires_at: Utc::now(),
        };
        assert!(!format!("{url:?}").contains("X-Amz-Signature"));
    }
}
