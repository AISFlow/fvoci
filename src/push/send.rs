//! One Web Push request: RFC 8291 `aes128gcm` encryption and the RFC 8292
//! VAPID JWT come from `web-push-native`/`jwt-simple`; FVOCI only adds the
//! policy (TTL, JWT lifetime, `aud`, transport and status handling).

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use url::Url;
use web_push_native::jwt_simple::algorithms::{
    ECDSAP256KeyPairLike, ECDSAP256PublicKeyLike, ES256KeyPair,
};
use web_push_native::jwt_simple::claims::Claims;
use web_push_native::p256::PublicKey;
use web_push_native::{Auth, WebPushBuilder};

use crate::integrations::outbound::{Outbound, OutboundError};

pub const PUSH_TIMEOUT: Duration = Duration::from_secs(5);
/// Push services keep an undelivered message this long (source `TTL: 86400`).
const PUSH_TTL_SECS: &str = "86400";
/// RFC 8292 allows at most 24 h; half leaves room for clock skew and retries.
const VAPID_JWT_TTL: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PushPayload {
    pub title: String,
    pub body: String,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushSendOutcome {
    Delivered,
    /// 404/410: the subscription is gone; the caller deletes the endpoint.
    ExpiredEndpoint,
    /// Any other status (including 401/403 from a stale VAPID key). The row
    /// stays; only `rotate-vapid` wipes rows.
    Failed(u16),
}

/// Endpoint URLs are bearer capabilities: logs carry the origin only.
pub fn endpoint_origin_for_log(endpoint: &str) -> String {
    Url::parse(endpoint)
        .ok()
        .map(|url| url.origin().ascii_serialization())
        .filter(|origin| origin != "null")
        .unwrap_or_else(|| "invalid".into())
}

/// RFC 8292 `aud`: the endpoint origin (scheme, host, non-default port).
fn vapid_audience(endpoint: &Url) -> Option<String> {
    let origin = endpoint.origin();
    origin.is_tuple().then(|| origin.ascii_serialization())
}

fn vapid_authorization(
    endpoint: &Url,
    vapid: &ES256KeyPair,
    subject: &str,
) -> Result<String, OutboundError> {
    let audience =
        vapid_audience(endpoint).ok_or_else(|| OutboundError::Transport("bad endpoint".into()))?;
    let claims = Claims::create(VAPID_JWT_TTL.into())
        .with_audience(audience)
        .with_subject(subject);
    let token = vapid
        .sign(claims)
        .map_err(|_| OutboundError::Transport("vapid sign".into()))?;
    let public = URL_SAFE_NO_PAD.encode(vapid.public_key().public_key().to_bytes_uncompressed());
    Ok(format!("vapid t={token}, k={public}"))
}

pub struct PushTarget<'a> {
    pub endpoint: &'a str,
    pub p256dh: &'a str,
    pub auth: &'a str,
}

/// Encrypts `payload` for one subscription and POSTs it through the pinned,
/// no-redirect, no-proxy outbound client. `subject` is the VAPID `sub`
/// (`FVOCI_PUBLIC_ORIGIN`).
pub async fn send_push(
    outbound: &Outbound,
    vapid: &ES256KeyPair,
    subject: &str,
    target: PushTarget<'_>,
    payload: &PushPayload,
    timeout: Duration,
) -> Result<PushSendOutcome, OutboundError> {
    let request = build_request(vapid, subject, &target, payload)?;
    let status = outbound
        .post(target.endpoint, &request.headers, request.body, timeout)
        .await?;
    Ok(match status {
        200..=299 => PushSendOutcome::Delivered,
        404 | 410 => PushSendOutcome::ExpiredEndpoint,
        other => PushSendOutcome::Failed(other),
    })
}

struct BuiltRequest {
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

fn build_request(
    vapid: &ES256KeyPair,
    subject: &str,
    target: &PushTarget<'_>,
    payload: &PushPayload,
) -> Result<BuiltRequest, OutboundError> {
    let bad = |what: &str| OutboundError::Transport(format!("bad subscription {what}"));
    let endpoint = Url::parse(target.endpoint).map_err(|_| bad("endpoint"))?;
    let uri = target
        .endpoint
        .parse::<axum::http::Uri>()
        .map_err(|_| bad("endpoint"))?;
    let ua_public = decode_p256dh(target.p256dh).ok_or_else(|| bad("p256dh"))?;
    let ua_auth = decode_auth(target.auth).ok_or_else(|| bad("auth"))?;
    let body = serde_json::to_vec(payload).map_err(|_| bad("payload"))?;
    let request = WebPushBuilder::new(uri, ua_public, ua_auth)
        .build(body)
        .map_err(|_| OutboundError::Transport("encrypt".into()))?;
    let mut headers = vec![
        ("TTL", PUSH_TTL_SECS.to_string()),
        (
            "Authorization",
            vapid_authorization(&endpoint, vapid, subject)?,
        ),
    ];
    for (name, key) in [
        ("Content-Encoding", axum::http::header::CONTENT_ENCODING),
        ("Content-Type", axum::http::header::CONTENT_TYPE),
    ] {
        if let Some(value) = request.headers().get(key).and_then(|v| v.to_str().ok()) {
            headers.push((name, value.to_string()));
        }
    }
    Ok(BuiltRequest {
        headers,
        body: request.into_body(),
    })
}

fn decode_p256dh(raw: &str) -> Option<PublicKey> {
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    PublicKey::from_sec1_bytes(&bytes).ok()
}

fn decode_auth(raw: &str) -> Option<Auth> {
    let bytes = URL_SAFE_NO_PAD.decode(raw).ok()?;
    (bytes.len() == 16).then(|| Auth::clone_from_slice(&bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use web_push_native::jwt_simple::prelude::{NoCustomClaims, VerificationOptions};
    use web_push_native::p256::SecretKey;

    #[test]
    fn origin_logs_drop_path_and_query() {
        assert_eq!(
            endpoint_origin_for_log("https://push.example.com/foo/bar?x=1"),
            "https://push.example.com"
        );
        assert_eq!(
            endpoint_origin_for_log("https://push.example.com:8443/secret"),
            "https://push.example.com:8443"
        );
        assert_eq!(endpoint_origin_for_log("not a url"), "invalid");
    }

    #[test]
    fn audience_is_origin_with_non_default_port() {
        let aud = |raw: &str| vapid_audience(&Url::parse(raw).unwrap()).unwrap();
        assert_eq!(
            aud("https://fcm.googleapis.com/fcm/send/abc"),
            "https://fcm.googleapis.com"
        );
        assert_eq!(
            aud("https://push.example.com:443/x"),
            "https://push.example.com"
        );
        assert_eq!(
            aud("https://push.example.com:80/x"),
            "https://push.example.com:80"
        );
        assert_eq!(aud("http://127.0.0.1:4000/x"), "http://127.0.0.1:4000");
    }

    #[test]
    fn request_carries_vapid_ttl_and_decryptable_body() {
        let vapid = ES256KeyPair::generate();
        let ua_secret =
            SecretKey::random(&mut web_push_native::p256::elliptic_curve::rand_core::OsRng);
        let p256dh = URL_SAFE_NO_PAD
            .encode(web_push_native::p256::EncodedPoint::from(ua_secret.public_key()).as_bytes());
        let auth_bytes = [7u8; 16];
        let auth = URL_SAFE_NO_PAD.encode(auth_bytes);
        let endpoint = "https://push.example.com:8443/send/secret-token";
        let payload = PushPayload {
            title: "홍길동".into(),
            body: "새 알림이 있습니다".into(),
            url: "/w/acme/ACME-1".into(),
        };
        let built = build_request(
            &vapid,
            "https://fvoci.example",
            &PushTarget {
                endpoint,
                p256dh: &p256dh,
                auth: &auth,
            },
            &payload,
        )
        .unwrap();
        let header = |name: &str| {
            built
                .headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
                .unwrap()
        };
        assert_eq!(header("TTL"), "86400");
        assert_eq!(header("Content-Encoding"), "aes128gcm");
        assert_eq!(header("Content-Type"), "application/octet-stream");

        let authz = header("Authorization");
        let rest = authz.strip_prefix("vapid t=").unwrap();
        let (token, key) = rest.split_once(", k=").unwrap();
        assert_eq!(
            key,
            URL_SAFE_NO_PAD.encode(vapid.public_key().public_key().to_bytes_uncompressed())
        );
        let claims = vapid
            .public_key()
            .verify_token::<NoCustomClaims>(token, Some(VerificationOptions::default()))
            .unwrap();
        assert_eq!(
            claims.audiences.unwrap().into_string().unwrap(),
            "https://push.example.com:8443"
        );
        assert_eq!(claims.subject.as_deref(), Some("https://fvoci.example"));
        let lifetime = claims.expires_at.unwrap().as_secs() - claims.issued_at.unwrap().as_secs();
        assert_eq!(lifetime, VAPID_JWT_TTL.as_secs());

        let plain =
            web_push_native::decrypt(built.body, &ua_secret, &Auth::clone_from_slice(&auth_bytes))
                .unwrap();
        let value: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(
            value,
            serde_json::json!({ "title": "홍길동", "body": "새 알림이 있습니다", "url": "/w/acme/ACME-1" })
        );
    }

    #[test]
    fn rejects_malformed_subscription_keys() {
        let vapid = ES256KeyPair::generate();
        let payload = PushPayload {
            title: String::new(),
            body: String::new(),
            url: "/".into(),
        };
        let err = build_request(
            &vapid,
            "https://fvoci.example",
            &PushTarget {
                endpoint: "https://push.example.com/x",
                p256dh: &"A".repeat(87),
                auth: &"A".repeat(22),
            },
            &payload,
        );
        assert!(err.is_err());
    }
}
