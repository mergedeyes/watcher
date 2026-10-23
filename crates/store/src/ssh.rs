//! SSH secrets and host keys. Secrets arrive sealed: this module never
//! sees them in plain text.

use crate::auth::conflict;
use crate::{Result, Store, from_epoch};
use serde::Serialize;
use std::time::SystemTime;

/// A stored secret, without its sealed value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SecretInfo {
    pub name: String,
    /// `key` or `password`.
    pub kind: String,
    pub public_key: Option<String>,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub created: SystemTime,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub updated: SystemTime,
}

/// A pinned host key, and a different one the host presented since.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HostKey {
    pub address: String,
    pub port: u16,
    pub key: String,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub pinned: SystemTime,
    pub offered: Option<String>,
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub offered_at: Option<SystemTime>,
}

type HostKeyRow = (String, i32, String, f64, Option<String>, Option<f64>);

const HOST_KEY_COLUMNS: &str = "address, port, key, extract(epoch FROM pinned)::float8,
    offered, extract(epoch FROM offered_at)::float8";

fn host_key((address, port, key, pinned, offered, offered_at): HostKeyRow) -> HostKey {
    HostKey {
        address,
        port: u16::try_from(port).unwrap_or_default(),
        key,
        pinned: from_epoch(pinned),
        offered,
        offered_at: offered_at.map(from_epoch),
    }
}

impl Store {
    pub async fn secrets(&self) -> Result<Vec<SecretInfo>> {
        let rows: Vec<(String, String, Option<String>, f64, f64)> = sqlx::query_as(
            "SELECT name, kind, public_key, extract(epoch FROM created)::float8,
                    extract(epoch FROM updated)::float8
             FROM secrets ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(name, kind, public_key, created, updated)| SecretInfo {
                name,
                kind,
                public_key,
                created: from_epoch(created),
                updated: from_epoch(updated),
            })
            .collect())
    }

    /// The kind and sealed value of secret `name`.
    pub async fn secret(&self, name: &str) -> Result<Option<(String, Vec<u8>)>> {
        Ok(
            sqlx::query_as("SELECT kind, sealed FROM secrets WHERE name = $1")
                .bind(name)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Stores a secret; with `replace`, over one of the same name.
    pub async fn put_secret(
        &self,
        name: &str,
        kind: &str,
        public_key: Option<&str>,
        sealed: &[u8],
        replace: bool,
    ) -> Result<()> {
        let sql = match replace {
            true => {
                "INSERT INTO secrets (name, kind, public_key, sealed) VALUES ($1, $2, $3, $4)
                 ON CONFLICT (name) DO UPDATE SET kind = $2, public_key = $3, sealed = $4,
                     updated = now()"
            }
            false => "INSERT INTO secrets (name, kind, public_key, sealed) VALUES ($1, $2, $3, $4)",
        };
        sqlx::query(sql)
            .bind(name)
            .bind(kind)
            .bind(public_key)
            .bind(sealed)
            .execute(&self.pool)
            .await
            .map_err(|e| conflict(e, || format!("secret {name:?} already exists")))?;
        Ok(())
    }

    /// `false` if there is no such secret.
    pub async fn delete_secret(&self, name: &str) -> Result<bool> {
        let done = sqlx::query("DELETE FROM secrets WHERE name = $1")
            .bind(name)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }

    pub async fn host_keys(&self) -> Result<Vec<HostKey>> {
        let sql = format!("SELECT {HOST_KEY_COLUMNS} FROM ssh_host_keys ORDER BY address, port");
        let rows: Vec<HostKeyRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(host_key).collect())
    }

    /// Checks the key a host presents. The first key is pinned; a different
    /// one is recorded as offered and `false` is returned.
    pub async fn check_host_key(&self, address: &str, port: u16, key: &str) -> Result<bool> {
        let pinned: String = sqlx::query_scalar(
            "INSERT INTO ssh_host_keys (address, port, key) VALUES ($1, $2, $3)
             ON CONFLICT (address, port) DO UPDATE SET address = excluded.address
             RETURNING key",
        )
        .bind(address)
        .bind(i32::from(port))
        .bind(key)
        .fetch_one(&self.pool)
        .await?;
        if pinned == key {
            return Ok(true);
        }
        sqlx::query(
            "UPDATE ssh_host_keys SET offered = $3, offered_at = now()
             WHERE address = $1 AND port = $2 AND offered IS DISTINCT FROM $3",
        )
        .bind(address)
        .bind(i32::from(port))
        .bind(key)
        .execute(&self.pool)
        .await?;
        Ok(false)
    }

    /// Pins the offered key. `None` if no key was offered.
    pub async fn accept_host_key(&self, address: &str, port: u16) -> Result<Option<HostKey>> {
        let sql = format!(
            "UPDATE ssh_host_keys SET key = offered, pinned = now(), offered = NULL,
                 offered_at = NULL
             WHERE address = $1 AND port = $2 AND offered IS NOT NULL
             RETURNING {HOST_KEY_COLUMNS}"
        );
        let row: Option<HostKeyRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(address)
            .bind(i32::from(port))
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(host_key))
    }

    /// Forgets the host's key: the next one is pinned. `false` if none was.
    pub async fn forget_host_key(&self, address: &str, port: u16) -> Result<bool> {
        let done = sqlx::query("DELETE FROM ssh_host_keys WHERE address = $1 AND port = $2")
            .bind(address)
            .bind(i32::from(port))
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}
