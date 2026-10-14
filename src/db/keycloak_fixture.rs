//! Fixed read-only OIDC witnesses for the allocated real-Keycloak check.
//! No caller-supplied SQL, mutation, auth bypass or product HTTP surface.
use super::backend::{Backend, DbTransaction, FamilyTx};
use super::codec::Cell;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[serde(tag = "operation", rename_all = "lowercase", deny_unknown_fields)]
pub enum Observation {
    Users {},
    States {},
    Links {},
    User { email: String },
    Sessions { email: String },
    Invitation { email: String },
    Membership { email: String, slug: String },
}

fn refused() -> sqlx::Error {
    sqlx::Error::Protocol("Keycloak observation contract refused".into())
}

impl Observation {
    pub fn validate(&self) -> Result<(), sqlx::Error> {
        let email = match self {
            Self::User { email }
            | Self::Sessions { email }
            | Self::Invitation { email }
            | Self::Membership { email, .. } => Some(email),
            _ => None,
        };
        if email.is_some_and(|e| {
            e.len() > 254
                || !e.is_ascii()
                || !e.ends_with("@example.com")
                || e.contains(char::is_control)
        }) || matches!(self, Self::Membership { slug, .. } if slug != "kc-e2e")
        {
            return Err(refused());
        }
        Ok(())
    }
}

/// Always explicitly rolls back; a failed read and failed finish both fail.
pub async fn observe(backend: &Backend, input: &Observation) -> Result<Value, sqlx::Error> {
    input.validate()?;
    let mut tx = backend.begin_read().await?;
    let value = match &mut tx {
        DbTransaction::Postgres(pg) => {
            let (sql, email, slug) = match input {
                Observation::Users {} => ("SELECT to_json(count(*)) FROM fvoci.users", "", ""),
                Observation::States {} => ("SELECT to_json(count(*)) FROM fvoci.oidc_states", "", ""),
                Observation::Links {} => ("SELECT coalesce(json_agg(json_build_object('user',u.email,'provider',l.provider,'subject',l.provider_user_id,'issuer',l.issuer,'email',l.email) ORDER BY u.email),'[]'::json) FROM fvoci.identity_links l JOIN fvoci.users u ON u.id=l.user_id", "", ""),
                Observation::User { email } => ("SELECT to_json(id::text) FROM fvoci.users WHERE email=$1", email.as_str(), ""),
                Observation::Sessions { email } => ("SELECT to_json(count(*)) FROM fvoci.sessions s JOIN fvoci.users u ON u.id=s.user_id WHERE u.email=$1 AND s.revoked_at IS NULL AND s.expires_at>now()", email.as_str(), ""),
                Observation::Invitation { email } => ("SELECT to_json(accepted_at IS NOT NULL) FROM fvoci.invitations WHERE email=$1", email.as_str(), ""),
                Observation::Membership { email, slug } => ("SELECT to_json(m.role) FROM fvoci.memberships m JOIN fvoci.workspaces w ON w.id=m.workspace_id JOIN fvoci.users u ON u.id=m.user_id WHERE u.email=$1 AND w.slug=$2", email.as_str(), slug.as_str()),
            };
            let mut query = sqlx::query_scalar::<_, Value>(sql);
            if !email.is_empty() {
                query = query.bind(email);
            }
            if !slug.is_empty() {
                query = query.bind(slug);
            }
            query.fetch_all(&mut **pg).await.and_then(|mut rows| {
                if rows.len() > 1 {
                    Err(refused())
                } else {
                    Ok(rows.pop().unwrap_or(Value::Null))
                }
            })
        }
        DbTransaction::SqliteFamily(family) => family_observe(family, input).await,
    };
    let rollback = tx.rollback().await;
    match (value, rollback) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

async fn family_observe(tx: &mut FamilyTx, input: &Observation) -> Result<Value, sqlx::Error> {
    let (sql, args) = match input {
        Observation::Users {} => ("SELECT count(*) FROM users", vec![]),
        Observation::States {} => ("SELECT count(*) FROM oidc_states", vec![]),
        Observation::Links {} => ("SELECT u.email,l.provider,l.provider_user_id,l.issuer,l.email FROM identity_links l JOIN users u ON u.id=l.user_id ORDER BY u.email", vec![]),
        Observation::User { email } => ("SELECT id FROM users WHERE email=?1", vec![Cell::text(email)]),
        Observation::Sessions { email } => ("SELECT count(*) FROM sessions s JOIN users u ON u.id=s.user_id WHERE u.email=?1 AND s.revoked_at IS NULL AND s.expires_at>(CAST(strftime('%s','now') AS INTEGER)*1000000+CAST(substr(strftime('%f','now'),4,3) AS INTEGER)*1000)", vec![Cell::text(email)]),
        Observation::Invitation { email } => ("SELECT accepted_at IS NOT NULL FROM invitations WHERE email=?1", vec![Cell::text(email)]),
        Observation::Membership { email, slug } => ("SELECT m.role FROM memberships m JOIN workspaces w ON w.id=m.workspace_id JOIN users u ON u.id=m.user_id WHERE u.email=?1 AND w.slug=?2", vec![Cell::text(email),Cell::text(slug)]),
    };
    let rows = tx.query(sql, &args).await?;
    if matches!(input, Observation::Links {}) {
        return rows
            .iter()
            .map(|row| {
                Ok(json!({
                    "user":row.cell(0)?.string()?, "provider":row.cell(1)?.string()?,
                    "subject":row.cell(2)?.string()?, "issuer":row.cell(3)?.optional(Cell::string)?,
                    "email":row.cell(4)?.optional(Cell::string)?
                }))
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()
            .map(Value::Array);
    }
    if rows.len() > 1 {
        return Err(refused());
    }
    let Some(row) = rows.first() else {
        return Ok(Value::Null);
    };
    let cell = row.cell(0)?;
    Ok(match input {
        Observation::User { .. } => json!(cell.id()?.to_string()),
        Observation::Membership { .. } => json!(cell.string()?),
        Observation::Invitation { .. } => json!(cell.boolean()?),
        _ => json!(cell.integer()?),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_fixed_operations_and_fields_are_accepted() {
        for text in [
            r#"{"operation":"sql","query":"SELECT 1"}"#,
            r#"{"operation":"users","email":"kc-owner@example.com"}"#,
            r#"{"operation":"user"}"#,
        ] {
            assert!(serde_json::from_str::<Observation>(text).is_err());
        }
        assert!(Observation::User {
            email: "real@external.invalid".into()
        }
        .validate()
        .is_err());
        assert!(Observation::Membership {
            email: "kc-owner@example.com".into(),
            slug: "foreign".into()
        }
        .validate()
        .is_err());
    }

    #[tokio::test]
    async fn readonly_sqlite_witness_reads_absence_and_releases_failed_reads() {
        use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
        let directory =
            std::env::temp_dir().join(format!("keycloak-observer-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join("witness.sqlite");
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE users(id BLOB,email TEXT); INSERT INTO users VALUES(x'00000000000000000000000000000001','kc-owner@example.com');").execute(&writer).await.unwrap();
        writer.close().await;
        let reader = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(SqliteConnectOptions::new().filename(&path).read_only(true))
            .await
            .unwrap();
        let backend = Backend::Sqlite(reader.clone());
        assert_eq!(
            observe(&backend, &Observation::Users {}).await.unwrap(),
            json!(1)
        );
        assert_eq!(
            observe(
                &backend,
                &Observation::User {
                    email: "absent@example.com".into()
                }
            )
            .await
            .unwrap(),
            Value::Null
        );
        assert_eq!(
            observe(
                &backend,
                &Observation::User {
                    email: "kc-owner@example.com".into()
                }
            )
            .await
            .unwrap(),
            json!("00000000-0000-0000-0000-000000000001")
        );
        assert!(observe(&backend, &Observation::States {}).await.is_err());
        assert_eq!(
            observe(&backend, &Observation::Users {}).await.unwrap(),
            json!(1)
        );
        assert!(sqlx::query("DELETE FROM users")
            .execute(&reader)
            .await
            .is_err());
        backend.close().await.unwrap();
        assert_eq!(backend.connection_stats().unwrap().size, 0);
        std::fs::remove_dir_all(directory).unwrap();
    }
}
