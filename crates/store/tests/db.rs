//! Runs against real databases when these are set (see compose.yaml):
//!
//! - `WATCHER_TEST_TIMESCALE_URL`: server with TimescaleDB
//! - `WATCHER_TEST_POSTGRES_URL`: plain PostgreSQL
//!
//! Each test creates its own database on the server and drops it afterwards.

use sqlx::{AssertSqlSafe, Connection, PgConnection};
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use tokio::sync::{mpsc, watch};
use watcher_core::{Format, RawOutput, ServiceState, State, StateType, Termination, evaluate};
use watcher_store::{
    Backend, EventFilter, HostSpec, Record, SeriesCache, ServiceSpec, Store, SyncMode, ingest,
};

/// A database created for one test.
struct TestDb {
    admin_url: String,
    name: String,
    url: String,
}

impl TestDb {
    async fn create(admin_url: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "watcher_test_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        );
        let mut admin = PgConnection::connect(admin_url)
            .await
            .expect("connect to test server");
        sqlx::query(AssertSqlSafe(format!("CREATE DATABASE {name}")))
            .execute(&mut admin)
            .await
            .unwrap();
        let (base, query) = admin_url
            .split_once('?')
            .map_or((admin_url, None), |(b, q)| (b, Some(q)));
        let base = &base[..base.rfind('/').expect("URL with database")];
        let url = match query {
            Some(q) => format!("{base}/{name}?{q}"),
            None => format!("{base}/{name}"),
        };
        TestDb {
            admin_url: admin_url.into(),
            name,
            url,
        }
    }

    async fn drop(self) {
        let mut admin = PgConnection::connect(&self.admin_url).await.unwrap();
        sqlx::query(AssertSqlSafe(format!(
            "DROP DATABASE {} WITH (FORCE)",
            self.name
        )))
        .execute(&mut admin)
        .await
        .unwrap();
    }
}

/// Runs `test` once per configured server.
async fn each_server<F, Fut>(test: F)
where
    F: Fn(Store, Backend, String) -> Fut,
    Fut: Future<Output = ()>,
{
    let servers = [
        (
            "WATCHER_TEST_TIMESCALE_URL",
            Backend::Timescale { compression: true },
        ),
        ("WATCHER_TEST_POSTGRES_URL", Backend::Postgres),
    ];
    let mut ran = false;
    for (var, expected) in servers {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        ran = true;
        let db = TestDb::create(&url).await;
        let store = Store::connect(&db.url).await.expect("connect store");
        assert_eq!(store.backend(), expected, "{var}");
        test(store, expected, db.url.clone()).await;
        db.drop().await;
    }
    if !ran {
        eprintln!("skipped: set WATCHER_TEST_TIMESCALE_URL and/or WATCHER_TEST_POSTGRES_URL");
    }
}

fn host(name: &str, node: Option<&str>) -> HostSpec {
    HostSpec {
        name: name.into(),
        address: format!("{name}.example"),
        node: node.map(Into::into),
        tags: vec!["linux".into(), "web".into()],
        vars: BTreeMap::from([("port".into(), "8080".into())]),
    }
}

fn service(host: &str, name: &str, hash: &str) -> ServiceSpec {
    ServiceSpec {
        host: host.into(),
        name: name.into(),
        check_hash: hash.into(),
    }
}

fn record(service: &str, exit: i32, stdout: &str, prev: Option<&ServiceState>) -> Record {
    let raw = RawOutput {
        termination: Termination::Exited(exit),
        stdout: stdout.into(),
        stderr: String::new(),
    };
    let result = evaluate(&raw, Format::Auto, &BTreeMap::new());
    let now = SystemTime::now();
    let (state, change) = match prev {
        Some(prev) => {
            let mut s = prev.clone();
            let change = s.apply(result.state, 3, now);
            (s, change)
        }
        None => {
            let (s, c) = ServiceState::first(result.state, 3, now);
            (s, Some(c))
        }
    };
    Record {
        service: service.into(),
        result,
        state,
        change,
        duration: Duration::from_millis(12),
        flap: None,
        host_check: false,
    }
}

/// Feeds records through the ingester and waits for it to flush.
async fn ingest_all(store: &Arc<Store>, ids: HashMap<String, i32>, records: Vec<Record>) {
    let (_ids_tx, ids_rx) = watch::channel(Arc::new(ids));
    let (tx, rx) = mpsc::channel(records.len().max(1));
    for r in records {
        tx.send(r).await.unwrap();
    }
    drop(tx);
    tokio::time::timeout(Duration::from_secs(20), ingest(store.clone(), ids_rx, rx))
        .await
        .expect("ingest flushed");
}

async fn count(store: &Store, sql: &'static str) -> i64 {
    sqlx::query_scalar(sql)
        .fetch_one(store.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn sync_ingest_and_restore() {
    each_server(|store, backend, url| async move {
        // Setup is idempotent: a second start on the same database works.
        let again = Store::connect(&url).await.unwrap();
        assert_eq!(again.backend(), backend);

        let hosts = [host("web01", Some("customer-a")), host("db01", None)];
        let services = [service("web01", "disk", "h1"), service("web01", "http", "h1"), service("db01", "disk", "h1")];
        let ids = store.sync(&hosts, &services).await.unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            count(&store, "SELECT count(*) FROM hosts h JOIN nodes n ON n.id = h.node_id WHERE n.name = 'customer-a'").await,
            1
        );
        let tags: Vec<String> = sqlx::query_scalar("SELECT tags FROM hosts WHERE name = 'web01'")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(tags, ["linux", "web"]);

        let store = Arc::new(store);
        let disk = record("web01/disk", 0, "DISK OK | /=2643MB;5948;5958;0;5968 'temp'=41", None);
        let http = record("web01/http", 2, "HTTP CRITICAL - down\0x | time=1.5s", None);
        let removed = record("gone/x", 0, "OK | a=1", None);
        let first_disk_state = disk.state.clone();
        ingest_all(&store, ids.clone(), vec![disk, http, removed]).await;

        assert_eq!(count(&store, "SELECT count(*) FROM series").await, 3);
        assert_eq!(count(&store, "SELECT count(*) FROM v_metrics").await, 3);
        let used: f64 = sqlx::query_scalar("SELECT value FROM v_metrics WHERE host = 'web01' AND metric = '/'")
            .fetch_one(store.pool())
            .await
            .unwrap();
        assert_eq!(used, 2643.0 * 1024.0 * 1024.0);
        let output: String =
            sqlx::query_scalar("SELECT output FROM v_service_state WHERE service = 'http'")
                .fetch_one(store.pool())
                .await
                .unwrap();
        assert_eq!(output, "HTTP CRITICAL - down\u{FFFD}x");
        assert_eq!(count(&store, "SELECT count(*) FROM state_events").await, 2);

        // Same series again: reused from the cache / table, samples grow.
        let disk2 = record("web01/disk", 1, "DISK WARNING | /=2700MB 'temp'=42", Some(&first_disk_state));
        ingest_all(&store, ids.clone(), vec![disk2]).await;
        assert_eq!(count(&store, "SELECT count(*) FROM series").await, 3);
        assert_eq!(count(&store, "SELECT count(*) FROM samples").await, 5);
        assert_eq!(
            count(&store, "SELECT count(*) FROM state_events WHERE to_state = 'warning' AND state_type = 'soft' AND NOT hard").await,
            1
        );

        // db01 leaves the config, web01/disk changes its check.
        let ids = store
            .sync(&hosts[..1], &[service("web01", "disk", "h2"), service("web01", "http", "h1")])
            .await
            .unwrap();
        assert_eq!(ids.len(), 2);
        assert_eq!(count(&store, "SELECT count(*) FROM hosts WHERE removed_at IS NOT NULL").await, 1);
        assert_eq!(count(&store, "SELECT count(*) FROM services WHERE removed_at IS NOT NULL").await, 1);
        assert_eq!(
            count(&store, "SELECT count(*) FROM check_changes c JOIN services s ON s.id = c.service_id WHERE s.name = 'disk' AND s.host_id = (SELECT id FROM hosts WHERE name = 'web01')").await,
            2
        );

        let states = store.load_states().await.unwrap();
        assert_eq!(states.len(), 2);
        let disk = &states["web01/disk"].state;
        assert_eq!((disk.state, disk.state_type, disk.attempt), (State::Warning, StateType::Soft, 1));
        assert_eq!(states["web01/http"].output, "HTTP CRITICAL - down\u{FFFD}x");
        let http = &states["web01/http"].state;
        assert_eq!(http.state, State::Critical);
        let drift = http
            .last_check
            .duration_since(first_disk_state.last_check)
            .unwrap_or_else(|e| e.duration());
        assert!(drift < Duration::from_secs(5));

        // Read queries for the API.
        let events = store.events(&EventFilter { limit: 10, ..Default::default() }).await.unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!((events[0].service.as_str(), events[0].to_state.as_str()), ("disk", "warning"));
        let hard = EventFilter { limit: 10, hard_only: true, host: Some("web01".into()), ..Default::default() };
        assert_eq!(store.events(&hard).await.unwrap().len(), 1);

        // Changes of a host check are host events too; flapping is recorded.
        let mut up = record("web01/http", 0, "HTTP OK", Some(http));
        up.host_check = true;
        up.flap = Some(true);
        ingest_all(&store, ids.clone(), vec![up]).await;
        let of = |kinds: &[&str]| EventFilter {
            limit: 10,
            kinds: kinds.iter().map(|k| k.to_string()).collect(),
            ..Default::default()
        };
        let host = store.events(&of(&["host"])).await.unwrap();
        assert_eq!(host.len(), 1);
        let e = &host[0];
        assert_eq!((e.from_state.as_deref(), e.to_state.as_str(), e.state_type.as_str(), e.hard), (Some("down"), "up", "hard", false));
        let flap = store.events(&of(&["flap_start", "flap_stop"])).await.unwrap();
        assert_eq!((flap.len(), flap[0].kind.as_str(), flap[0].to_state.as_str()), (1, "flap_start", "ok"));
        assert!(flap[0].output.ends_with("% state change"), "{}", flap[0].output);
        assert_eq!(store.events(&of(&["service"])).await.unwrap().len(), 4);
        let soft = EventFilter { limit: 10, state_type: Some("soft".into()), ..Default::default() };
        assert_eq!(store.events(&soft).await.unwrap().len(), 2);
        let down_or_up = EventFilter { limit: 10, states: vec!["down".into(), "up".into()], ..Default::default() };
        assert_eq!(store.events(&down_or_up).await.unwrap().len(), 1);
        let later = EventFilter { limit: 10, since: Some(SystemTime::now() + Duration::from_secs(60)), ..Default::default() };
        assert!(store.events(&later).await.unwrap().is_empty());

        let series = store.series("web01", "disk").await.unwrap().unwrap();
        let names: Vec<_> = series.iter().map(|s| s.metric.as_str()).collect();
        assert_eq!(names, ["/", "temp"]);
        assert_eq!(series[0].unit, "B");
        // Two samples of "/" were written; first and last bracket them.
        let (first, last) = (series[0].first.unwrap(), series[0].last.unwrap());
        assert!(first < last && last <= SystemTime::now());
        assert!(store.series("nope", "x").await.unwrap().is_none());

        let now = SystemTime::now();
        let hour = Duration::from_secs(3600);
        let points = store.points(series[0].id, now - hour, now + hour, 600).await.unwrap().unwrap();
        assert_eq!(points.bucket, 12.0);
        let [(_, avg, min, max, n)] = points.points[..] else { panic!("{points:?}") };
        let mb = 1024.0 * 1024.0;
        assert_eq!((min, max, n), (2643.0 * mb, 2700.0 * mb, 2));
        assert_eq!(avg, (2643.0 + 2700.0) / 2.0 * mb);
        assert!(store.points(999_999, now - hour, now, 600).await.unwrap().is_none());

        if let Backend::Timescale { .. } = backend {
            assert_eq!(
                count(&store, "SELECT count(*) FROM timescaledb_information.hypertables WHERE hypertable_name = 'samples' AND compression_enabled").await,
                1
            );
        }
    })
    .await;
}

#[tokio::test]
async fn sink_batches_apply_exactly_once() {
    each_server(|store, _, _| async move {
        let ids = store
            .sync(&[host("web01", None)], &[service("web01", "disk", "h1")])
            .await
            .unwrap();
        let first = record("web01/disk", 0, "OK | used=10%", None);
        let records: Vec<Record> = (1..=3)
            .map(|i| {
                record(
                    "web01/disk",
                    0,
                    &format!("OK | used={}%", 10 + i),
                    Some(&first.state),
                )
            })
            .collect();
        // What a sink reads back from its spool must equal what it wrote.
        let json = serde_json::to_string(&records[0]).unwrap();
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), records[0]);

        let batch = |seqs: &[u64]| -> Vec<(u64, Record)> {
            seqs.iter()
                .map(|&s| (s, records[s as usize - 1].clone()))
                .collect()
        };
        let mut series = SeriesCache::default();
        store
            .write_sink("i1/central", &batch(&[1, 2]), &ids, &mut series)
            .await
            .unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM samples").await, 2);
        // Retried after a lost acknowledgement: nothing new.
        store
            .write_sink("i1/central", &batch(&[1, 2]), &ids, &mut series)
            .await
            .unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM samples").await, 2);
        // Overlapping batch: only record 3 is new.
        store
            .write_sink("i1/central", &batch(&[2, 3]), &ids, &mut series)
            .await
            .unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM samples").await, 3);
        assert_eq!(
            count(
                &store,
                "SELECT seq FROM sink_progress WHERE source = 'i1/central'"
            )
            .await,
            3
        );
        // Another source counts separately.
        store
            .write_sink("i2/central", &batch(&[1]), &ids, &mut series)
            .await
            .unwrap();
        assert_eq!(count(&store, "SELECT count(*) FROM samples").await, 4);

        // A second instance syncing additively leaves the first one's hosts alone.
        store
            .sync_with(
                &[host("db02", None)],
                &[service("db02", "disk", "h1")],
                SyncMode::Additive,
            )
            .await
            .unwrap();
        assert_eq!(
            count(
                &store,
                "SELECT count(*) FROM hosts WHERE removed_at IS NOT NULL"
            )
            .await,
            0
        );
        assert_eq!(count(&store, "SELECT count(*) FROM hosts").await, 2);
    })
    .await;
}

#[tokio::test]
async fn readers_see_monitoring_data_only() {
    let Ok(admin_url) = std::env::var("WATCHER_TEST_POSTGRES_URL") else {
        eprintln!("skipped: set WATCHER_TEST_POSTGRES_URL");
        return;
    };
    let db = TestDb::create(&admin_url).await;
    let store = Store::connect(&db.url).await.unwrap();
    let role = format!("watcher_reader_{}", std::process::id());
    store.grant_reader(&role, "it's 'quoted'").await.unwrap();
    // Again: updates the password.
    store.grant_reader(&role, "reader password").await.unwrap();

    let (scheme, rest) = db.url.split_once("://").unwrap();
    let host = &rest[rest.find('@').unwrap() + 1..];
    let url = format!("{scheme}://{role}:reader%20password@{host}");
    let mut reader = PgConnection::connect(&url).await.expect("reader logs in");
    let metrics: i64 = sqlx::query_scalar("SELECT count(*) FROM v_metrics")
        .fetch_one(&mut reader)
        .await
        .unwrap();
    assert_eq!(metrics, 0);
    for secret in ["users", "sessions", "api_tokens"] {
        let err = sqlx::query(AssertSqlSafe(format!("SELECT * FROM {secret}")))
            .fetch_all(&mut reader)
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("permission denied"),
            "{secret}: {err}"
        );
    }
    assert!(
        sqlx::query("DELETE FROM hosts")
            .execute(&mut reader)
            .await
            .is_err()
    );
    drop(reader);
    drop(store);

    db.drop().await;
    let mut admin = PgConnection::connect(&admin_url).await.unwrap();
    sqlx::query(AssertSqlSafe(format!("DROP ROLE {role}")))
        .execute(&mut admin)
        .await
        .unwrap();
}
