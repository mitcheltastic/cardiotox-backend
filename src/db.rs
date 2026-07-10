use anyhow::{Context, Result};
use sqlx::{postgres::PgPoolOptions, PgPool};
use tracing::info;

/// Connects to the Neon Postgres database and applies migrations.
/// 
/// NEON constraints:
/// - min_connections(0) because Neon free tier auto-suspends after ~5 min idle and drops connections; 
///   don't cling to dead ones.
/// - Assume the DATABASE_URL is the DIRECT (non-pooler) connection ending in ?sslmode=require.
pub async fn connect_and_migrate(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .min_connections(0)
        .acquire_timeout(std::time::Duration::from_secs(15))
        .idle_timeout(Some(std::time::Duration::from_secs(5 * 60)))
        .max_lifetime(Some(std::time::Duration::from_secs(30 * 60)))
        .test_before_acquire(true)
        .connect(database_url)
        .await
        .context("Failed to connect to the database")?;

    info!("database connected, running migrations...");
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("Failed to run database migrations")?;
    
    info!("database connected, migrations applied");

    Ok(pool)
}
