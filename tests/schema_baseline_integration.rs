//! 0.6 new-install baseline: catalog extraction and remote-rendering controls.
//!
//! `postgres_catalog_dump` is the extractor half of the one-time semantic
//! comparison between a database installed by the retired development lineage
//! (001..056, applied by the fvoci-migrate built from that tree) and a database
//! installed by the fvoci-postgres-060 baseline. It connects to
//! `FVOCI_SCHEMA_CATALOG_DATABASE_URL` (the migration owner URL of a throwaway
//! database that already has the schema and, when `FVOCI_SCHEMA_CATALOG_APP_ROLE`
//! is set, the app-role grants) and writes a normalized JSON catalog to
//! `FVOCI_SCHEMA_CATALOG_OUT`. `scripts/schema-baseline/compare-catalogs.py`
//! diffs two dumps. Raw SQL equality is never the criterion: the dump carries
//! catalog facts (pg_get_*def, attributes, ACLs, seeds).
//!
//! The SQLite controls run without any database: the remote libSQL SDK renders
//! every statement through libsql-sqlite3-parser before the remote engine
//! stores it, and the runner feeds its reference engine the same rendering.
//! The archived actual SDK schema response of the retired lineage (236 rows,
//! loopback sqld, exchange 10) is the oracle for that rendering.
#![cfg(feature = "db-tests")]

use fvoci_server::db::migrate;
use serde_json::{json, Value};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scripts/schema-baseline/fixtures")
        .join(name)
}

/// The archived loopback-SDK sqlite_schema rows of legacy 001..004 must be
/// reproduced byte-for-byte by rendering the same texts with the runner's
/// SDK rendering and executing them in the pinned local engine. This proves
/// the rendering replica and the engine's text storage, and is the positive
/// control for the remote schema admission mode.
#[tokio::test]
async fn sdk_rendering_reproduces_archived_remote_sqlite_schema_rows() {
    use sqlx::Connection;
    let archived: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture(
            "legacy-sqlite-001-004-actual-sdk-schema-response.json",
        ))
        .unwrap(),
    )
    .unwrap();
    let expected: Vec<(String, String, String, String)> = archived["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            let cell = |i: usize| row[i].as_str().unwrap().to_string();
            (cell(0), cell(1), cell(2), cell(3))
        })
        .collect();
    assert_eq!(
        expected.len(),
        236,
        "archived exchange 10 holds 236 objects"
    );

    let mut reference = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    let pin: (String,) = sqlx::query_as("SELECT sqlite_version()")
        .fetch_one(&mut reference)
        .await
        .unwrap();
    assert_eq!(pin.0, fvoci_server::db::pool::SQLITE_VERSION);
    for name in [
        "legacy-sqlite-001_current_schema.sql",
        "legacy-sqlite-002_wiki_create_commands.sql",
        "legacy-sqlite-003_collab_room_fences.sql",
        "legacy-sqlite-004_maintenance_claims.sql",
    ] {
        let sql = std::fs::read_to_string(fixture(name)).unwrap();
        for statement in migrate::sdk_rendered_statements(&sql).unwrap() {
            sqlx::raw_sql(&statement)
                .execute(&mut reference)
                .await
                .unwrap_or_else(|error| panic!("{name}: {error}\n{statement}"));
        }
    }
    let actual: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type COLLATE BINARY,name COLLATE BINARY",
    )
    .fetch_all(&mut reference)
    .await
    .unwrap();
    assert_eq!(actual.len(), expected.len());
    for (index, (want, got)) in expected.iter().zip(&actual).enumerate() {
        assert_eq!(got, want, "row {index} ({} {})", want.0, want.1);
    }
    // The raw compiled text of the same steps differs from the remote rows
    // (this is the mismatch the SDK rendering exists for), so a raw reference
    // must not be accepted as a remote reference.
    let mut raw = sqlx::SqliteConnection::connect_with(
        &sqlx::sqlite::SqliteConnectOptions::new()
            .in_memory(true)
            .foreign_keys(true),
    )
    .await
    .unwrap();
    for name in [
        "legacy-sqlite-001_current_schema.sql",
        "legacy-sqlite-002_wiki_create_commands.sql",
        "legacy-sqlite-003_collab_room_fences.sql",
        "legacy-sqlite-004_maintenance_claims.sql",
    ] {
        sqlx::raw_sql(&std::fs::read_to_string(fixture(name)).unwrap())
            .execute(&mut raw)
            .await
            .unwrap();
    }
    let raw_rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT type,name,tbl_name,sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type COLLATE BINARY,name COLLATE BINARY",
    )
    .fetch_all(&mut raw)
    .await
    .unwrap();
    assert_ne!(raw_rows, expected, "raw text is not the remote rendering");
    reference.close().await.unwrap();
    raw.close().await.unwrap();
}

/// Every baseline step renders through the SDK parser and the rendered texts
/// create the same structural catalog (tables, columns, FKs, indexes) as the
/// raw texts: rendering changes spelling of stored definitions, never the
/// schema the engine builds.
#[tokio::test]
async fn baseline_sqlite_steps_render_to_the_same_structural_catalog() {
    use sqlx::Connection;
    async fn structure(sql_texts: Vec<String>) -> Value {
        let mut conn = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new()
                .in_memory(true)
                .foreign_keys(true),
        )
        .await
        .unwrap();
        for text in sql_texts {
            sqlx::raw_sql(&text).execute(&mut conn).await.unwrap();
        }
        let tables: Vec<(String,)> = sqlx::query_as(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT GLOB 'sqlite_*' ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(&mut conn)
        .await
        .unwrap();
        let mut out = serde_json::Map::new();
        for (table,) in tables {
            let columns: Vec<(i64, String, String, i64, Option<String>, i64)> =
                sqlx::query_as(&format!("PRAGMA table_info(\"{table}\")"))
                    .fetch_all(&mut conn)
                    .await
                    .unwrap();
            let fks: Vec<(i64, i64, String, String, String, String, String, String)> =
                sqlx::query_as(&format!("PRAGMA foreign_key_list(\"{table}\")"))
                    .fetch_all(&mut conn)
                    .await
                    .unwrap();
            let indexes: Vec<(i64, String, i64, String, i64)> =
                sqlx::query_as(&format!("PRAGMA index_list(\"{table}\")"))
                    .fetch_all(&mut conn)
                    .await
                    .unwrap();
            let mut index_columns = Vec::new();
            for index in &indexes {
                let cols: Vec<(i64, i64, Option<String>)> =
                    sqlx::query_as(&format!("PRAGMA index_info(\"{}\")", index.1))
                        .fetch_all(&mut conn)
                        .await
                        .unwrap();
                index_columns.push(json!({"name": index.1.to_lowercase(), "unique": index.2, "origin": index.3, "partial": index.4, "columns": cols.iter().map(|c| json!([c.0, c.1, c.2.as_deref().map(|s| s.to_lowercase())])).collect::<Vec<_>>()}));
            }
            index_columns.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
            out.insert(
                table.to_lowercase(),
                json!({
                    "columns": columns.iter().map(|c| json!([c.0, c.1.to_lowercase(), c.2.to_uppercase(), c.3, c.4, c.5])).collect::<Vec<_>>(),
                    "fks": fks.iter().map(|f| json!([f.0, f.1, f.2.to_lowercase(), f.3.to_lowercase(), f.4.to_lowercase(), f.5, f.6, f.7])).collect::<Vec<_>>(),
                    "indexes": index_columns,
                }),
            );
        }
        conn.close().await.unwrap();
        Value::Object(out)
    }
    let raw = structure(
        migrate::compiled_sqlite_steps()
            .iter()
            .map(|step| step.sql.to_string())
            .collect(),
    )
    .await;
    let mut rendered_texts = Vec::new();
    for step in migrate::compiled_sqlite_steps() {
        rendered_texts.extend(migrate::sdk_rendered_statements(step.sql).unwrap());
    }
    let rendered = structure(rendered_texts).await;
    assert_eq!(raw, rendered);
    assert_eq!(raw.as_object().unwrap().len(), 99);
}

/// Dumps the normalized PostgreSQL catalog of an already installed database.
/// Skipped (not passed) without the environment: it is a tool, not a check.
#[tokio::test]
async fn postgres_catalog_dump() {
    let Ok(url) = std::env::var("FVOCI_SCHEMA_CATALOG_DATABASE_URL") else {
        eprintln!("SKIP postgres_catalog_dump: FVOCI_SCHEMA_CATALOG_DATABASE_URL unset");
        return;
    };
    let out = std::env::var("FVOCI_SCHEMA_CATALOG_OUT")
        .expect("FVOCI_SCHEMA_CATALOG_OUT names the output file");
    let app_role = std::env::var("FVOCI_SCHEMA_CATALOG_APP_ROLE").ok();
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&url)
        .await
        .unwrap();
    let mut catalog = serde_json::Map::new();
    let version: (String,) = sqlx::query_as("SELECT current_setting('server_version')")
        .fetch_one(&pool)
        .await
        .unwrap();
    catalog.insert("server_version".into(), json!(version.0));
    let json_queries: &[(&str, &str)] = &[
        ("schemas", "SELECT coalesce(jsonb_agg(jsonb_build_object('name', nspname, 'acl', nspacl::text) ORDER BY nspname), '[]')
            FROM pg_namespace WHERE nspname IN ('fvoci', 'public')"),
        ("tables", "SELECT coalesce(jsonb_agg(jsonb_build_object(
                'name', c.relname, 'kind', c.relkind, 'rls', c.relrowsecurity, 'force_rls', c.relforcerowsecurity,
                'acl', c.relacl::text,
                'columns', (SELECT jsonb_agg(jsonb_build_object(
                        'num', a.attnum, 'name', a.attname, 'type', format_type(a.atttypid, a.atttypmod),
                        'notnull', a.attnotnull, 'default', pg_get_expr(d.adbin, d.adrelid),
                        'identity', a.attidentity, 'generated', a.attgenerated, 'collation', a.attcollation::regcollation::text,
                        'acl', a.attacl::text)
                    ORDER BY a.attnum)
                    FROM pg_attribute a LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
                    WHERE a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped),
                'constraints', (SELECT coalesce(jsonb_agg(jsonb_build_object(
                        'name', conname, 'type', contype, 'def', pg_get_constraintdef(oid, true),
                        'deferrable', condeferrable, 'deferred', condeferred, 'validated', convalidated)
                    ORDER BY conname), '[]') FROM pg_constraint WHERE conrelid = c.oid),
                'indexes', (SELECT coalesce(jsonb_agg(jsonb_build_object(
                        'name', i.indexrelid::regclass::text, 'def', pg_get_indexdef(i.indexrelid),
                        'unique', i.indisunique, 'primary', i.indisprimary, 'valid', i.indisvalid)
                    ORDER BY i.indexrelid::regclass::text), '[]') FROM pg_index i WHERE i.indrelid = c.oid),
                'triggers', (SELECT coalesce(jsonb_agg(jsonb_build_object(
                        'name', tgname, 'def', pg_get_triggerdef(oid, true), 'enabled', tgenabled,
                        'deferrable', tgdeferrable, 'initdeferred', tginitdeferred)
                    ORDER BY tgname), '[]') FROM pg_trigger WHERE tgrelid = c.oid AND NOT tgisinternal),
                'policies', (SELECT coalesce(jsonb_agg(jsonb_build_object(
                        'name', pol.polname, 'cmd', pol.polcmd, 'permissive', pol.polpermissive,
                        'roles', (SELECT array_agg(r.rolname ORDER BY r.rolname) FROM pg_roles r WHERE r.oid = ANY (pol.polroles)),
                        'public', 0 = ANY (pol.polroles),
                        'using', pg_get_expr(pol.polqual, pol.polrelid, true),
                        'check', pg_get_expr(pol.polwithcheck, pol.polrelid, true))
                    ORDER BY pol.polname), '[]') FROM pg_policy pol WHERE pol.polrelid = c.oid)
            ) ORDER BY c.relname), '[]')
            FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
            WHERE n.nspname = 'fvoci' AND c.relkind IN ('r', 'p', 'v', 'm')"),
        ("sequences", "SELECT coalesce(jsonb_agg(jsonb_build_object('name', sequencename, 'type', data_type::text,
                'start', start_value, 'min', min_value, 'max', max_value, 'increment', increment_by, 'cycle', cycle,
                'cache', cache_size) ORDER BY sequencename), '[]')
            FROM pg_sequences WHERE schemaname = 'fvoci'"),
        ("functions", "SELECT coalesce(jsonb_agg(jsonb_build_object(
                'schema', n.nspname, 'signature', p.oid::regprocedure::text, 'kind', p.prokind,
                'language', l.lanname, 'returns', pg_get_function_result(p.oid), 'args', pg_get_function_arguments(p.oid),
                'security_definer', p.prosecdef, 'volatility', p.provolatile, 'parallel', p.proparallel,
                'strict', p.proisstrict, 'leakproof', p.proleakproof, 'config', p.proconfig,
                'body', p.prosrc, 'acl', p.proacl::text)
            ORDER BY n.nspname, p.oid::regprocedure::text), '[]')
            FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace JOIN pg_language l ON l.oid = p.prolang
            WHERE n.nspname = 'fvoci' OR (n.nspname = 'public' AND (p.proname LIKE 'app\\_%' OR p.proname = 'uuidv7'))"),
        ("views", "SELECT coalesce(jsonb_agg(jsonb_build_object('name', viewname, 'def', definition) ORDER BY viewname), '[]')
            FROM pg_views WHERE schemaname = 'fvoci'"),
        ("extensions", "SELECT coalesce(jsonb_agg(extname ORDER BY extname), '[]') FROM pg_extension"),
        ("seeds", "SELECT jsonb_build_object(
                'instance_settings_meta', (SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id), '[]') FROM fvoci.instance_settings_meta t),
                'instance_config', (SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY id), '[]') FROM fvoci.instance_config t),
                'outbox_consumers', (SELECT coalesce(jsonb_agg(to_jsonb(t) ORDER BY consumer), '[]') FROM fvoci.outbox_consumers t),
                'row_counts', (SELECT jsonb_object_agg(relname, n) FROM (
                    SELECT c.relname, (xpath('/row/n/text()', query_to_xml(format('SELECT count(*) AS n FROM %I.%I', n.nspname, c.relname), false, true, '')))[1]::text::bigint AS n
                    FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
                    WHERE n.nspname = 'fvoci' AND c.relkind = 'r' AND c.relname <> 'schema_migrations') counts))"),
    ];
    for (key, sql) in json_queries {
        let value: Value = sqlx::query_scalar(sql).fetch_one(&pool).await.unwrap();
        catalog.insert((*key).into(), value);
    }
    if let Some(role) = app_role {
        let value: Value = sqlx::query_scalar(
            "SELECT jsonb_build_object(
                'role', $1::text,
                'table_privileges', (SELECT coalesce(jsonb_agg(jsonb_build_object('table', table_name, 'privilege', privilege_type) ORDER BY table_name, privilege_type), '[]')
                    FROM information_schema.role_table_grants WHERE grantee = $1 AND table_schema = 'fvoci'),
                'column_privileges', (SELECT coalesce(jsonb_agg(jsonb_build_object('table', table_name, 'column', column_name, 'privilege', privilege_type) ORDER BY table_name, column_name, privilege_type), '[]')
                    FROM information_schema.column_privileges WHERE grantee = $1 AND table_schema = 'fvoci'),
                'routine_privileges', (SELECT coalesce(jsonb_agg(jsonb_build_object('routine', p.oid::regprocedure::text, 'execute', has_function_privilege($1, p.oid, 'EXECUTE')) ORDER BY p.oid::regprocedure::text), '[]')
                    FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
                    WHERE n.nspname = 'fvoci' OR (n.nspname = 'public' AND p.proname LIKE 'app\\_%')),
                'sequence_usage', has_sequence_privilege($1, 'fvoci.events_seq', 'USAGE'),
                'schema_usage', has_schema_privilege($1, 'fvoci', 'USAGE'))",
        )
        .bind(&role)
        .fetch_one(&pool)
        .await
        .unwrap();
        catalog.insert("app_role".into(), value);
    }
    let ledger: Value = sqlx::query_scalar(
        "SELECT coalesce((SELECT jsonb_agg(to_jsonb(t) - 'applied_at' ORDER BY version) FROM fvoci.schema_migrations t), '[]')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    catalog.insert("ledger".into(), ledger);
    pool.close().await;
    std::fs::write(
        &out,
        serde_json::to_vec_pretty(&Value::Object(catalog)).unwrap(),
    )
    .unwrap();
    eprintln!("wrote {out}");
}
