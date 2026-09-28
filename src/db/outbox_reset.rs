//! `fvoci-migrate --outbox-reset`: source `fvoci outbox-reset`
//! (apps/server/src/cli.ts runOutboxReset, packages/db/src/pg/repos/events.ts
//! diagnoseResetSkips / resetCursorToProcessed).
//!
//! The source kept one relay cursor and moved it to just before the first
//! event in the last 29 days that the `notifications` consumer had not marked
//! processed. Here every consumer owns its cursor, so the same rule runs per
//! consumer against that consumer's own `processed_events` marks. Events,
//! marks and failures are never deleted; only `outbox_consumers` positions
//! move. Events older than the window that the move would pass unprocessed
//! are a forward skip and need `--override-reason`.
//!
//! The app role has no access to the cursor tables (grant-app-role.sql), so
//! this runs as the owner like `--recover-outbox`, in system context because
//! `events` forces row-level security.

use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Postgres, Row, Transaction};

use super::outbox_recover::RESET_SCAN_WINDOW_DAYS;

/// Consumers of this build that mark every event they pass in
/// `processed_events`, so their marks show how far they really got.
pub const MARKING_CONSUMERS: &[&str] =
    &["notifications", "mail", "push", "webhooks", "search-index"];
const SEARCH_INDEX_CONSUMER: &str = "search-index";
const GITHUB_CONSUMER: &str = "github";
const SKIP_SAMPLE_LIMIT: i64 = 100;

#[derive(Debug, Clone, Default)]
pub struct OutboxResetOptions {
    pub consumers: Vec<String>,
    pub apply: bool,
    pub reason: Option<String>,
    pub override_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CursorPos {
    pub last_xact: String,
    pub last_seq: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedEvent {
    pub event_id: String,
    pub verb: String,
}

/// Source `ResetCursorSkip`: unprocessed events older than the window that
/// the move passes. Coordinates are the (xact, seq) lexical min and max.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ResetSkip {
    pub skipped_count: i64,
    pub min_xact: String,
    pub min_seq: i64,
    pub max_xact: String,
    pub max_seq: i64,
    pub oldest_created_at: chrono::DateTime<chrono::Utc>,
    pub sample: Vec<SkippedEvent>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsumerReset {
    pub consumer: String,
    pub lease_active: bool,
    pub before: CursorPos,
    pub target: CursorPos,
    /// `forward`, `backward` or `unchanged`.
    pub direction: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub noop_reason: Option<String>,
    /// Events between the target and the current cursor without a mark: the
    /// consumer delivers them again after a backward move.
    pub redelivered: i64,
    pub skip: Option<ResetSkip>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedConsumer {
    pub consumer: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutboxResetReport {
    /// `diagnose` (no writes) or `apply`.
    pub mode: &'static str,
    pub applied: bool,
    pub window_days: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub override_reason: Option<String>,
    pub consumers: Vec<ConsumerReset>,
    pub excluded: Vec<ExcludedConsumer>,
}

#[derive(Debug, thiserror::Error)]
pub enum OutboxResetError {
    #[error("{0}")]
    Rejected(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

fn rejected(message: impl Into<String>) -> OutboxResetError {
    OutboxResetError::Rejected(message.into())
}

pub fn parse_outbox_reset_args(args: &[String]) -> Result<OutboxResetOptions, OutboxResetError> {
    let mut opts = OutboxResetOptions::default();
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let (name, inline) = match arg.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (arg.as_str(), None),
        };
        match name {
            "--apply" => {
                if inline.is_some() {
                    return Err(rejected("--apply does not take a value"));
                }
                opts.apply = true;
            }
            "--consumer" | "--reason" | "--override-reason" => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        i += 1;
                        args.get(i)
                            .filter(|next| !next.starts_with("--"))
                            .cloned()
                            .unwrap_or_default()
                    }
                };
                if value.trim().is_empty() {
                    return Err(rejected(format!("{name} requires a non-empty value")));
                }
                match name {
                    "--consumer" => {
                        if !valid_consumer_name(&value) {
                            return Err(rejected(format!("invalid consumer name {value:?}")));
                        }
                        if !opts.consumers.contains(&value) {
                            opts.consumers.push(value);
                        }
                    }
                    "--reason" => opts.reason = Some(value),
                    _ => opts.override_reason = Some(value),
                }
            }
            _ => return Err(rejected(format!("unknown option: {name}"))),
        }
        i += 1;
    }
    if opts.apply && opts.reason.is_none() {
        return Err(rejected("--apply requires --reason"));
    }
    if !opts.apply && (opts.reason.is_some() || opts.override_reason.is_some()) {
        return Err(rejected(
            "--reason and --override-reason only apply with --apply; without it outbox-reset only diagnoses",
        ));
    }
    Ok(opts)
}

/// Same rule as `outbox_consumers_name_check`.
fn valid_consumer_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some('a'..='z'))
        && name.len() <= 63
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '_' | '-'))
}

/// `search_configured`: whether this environment configures the search index,
/// the only default consumer the server registers conditionally.
pub async fn outbox_reset(
    url: &str,
    opts: OutboxResetOptions,
    search_configured: bool,
) -> Result<OutboxResetReport, OutboxResetError> {
    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let result = outbox_reset_on(&pool, &opts, search_configured).await;
    pool.close().await;
    result
}

struct CursorRow {
    consumer: String,
    pos: CursorPos,
    lease_active: bool,
}

async fn outbox_reset_on(
    pool: &PgPool,
    opts: &OutboxResetOptions,
    search_configured: bool,
) -> Result<OutboxResetReport, OutboxResetError> {
    let mut tx = pool.begin().await?;
    if !opts.apply {
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
    }
    // events FORCEs row-level security; the owner reads it like systemTx.
    sqlx::query("SELECT set_config('app.system_ctx', 'on', true)")
        .execute(&mut *tx)
        .await?;

    if opts.apply {
        let other_sessions: i64 = sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM pg_stat_activity
            WHERE datname = current_database()
              AND pid <> pg_backend_pid()
              AND backend_type = 'client backend'
            "#,
        )
        .fetch_one(&mut *tx)
        .await?;
        if other_sessions > 0 {
            return Err(rejected(format!(
                "stop the server and every other database session before outbox-reset --apply ({other_sessions} other sessions connected)"
            )));
        }
        sqlx::query("LOCK TABLE fvoci.outbox_consumers IN ACCESS EXCLUSIVE MODE NOWAIT")
            .execute(&mut *tx)
            .await
            .map_err(|err| rejected(format!("could not lock outbox cursors: {err}")))?;
        sqlx::query("LOCK TABLE fvoci.events, fvoci.processed_events IN SHARE MODE NOWAIT")
            .execute(&mut *tx)
            .await
            .map_err(|err| rejected(format!("could not lock outbox events: {err}")))?;
    }

    // Snapshot and comparison in one statement, as app_outbox_read does.
    let epoch_mismatch: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.outbox_consumers AS c
            WHERE c.last_xact >= pg_snapshot_xmax(pg_current_snapshot())
        ) OR EXISTS (
            SELECT 1 FROM fvoci.events AS e
            WHERE e.xact >= pg_snapshot_xmax(pg_current_snapshot())
        )
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    if epoch_mismatch {
        return Err(rejected(
            "outbox xid epoch mismatch (restored cluster); use fvoci-migrate --recover-outbox instead",
        ));
    }

    let rows = sqlx::query(
        r#"
        SELECT consumer, last_xact::text AS last_xact, last_seq,
               COALESCE(lease_until >= now(), false) AS lease_active
        FROM fvoci.outbox_consumers
        ORDER BY consumer
        "#,
    )
    .fetch_all(&mut *tx)
    .await?;
    let cursors: Vec<CursorRow> = rows
        .iter()
        .map(|row| CursorRow {
            consumer: row.get("consumer"),
            pos: CursorPos {
                last_xact: row.get("last_xact"),
                last_seq: row.get("last_seq"),
            },
            lease_active: row.get("lease_active"),
        })
        .collect();

    let (selected, excluded) = select_consumers(&cursors, &opts.consumers, search_configured)?;

    let mut plans = Vec::with_capacity(selected.len());
    for cursor in selected {
        plans.push(plan_consumer(&mut tx, cursor).await?);
    }

    if opts.apply {
        if let Some(leased) = plans.iter().find(|plan| plan.lease_active) {
            return Err(rejected(format!(
                "active lease held by consumer {}; stop the server and wait for the lease to expire",
                leased.consumer
            )));
        }
        if opts.override_reason.is_none() {
            if let Some(plan) = plans.iter().find(|plan| plan.skip.is_some()) {
                let count = plan.skip.as_ref().map_or(0, |skip| skip.skipped_count);
                return Err(rejected(format!(
                    "forward skip of {count} unprocessed events older than {RESET_SCAN_WINDOW_DAYS} days for consumer {}; refused (--override-reason required)",
                    plan.consumer
                )));
            }
        }
        for plan in plans.iter().filter(|plan| plan.direction != "unchanged") {
            sqlx::query(
                r#"
                UPDATE fvoci.outbox_consumers
                SET last_xact = $2::xid8, last_seq = $3, updated_at = now()
                WHERE consumer = $1
                "#,
            )
            .bind(&plan.consumer)
            .bind(&plan.target.last_xact)
            .bind(plan.target.last_seq)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        for plan in &plans {
            tracing::warn!(
                event = "outbox.reset",
                consumer = %plan.consumer,
                direction = plan.direction,
                reason = opts.reason.as_deref().unwrap_or(""),
                override_reason = opts.override_reason.as_deref().unwrap_or(""),
                skipped_count = plan.skip.as_ref().map_or(0, |skip| skip.skipped_count),
                redelivered = plan.redelivered,
                "outbox consumer cursor reset"
            );
        }
    } else {
        tx.rollback().await?;
    }

    Ok(OutboxResetReport {
        mode: if opts.apply { "apply" } else { "diagnose" },
        applied: opts.apply,
        window_days: RESET_SCAN_WINDOW_DAYS,
        reason: opts.reason.clone(),
        override_reason: opts.override_reason.clone(),
        consumers: plans,
        excluded,
    })
}

fn select_consumers<'a>(
    cursors: &'a [CursorRow],
    named: &[String],
    search_configured: bool,
) -> Result<(Vec<&'a CursorRow>, Vec<ExcludedConsumer>), OutboxResetError> {
    if !named.is_empty() {
        let mut selected = Vec::with_capacity(named.len());
        for name in named {
            let cursor = cursors
                .iter()
                .find(|cursor| &cursor.consumer == name)
                .ok_or_else(|| rejected(format!("unknown outbox consumer {name:?}")))?;
            selected.push(cursor);
        }
        return Ok((selected, Vec::new()));
    }
    let mut selected = Vec::new();
    let mut excluded = Vec::new();
    for cursor in cursors {
        let name = cursor.consumer.as_str();
        let reason = if name == GITHUB_CONSUMER {
            Some("does not mark processed events while the GitHub app is unconfigured; name it with --consumer github to include it")
        } else if name == SEARCH_INDEX_CONSUMER && !search_configured {
            Some("search index is not configured in this environment (FVOCI_MEILI_URL); name it with --consumer search-index to include it")
        } else if !MARKING_CONSUMERS.contains(&name) {
            Some("not a consumer of this build; name it with --consumer to include it")
        } else {
            None
        };
        match reason {
            Some(reason) => excluded.push(ExcludedConsumer {
                consumer: cursor.consumer.clone(),
                reason: reason.into(),
            }),
            None => selected.push(cursor),
        }
    }
    Ok((selected, excluded))
}

/// Source resetCursorToProcessed for one consumer: the target is the event
/// just before the first unmarked event of the window, else the newest event
/// of the window; an empty window leaves the cursor where it is.
async fn plan_consumer(
    tx: &mut Transaction<'_, Postgres>,
    cursor: &CursorRow,
) -> Result<ConsumerReset, OutboxResetError> {
    let consumer = cursor.consumer.as_str();
    let skip = collect_skip(tx, consumer, &cursor.pos).await?;

    let first_unprocessed = sqlx::query(
        r#"
        SELECT e.xact::text AS xact, e.seq
        FROM fvoci.events AS e
        WHERE e.created_at >= now() - make_interval(days => $2)
          AND NOT EXISTS (
              SELECT 1 FROM fvoci.processed_events AS p
              WHERE p.consumer = $1 AND p.event_id = e.id
          )
        ORDER BY e.xact, e.seq
        LIMIT 1
        "#,
    )
    .bind(consumer)
    .bind(RESET_SCAN_WINDOW_DAYS)
    .fetch_optional(&mut **tx)
    .await?;

    let mut noop_reason = None;
    let target = match first_unprocessed {
        Some(first) => {
            let prev = sqlx::query(
                r#"
                SELECT xact::text AS xact, seq
                FROM fvoci.events
                WHERE (xact, seq) < ($1::xid8, $2)
                ORDER BY xact DESC, seq DESC
                LIMIT 1
                "#,
            )
            .bind(first.get::<String, _>("xact"))
            .bind(first.get::<i64, _>("seq"))
            .fetch_optional(&mut **tx)
            .await?;
            prev.map_or(
                CursorPos {
                    last_xact: "0".into(),
                    last_seq: 0,
                },
                |row| CursorPos {
                    last_xact: row.get("xact"),
                    last_seq: row.get("seq"),
                },
            )
        }
        None => {
            let newest = sqlx::query(
                r#"
                SELECT xact::text AS xact, seq
                FROM fvoci.events
                WHERE created_at >= now() - make_interval(days => $1)
                ORDER BY xact DESC, seq DESC
                LIMIT 1
                "#,
            )
            .bind(RESET_SCAN_WINDOW_DAYS)
            .fetch_optional(&mut **tx)
            .await?;
            match newest {
                Some(row) => CursorPos {
                    last_xact: row.get("xact"),
                    last_seq: row.get("seq"),
                },
                None => {
                    noop_reason = Some(format!(
                        "no events in the {RESET_SCAN_WINDOW_DAYS}-day window; cursor unchanged"
                    ));
                    cursor.pos.clone()
                }
            }
        }
    };

    let direction = compare(&target, &cursor.pos)?;
    let redelivered = if direction == "backward" {
        sqlx::query_scalar(
            r#"
            SELECT count(*)
            FROM fvoci.events AS e
            WHERE (e.xact, e.seq) > ($2::xid8, $3)
              AND (e.xact, e.seq) <= ($4::xid8, $5)
              AND NOT EXISTS (
                  SELECT 1 FROM fvoci.processed_events AS p
                  WHERE p.consumer = $1 AND p.event_id = e.id
              )
            "#,
        )
        .bind(consumer)
        .bind(&target.last_xact)
        .bind(target.last_seq)
        .bind(&cursor.pos.last_xact)
        .bind(cursor.pos.last_seq)
        .fetch_one(&mut **tx)
        .await?
    } else {
        0
    };

    Ok(ConsumerReset {
        consumer: consumer.to_string(),
        lease_active: cursor.lease_active,
        before: cursor.pos.clone(),
        target,
        direction,
        noop_reason,
        redelivered,
        skip,
    })
}

fn compare(target: &CursorPos, current: &CursorPos) -> Result<&'static str, OutboxResetError> {
    let parse = |xact: &str| {
        xact.parse::<u64>()
            .map_err(|_| rejected(format!("invalid xid8 {xact:?}")))
    };
    let target_key = (parse(&target.last_xact)?, target.last_seq);
    let current_key = (parse(&current.last_xact)?, current.last_seq);
    Ok(match target_key.cmp(&current_key) {
        std::cmp::Ordering::Greater => "forward",
        std::cmp::Ordering::Less => "backward",
        std::cmp::Ordering::Equal => "unchanged",
    })
}

/// Source collectResetSkips: unmarked events after the cursor and older than
/// the window. A forward move passes them without delivery.
async fn collect_skip(
    tx: &mut Transaction<'_, Postgres>,
    consumer: &str,
    cursor: &CursorPos,
) -> Result<Option<ResetSkip>, OutboxResetError> {
    const WHERE: &str = r#"
        (e.xact, e.seq) > ($2::xid8, $3)
        AND e.created_at < now() - make_interval(days => $4)
        AND NOT EXISTS (
            SELECT 1 FROM fvoci.processed_events AS p
            WHERE p.consumer = $1 AND p.event_id = e.id
        )
    "#;
    let agg = sqlx::query(&format!(
        "SELECT count(*) AS n, min(e.created_at) AS oldest FROM fvoci.events AS e WHERE {WHERE}"
    ))
    .bind(consumer)
    .bind(&cursor.last_xact)
    .bind(cursor.last_seq)
    .bind(RESET_SCAN_WINDOW_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    let count: i64 = agg.get("n");
    if count == 0 {
        return Ok(None);
    }
    let ends = sqlx::query(&format!(
        r#"
        SELECT
            (SELECT e.xact::text FROM fvoci.events AS e WHERE {WHERE} ORDER BY e.xact, e.seq LIMIT 1) AS min_xact,
            (SELECT e.seq FROM fvoci.events AS e WHERE {WHERE} ORDER BY e.xact, e.seq LIMIT 1) AS min_seq,
            (SELECT e.xact::text FROM fvoci.events AS e WHERE {WHERE} ORDER BY e.xact DESC, e.seq DESC LIMIT 1) AS max_xact,
            (SELECT e.seq FROM fvoci.events AS e WHERE {WHERE} ORDER BY e.xact DESC, e.seq DESC LIMIT 1) AS max_seq
        "#
    ))
    .bind(consumer)
    .bind(&cursor.last_xact)
    .bind(cursor.last_seq)
    .bind(RESET_SCAN_WINDOW_DAYS)
    .fetch_one(&mut **tx)
    .await?;
    let sample = sqlx::query(&format!(
        "SELECT e.id::text AS event_id, e.verb FROM fvoci.events AS e WHERE {WHERE} ORDER BY e.xact, e.seq LIMIT $5"
    ))
    .bind(consumer)
    .bind(&cursor.last_xact)
    .bind(cursor.last_seq)
    .bind(RESET_SCAN_WINDOW_DAYS)
    .bind(SKIP_SAMPLE_LIMIT)
    .fetch_all(&mut **tx)
    .await?
    .iter()
    .map(|row| SkippedEvent {
        event_id: row.get("event_id"),
        verb: row.get("verb"),
    })
    .collect();
    Ok(Some(ResetSkip {
        skipped_count: count,
        min_xact: ends.get("min_xact"),
        min_seq: ends.get("min_seq"),
        max_xact: ends.get("max_xact"),
        max_seq: ends.get("max_seq"),
        oldest_created_at: agg.get("oldest"),
        sample,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn diagnose_is_the_default() {
        let opts = parse_outbox_reset_args(&[]).unwrap();
        assert!(!opts.apply);
        assert!(opts.consumers.is_empty());
    }

    #[test]
    fn apply_requires_reason_and_values_must_be_non_empty() {
        for bad in [
            &["--apply"][..],
            &["--apply", "--reason"],
            &["--apply", "--reason="],
            &["--apply", "--reason", "  "],
            &["--apply", "--reason", "x", "--override-reason"],
            &["--apply", "--reason", "x", "--override-reason="],
            &["--reason", "x"],
            &["--override-reason=x"],
            &["--consumer"],
            &["--consumer", "Bad Name"],
            &["--apply=yes", "--reason", "x"],
            &["--force"],
        ] {
            assert!(parse_outbox_reset_args(&args(bad)).is_err(), "{bad:?}");
        }
        let opts = parse_outbox_reset_args(&args(&[
            "--apply",
            "--reason=ops ticket 1",
            "--override-reason",
            "ack skip",
            "--consumer",
            "github",
            "--consumer=github",
            "--consumer=mail",
        ]))
        .unwrap();
        assert!(opts.apply);
        assert_eq!(opts.reason.as_deref(), Some("ops ticket 1"));
        assert_eq!(opts.override_reason.as_deref(), Some("ack skip"));
        assert_eq!(opts.consumers, vec!["github", "mail"]);
    }

    #[test]
    fn compare_orders_by_xact_then_seq_numerically() {
        let pos = |x: &str, s| CursorPos {
            last_xact: x.into(),
            last_seq: s,
        };
        assert_eq!(compare(&pos("10", 1), &pos("9", 5)).unwrap(), "forward");
        assert_eq!(compare(&pos("10", 1), &pos("10", 2)).unwrap(), "backward");
        assert_eq!(compare(&pos("10", 2), &pos("10", 2)).unwrap(), "unchanged");
    }
}
