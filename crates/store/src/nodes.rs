//! Nodes: machines at other sites that connect to the hub. A node joins
//! with a one-time token and is known by its certificate from then on.

use crate::{Error, Result, Store, epoch, from_epoch};
use serde::Serialize;
use std::time::SystemTime;

/// A node as the database knows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct NodeInfo {
    pub name: String,
    /// When the node joined; `None` before it has.
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub joined: Option<SystemTime>,
    /// When the unused join token expires.
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub token_expires: Option<SystemTime>,
    /// The end of the last connection, or a recent time while connected.
    #[serde(serialize_with = "crate::query::rfc3339_opt")]
    pub last_seen: Option<SystemTime>,
    /// What the node reported when it last connected.
    pub version: Option<String>,
    pub hostname: Option<String>,
    /// The address the last connection came from.
    pub address: Option<String>,
    /// Fingerprint of the node's certificate.
    #[serde(skip)]
    pub cert_fp: Option<String>,
}

type NodeRow = (
    String,
    Option<f64>,
    Option<f64>,
    Option<f64>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

fn node((name, joined, expires, seen, version, hostname, address, cert_fp): NodeRow) -> NodeInfo {
    NodeInfo {
        name,
        joined: joined.map(from_epoch),
        token_expires: expires.map(from_epoch),
        last_seen: seen.map(from_epoch),
        version,
        hostname,
        address,
        cert_fp,
    }
}

impl Store {
    /// Every node row, also those that only exist because a host names
    /// them.
    pub async fn nodes(&self) -> Result<Vec<NodeInfo>> {
        let rows: Vec<NodeRow> = sqlx::query_as(
            "SELECT name, extract(epoch FROM joined)::float8,
                    extract(epoch FROM token_expires)::float8,
                    extract(epoch FROM last_seen)::float8,
                    version, hostname, address, cert_fp
             FROM nodes ORDER BY name",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(node).collect())
    }

    /// Stores a join token for node `name`, creating the node if needed.
    /// A node that has joined already gets one only with `rejoin`; its
    /// certificate stays valid until the new join.
    pub async fn set_node_token(
        &self,
        name: &str,
        hash: &[u8],
        expires: SystemTime,
        rejoin: bool,
    ) -> Result<()> {
        let done = sqlx::query(
            "INSERT INTO nodes (tenant_id, name, token_hash, token_expires)
             VALUES ((SELECT id FROM tenants WHERE name = 'default'), $1, $2, to_timestamp($3))
             ON CONFLICT (name) DO UPDATE
             SET token_hash = EXCLUDED.token_hash, token_expires = EXCLUDED.token_expires
             WHERE nodes.cert_fp IS NULL OR $4",
        )
        .bind(name)
        .bind(hash)
        .bind(epoch(expires))
        .bind(rejoin)
        .execute(&self.pool)
        .await?;
        if done.rows_affected() == 0 {
            return Err(Error::Conflict(format!(
                "node {name} has joined already; get a new join token to join it again"
            )));
        }
        Ok(())
    }

    /// The node an unexpired join token is for.
    pub async fn node_for_token(&self, hash: &[u8]) -> Result<Option<String>> {
        Ok(sqlx::query_scalar(
            "SELECT name FROM nodes WHERE token_hash = $1 AND token_expires > now()",
        )
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?)
    }

    /// Uses up the join token of node `name`: from now on the node is known
    /// by the certificate with fingerprint `cert_fp`, and an older
    /// certificate no longer works. `false` if the token is no longer valid.
    pub async fn join_node(&self, name: &str, hash: &[u8], cert_fp: &str) -> Result<bool> {
        let done = sqlx::query(
            "UPDATE nodes
             SET cert_fp = $3, joined = now(), token_hash = NULL, token_expires = NULL
             WHERE name = $1 AND token_hash = $2 AND token_expires > now()",
        )
        .bind(name)
        .bind(hash)
        .bind(cert_fp)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected() == 1)
    }

    /// The node with the certificate of fingerprint `cert_fp`.
    pub async fn node_by_cert(&self, cert_fp: &str) -> Result<Option<String>> {
        Ok(
            sqlx::query_scalar("SELECT name FROM nodes WHERE cert_fp = $1")
                .bind(cert_fp)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Records a new connection of node `name`.
    pub async fn node_connected(
        &self,
        name: &str,
        version: &str,
        hostname: &str,
        address: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE nodes SET last_seen = now(), version = $2, hostname = $3, address = $4
             WHERE name = $1",
        )
        .bind(name)
        .bind(version)
        .bind(hostname)
        .bind(address)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Sets `last_seen` of the nodes `names` to now.
    pub async fn nodes_seen(&self, names: &[&str]) -> Result<()> {
        if names.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE nodes SET last_seen = now() WHERE name = ANY($1)")
            .bind(names)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Removes node `name`: its certificate and join token stop working.
    /// The row stays while hosts refer to it (also removed hosts, for
    /// their history), without what the node had reported. `false` if
    /// there is no such node.
    pub async fn remove_node(&self, name: &str) -> Result<bool> {
        let mut tx = self.pool.begin().await?;
        let deleted = sqlx::query(
            "DELETE FROM nodes n WHERE name = $1
             AND NOT EXISTS (SELECT 1 FROM hosts h WHERE h.node_id = n.id)",
        )
        .bind(name)
        .execute(&mut *tx)
        .await?;
        let reset = sqlx::query(
            "UPDATE nodes
             SET cert_fp = NULL, token_hash = NULL, token_expires = NULL, joined = NULL,
                 last_seen = NULL, version = NULL, hostname = NULL, address = NULL
             WHERE name = $1",
        )
        .bind(name)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(deleted.rows_affected() + reset.rows_affected() > 0)
    }
}
