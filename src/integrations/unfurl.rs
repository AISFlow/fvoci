//! Authenticated workspace link preview (source `packages/core/src/unfurl.ts`
//! plus `apps/server/src/domains/search/oembed.ts`).
//!
//! Outbound GETs reuse pinned DNS from [`super::outbound`]. Unfurl always
//! applies [`OutboundPolicy::default`]; webhook `FVOCI_WEBHOOK_ALLOW_TARGETS`
//! is never consulted. Invalid input and SSRF (private literals, mixed DNS,
//! private redirect hops) fail closed. Transport timeouts, NXDOMAIN, and
//! non-OK HTTP yield empty OG metadata — the source `unfurlOg` catch-all that
//! also swallowed SSRF is treated as a bug, not the product contract.
//! GitHub API → HTML OG fallback runs only after a semantic metadata miss,
//! never after an SSRF/security reject.

use std::sync::LazyLock;
use std::time::Duration;

use html5gum::{StartTag, Token, Tokenizer};
use regex::Regex;
use serde::{Deserialize, Serialize};
use url::Url;

use super::outbound::{
    parse_target_url, Outbound, OutboundError, OutboundRejected, PinnedGet, RESPONSE_READ_CAP,
    URL_MAX,
};

pub const UNFURL_TIMEOUT: Duration = Duration::from_millis(3000);
pub const UNFURL_USER_AGENT: &str = "FVOCI";
pub const UNFURL_MAX_REDIRECTS: u8 = 3;
const TITLE_MAX: usize = 500;
const DESC_MAX: usize = 2000;
const IMAGE_MAX: usize = 2048;
const STATE_MAX: usize = 32;

static GITHUB_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^https?://(?:www\.)?github\.com/([^/]+)/([^/]+)/(issues|pull)/(\d+)(?:[/?#]|$)",
    )
    .expect("github ref")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub enum UnfurlKind {
    GithubIssue,
    GithubPull,
    Og,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "api-schema", derive(utoipa::ToSchema))]
pub struct UnfurlResult {
    pub kind: UnfurlKind,
    pub url: String,
    pub title: Option<String>,
    pub description: Option<String>,
    pub image_url: Option<String>,
    pub state: Option<String>,
    pub number: Option<i64>,
    pub owner: Option<String>,
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub html: Option<String>,
}

impl UnfurlResult {
    fn empty(kind: UnfurlKind, url: String) -> Self {
        Self {
            kind,
            url,
            title: None,
            description: None,
            image_url: None,
            state: None,
            number: None,
            owner: None,
            repo: None,
            html: None,
        }
    }
}

pub fn is_http_url(value: &str) -> bool {
    Url::parse(value)
        .ok()
        .is_some_and(|url| url.scheme() == "http" || url.scheme() == "https")
}

/// Query contract: 1..=2048 bytes and http(s) only. Stricter SSRF rules run
/// inside [`fetch_unfurl`].
pub fn validate_unfurl_query(raw: &str) -> Result<(), OutboundRejected> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.len() > URL_MAX || !is_http_url(trimmed) {
        return Err(OutboundRejected::Url);
    }
    Ok(())
}

fn clip(value: Option<&str>, max: usize) -> Option<String> {
    let trimmed = value.map(str::trim).filter(|s| !s.is_empty())?;
    if trimmed.chars().count() <= max {
        Some(trimmed.to_string())
    } else {
        Some(trimmed.chars().take(max).collect())
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn security_reject(err: &OutboundRejected) -> bool {
    matches!(
        err,
        OutboundRejected::Url | OutboundRejected::Address | OutboundRejected::Redirect
    )
}

fn attr<'a>(tag: &'a StartTag<()>, name: &[u8]) -> Option<&'a [u8]> {
    tag.attributes
        .iter()
        .find(|(key, _)| key.as_slice() == name)
        .map(|(_, value)| value.value.as_slice())
}

fn attr_eq_ignore_ascii(tag: &StartTag<()>, name: &[u8], want: &str) -> bool {
    attr(tag, name).is_some_and(|got| got.eq_ignore_ascii_case(want.as_bytes()))
}

fn is_og_meta(tag: &StartTag<()>, property: &str) -> bool {
    attr_eq_ignore_ascii(tag, b"property", property) || attr_eq_ignore_ascii(tag, b"name", property)
}

fn content_string(tag: &StartTag<()>) -> Option<String> {
    let raw = attr(tag, b"content")?;
    let text = std::str::from_utf8(raw).ok()?.trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_string())
    }
}

pub fn parse_og(html: &str, page_url: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut title = None;
    let mut description = None;
    let mut image = None;
    for token in Tokenizer::new(html).flatten() {
        let Token::StartTag(tag) = token else {
            continue;
        };
        if tag.name.as_slice() != b"meta" {
            continue;
        }
        if title.is_none() && is_og_meta(&tag, "og:title") {
            title = clip(content_string(&tag).as_deref(), TITLE_MAX);
        } else if description.is_none() && is_og_meta(&tag, "og:description") {
            description = clip(content_string(&tag).as_deref(), DESC_MAX);
        } else if image.is_none() && is_og_meta(&tag, "og:image") {
            image = content_string(&tag).and_then(|raw| {
                let resolved = Url::parse(&raw)
                    .or_else(|_| Url::parse(page_url).and_then(|base| base.join(&raw)))
                    .ok()?;
                if resolved.scheme() != "http" && resolved.scheme() != "https" {
                    return None;
                }
                clip(Some(resolved.as_str()), IMAGE_MAX)
            });
        }
        if title.is_some() && description.is_some() && image.is_some() {
            break;
        }
    }
    (title, description, image)
}

fn parse_github_ref(url: &str) -> Option<(UnfurlKind, String, String, i64)> {
    let captures = GITHUB_REF_RE.captures(url)?;
    let owner = captures.get(1)?.as_str().to_string();
    let repo = captures.get(2)?.as_str().to_string();
    let kind = match captures.get(3)?.as_str().to_ascii_lowercase().as_str() {
        "pull" => UnfurlKind::GithubPull,
        _ => UnfurlKind::GithubIssue,
    };
    let number = captures.get(4)?.as_str().parse().ok()?;
    Some((kind, owner, repo, number))
}

async fn get_follow(
    outbound: &Outbound,
    start: Url,
    accept: &str,
    deadline: tokio::time::Instant,
) -> Result<(Url, PinnedGet), OutboundError> {
    let headers = [
        ("Accept", accept.to_string()),
        ("User-Agent", UNFURL_USER_AGENT.to_string()),
    ];
    let mut current = start;
    for hop in 0..=UNFURL_MAX_REDIRECTS {
        let remaining = deadline
            .saturating_duration_since(tokio::time::Instant::now())
            .max(Duration::from_millis(1));
        let response = outbound.get(current.as_str(), &headers, remaining).await?;
        if !is_redirect(response.status) {
            return Ok((current, response));
        }
        if hop == UNFURL_MAX_REDIRECTS {
            return Err(OutboundRejected::Redirect.into());
        }
        let location = response.location.as_deref().ok_or(OutboundRejected::Url)?;
        let joined = current
            .join(location)
            .or_else(|_| Url::parse(location))
            .map_err(|_| OutboundRejected::Url)?;
        current = parse_target_url(joined.as_str(), outbound.policy())?;
    }
    Err(OutboundRejected::Redirect.into())
}

fn http_ok(status: u16) -> bool {
    (200..300).contains(&status)
}

async fn unfurl_og(
    outbound: &Outbound,
    page_url: Url,
    deadline: tokio::time::Instant,
) -> Result<UnfurlResult, OutboundRejected> {
    let empty = UnfurlResult::empty(UnfurlKind::Og, page_url.as_str().to_string());
    match get_follow(
        outbound,
        page_url.clone(),
        "text/html,application/xhtml+xml",
        deadline,
    )
    .await
    {
        Err(OutboundError::Rejected(err)) if security_reject(&err) => Err(err),
        Err(_) => Ok(empty),
        Ok((_, response)) if !http_ok(response.status) => Ok(empty),
        Ok((final_url, response)) => {
            let html = String::from_utf8_lossy(&response.body);
            let (title, description, image_url) = parse_og(&html, final_url.as_str());
            Ok(UnfurlResult {
                title,
                description,
                image_url,
                ..empty
            })
        }
    }
}

fn json_object(raw: &[u8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    match serde_json::from_slice::<serde_json::Value>(raw) {
        Ok(serde_json::Value::Object(map)) => Some(map),
        _ => None,
    }
}

fn json_string<'a>(
    obj: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    obj.get(key).and_then(|value| value.as_str())
}

fn json_number(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<i64> {
    obj.get(key)
        .and_then(|value| value.as_i64().or_else(|| value.as_u64().map(|n| n as i64)))
}

async fn unfurl_github(
    outbound: &Outbound,
    page_url: Url,
    kind: UnfurlKind,
    owner: String,
    repo: String,
    number: i64,
    deadline: tokio::time::Instant,
) -> Result<UnfurlResult, OutboundRejected> {
    let empty = UnfurlResult {
        owner: Some(owner.clone()),
        repo: Some(repo.clone()),
        number: Some(number),
        ..UnfurlResult::empty(kind, page_url.as_str().to_string())
    };
    let api_path = if kind == UnfurlKind::GithubPull {
        "pulls"
    } else {
        "issues"
    };
    let api_url = format!("https://api.github.com/repos/{owner}/{repo}/{api_path}/{number}");
    let from_api = match get_follow(
        outbound,
        Url::parse(&api_url).map_err(|_| OutboundRejected::Url)?,
        "application/vnd.github+json",
        deadline,
    )
    .await
    {
        Err(OutboundError::Rejected(err)) if security_reject(&err) => return Err(err),
        Err(_) => None,
        Ok((_, response)) if !http_ok(response.status) => None,
        Ok((_, response)) => json_object(&response.body).map(|obj| UnfurlResult {
            kind,
            url: clip(json_string(&obj, "html_url"), IMAGE_MAX)
                .unwrap_or_else(|| page_url.as_str().to_string()),
            title: clip(json_string(&obj, "title"), TITLE_MAX),
            description: clip(json_string(&obj, "body"), DESC_MAX),
            image_url: None,
            state: clip(json_string(&obj, "state"), STATE_MAX),
            number: json_number(&obj, "number").or(Some(number)),
            owner: Some(owner.clone()),
            repo: Some(repo.clone()),
            html: None,
        }),
    };
    if let Some(from_api) = from_api {
        return Ok(from_api);
    }
    let og = unfurl_og(outbound, page_url, deadline).await?;
    Ok(UnfurlResult {
        title: og.title,
        description: og.description,
        image_url: og.image_url,
        ..empty
    })
}

/// Fetch preview metadata. `outbound` must already be
/// [`Outbound::without_allow_list`].
pub async fn fetch_unfurl(
    outbound: &Outbound,
    raw_url: &str,
    timeout: Duration,
) -> Result<UnfurlResult, OutboundRejected> {
    let parsed = parse_target_url(raw_url, outbound.policy())?;
    let deadline = tokio::time::Instant::now() + timeout;
    if let Some((kind, owner, repo, number)) = parse_github_ref(parsed.as_str()) {
        return unfurl_github(outbound, parsed, kind, owner, repo, number, deadline).await;
    }
    unfurl_og(outbound, parsed, deadline).await
}

fn youtube_id(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    let id = if host == "youtu.be" {
        url.path()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("")
            .to_string()
    } else if host == "youtube.com" || host == "www.youtube.com" {
        url.query_pairs()
            .find(|(k, _)| k == "v")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let ok = id.len() >= 6
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    ok.then_some(id)
}

fn vimeo_id(url: &Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    if host != "vimeo.com" && host != "player.vimeo.com" {
        return None;
    }
    let id = url
        .path()
        .split('/')
        .rfind(|p| !p.is_empty())
        .unwrap_or("");
    id.chars()
        .all(|c| c.is_ascii_digit())
        .then(|| id.to_string())
        .filter(|s| !s.is_empty())
}

/// Sandboxed iframe HTML when the URL host is in the live `embed.hosts`
/// setting. Unknown hosts in the allow-list still produce no `html`.
pub fn oembed_iframe(raw: &str, allowed_hosts: &[String]) -> Option<String> {
    let url = Url::parse(raw).ok()?;
    if url.scheme() != "http" && url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    if !allowed_hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&host))
    {
        return None;
    }
    let mut src = String::new();
    if let Some(id) = youtube_id(&url) {
        src = format!("https://www.youtube.com/embed/{id}");
    }
    if let Some(id) = vimeo_id(&url) {
        src = format!("https://player.vimeo.com/video/{id}");
    }
    if host == "figma.com" || host == "www.figma.com" {
        src = format!(
            "https://www.figma.com/embed?embed_host=fvoci&url={}",
            urlencoding_plus(url.as_str())
        );
    }
    if src.is_empty() {
        return None;
    }
    Some(format!(
        "<iframe src=\"{src}\" sandbox=\"allow-scripts allow-same-origin\" loading=\"lazy\" referrerpolicy=\"no-referrer\"></iframe>"
    ))
}

fn urlencoding_plus(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char);
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

const _: usize = RESPONSE_READ_CAP;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integrations::outbound::{GetClient, GetFuture, Resolve, ResolveFuture};
    use std::collections::{HashMap, VecDeque};
    use std::net::{IpAddr, SocketAddr};
    use std::sync::{Arc, Mutex};

    fn none_policy_outbound(resolver: Arc<dyn Resolve>, client: Arc<dyn GetClient>) -> Outbound {
        Outbound::with_get_client(Default::default(), resolver, client)
    }

    struct Fixed(Vec<IpAddr>);

    impl Resolve for Fixed {
        fn lookup<'a>(&'a self, _host: &'a str) -> ResolveFuture<'a> {
            let answers = self.0.clone();
            Box::pin(async move { Ok(answers) })
        }
    }

    struct Table(HashMap<String, Vec<IpAddr>>);

    impl Resolve for Table {
        fn lookup<'a>(&'a self, host: &'a str) -> ResolveFuture<'a> {
            let answers = self.0.get(host).cloned().unwrap_or_default();
            Box::pin(async move { Ok(answers) })
        }
    }

    type ScriptedHop = Result<PinnedGet, OutboundError>;
    type ScriptedHops = HashMap<String, VecDeque<ScriptedHop>>;
    type ScriptedSeenEntry = (String, u16, Vec<(String, String)>);

    #[derive(Clone)]
    struct Scripted {
        hops: Arc<Mutex<ScriptedHops>>,
        seen: Arc<Mutex<Vec<ScriptedSeenEntry>>>,
        fetched: Arc<Mutex<u32>>,
    }

    impl Scripted {
        fn new(hops: &[(&str, PinnedGet)]) -> Self {
            let mut map = HashMap::new();
            for (url, hop) in hops {
                map.entry((*url).to_string())
                    .or_insert_with(VecDeque::new)
                    .push_back(Ok(hop.clone()));
            }
            Self {
                hops: Arc::new(Mutex::new(map)),
                seen: Arc::new(Mutex::new(Vec::new())),
                fetched: Arc::new(Mutex::new(0)),
            }
        }
    }

    impl GetClient for Scripted {
        fn get<'a>(
            &'a self,
            url: &'a Url,
            pinned: SocketAddr,
            headers: &'a [(&'a str, String)],
            _timeout: Duration,
        ) -> GetFuture<'a> {
            *self.fetched.lock().unwrap() += 1;
            let header_pairs = headers
                .iter()
                .map(|(n, v)| ((*n).to_string(), v.clone()))
                .collect();
            self.seen
                .lock()
                .unwrap()
                .push((url.to_string(), pinned.port(), header_pairs));
            let next = self
                .hops
                .lock()
                .unwrap()
                .get_mut(url.as_str())
                .and_then(|q| q.pop_front());
            Box::pin(async move {
                next.unwrap_or_else(|| {
                    Ok(PinnedGet {
                        status: 200,
                        location: None,
                        body: Vec::new(),
                    })
                })
            })
        }
    }

    fn html(body: &str) -> PinnedGet {
        PinnedGet {
            status: 200,
            location: None,
            body: body.as_bytes().to_vec(),
        }
    }

    fn redirect(location: &str) -> PinnedGet {
        PinnedGet {
            status: 302,
            location: Some(location.to_string()),
            body: Vec::new(),
        }
    }

    fn json_body(value: serde_json::Value) -> PinnedGet {
        PinnedGet {
            status: 200,
            location: None,
            body: serde_json::to_vec(&value).expect("json"),
        }
    }

    fn public_dns() -> Arc<dyn Resolve> {
        Arc::new(Fixed(vec!["1.1.1.1".parse().unwrap()]))
    }

    #[test]
    fn query_rejects_non_http() {
        assert!(validate_unfurl_query("javascript:alert(1)").is_err());
        assert!(validate_unfurl_query("ftp://files.example/x").is_err());
        assert!(validate_unfurl_query("").is_err());
        assert!(validate_unfurl_query("https://example.com/x").is_ok());
    }

    #[test]
    fn og_both_attribute_orders_entities_and_unicode() {
        let html = concat!(
            "<html><head>",
            r#"<meta property="og:title" content="Hello &amp; Co 한글">"#,
            r#"<meta content="Desc" property="og:description">"#,
            r#"<meta property="og:image" content="/img.png">"#,
            "</head></html>",
        );
        let (title, description, image) = parse_og(html, "https://example.com/page");
        assert_eq!(title.as_deref(), Some("Hello & Co 한글"));
        assert_eq!(description.as_deref(), Some("Desc"));
        assert_eq!(image.as_deref(), Some("https://example.com/img.png"));
    }

    #[test]
    fn og_skips_javascript_image() {
        let html = r#"<meta property="og:image" content="javascript:alert(1)">"#;
        let (_, _, image) = parse_og(html, "https://example.com/");
        assert!(image.is_none());
    }

    #[tokio::test]
    async fn github_issue_json_sets_ua_and_skips_html() {
        let client = Scripted::new(&[(
            "https://api.github.com/repos/fvoci/FVOCI/issues/12",
            json_body(serde_json::json!({
                "title": "버그",
                "body": "본문",
                "state": "open",
                "number": 12,
                "html_url": "https://github.com/fvoci/FVOCI/issues/12",
            })),
        )]);
        let outbound = none_policy_outbound(public_dns(), Arc::new(client.clone()));
        let result = fetch_unfurl(
            &outbound,
            "https://github.com/fvoci/FVOCI/issues/12",
            UNFURL_TIMEOUT,
        )
        .await
        .expect("ok");
        assert_eq!(result.kind, UnfurlKind::GithubIssue);
        assert_eq!(result.title.as_deref(), Some("버그"));
        assert_eq!(result.description.as_deref(), Some("본문"));
        assert_eq!(result.state.as_deref(), Some("open"));
        assert_eq!(result.number, Some(12));
        assert_eq!(result.owner.as_deref(), Some("fvoci"));
        assert_eq!(result.repo.as_deref(), Some("FVOCI"));
        assert!(result.html.is_none());
        let seen = client.seen.lock().unwrap();
        assert_eq!(
            seen[0].0,
            "https://api.github.com/repos/fvoci/FVOCI/issues/12"
        );
        let headers = &seen[0].2;
        assert!(headers
            .iter()
            .any(|(n, v)| n == "User-Agent" && v == "FVOCI"));
        assert!(headers
            .iter()
            .any(|(n, v)| n == "Accept" && v == "application/vnd.github+json"));
    }

    #[tokio::test]
    async fn github_pull_json() {
        let client = Scripted::new(&[(
            "https://api.github.com/repos/o/r/pulls/3",
            json_body(serde_json::json!({
                "title": "PR",
                "body": null,
                "state": "closed",
                "number": 3,
                "html_url": "https://github.com/o/r/pull/3",
            })),
        )]);
        let outbound = none_policy_outbound(public_dns(), Arc::new(client));
        let result = fetch_unfurl(&outbound, "https://github.com/o/r/pull/3", UNFURL_TIMEOUT)
            .await
            .expect("ok");
        assert_eq!(result.kind, UnfurlKind::GithubPull);
        assert_eq!(result.title.as_deref(), Some("PR"));
        assert_eq!(result.state.as_deref(), Some("closed"));
        assert!(result.description.is_none());
    }

    #[tokio::test]
    async fn github_api_transport_falls_back_to_og() {
        let client = Scripted::new(&[(
            "https://github.com/o/r/issues/1",
            html(r#"<meta property="og:title" content="from-html">"#),
        )]);
        // Missing API hop → default 200 empty, treated as semantic miss, then OG.
        let outbound = none_policy_outbound(public_dns(), Arc::new(client));
        let result = fetch_unfurl(&outbound, "https://github.com/o/r/issues/1", UNFURL_TIMEOUT)
            .await
            .expect("ok");
        assert_eq!(result.title.as_deref(), Some("from-html"));
        assert_eq!(result.kind, UnfurlKind::GithubIssue);
    }

    #[tokio::test]
    async fn github_api_ssrf_does_not_fall_back_to_og() {
        let mut map = HashMap::new();
        map.insert("api.github.com".into(), vec!["10.0.0.9".parse().unwrap()]);
        map.insert("github.com".into(), vec!["1.1.1.1".parse().unwrap()]);
        let client = Scripted::new(&[(
            "https://github.com/o/r/issues/1",
            html(r#"<meta property="og:title" content="should-not-run">"#),
        )]);
        let outbound = none_policy_outbound(Arc::new(Table(map)), Arc::new(client.clone()));
        let err = fetch_unfurl(&outbound, "https://github.com/o/r/issues/1", UNFURL_TIMEOUT)
            .await
            .expect_err("ssrf");
        assert_eq!(err, OutboundRejected::Address);
        assert_eq!(*client.fetched.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn private_dns_is_ssrf_not_empty_og() {
        let client = Scripted::new(&[]);
        let outbound = none_policy_outbound(
            Arc::new(Fixed(vec!["192.168.1.8".parse().unwrap()])),
            Arc::new(client.clone()),
        );
        let err = fetch_unfurl(&outbound, "https://example.com/x", UNFURL_TIMEOUT)
            .await
            .expect_err("ssrf");
        assert_eq!(err, OutboundRejected::Address);
        assert_eq!(*client.fetched.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn mixed_public_and_private_dns_is_ssrf() {
        let client = Scripted::new(&[]);
        let outbound = none_policy_outbound(
            Arc::new(Fixed(vec![
                "1.1.1.1".parse().unwrap(),
                "10.0.0.1".parse().unwrap(),
            ])),
            Arc::new(client.clone()),
        );
        let err = fetch_unfurl(&outbound, "https://example.com/x", UNFURL_TIMEOUT)
            .await
            .expect_err("ssrf");
        assert_eq!(err, OutboundRejected::Address);
        assert_eq!(*client.fetched.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn redirect_hop_rechecks_and_rejects_private_dns() {
        let mut map = HashMap::new();
        map.insert("example.com".into(), vec!["1.1.1.1".parse().unwrap()]);
        map.insert("evil.example".into(), vec!["10.0.0.9".parse().unwrap()]);
        let client = Scripted::new(&[(
            "https://example.com/start",
            redirect("https://evil.example/secret"),
        )]);
        let outbound = none_policy_outbound(Arc::new(Table(map)), Arc::new(client.clone()));
        let err = fetch_unfurl(&outbound, "https://example.com/start", UNFURL_TIMEOUT)
            .await
            .expect_err("ssrf redirect");
        assert_eq!(err, OutboundRejected::Address);
        assert_eq!(*client.fetched.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn each_redirect_is_validated() {
        let client = Scripted::new(&[
            ("https://example.com/a", redirect("https://example.com/b")),
            ("https://example.com/b", redirect("https://example.com/c")),
            (
                "https://example.com/c",
                html(r#"<meta property="og:title" content="done">"#),
            ),
        ]);
        let outbound = none_policy_outbound(public_dns(), Arc::new(client.clone()));
        let result = fetch_unfurl(&outbound, "https://example.com/a", UNFURL_TIMEOUT)
            .await
            .expect("ok");
        assert_eq!(result.title.as_deref(), Some("done"));
        assert_eq!(*client.fetched.lock().unwrap(), 3);
    }

    #[tokio::test]
    async fn fourth_redirect_is_rejected() {
        let client = Scripted::new(&[
            ("https://example.com/a", redirect("https://example.com/b")),
            ("https://example.com/b", redirect("https://example.com/c")),
            ("https://example.com/c", redirect("https://example.com/d")),
            ("https://example.com/d", redirect("https://example.com/e")),
        ]);
        let outbound = none_policy_outbound(public_dns(), Arc::new(client));
        let err = fetch_unfurl(&outbound, "https://example.com/a", UNFURL_TIMEOUT)
            .await
            .expect_err("max redirects");
        assert_eq!(err, OutboundRejected::Redirect);
    }

    #[tokio::test]
    async fn timeout_returns_empty_og() {
        struct Hang;
        impl GetClient for Hang {
            fn get<'a>(
                &'a self,
                _url: &'a Url,
                _pinned: SocketAddr,
                _headers: &'a [(&'a str, String)],
                _timeout: Duration,
            ) -> GetFuture<'a> {
                Box::pin(std::future::pending())
            }
        }
        let outbound = none_policy_outbound(public_dns(), Arc::new(Hang));
        let result = fetch_unfurl(
            &outbound,
            "https://example.com/slow",
            Duration::from_millis(30),
        )
        .await
        .expect("empty");
        assert_eq!(result.kind, UnfurlKind::Og);
        assert!(result.title.is_none());
    }

    #[tokio::test]
    async fn webhook_allow_list_is_ignored() {
        let policy = crate::integrations::outbound::OutboundPolicy::parse_allow_list("127.0.0.1")
            .expect("policy");
        let client = Scripted::new(&[]);
        let webhook = Outbound::with_get_client(
            policy,
            Arc::new(Fixed(vec!["127.0.0.1".parse().unwrap()])),
            Arc::new(client.clone()),
        );
        let unfurl = webhook.without_allow_list();
        let err = fetch_unfurl(&unfurl, "http://127.0.0.1/secret", UNFURL_TIMEOUT)
            .await
            .expect_err("strict");
        assert_eq!(err, OutboundRejected::Url);
        assert_eq!(*client.fetched.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn oversized_body_is_truncated_before_parse() {
        let mut body = br#"<meta property="og:title" content="keep">"#.to_vec();
        body.extend(std::iter::repeat_n(b'x', RESPONSE_READ_CAP + 8));
        let client = Scripted::new(&[(
            "https://example.com/big",
            PinnedGet {
                status: 200,
                location: None,
                body,
            },
        )]);
        let outbound = none_policy_outbound(public_dns(), Arc::new(client));
        let result = fetch_unfurl(&outbound, "https://example.com/big", UNFURL_TIMEOUT)
            .await
            .expect("ok");
        assert_eq!(result.title.as_deref(), Some("keep"));
    }

    #[test]
    fn oembed_youtube_and_allowlist_miss() {
        let defaults = [
            "youtube.com".into(),
            "www.youtube.com".into(),
            "youtu.be".into(),
        ];
        let html = oembed_iframe("https://www.youtube.com/watch?v=dQw4w9WgXcQ", &defaults)
            .expect("iframe");
        assert!(html.contains("youtube.com/embed/dQw4w9WgXcQ"));
        assert!(html.contains("sandbox=\"allow-scripts allow-same-origin\""));
        assert!(oembed_iframe("https://example.com/page", &defaults).is_none());
    }

    #[test]
    fn oembed_uses_live_custom_hosts() {
        let custom = vec!["example.com".into()];
        assert!(oembed_iframe("https://www.youtube.com/watch?v=dQw4w9WgXcQ", &custom).is_none());
        let figma = vec!["www.figma.com".into()];
        let html = oembed_iframe("https://www.figma.com/file/abc", &figma).expect("iframe");
        assert!(html.contains("figma.com/embed"));
        assert!(html.contains("sandbox="));
    }

    #[test]
    fn private_literals_are_invalid_input() {
        for bad in [
            "ftp://example.com/",
            "http://127.0.0.1/",
            "http://localhost/",
            "https://example.com:8443/",
            "http://169.254.169.254/latest/meta-data/",
            "http://[::ffff:127.0.0.1]/x",
        ] {
            assert!(parse_target_url(bad, &Default::default()).is_err(), "{bad}");
        }
    }
}
