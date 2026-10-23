-- Runtime operations: acknowledgements, muted notifications, disabled
-- checks, downtimes and global switches. Targets are names as in the
-- config: a host ("web01") or a service ("web01/http").

CREATE TABLE flags (
    kind    text NOT NULL CHECK (kind IN ('ack', 'mute', 'no_checks')),
    target  text NOT NULL,
    author  text NOT NULL,
    comment text NOT NULL DEFAULT '',
    created timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (kind, target)
);

CREATE TABLE downtimes (
    id        bigint GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    kind      text NOT NULL
        CHECK (kind IN ('host', 'service', 'hostgroup', 'servicegroup', 'downtimegroup')),
    target    text NOT NULL,
    starts    timestamptz NOT NULL,
    ends      timestamptz NOT NULL CHECK (ends > starts),
    author    text NOT NULL,
    comment   text NOT NULL DEFAULT '',
    created   timestamptz NOT NULL DEFAULT now(),
    cancelled timestamptz
);
CREATE INDEX downtimes_open ON downtimes (ends) WHERE cancelled IS NULL;

-- Global switches, e.g. notifications on/off.
CREATE TABLE settings (
    name  text PRIMARY KEY,
    value jsonb NOT NULL
);
