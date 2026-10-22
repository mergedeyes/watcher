-- The alert history also holds host UP/DOWN changes (derived from the host
-- check, stored with its service) and flapping start/stop.
ALTER TABLE state_events
    ADD COLUMN kind text NOT NULL DEFAULT 'service'
        CHECK (kind IN ('service', 'host', 'flap_start', 'flap_stop'));
-- Alerts of all services, newest first.
CREATE INDEX state_events_ts ON state_events (ts DESC);
