#![cfg(feature = "db-tests")]

use fvoci_server::auth::password::{hash_password, Keyring};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let admin_url = std::env::var("DATABASE_URL")?;
    let email = std::env::var("E2E_USER_EMAIL").unwrap_or_else(|_| "member@example.com".into());
    let password = std::env::var("E2E_USER_PASSWORD").unwrap_or_else(|_| "memberpass1".into());
    let given_name = std::env::var("E2E_USER_GIVEN_NAME").unwrap_or_else(|_| "멤버".into());
    let family_name = std::env::var("E2E_USER_FAMILY_NAME").ok();
    let workspace_slug = std::env::var("E2E_WORKSPACE_SLUG").ok();
    let membership_role =
        std::env::var("E2E_MEMBERSHIP_ROLE").unwrap_or_else(|_| "member".into());
    let pepper = std::env::var("PASSWORD_PEPPER_KEYS")?;
    let active = std::env::var("PASSWORD_PEPPER_ACTIVE_KEY_ID")?;
    let keys = Keyring::parse(&pepper, &active)?;
    let hash = hash_password(&password, &keys).await?;
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&admin_url)
        .await?;
    let user_id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO fvoci.users (id, email, password_hash, given_name, family_name) VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(user_id)
    .bind(&email)
    .bind(&hash)
    .bind(&given_name)
    .bind(family_name)
    .execute(&pool)
    .await?;

    if let Some(slug) = workspace_slug {
        let workspace_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM fvoci.workspaces WHERE slug = $1",
        )
        .bind(&slug)
        .fetch_optional(&pool)
        .await?;
        if let Some(workspace_id) = workspace_id {
            sqlx::query(
                "INSERT INTO fvoci.memberships (workspace_id, user_id, role) VALUES ($1, $2, $3)",
            )
            .bind(workspace_id)
            .bind(user_id)
            .bind(&membership_role)
            .execute(&pool)
            .await?;
        }
    }

    pool.close().await;
    println!("{}", user_id);
    Ok(())
}
