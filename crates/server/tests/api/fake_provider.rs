//! The HTTP APIs of the mail providers, faked for the tests: the sending requests of
//! Cloudflare, Amazon SES and OCI are recorded with whether they were authorized or
//! signed as the provider checks it, and answered as it answers: by default as taken,
//! or with the statuses (and error names) the test queues, in its own shape.
#![allow(dead_code, reason = "each provider's tests use a part of it")]

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use aws_lc_rs::encoding::AsDer as _;
use aws_lc_rs::hmac;
use aws_lc_rs::rsa::{KeyPair, KeySize};
use aws_lc_rs::signature::{KeyPair as _, RSA_PKCS1_2048_8192_SHA256, UnparsedPublicKey};
use axum::Router;
use axum::body::Bytes;
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use fvoci_server::config::mail::{MailSettings, MailTransport};
use fvoci_server::config::smtp::SmtpSettings;
use mail_parser::MessageParser;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, Notify};
use tokio::task::JoinHandle;

/// The providers, by the path their sending request goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Provider {
    Cloudflare,
    Ses,
    Oci,
}

/// The account of the fake Cloudflare.
pub(crate) const CLOUDFLARE_ACCOUNT: &str = "0123456789abcdef0123456789abcdef";
/// Its API token.
pub(crate) const CLOUDFLARE_TOKEN: &str = "fake-cloudflare-token_0123456789";
/// The region of the fake SES.
pub(crate) const SES_REGION: &str = "eu-west-2";
/// The access key the fake SES takes.
pub(crate) const SES_ACCESS_KEY: &str = "AKIDFAKEMAILAPI";
/// Its secret.
pub(crate) const SES_SECRET: &str = "fake/ses+secret-0123456789";
/// The region of the fake OCI.
pub(crate) const OCI_REGION: &str = "ap-seoul-1";
/// The tenancy, user and fingerprint of the fake OCI's API key: its `keyId`.
pub(crate) const OCI_KEY_ID: &str =
    "ocid1.tenancy.oc1..fake/ocid1.user.oc1..fake/12:34:56:78:9a:bc:de:f0:12:34:56:78:9a:bc:de:f0";
/// The compartment of its approved sender.
pub(crate) const OCI_COMPARTMENT: &str = "ocid1.compartment.oc1..fake";

/// One request the fake was sent.
#[derive(Debug, Clone)]
pub(crate) struct Recorded {
    pub(crate) provider: Provider,
    pub(crate) headers: HeaderMap,
    /// The body as JSON.
    pub(crate) body: Value,
    /// The one recipient.
    pub(crate) to: String,
    /// The plain text of the message (SES's decoded from its MIME).
    pub(crate) text: String,
    /// The MIME text of the message, for SES.
    pub(crate) mime: String,
    /// Whether the request carried the provider's token or signature, as it checks it.
    pub(crate) signed: bool,
    /// The status it was answered with.
    pub(crate) status: u16,
}

/// An answer the test queued.
#[derive(Debug, Clone, Copy)]
struct Scripted {
    status: u16,
    /// `Retry-After`, in seconds.
    retry_after: Option<u64>,
    /// SES's name of the error (`X-Amzn-ErrorType`), instead of the one of the status.
    name: Option<&'static str>,
}

struct State {
    /// The API key of the fake OCI, whose public half checks the signatures.
    oci_public_key: Vec<u8>,
    recorded: Mutex<Vec<Recorded>>,
    /// The next answers; an empty queue answers as taken.
    script: Mutex<VecDeque<Scripted>>,
    changed: Notify,
}

pub(crate) struct FakeProvider {
    pub(crate) address: SocketAddr,
    state: Arc<State>,
    serving: JoinHandle<()>,
    /// The file of the fake OCI's API key, as the server reads it (PKCS#8 PEM).
    oci_key_file: std::path::PathBuf,
}

impl FakeProvider {
    pub(crate) async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("a local port is free");
        let address = listener.local_addr().expect("the listener's address");
        let key = KeyPair::generate(KeySize::Rsa2048).expect("an RSA key");
        let pkcs8 = key.as_der().expect("the key as PKCS#8");
        let oci_key_file = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fake-oci-key-{}.pem", address.port()));
        std::fs::write(&oci_key_file, pem("PRIVATE KEY", pkcs8.as_ref()))
            .expect("the key's file is written");
        let state = Arc::new(State {
            oci_public_key: key.public_key().as_ref().to_vec(),
            recorded: Mutex::default(),
            script: Mutex::default(),
            changed: Notify::new(),
        });
        let serving_state = Arc::clone(&state);
        let app = Router::new().fallback(move |uri: Uri, headers: HeaderMap, body: Bytes| {
            let state = Arc::clone(&serving_state);
            async move { answer(&state, &uri, headers, &body).await }
        });
        let serving = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self {
            address,
            state,
            serving,
            oci_key_file,
        }
    }

    /// `mail_api_endpoint` for this fake.
    pub(crate) fn endpoint(&self) -> String {
        format!("http://{}", self.address)
    }

    /// The settings of a server that sends through this fake as `provider`: the limits
    /// of all mail, and the way. The pace has tests of its own; these send without
    /// waiting for it.
    pub(crate) fn settings(&self, provider: Provider) -> (SmtpSettings, MailSettings) {
        let smtp = SmtpSettings {
            mail_from: "sender@example.test".to_owned(),
            mail_per_second: 1000,
            ..SmtpSettings::default()
        };
        let mail = match provider {
            Provider::Cloudflare => MailSettings {
                mail_transport: Some(MailTransport::Cloudflare),
                mail_api_endpoint: self.endpoint(),
                cloudflare_account_id: CLOUDFLARE_ACCOUNT.to_owned(),
                cloudflare_api_token: CLOUDFLARE_TOKEN.to_owned(),
                ..MailSettings::default()
            },
            Provider::Ses => MailSettings {
                mail_transport: Some(MailTransport::Ses),
                mail_api_endpoint: self.endpoint(),
                ses_region: SES_REGION.to_owned(),
                ses_access_key_id: SES_ACCESS_KEY.to_owned(),
                ses_secret_access_key: SES_SECRET.to_owned(),
                ..MailSettings::default()
            },
            Provider::Oci => {
                let mut parts = OCI_KEY_ID.split('/');
                let mut part = || parts.next().expect("three parts").to_owned();
                MailSettings {
                    mail_transport: Some(MailTransport::Oci),
                    mail_api_endpoint: self.endpoint(),
                    oci_region: OCI_REGION.to_owned(),
                    oci_tenancy: part(),
                    oci_user: part(),
                    oci_fingerprint: part(),
                    oci_private_key_file: self.oci_key_file.display().to_string(),
                    oci_compartment: OCI_COMPARTMENT.to_owned(),
                    ..MailSettings::default()
                }
            }
        };
        (smtp, mail)
    }

    /// Answers the next requests with these statuses, in order, then as taken; a `429`
    /// says `Retry-After: 0`.
    pub(crate) async fn answer_next(&self, statuses: &[u16]) {
        self.state
            .script
            .lock()
            .await
            .extend(statuses.iter().map(|&status| Scripted {
                status,
                retry_after: (status == 429).then_some(0),
                name: None,
            }));
    }

    /// Answers the next request with `status` and `Retry-After: seconds`.
    pub(crate) async fn answer_next_after(&self, status: u16, seconds: u64) {
        self.state.script.lock().await.push_back(Scripted {
            status,
            retry_after: Some(seconds),
            name: None,
        });
    }

    /// Answers the next request with `status` and the error `name` (SES's
    /// `X-Amzn-ErrorType`, OCI's `code`).
    pub(crate) async fn answer_next_named(&self, status: u16, name: &'static str) {
        self.state.script.lock().await.push_back(Scripted {
            status,
            retry_after: None,
            name: Some(name),
        });
    }

    pub(crate) async fn recorded(&self) -> Vec<Recorded> {
        self.state.recorded.lock().await.clone()
    }

    /// Waits until `count` requests were answered as taken; returns those.
    pub(crate) async fn taken(&self, count: usize) -> Vec<Recorded> {
        self.until(|recorded| {
            let taken: Vec<Recorded> = recorded
                .iter()
                .filter(|recorded| (200..300).contains(&recorded.status))
                .cloned()
                .collect();
            (taken.len() >= count).then_some(taken)
        })
        .await
    }

    /// Waits until `count` requests arrived, whatever they were answered.
    pub(crate) async fn requests(&self, count: usize) -> Vec<Recorded> {
        self.until(|recorded| (recorded.len() >= count).then(|| recorded.to_vec()))
            .await
    }

    async fn until<T>(&self, done: impl Fn(&[Recorded]) -> Option<T>) -> T {
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let changed = self.state.changed.notified();
                if let Some(done) = done(&self.recorded().await) {
                    return done;
                }
                changed.await;
            }
        })
        .await
        .expect("the provider is sent the requests")
    }

    pub(crate) fn stop(self) {
        self.serving.abort();
        let _ = std::fs::remove_file(&self.oci_key_file);
    }
}

async fn answer(state: &State, uri: &Uri, headers: HeaderMap, body: &[u8]) -> Response {
    let path = uri.path();
    let provider = if path == format!("/client/v4/accounts/{CLOUDFLARE_ACCOUNT}/email/sending/send")
    {
        Provider::Cloudflare
    } else if path == "/v2/email/outbound-emails" {
        Provider::Ses
    } else if path == "/20220926/actions/submitEmail" {
        Provider::Oci
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let json: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let (to, text, mime, signed) = read(state, provider, &json, &headers, body);
    let Scripted {
        status,
        retry_after,
        name,
    } = state.script.lock().await.pop_front().unwrap_or(Scripted {
        status: 200,
        retry_after: None,
        name: None,
    });
    let mut response = match (provider, status) {
        (Provider::Cloudflare, 200) => (
            StatusCode::OK,
            axum::Json(json!({
                "success": true, "errors": [], "messages": [],
                "result": {"delivered": [to], "permanent_bounces": [], "queued": []},
            })),
        )
            .into_response(),
        (Provider::Cloudflare, status) => cloudflare_error(status),
        (Provider::Ses, 200) => (
            StatusCode::OK,
            axum::Json(json!({"MessageId": "fake-message-id"})),
        )
            .into_response(),
        (Provider::Ses, status) => ses_error(status, name),
        (Provider::Oci, 200) => (
            StatusCode::OK,
            axum::Json(json!({
                "messageId": "fake-message-id", "envelopeId": "fake-envelope-id",
                "suppressedRecipients": [],
            })),
        )
            .into_response(),
        (Provider::Oci, status) => oci_error(status, name),
    };
    if let Some(seconds) = retry_after {
        response.headers_mut().insert(
            "retry-after",
            seconds.to_string().parse().expect("a header value"),
        );
    }
    state.recorded.lock().await.push(Recorded {
        provider,
        headers,
        body: json,
        to,
        text,
        mime,
        signed,
        status,
    });
    state.changed.notify_waiters();
    response
}

fn header(headers: &HeaderMap, name: &str) -> String {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

/// Cloudflare's error answer of `status`, with the code its documentation gives.
fn cloudflare_error(status: u16) -> Response {
    let (code, message) = match status {
        400 => (10001, "email.sending.error.invalid_request_schema"),
        401 => (10101, "email.sending.error.authentication.unauthorized"),
        403 => (10102, "email.sending.error.authentication.forbidden"),
        404 => (
            7003,
            "Could not route to /client/v4/accounts/…/email/sending/send, perhaps your object identifier is invalid?",
        ),
        429 => (10004, "email.sending.error.throttled"),
        _ => (10002, "email.sending.error.internal_server"),
    };
    (
        StatusCode::from_u16(status).expect("a status"),
        axum::Json(json!({
            "success": false, "errors": [{"code": code, "message": message}],
            "messages": [], "result": null,
        })),
    )
        .into_response()
}

/// SES's error answer of `status`: the error's name in `X-Amzn-ErrorType`, as its API
/// v2 gives it, and a message in the body.
fn ses_error(status: u16, name: Option<&'static str>) -> Response {
    let name = name.unwrap_or(match status {
        400 => "BadRequestException",
        403 => "SignatureDoesNotMatch",
        404 => "NotFoundException",
        429 => "TooManyRequestsException",
        _ => "InternalFailure",
    });
    let mut response = (
        StatusCode::from_u16(status).expect("a status"),
        axum::Json(json!({"message": "fake"})),
    )
        .into_response();
    response.headers_mut().insert(
        "x-amzn-errortype",
        format!("{name}:http://internal.amazon.com/coral/com.amazonaws.sesv2/")
            .parse()
            .expect("a header value"),
    );
    response
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut text, byte| {
        let _ = write!(text, "{byte:02x}");
        text
    })
}

/// Whether the request carries a Signature Version 4 that SES would take: made with
/// [`SES_SECRET`] for the fake's region and the service `ses`, over the content type,
/// the host and the date, and over the body as it arrived. Written apart from the
/// server's signer, after AWS's description of `SigV4`.
fn ses_signed(headers: &HeaderMap, body: &[u8]) -> bool {
    let authorization = header(headers, "authorization");
    let amz_date = header(headers, "x-amz-date");
    let Some(day) = amz_date.get(..8) else {
        return false;
    };
    let Some((credential, rest)) = authorization
        .strip_prefix("AWS4-HMAC-SHA256 Credential=")
        .and_then(|rest| rest.split_once(", SignedHeaders="))
    else {
        return false;
    };
    let Some((signed, signature)) = rest.split_once(", Signature=") else {
        return false;
    };
    let scope = format!("{day}/{SES_REGION}/ses/aws4_request");
    if credential != format!("{SES_ACCESS_KEY}/{scope}") || signed != "content-type;host;x-amz-date"
    {
        return false;
    }
    let canonical = format!(
        "POST\n/v2/email/outbound-emails\n\ncontent-type:{}\nhost:{}\nx-amz-date:{amz_date}\n\n\
         {signed}\n{}",
        header(headers, "content-type"),
        header(headers, "host"),
        hex(&Sha256::digest(body))
    );
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        hex(&Sha256::digest(canonical.as_bytes()))
    );
    let sign = |key: &[u8], data: &str| {
        hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, key), data.as_bytes())
    };
    let key = [SES_REGION, "ses", "aws4_request"].iter().fold(
        sign(format!("AWS4{SES_SECRET}").as_bytes(), day),
        |key, part| sign(key.as_ref(), part),
    );
    hex(sign(key.as_ref(), &string_to_sign).as_ref()) == signature
}

/// OCI's error answer of `status`: its `code`, as the API reference gives it.
fn oci_error(status: u16, name: Option<&'static str>) -> Response {
    let code = name.unwrap_or(match status {
        400 => "InvalidParameter",
        401 => "NotAuthenticated",
        404 => "NotAuthorizedOrNotFound",
        413 => "RequestEntityTooLarge",
        429 => "TooManyRequests",
        _ => "InternalServerError",
    });
    (
        StatusCode::from_u16(status).expect("a status"),
        axum::Json(json!({"code": code, "message": "fake"})),
    )
        .into_response()
}

/// `der` as a PEM block of `label`.
fn pem(label: &str, der: &[u8]) -> String {
    let text = STANDARD.encode(der);
    let lines: Vec<&str> = text
        .as_bytes()
        .chunks(64)
        .map(|line| std::str::from_utf8(line).expect("base64 is ASCII"))
        .collect();
    format!(
        "-----BEGIN {label}-----\n{}\n-----END {label}-----\n",
        lines.join("\n")
    )
}

/// Whether the request carries a signature OCI would take: of the key [`OCI_KEY_ID`]
/// names, `rsa-sha256`, over the date, the request target, the host, the length, the
/// type and the hash of the body as it arrived, each of these as it was sent. Written
/// apart from the server's signer, after OCI's description of request signatures.
fn oci_signed(public_key: &[u8], headers: &HeaderMap, body: &[u8]) -> bool {
    let authorization = header(headers, "authorization");
    let Some(parameters) = authorization.strip_prefix("Signature ") else {
        return false;
    };
    let parameter = |name: &str| {
        parameters.split(',').find_map(|part| {
            part.strip_prefix(&format!("{name}=\""))
                .and_then(|value| value.strip_suffix('"'))
        })
    };
    let signed = "date (request-target) host content-length content-type x-content-sha256";
    if parameter("version") != Some("1")
        || parameter("keyId") != Some(OCI_KEY_ID)
        || parameter("algorithm") != Some("rsa-sha256")
        || parameter("headers") != Some(signed)
        || header(headers, "x-content-sha256") != STANDARD.encode(Sha256::digest(body))
        || header(headers, "content-length") != body.len().to_string()
        || header(headers, "date").is_empty()
    {
        return false;
    }
    let lines: Vec<String> = signed
        .split(' ')
        .map(|name| match name {
            "(request-target)" => format!("{name}: post /20220926/actions/submitEmail"),
            _ => format!("{name}: {}", header(headers, name)),
        })
        .collect();
    let Some(signature) = parameter("signature").and_then(|text| STANDARD.decode(text).ok()) else {
        return false;
    };
    UnparsedPublicKey::new(&RSA_PKCS1_2048_8192_SHA256, public_key)
        .verify(lines.join("\n").as_bytes(), &signature)
        .is_ok()
}

/// The recipient, the plain text and the MIME text of a request to `provider`, and
/// whether it was signed or authorized as the provider checks it.
fn read(
    state: &State,
    provider: Provider,
    json: &Value,
    headers: &HeaderMap,
    body: &[u8],
) -> (String, String, String, bool) {
    match provider {
        Provider::Cloudflare => (
            json["to"].as_str().unwrap_or_default().to_owned(),
            json["text"].as_str().unwrap_or_default().to_owned(),
            String::new(),
            header(headers, "authorization") == format!("Bearer {CLOUDFLARE_TOKEN}"),
        ),
        Provider::Ses => {
            let mime = json["Content"]["Raw"]["Data"]
                .as_str()
                .and_then(|data| STANDARD.decode(data).ok())
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .unwrap_or_default();
            let text = MessageParser::default()
                .parse(mime.as_bytes())
                .and_then(|parsed| parsed.body_text(0).map(std::borrow::Cow::into_owned))
                .unwrap_or_default();
            (
                json["Destination"]["ToAddresses"][0]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                text,
                mime,
                ses_signed(headers, body),
            )
        }
        Provider::Oci => (
            json["recipients"]["to"][0]["email"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            json["bodyText"].as_str().unwrap_or_default().to_owned(),
            String::new(),
            oci_signed(&state.oci_public_key, headers, body),
        ),
    }
}
