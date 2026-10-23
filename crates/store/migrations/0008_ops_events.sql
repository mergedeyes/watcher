-- Acknowledgements and downtimes in the alert history. Events about a
-- host without a service (a host without host check, a host downtime)
-- have host_id instead of service_id.
ALTER TABLE state_events ALTER COLUMN service_id DROP NOT NULL;
ALTER TABLE state_events ADD COLUMN host_id int REFERENCES hosts ON DELETE CASCADE;
ALTER TABLE state_events ADD CONSTRAINT state_events_subject
    CHECK (service_id IS NOT NULL OR host_id IS NOT NULL);
ALTER TABLE state_events DROP CONSTRAINT state_events_kind_check;
ALTER TABLE state_events ADD CONSTRAINT state_events_kind_check
    CHECK (kind IN ('service', 'host', 'flap_start', 'flap_stop',
                    'ack', 'downtime_start', 'downtime_end'));
CREATE INDEX state_events_host_ts ON state_events (host_id, ts DESC)
    WHERE host_id IS NOT NULL;
