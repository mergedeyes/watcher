//! Read-only login roles for Grafana, BI tools and ad-hoc SQL.

use crate::{Result, Store};
use sqlx::AssertSqlSafe;

/// What a reader may see: monitoring data, not accounts or sessions.
pub const READABLE: &[&str] = &[
    "tenants",
    "nodes",
    "hosts",
    "services",
    "check_changes",
    "series",
    "samples",
    "service_state",
    "state_events",
    "v_metrics",
    "v_service_state",
];

impl Store {
    /// Creates the login role `name` (or sets its password) with SELECT on
    /// [`READABLE`] only. Run it again after upgrades to cover new tables.
    /// Needs a database user with CREATEROLE.
    pub async fn grant_reader(&self, name: &str, password: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_roles WHERE rolname = $1)")
                .bind(name)
                .fetch_one(&mut *tx)
                .await?;
        let verb = if exists { "ALTER" } else { "CREATE" };
        // Identifiers and literals are quoted by PostgreSQL itself.
        let statements: Vec<String> = sqlx::query_scalar(
            "SELECT unnest(ARRAY[
                 format('%s ROLE %I LOGIN PASSWORD %L', $3::text, $1::text, $2::text),
                 format('GRANT CONNECT ON DATABASE %I TO %I', current_database(), $1::text),
                 format('GRANT USAGE ON SCHEMA public TO %I', $1::text),
                 format('GRANT SELECT ON %s TO %I',
                        (SELECT string_agg(quote_ident(t), ', ') FROM unnest($4::text[]) AS t),
                        $1::text)
             ])",
        )
        .bind(name)
        .bind(password)
        .bind(verb)
        .bind(READABLE)
        .fetch_all(&mut *tx)
        .await?;
        for statement in statements {
            sqlx::query(AssertSqlSafe(statement))
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}
