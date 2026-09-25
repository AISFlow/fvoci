use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use sqlx::{PgPool, Row};

pub const RESET_SCAN_WINDOW_DAYS: i32 = 29;
const PG_MIN_VERSION_NUM: i32 = 160_000;

static RECOVERY_UTC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,6})?Z$").expect("recovery utc regex")
});

#[derive(Debug, Clone)]
pub struct RecoverOutboxOptions {
    pub since: String,
    pub snapshot_at: String,
    pub apply: bool,
    pub reason: Option<String>,
    pub acknowledge_external_replay: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverOutboxReport {
    pub applied: bool,
    pub since: String,
    pub snapshot_at: String,
    pub eligible: i64,
    pub excluded: i64,
    pub consumers_rebased: i64,
    pub stored_attachments: i64,
    pub imports_requiring_resubmission: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_hash: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum RecoverOutboxError {
    #[error("{0}")]
    Rejected(String),
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
}

pub async fn recover_outbox(
    url: &str,
    opts: RecoverOutboxOptions,
) -> Result<RecoverOutboxReport, RecoverOutboxError> {
    let since = parse_recovery_utc("since", &opts.since)?;
    let snapshot_at = parse_recovery_utc("snapshot-at", &opts.snapshot_at)?;
    if opts.apply
        && (opts
            .reason
            .as_deref()
            .map(str::trim)
            .unwrap_or("")
            .is_empty()
            || !opts.acknowledge_external_replay)
    {
        return Err(RecoverOutboxError::Rejected(
            "apply requires a non-empty reason and acknowledgment of at-least-once external replay"
                .into(),
        ));
    }

    let pool = PgPoolOptions::new().max_connections(1).connect(url).await?;
    let result = recover_outbox_on(&pool, &opts, since, snapshot_at).await;
    pool.close().await;
    result
}

async fn recover_outbox_on(
    pool: &PgPool,
    opts: &RecoverOutboxOptions,
    since: DateTime<Utc>,
    snapshot_at: DateTime<Utc>,
) -> Result<RecoverOutboxReport, RecoverOutboxError> {
    let mut tx = pool.begin().await?;
    let clock = sqlx::query(
        r#"
        SELECT
            current_setting('server_version_num') AS version,
            pg_current_xact_id()::text AS xact,
            current_setting('is_superuser') = 'on' AS superuser,
            $1::timestamptz <= $2::timestamptz
                AND $2::timestamptz <= now()
                AND $1::timestamptz >= $2::timestamptz
                    - (($3::text || ' days')::interval) AS valid
        "#,
    )
    .bind(since)
    .bind(snapshot_at)
    .bind(RESET_SCAN_WINDOW_DAYS)
    .fetch_one(&mut *tx)
    .await?;

    let version: String = clock.get("version");
    let version_num: i32 = version.parse().map_err(|_| {
        RecoverOutboxError::Rejected(format!("invalid server_version_num {version}"))
    })?;
    if version_num < PG_MIN_VERSION_NUM {
        return Err(RecoverOutboxError::Rejected(format!(
            "refusing to start: Postgres 16+ required, got {version_num}"
        )));
    }
    let superuser: bool = clock.get("superuser");
    if !superuser {
        return Err(RecoverOutboxError::Rejected(
            "recovery requires the offline postgres operator role".into(),
        ));
    }
    let valid: bool = clock.get("valid");
    if !valid {
        return Err(RecoverOutboxError::Rejected(
            "unknown or future recovery boundary; since must be within 29 days before the snapshot"
                .into(),
        ));
    }

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
        return Err(RecoverOutboxError::Rejected(
            "stop all application writers, relay, consumers and other database sessions before recovery"
                .into(),
        ));
    }

    sqlx::query(
        r#"
        LOCK TABLE fvoci.events, fvoci.outbox_consumers, fvoci.processed_events, fvoci.outbox_failures
        IN ACCESS EXCLUSIVE MODE NOWAIT
        "#,
    )
    .execute(&mut *tx)
    .await
    .map_err(|err| RecoverOutboxError::Rejected(format!("could not lock outbox tables: {err}")))?;

    let active_lease: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM fvoci.outbox_consumers
            WHERE lease_until IS NOT NULL AND lease_until >= now()
        )
        "#,
    )
    .fetch_one(&mut *tx)
    .await?;
    if active_lease {
        return Err(RecoverOutboxError::Rejected(
            "missing cursor or active relay lease; stop the relay and wait for lease expiry".into(),
        ));
    }

    let newer: bool = sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1 FROM fvoci.events WHERE created_at > $1::timestamptz
        )
        "#,
    )
    .bind(snapshot_at)
    .fetch_one(&mut *tx)
    .await?;
    if newer {
        return Err(RecoverOutboxError::Rejected(
            "snapshot boundary excludes existing events; keep writers stopped and supply the actual snapshot time"
                .into(),
        ));
    }

    let counts = sqlx::query(
        r#"
        SELECT
            count(*) FILTER (WHERE created_at >= $1::timestamptz) AS eligible,
            count(*) FILTER (WHERE created_at < $1::timestamptz) AS excluded
        FROM fvoci.events
        "#,
    )
    .bind(since)
    .fetch_one(&mut *tx)
    .await?;
    let eligible: i64 = counts.get("eligible");
    let excluded: i64 = counts.get("excluded");

    let stored_attachments = if relation_exists(&mut tx, "attachments").await? {
        sqlx::query_scalar("SELECT count(*) FROM fvoci.attachments WHERE status = 'stored'")
            .fetch_one(&mut *tx)
            .await?
    } else {
        0
    };

    let imports_requiring_resubmission = if relation_exists(&mut tx, "import_jobs").await? {
        sqlx::query_scalar(
            "SELECT count(*) FROM fvoci.import_jobs WHERE status IN ('pending','running')",
        )
        .fetch_one(&mut *tx)
        .await?
    } else {
        0
    };

    let mut consumers_rebased = 0_i64;
    let recovery_xid: String = clock.get("xact");
    if opts.apply {
        sqlx::query(
            r#"
            UPDATE fvoci.events
            SET xact = CASE
                WHEN created_at >= $1::timestamptz THEN $2::xid8
                ELSE '0'::xid8
            END
            "#,
        )
        .bind(since)
        .bind(&recovery_xid)
        .execute(&mut *tx)
        .await?;

        let updated = sqlx::query(
            r#"
            UPDATE fvoci.outbox_consumers
            SET
                last_xact = $1::xid8,
                last_seq = 0,
                lease_owner = NULL,
                lease_until = NULL,
                updated_at = now()
            "#,
        )
        .bind(&recovery_xid)
        .execute(&mut *tx)
        .await?;
        consumers_rebased = updated.rows_affected() as i64;

        sqlx::query("DELETE FROM fvoci.outbox_failures")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }

    Ok(RecoverOutboxReport {
        applied: opts.apply,
        since: opts.since.clone(),
        snapshot_at: opts.snapshot_at.clone(),
        eligible,
        excluded,
        consumers_rebased,
        stored_attachments,
        imports_requiring_resubmission,
        reason_hash: opts.apply.then(|| {
            hex::encode(Sha256::digest(
                opts.reason.as_deref().unwrap_or("").as_bytes(),
            ))
        }),
    })
}

async fn relation_exists(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    table: &str,
) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        r#"
        SELECT EXISTS (
            SELECT 1
            FROM information_schema.tables
            WHERE table_schema = 'fvoci' AND table_name = $1
        )
        "#,
    )
    .bind(table)
    .fetch_one(&mut **tx)
    .await
}

fn parse_recovery_utc(label: &str, value: &str) -> Result<DateTime<Utc>, RecoverOutboxError> {
    if !RECOVERY_UTC.is_match(value) || value.parse::<DateTime<Utc>>().is_err() {
        return Err(RecoverOutboxError::Rejected(format!(
            "recovery requires explicit UTC {label} and snapshot-at boundaries"
        )));
    }
    value.parse::<DateTime<Utc>>().map_err(|_| {
        RecoverOutboxError::Rejected(
            "recovery requires explicit UTC since and snapshot-at boundaries".into(),
        )
    })
}

pub fn parse_recover_outbox_args(
    args: &[String],
) -> Result<RecoverOutboxOptions, RecoverOutboxError> {
    let mut since = String::new();
    let mut snapshot_at = String::new();
    let mut apply = false;
    let mut reason = None;
    let mut acknowledge_external_replay = false;
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
                    return Err(RecoverOutboxError::Rejected(
                        "--apply does not take a value".into(),
                    ));
                }
                apply = true;
            }
            "--ack-external-replay" => {
                if inline.is_some() {
                    return Err(RecoverOutboxError::Rejected(
                        "--ack-external-replay does not take a value".into(),
                    ));
                }
                acknowledge_external_replay = true;
            }
            "--since" | "--snapshot-at" | "--reason" => {
                let value = match inline {
                    Some(value) => value,
                    None => {
                        i += 1;
                        args.get(i)
                            .filter(|next| !next.starts_with("--"))
                            .cloned()
                            .ok_or_else(|| {
                                RecoverOutboxError::Rejected(format!("{name} requires a value"))
                            })?
                    }
                };
                if value.is_empty() {
                    return Err(RecoverOutboxError::Rejected(format!(
                        "{name} requires a value"
                    )));
                }
                match name {
                    "--since" => since = value,
                    "--snapshot-at" => snapshot_at = value,
                    _ => reason = Some(value),
                }
            }
            _ => {
                return Err(RecoverOutboxError::Rejected(format!(
                    "unknown option: {name}"
                )))
            }
        }
        i += 1;
    }
    if since.is_empty() || snapshot_at.is_empty() {
        return Err(RecoverOutboxError::Rejected(
            "--since and --snapshot-at are required".into(),
        ));
    }
    if apply
        && (reason.as_deref().map(str::trim).unwrap_or("").is_empty()
            || !acknowledge_external_replay)
    {
        return Err(RecoverOutboxError::Rejected(
            "--apply requires --reason and --ack-external-replay".into(),
        ));
    }
    Ok(RecoverOutboxOptions {
        since,
        snapshot_at,
        apply,
        reason,
        acknowledge_external_replay,
    })
}
