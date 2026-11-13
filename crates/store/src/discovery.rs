//! Guests that discovery found, and which of them an admin ignores.

use crate::{Result, Store, from_epoch};
use serde::Serialize;
use std::time::SystemTime;

/// A VM or container as discovery last saw it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DiscoveredGuest {
    pub discovery: String,
    pub vmid: u32,
    /// `qemu` or `lxc`.
    pub kind: String,
    pub name: String,
    pub node: String,
    pub running: bool,
    pub address: Option<String>,
    pub ignored: bool,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub first_seen: SystemTime,
    #[serde(serialize_with = "crate::query::rfc3339")]
    pub last_seen: SystemTime,
}

/// What one discovery run found of a guest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundGuest {
    pub vmid: u32,
    pub kind: String,
    pub name: String,
    pub node: String,
    pub running: bool,
    pub address: Option<String>,
}

type Row = (
    String,
    i32,
    String,
    String,
    String,
    bool,
    Option<String>,
    bool,
    f64,
    f64,
);

impl Store {
    /// The guests of `discovery`, or of all, by discovery and VM ID.
    pub async fn discovered(&self, discovery: Option<&str>) -> Result<Vec<DiscoveredGuest>> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT discovery, vmid, kind, name, node, running, address, ignored,
                    extract(epoch FROM first_seen)::float8, extract(epoch FROM last_seen)::float8
             FROM discovered_guests
             WHERE $1::text IS NULL OR discovery = $1
             ORDER BY discovery, vmid",
        )
        .bind(discovery)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(
                |(discovery, vmid, kind, name, node, running, address, ignored, first, last)| {
                    DiscoveredGuest {
                        discovery,
                        vmid: u32::try_from(vmid).unwrap_or_default(),
                        kind,
                        name,
                        node,
                        running,
                        address,
                        ignored,
                        first_seen: from_epoch(first),
                        last_seen: from_epoch(last),
                    }
                },
            )
            .collect())
    }

    /// Records what a run of `discovery` found: new guests are added,
    /// known ones updated (a guest without an address keeps its last one),
    /// and guests it didn't find are removed.
    pub async fn sync_discovered(&self, discovery: &str, found: &[FoundGuest]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for g in found {
            sqlx::query(
                "INSERT INTO discovered_guests (discovery, vmid, kind, name, node, running, address)
                 VALUES ($1, $2, $3, $4, $5, $6, $7)
                 ON CONFLICT (discovery, vmid) DO UPDATE SET
                     kind = $3, name = $4, node = $5, running = $6,
                     address = COALESCE($7, discovered_guests.address), last_seen = now()",
            )
            .bind(discovery)
            .bind(i32::try_from(g.vmid).unwrap_or(i32::MAX))
            .bind(&g.kind)
            .bind(&g.name)
            .bind(&g.node)
            .bind(g.running)
            .bind(&g.address)
            .execute(&mut *tx)
            .await?;
        }
        let vmids: Vec<i32> = found
            .iter()
            .map(|g| i32::try_from(g.vmid).unwrap_or(i32::MAX))
            .collect();
        sqlx::query("DELETE FROM discovered_guests WHERE discovery = $1 AND vmid <> ALL($2)")
            .bind(discovery)
            .bind(&vmids)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Sets guests of `discovery` aside (`ignored`), or back. The number of
    /// guests changed.
    pub async fn ignore_discovered(
        &self,
        discovery: &str,
        vmids: &[u32],
        ignored: bool,
    ) -> Result<u64> {
        let vmids: Vec<i32> = vmids
            .iter()
            .map(|v| i32::try_from(*v).unwrap_or(i32::MAX))
            .collect();
        let done = sqlx::query(
            "UPDATE discovered_guests SET ignored = $3 WHERE discovery = $1 AND vmid = ANY($2)",
        )
        .bind(discovery)
        .bind(&vmids)
        .bind(ignored)
        .execute(&self.pool)
        .await?;
        Ok(done.rows_affected())
    }

    /// Removes the guests of discoveries that are no longer configured.
    pub async fn forget_discoveries_except(&self, keep: &[String]) -> Result<u64> {
        let done = sqlx::query("DELETE FROM discovered_guests WHERE discovery <> ALL($1)")
            .bind(keep)
            .execute(&self.pool)
            .await?;
        Ok(done.rows_affected())
    }
}
