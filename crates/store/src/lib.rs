//! Storage on PostgreSQL, with TimescaleDB when available.
//!
//! The schema is plain PostgreSQL (see `migrations/`). At startup the store
//! turns `samples` into a hypertable with compression if the `timescaledb`
//! extension can be used, and otherwise keeps working on the plain table.

mod auth;
mod ingest;
mod query;
mod reader;
mod spool;
mod sync;
mod write;

pub use auth::{ApiToken, TokenRef, User};
pub use ingest::{Record, ingest};
pub use query::{EventFilter, Points, Series, StateEvent};
pub use reader::READABLE;
pub use spool::Spool;
pub use sync::{HostSpec, ServiceSpec, StoredState, SyncMode};
pub use write::SeriesCache;

use sqlx::PgPool;
use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
use std::fmt;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("migration failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// A unique constraint was violated, e.g. the user already exists.
    #[error("{0}")]
    Conflict(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// What the database offers for time series.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Postgres,
    Timescale {
        /// Compression needs the Timescale License build; the Apache-only
        /// build has hypertables but no compression.
        compression: bool,
    },
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Backend::Postgres => f.write_str("PostgreSQL (no TimescaleDB)"),
            Backend::Timescale { compression: true } => f.write_str("TimescaleDB"),
            Backend::Timescale { compression: false } => {
                f.write_str("TimescaleDB (Apache build, no compression)")
            }
        }
    }
}

pub struct Store {
    pool: PgPool,
    backend: Backend,
}

impl Store {
    /// Connects, applies migrations and sets up TimescaleDB if possible.
    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with(url, None).await
    }

    /// Like [`Store::connect`], with a password kept out of the URL.
    pub async fn connect_with(url: &str, password: Option<&str>) -> Result<Self> {
        let mut options: PgConnectOptions = url.parse()?;
        if let Some(password) = password {
            options = options.password(password);
        }
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .acquire_timeout(Duration::from_secs(10))
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        let backend = setup_timescale(&pool).await?;
        Ok(Store { pool, backend })
    }

    pub fn backend(&self) -> Backend {
        self.backend
    }

    pub fn pool(&self) -> &PgPool {
        &self.pool
    }
}

/// Enables TimescaleDB features on `samples` where available. Safe to run on
/// every start.
async fn setup_timescale(pool: &PgPool) -> Result<Backend> {
    let available: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_available_extensions WHERE name = 'timescaledb')",
    )
    .fetch_one(pool)
    .await?;
    if !available {
        return Ok(Backend::Postgres);
    }
    // Fails without superuser rights or without the library preloaded;
    // the plain schema keeps working then.
    if let Err(err) = sqlx::query("CREATE EXTENSION IF NOT EXISTS timescaledb")
        .execute(pool)
        .await
    {
        tracing::warn!(
            "TimescaleDB is installed but cannot be enabled, using plain PostgreSQL: {err}"
        );
        return Ok(Backend::Postgres);
    }

    sqlx::query(
        "SELECT create_hypertable('samples', by_range('ts', INTERVAL '1 day'),
                                  if_not_exists => TRUE, migrate_data => TRUE)",
    )
    .execute(pool)
    .await?;

    let license: Option<String> =
        sqlx::query_scalar("SELECT current_setting('timescaledb.license', true)")
            .fetch_one(pool)
            .await?;
    if license.as_deref() != Some("timescale") {
        return Ok(Backend::Timescale { compression: false });
    }

    let enabled: bool = sqlx::query_scalar(
        "SELECT compression_enabled FROM timescaledb_information.hypertables
         WHERE hypertable_name = 'samples'",
    )
    .fetch_one(pool)
    .await?;
    if !enabled {
        sqlx::query(
            "ALTER TABLE samples SET (timescaledb.compress,
                                      timescaledb.compress_segmentby = 'series_id',
                                      timescaledb.compress_orderby = 'ts DESC')",
        )
        .execute(pool)
        .await?;
    }
    sqlx::query(
        "SELECT add_compression_policy('samples', INTERVAL '2 days', if_not_exists => TRUE)",
    )
    .execute(pool)
    .await?;
    Ok(Backend::Timescale { compression: true })
}

/// Seconds since the epoch, for binding to `to_timestamp()`.
fn epoch(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

fn from_epoch(secs: f64) -> SystemTime {
    if secs >= 0.0 {
        UNIX_EPOCH + Duration::from_secs_f64(secs)
    } else {
        UNIX_EPOCH - Duration::from_secs_f64(-secs)
    }
}
