use crate::{Result, Store, from_epoch};
use std::collections::{BTreeMap, HashMap};
use watcher_core::{ServiceState, State, StateType};

/// A host as the config defines it.
#[derive(Debug, Clone, PartialEq)]
pub struct HostSpec {
    pub name: String,
    pub address: String,
    pub node: Option<String>,
    pub tags: Vec<String>,
    pub vars: BTreeMap<String, String>,
}

/// A service as the config defines it.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceSpec {
    pub host: String,
    pub name: String,
    pub check_hash: String,
}

impl Store {
    /// Makes the hosts and services tables match the config. Objects that
    /// left the config are marked removed, not deleted.
    ///
    /// Returns the database id of every service, keyed by `host/service`.
    pub async fn sync(
        &self,
        hosts: &[HostSpec],
        services: &[ServiceSpec],
    ) -> Result<HashMap<String, i32>> {
        let mut tx = self.pool.begin().await?;

        let nodes: Vec<&str> = hosts.iter().filter_map(|h| h.node.as_deref()).collect();
        sqlx::query(
            "INSERT INTO nodes (tenant_id, name)
             SELECT (SELECT id FROM tenants WHERE name = 'default'), n
             FROM UNNEST($1::text[]) AS n
             ON CONFLICT (name) DO NOTHING",
        )
        .bind(&nodes)
        .execute(&mut *tx)
        .await?;

        let names: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
        let addresses: Vec<&str> = hosts.iter().map(|h| h.address.as_str()).collect();
        let host_nodes: Vec<Option<&str>> = hosts.iter().map(|h| h.node.as_deref()).collect();
        // Tags go in as JSON arrays: UNNEST cannot take an array of arrays.
        let tags: Vec<serde_json::Value> =
            hosts.iter().map(|h| serde_json::json!(h.tags)).collect();
        let vars: Vec<serde_json::Value> =
            hosts.iter().map(|h| serde_json::json!(h.vars)).collect();
        sqlx::query(
            "INSERT INTO hosts (tenant_id, node_id, name, address, tags, vars)
             SELECT (SELECT id FROM tenants WHERE name = 'default'),
                    (SELECT id FROM nodes WHERE name = u.node),
                    u.name, u.address,
                    ARRAY(SELECT jsonb_array_elements_text(u.tags)), u.vars
             FROM UNNEST($1::text[], $2::text[], $3::text[], $4::jsonb[], $5::jsonb[])
                  AS u(name, address, node, tags, vars)
             ON CONFLICT (name) DO UPDATE
             SET node_id = EXCLUDED.node_id, address = EXCLUDED.address,
                 tags = EXCLUDED.tags, vars = EXCLUDED.vars, removed_at = NULL",
        )
        .bind(&names)
        .bind(&addresses)
        .bind(&host_nodes)
        .bind(&tags)
        .bind(&vars)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE hosts SET removed_at = now()
             WHERE removed_at IS NULL AND NOT (name = ANY($1))",
        )
        .bind(&names)
        .execute(&mut *tx)
        .await?;

        let svc_hosts: Vec<&str> = services.iter().map(|s| s.host.as_str()).collect();
        let svc_names: Vec<&str> = services.iter().map(|s| s.name.as_str()).collect();
        let hashes: Vec<&str> = services.iter().map(|s| s.check_hash.as_str()).collect();
        sqlx::query(
            "INSERT INTO services (host_id, name, check_hash)
             SELECT h.id, u.name, u.hash
             FROM UNNEST($1::text[], $2::text[], $3::text[]) AS u(host, name, hash)
             JOIN hosts h ON h.name = u.host
             ON CONFLICT (host_id, name) DO UPDATE
             SET check_hash = EXCLUDED.check_hash, removed_at = NULL",
        )
        .bind(&svc_hosts)
        .bind(&svc_names)
        .bind(&hashes)
        .execute(&mut *tx)
        .await?;

        let keys: Vec<String> = services
            .iter()
            .map(|s| format!("{}/{}", s.host, s.name))
            .collect();
        sqlx::query(
            "UPDATE services s SET removed_at = now()
             FROM hosts h
             WHERE h.id = s.host_id AND s.removed_at IS NULL
               AND NOT (h.name || '/' || s.name = ANY($1))",
        )
        .bind(&keys)
        .execute(&mut *tx)
        .await?;

        let rows: Vec<(i32, String)> = sqlx::query_as(
            "SELECT s.id, h.name || '/' || s.name
             FROM services s JOIN hosts h ON h.id = s.host_id
             WHERE s.removed_at IS NULL",
        )
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows.into_iter().map(|(id, key)| (key, id)).collect())
    }

    /// The last persisted state of every active service, keyed by
    /// `host/service`, so a restart continues where it stopped.
    pub async fn load_states(&self) -> Result<HashMap<String, ServiceState>> {
        let rows: Vec<(String, String, String, i32, f64, f64, f64)> = sqlx::query_as(
            "SELECT h.name || '/' || s.name, st.state, st.state_type, st.attempt,
                    extract(epoch FROM st.last_check)::float8,
                    extract(epoch FROM st.last_change)::float8,
                    extract(epoch FROM st.last_hard_change)::float8
             FROM service_state st
             JOIN services s ON s.id = st.service_id
             JOIN hosts h ON h.id = s.host_id
             WHERE s.removed_at IS NULL",
        )
        .fetch_all(&self.pool)
        .await?;

        let mut states = HashMap::new();
        for (key, state, state_type, attempt, last_check, last_change, last_hard_change) in rows {
            let (Some(state), Some(state_type)) =
                (State::parse(&state), parse_state_type(&state_type))
            else {
                tracing::warn!(
                    "ignoring stored state of {key}: invalid state {state:?}/{state_type:?}"
                );
                continue;
            };
            states.insert(
                key,
                ServiceState {
                    state,
                    state_type,
                    attempt: attempt.max(1) as u32,
                    last_check: from_epoch(last_check),
                    last_change: from_epoch(last_change),
                    last_hard_change: from_epoch(last_hard_change),
                },
            );
        }
        Ok(states)
    }
}

pub(crate) fn state_type_str(t: StateType) -> &'static str {
    match t {
        StateType::Soft => "soft",
        StateType::Hard => "hard",
    }
}

fn parse_state_type(s: &str) -> Option<StateType> {
    match s {
        "soft" => Some(StateType::Soft),
        "hard" => Some(StateType::Hard),
        _ => None,
    }
}
