//! Operational probes outside `/api/v1` (source `INFRA_PATHS`,
//! `platform/observability.ts`, `_shared/metrics-allow.ts`, `server.ts`).
//!
//! - `GET /health`: liveness, always `200 {"ok":true}`.
//! - `GET /ready`: PostgreSQL ping and, when the collaboration hub runs in
//!   this process, whether it still accepts connections. `200 {"ok":true}`
//!   or `503 {"ok":false,"checks":{...}}`. Each check has a 2 s budget.
//! - `GET /metrics`: Prometheus scrape, answered only to direct socket peers
//!   inside `METRICS_ALLOW_IPS`; everyone else gets the generic 404 so the
//!   surface does not announce itself. Unset or empty denies every peer.
//!
//! None of them read a session, consent or bearer token, and no label carries
//! a tenant, user, document or concrete request path.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use prometheus_client::encoding::EncodeLabelSet;
use prometheus_client::metrics::family::Family;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::metrics::histogram::Histogram;
use prometheus_client::registry::Registry;
use serde_json::{json, Map, Value};
use sqlx::PgPool;

use crate::error::{AppError, ProblemCode};
use crate::http::state::AppState;

pub const HEALTH_PATH: &str = "/health";
pub const READY_PATH: &str = "/ready";
pub const METRICS_PATH: &str = "/metrics";

/// Paths the SPA shell never answers (source `SHELL_EXCLUDED_PREFIXES`).
pub const PROBE_PATHS: [&str; 3] = [HEALTH_PATH, READY_PATH, METRICS_PATH];

/// Source `READY_CHECK_TIMEOUT_MS`: per readiness check and per metrics
/// refresh query.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(2);

/// Source `metricsCacheMs` outside tests: scrapes within this window reuse
/// the last database-derived values.
pub const METRICS_REFRESH_INTERVAL: Duration = Duration::from_secs(15);

const OPENMETRICS_CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

/// Source `fvoci_http_request_duration_seconds` buckets.
const HTTP_BUCKETS: [f64; 10] = [0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0];

/// Route label for requests no router path matched (source `"unmatched"`).
pub const UNMATCHED_ROUTE_LABEL: &str = "unmatched";

pub fn is_probe_path(path: &str) -> bool {
    PROBE_PATHS.iter().any(|probe| {
        path == *probe
            || path
                .strip_prefix(probe)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// `METRICS_ALLOW_IPS`: comma-separated IPv4 addresses or CIDRs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MetricsAllowList {
    entries: Vec<(u32, u32)>,
}

impl MetricsAllowList {
    /// Source `parseAllowList`: a CIDR prefix is required after `/` and must
    /// be 1-32 (`/0` would switch the control off); IPv6 entries are
    /// refused; one bad entry fails startup instead of silently opening.
    pub fn parse(raw: Option<&str>) -> Result<Self, String> {
        let mut entries = Vec::new();
        for token in raw.unwrap_or("").split(',') {
            let entry = token.trim();
            if entry.is_empty() {
                continue;
            }
            let parsed = parse_entry(entry).ok_or_else(|| {
                format!(
                    "METRICS_ALLOW_IPS: invalid entry {}",
                    serde_json::to_string(entry).unwrap_or_default()
                )
            })?;
            entries.push(parsed);
        }
        Ok(Self { entries })
    }

    pub fn from_env() -> Result<Self, String> {
        match std::env::var("METRICS_ALLOW_IPS") {
            Ok(raw) => Self::parse(Some(&raw)),
            Err(std::env::VarError::NotPresent) => Ok(Self::default()),
            Err(std::env::VarError::NotUnicode(_)) => {
                Err("METRICS_ALLOW_IPS: not valid UTF-8".to_string())
            }
        }
    }

    /// IPv4-mapped IPv6 peers (dual-stack sockets) compare as IPv4; other
    /// IPv6 peers never match.
    pub fn allows(&self, ip: IpAddr) -> bool {
        let v4 = match ip {
            IpAddr::V4(v4) => v4,
            IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
                Some(v4) => v4,
                None => return false,
            },
        };
        let target = u32::from(v4);
        self.entries
            .iter()
            .any(|(base, mask)| target & mask == *base)
    }
}

fn parse_entry(entry: &str) -> Option<(u32, u32)> {
    let Some((addr, prefix)) = entry.split_once('/') else {
        return Some((parse_v4(strip_mapped(entry))?, u32::MAX));
    };
    let bits = match prefix.as_bytes() {
        [d @ b'1'..=b'9'] => u32::from(d - b'0'),
        [t @ (b'1' | b'2'), d @ b'0'..=b'9'] => u32::from(t - b'0') * 10 + u32::from(d - b'0'),
        [b'3', d @ b'0'..=b'2'] => 30 + u32::from(d - b'0'),
        _ => return None,
    };
    let mask = u32::MAX << (32 - bits);
    Some((parse_v4(strip_mapped(addr))? & mask, mask))
}

/// `::ffff:a.b.c.d` (any case) as written by dual-stack tooling.
fn strip_mapped(entry: &str) -> &str {
    match entry.get(..7) {
        Some(head) if head.eq_ignore_ascii_case("::ffff:") => &entry[7..],
        _ => entry,
    }
}

/// Dotted quad with no leading zeros, each octet at most 255.
fn parse_v4(text: &str) -> Option<u32> {
    let mut value: u32 = 0;
    let mut parts = 0;
    for part in text.split('.') {
        parts += 1;
        let bytes = part.as_bytes();
        let well_formed = matches!(bytes, [b'0'])
            || (matches!(bytes.first(), Some(b'1'..=b'9'))
                && bytes.len() <= 3
                && bytes.iter().all(u8::is_ascii_digit));
        if parts > 4 || !well_formed {
            return None;
        }
        let octet: u32 = part.parse().ok()?;
        if octet > 255 {
            return None;
        }
        value = (value << 8) | octet;
    }
    (parts == 4).then_some(value)
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct HttpLabels {
    method: &'static str,
    route: String,
    status: u16,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
struct PoolLabels {
    state: &'static str,
}

/// Process-wide metrics and the `/metrics` access list.
pub struct Observability {
    registry: Registry,
    http_duration: Family<HttpLabels, Histogram>,
    outbox_lag: Gauge,
    outbox_xmin_stall: Gauge,
    task_stream_subscribers: Gauge,
    db_pool_connections: Family<PoolLabels, Gauge>,
    db_pool_max_connections: Gauge,
    allow: MetricsAllowList,
    outbox_consumers: Vec<String>,
    refresh_interval: Duration,
    last_refresh: tokio::sync::Mutex<Option<Instant>>,
}

#[derive(Clone, Debug, Default)]
pub struct ObservabilitySettings {
    pub allow: MetricsAllowList,
    /// Outbox consumer names whose backlog feeds `fvoci_outbox_lag_seconds`.
    pub outbox_consumers: Vec<String>,
    /// Zero refreshes on every scrape (source test mode).
    pub refresh_interval: Duration,
}

impl Observability {
    pub fn new(settings: ObservabilitySettings) -> Self {
        let mut registry = Registry::default();
        let http_duration =
            Family::<HttpLabels, Histogram>::new_with_constructor(|| Histogram::new(HTTP_BUCKETS));
        registry.register(
            "fvoci_http_request_duration_seconds",
            "HTTP request duration",
            http_duration.clone(),
        );
        let outbox_lag = Gauge::default();
        registry.register(
            "fvoci_outbox_lag_seconds",
            "Age in seconds of the oldest outbox event some consumer cursor has not \
             passed, including events held behind a long-running transaction",
            outbox_lag.clone(),
        );
        let outbox_xmin_stall = Gauge::default();
        registry.register(
            "fvoci_outbox_xmin_stall_seconds",
            "Age in seconds of the oldest transaction holding an xid in the \
             PostgreSQL cluster; it holds back the snapshot xmin outbox delivery waits on",
            outbox_xmin_stall.clone(),
        );
        let task_stream_subscribers = Gauge::default();
        registry.register(
            "fvoci_task_stream_subscribers",
            "Active project task SSE subscribers",
            task_stream_subscribers.clone(),
        );
        let db_pool_connections = Family::<PoolLabels, Gauge>::default();
        registry.register(
            "fvoci_db_pool_connections",
            "Application database pool connections by state",
            db_pool_connections.clone(),
        );
        let db_pool_max_connections = Gauge::default();
        registry.register(
            "fvoci_db_pool_max_connections",
            "Application database pool connection limit",
            db_pool_max_connections.clone(),
        );
        Self {
            registry,
            http_duration,
            outbox_lag,
            outbox_xmin_stall,
            task_stream_subscribers,
            db_pool_connections,
            db_pool_max_connections,
            allow: settings.allow,
            outbox_consumers: settings.outbox_consumers,
            refresh_interval: settings.refresh_interval,
            last_refresh: tokio::sync::Mutex::new(None),
        }
    }

    fn observe_http(&self, method: &'static str, route: String, status: u16, elapsed: Duration) {
        self.http_duration
            .get_or_create(&HttpLabels {
                method,
                route,
                status,
            })
            .observe(elapsed.as_secs_f64());
    }

    /// Database-derived values, at most once per `refresh_interval`;
    /// concurrent scrapes wait for the one in flight. A failed query keeps
    /// the last value: `/metrics` never answers 503 (source).
    async fn refresh(&self, pool: &PgPool) {
        let mut last = self.last_refresh.lock().await;
        if last.is_some_and(|at| at.elapsed() < self.refresh_interval) {
            return;
        }
        match tokio::time::timeout(CHECK_TIMEOUT, outbox_ages(pool, &self.outbox_consumers)).await {
            Ok(Ok((lag, stall))) => {
                self.outbox_lag.set(lag);
                self.outbox_xmin_stall.set(stall);
            }
            Ok(Err(err)) => tracing::debug!(error = %err, "metrics: outbox lag query failed"),
            Err(_) => tracing::debug!("metrics: outbox lag query timed out"),
        }
        *last = Some(Instant::now());
    }

    fn sample_live(&self, state: &AppState) {
        let pool = &state.auth.db.pool;
        let size = i64::from(pool.size());
        let idle = i64::try_from(pool.num_idle()).unwrap_or(i64::MAX);
        self.db_pool_connections
            .get_or_create(&PoolLabels { state: "idle" })
            .set(idle);
        self.db_pool_connections
            .get_or_create(&PoolLabels { state: "active" })
            .set((size - idle).max(0));
        self.db_pool_max_connections
            .set(i64::from(pool.options().get_max_connections()));
        self.task_stream_subscribers
            .set(i64::try_from(state.streams.active_count()).unwrap_or(i64::MAX));
    }

    fn encode(&self) -> Result<String, std::fmt::Error> {
        let mut out = String::new();
        prometheus_client::encoding::text::encode(&mut out, &self.registry)?;
        Ok(out)
    }
}

/// Oldest undelivered event age across `consumers` (each has its own
/// cursor, no snapshot-xmin filter; 0 when every consumer is caught up) and
/// the age of the oldest xid holder in the cluster, in one statement.
async fn outbox_ages(pool: &PgPool, consumers: &[String]) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT fvoci.app_outbox_lag_seconds($1), fvoci.app_oldest_write_xact_age_seconds()",
    )
    .bind(consumers)
    .fetch_one(pool)
    .await
}

#[derive(Clone)]
struct ProbeState {
    app: AppState,
    observability: Arc<Observability>,
}

/// The probe routes, merged outside the consent gate and the bearer path
/// canonicalization (source `CONSENT_ALLOWLIST`, `SESSIONLESS_PATHS`).
pub fn router(state: AppState, observability: Arc<Observability>) -> Router {
    Router::new()
        .route(HEALTH_PATH, any(health))
        .route(READY_PATH, any(ready))
        .route(METRICS_PATH, any(metrics))
        .with_state(ProbeState {
            app: state,
            observability,
        })
}

/// Methods other than GET/HEAD get the same generic 404 as a denied
/// `/metrics` peer. The routes use `any` because axum's `get(..).fallback(..)`
/// would still add `Allow: GET,HEAD` and announce the route.
fn reject_method(method: &Method) -> Option<Response> {
    (method != Method::GET && method != Method::HEAD)
        .then(|| AppError::from_code(ProblemCode::NotFound).into_response())
}

async fn health(method: Method) -> Response {
    if let Some(rejected) = reject_method(&method) {
        return rejected;
    }
    Json(json!({ "ok": true })).into_response()
}

async fn ready(method: Method, State(probe): State<ProbeState>) -> Response {
    if let Some(rejected) = reject_method(&method) {
        return rejected;
    }
    let state = &probe.app;
    let pg = ping_database(&state.auth.db.pool).await;
    let mut checks = Map::new();
    checks.insert("pg".into(), Value::Bool(pg));
    let mut ok = pg;
    if let Some(hub) = state.collab.as_ref() {
        let collab = !hub.is_shutting_down();
        checks.insert("collab".into(), Value::Bool(collab));
        ok &= collab;
    }
    if ok {
        Json(json!({ "ok": true })).into_response()
    } else {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "ok": false, "checks": checks })),
        )
            .into_response()
    }
}

async fn ping_database(pool: &PgPool) -> bool {
    matches!(
        tokio::time::timeout(CHECK_TIMEOUT, sqlx::query("SELECT 1").execute(pool)).await,
        Ok(Ok(_))
    )
}

/// The direct socket peer decides (like the rate limiter); forwarded
/// headers are never read. No peer info (in-process calls) is denied.
async fn metrics(State(probe): State<ProbeState>, req: Request) -> Response {
    if let Some(rejected) = reject_method(req.method()) {
        return rejected;
    }
    let observability = &probe.observability;
    let allowed = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .is_some_and(|ConnectInfo(addr)| observability.allow.allows(addr.ip()));
    if !allowed {
        return AppError::from_code(ProblemCode::NotFound).into_response();
    }
    observability.refresh(&probe.app.auth.db.pool).await;
    observability.sample_live(&probe.app);
    match observability.encode() {
        Ok(body) => (
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static(OPENMETRICS_CONTENT_TYPE),
            )],
            body,
        )
            .into_response(),
        Err(_) => AppError::internal().into_response(),
    }
}

/// Records `fvoci_http_request_duration_seconds` for every request. Must be
/// added with `Router::layer` so the matched route template is visible.
pub async fn record_http(
    State(observability): State<Arc<Observability>>,
    req: Request,
    next: Next,
) -> Response {
    let method = crate::http::request_trace::method_label(req.method());
    let route = req
        .extensions()
        .get::<MatchedPath>()
        .map_or(UNMATCHED_ROUTE_LABEL, MatchedPath::as_str)
        .to_string();
    let started = Instant::now();
    let response = next.run(req).await;
    observability.observe_http(method, route, response.status().as_u16(), started.elapsed());
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn allow_list_matches_source_parser() {
        let list = MetricsAllowList::parse(Some(" 172.30.0.0/24, 127.0.0.1/32 ,,10.1.2.3"))
            .expect("valid list");
        assert!(list.allows(ip("172.30.0.9")));
        assert!(list.allows(ip("127.0.0.1")));
        assert!(list.allows(ip("10.1.2.3")));
        assert!(list.allows(ip("::ffff:172.30.0.200")));
        assert!(!list.allows(ip("172.30.1.1")));
        assert!(!list.allows(ip("10.1.2.4")));
        assert!(!list.allows(ip("::1")));
        let mapped = MetricsAllowList::parse(Some("::FFFF:192.168.1.0/24")).unwrap();
        assert!(mapped.allows(ip("192.168.1.77")));
    }

    #[test]
    fn allow_list_unset_or_empty_denies_everyone() {
        for raw in [None, Some(""), Some(" , ")] {
            let list = MetricsAllowList::parse(raw).unwrap();
            assert!(!list.allows(ip("127.0.0.1")), "{raw:?}");
        }
    }

    #[test]
    fn allow_list_rejects_malformed_entries_fail_closed() {
        for bad in [
            "172.30.0.0/",
            "0.0.0.0/0",
            "10.0.0.0/33",
            "10.0.0.0/08",
            "10.0.0.0/ 8",
            "01.2.3.4",
            "1.2.3",
            "1.2.3.4.5",
            "256.1.1.1",
            "::1",
            "fd00::/8",
            "localhost",
            "1.2.3.4/+8",
        ] {
            let err = MetricsAllowList::parse(Some(&format!("127.0.0.1,{bad}"))).expect_err(bad);
            assert!(
                err.starts_with("METRICS_ALLOW_IPS: invalid entry \""),
                "{err}"
            );
        }
    }

    #[test]
    fn probe_paths_match_whole_segments() {
        assert!(is_probe_path("/health"));
        assert!(is_probe_path("/ready/x"));
        assert!(is_probe_path("/metrics/"));
        assert!(!is_probe_path("/healthz"));
        assert!(!is_probe_path("/api/v1/health"));
    }
}
