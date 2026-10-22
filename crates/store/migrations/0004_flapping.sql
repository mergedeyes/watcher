-- Flap detection: the last 21 states (2 bits each, newest lowest) and
-- whether the service is flapping, so a restart continues detection.
ALTER TABLE service_state
    ADD COLUMN history     bigint   NOT NULL DEFAULT 0,
    ADD COLUMN history_len smallint NOT NULL DEFAULT 0,
    ADD COLUMN flapping    boolean  NOT NULL DEFAULT false;

CREATE OR REPLACE VIEW v_service_state AS
SELECT t.name AS tenant, h.name AS host, s.name AS service,
       st.state, st.state_type, st.attempt, st.last_check, st.last_change,
       st.last_hard_change, st.output, st.flapping
FROM service_state st
JOIN services s ON s.id = st.service_id
JOIN hosts h ON h.id = s.host_id
JOIN tenants t ON t.id = h.tenant_id
WHERE s.removed_at IS NULL;
