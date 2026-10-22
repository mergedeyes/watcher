-- Core schema. Plain PostgreSQL; TimescaleDB features (hypertable,
-- compression) are added at startup when the extension is available.

CREATE TABLE tenants (
    id   int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    name text NOT NULL UNIQUE
);
INSERT INTO tenants (name) VALUES ('default');

CREATE TABLE nodes (
    id        int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id int NOT NULL REFERENCES tenants,
    name      text NOT NULL UNIQUE,
    cert_fp   text,
    last_seen timestamptz,
    version   text
);

-- Objects from the config. Rows are never deleted when they leave the
-- config, only marked removed, so their history stays queryable.
CREATE TABLE hosts (
    id         int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id  int NOT NULL REFERENCES tenants,
    node_id    int REFERENCES nodes,
    name       text NOT NULL UNIQUE,
    address    text NOT NULL,
    tags       text[] NOT NULL DEFAULT '{}',
    vars       jsonb NOT NULL DEFAULT '{}',
    removed_at timestamptz
);

CREATE TABLE services (
    id         int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    host_id    int NOT NULL REFERENCES hosts ON DELETE CASCADE,
    name       text NOT NULL,
    -- Hash of the effective check definition; see check_changes.
    check_hash text NOT NULL,
    removed_at timestamptz,
    UNIQUE (host_id, name)
);

-- Every change of a service's check definition, for "mask everything
-- since check change X".
CREATE TABLE check_changes (
    service_id int NOT NULL REFERENCES services ON DELETE CASCADE,
    ts         timestamptz NOT NULL,
    check_hash text NOT NULL
);
CREATE INDEX check_changes_service_ts ON check_changes (service_id, ts);

CREATE FUNCTION record_check_change() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF TG_OP = 'INSERT' OR NEW.check_hash IS DISTINCT FROM OLD.check_hash THEN
        INSERT INTO check_changes (service_id, ts, check_hash)
        VALUES (NEW.id, now(), NEW.check_hash);
    END IF;
    RETURN NEW;
END $$;

CREATE TRIGGER services_check_change
    AFTER INSERT OR UPDATE OF check_hash ON services
    FOR EACH ROW EXECUTE FUNCTION record_check_change();

-- One row per metric and label set of a service.
CREATE TABLE series (
    id         int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    service_id int NOT NULL REFERENCES services ON DELETE CASCADE,
    metric     text NOT NULL,
    labels     jsonb NOT NULL DEFAULT '{}',
    unit       text NOT NULL DEFAULT '',
    UNIQUE (service_id, metric, labels)
);

-- Values are normalized (seconds, bytes, ...). flags: 1 = SYNTHETIC,
-- 2 = SUSPECT, 4 = MASKED.
CREATE TABLE samples (
    series_id int NOT NULL,
    ts        timestamptz NOT NULL,
    value     float8 NOT NULL,
    flags     int2 NOT NULL DEFAULT 0
);
CREATE INDEX samples_series_ts ON samples (series_id, ts DESC);

CREATE TABLE service_state (
    service_id       int PRIMARY KEY REFERENCES services ON DELETE CASCADE,
    state            text NOT NULL CHECK (state IN ('ok', 'warning', 'critical', 'unknown')),
    state_type       text NOT NULL CHECK (state_type IN ('soft', 'hard')),
    attempt          int NOT NULL,
    last_check       timestamptz NOT NULL,
    last_change      timestamptz NOT NULL,
    last_hard_change timestamptz NOT NULL,
    output           text NOT NULL,
    long_output      text NOT NULL DEFAULT '',
    duration         float8 NOT NULL DEFAULT 0
);

CREATE TABLE state_events (
    service_id int NOT NULL REFERENCES services ON DELETE CASCADE,
    ts         timestamptz NOT NULL,
    from_state text,
    to_state   text NOT NULL,
    state_type text NOT NULL,
    hard       boolean NOT NULL,
    output     text NOT NULL
);
CREATE INDEX state_events_service_ts ON state_events (service_id, ts DESC);

-- Views for Grafana, BI tools and ad-hoc SQL.
CREATE VIEW v_metrics AS
SELECT t.name AS tenant, h.name AS host, s.name AS service,
       se.metric, se.labels, se.unit, sa.ts, sa.value, sa.flags
FROM samples sa
JOIN series se ON se.id = sa.series_id
JOIN services s ON s.id = se.service_id
JOIN hosts h ON h.id = s.host_id
JOIN tenants t ON t.id = h.tenant_id
WHERE sa.flags & 4 = 0;

CREATE VIEW v_service_state AS
SELECT t.name AS tenant, h.name AS host, s.name AS service,
       st.state, st.state_type, st.attempt, st.last_check, st.last_change,
       st.last_hard_change, st.output
FROM service_state st
JOIN services s ON s.id = st.service_id
JOIN hosts h ON h.id = s.host_id
JOIN tenants t ON t.id = h.tenant_id
WHERE s.removed_at IS NULL;
