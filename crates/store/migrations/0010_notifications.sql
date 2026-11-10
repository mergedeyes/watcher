-- The notification log: one row per contact and channel. Names as in the
-- config (like flags), so rows outlive renamed or removed objects.
CREATE TABLE notifications (
    id         bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    ts         timestamptz NOT NULL,
    host       text NOT NULL,
    -- For host notifications: the host check.
    service    text NOT NULL,
    host_check boolean NOT NULL,
    kind       text NOT NULL CHECK (kind IN ('problem', 'recovery', 'flappingstart',
                   'flappingstop', 'downtimestart', 'downtimeend')),
    -- ok, warning, critical, unknown, up, down
    state      text NOT NULL,
    -- A problem sent again (repeat or escalation).
    repeat     boolean NOT NULL DEFAULT false,
    -- Problems: when the problem reached its state.
    since      timestamptz,
    contact    text NOT NULL,
    -- email, webhook; '' if the contact has neither.
    channel    text NOT NULL,
    -- NULL: delivered.
    error      text
);
CREATE INDEX notifications_target ON notifications (host, service, ts DESC);
CREATE INDEX notifications_ts ON notifications (ts);
