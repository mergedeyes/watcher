-- Exactly-once delivery from sinks: per sending instance and sink, the
-- last sequence number applied. Updated in the same transaction as the data.
CREATE TABLE sink_progress (
    source  text PRIMARY KEY,
    seq     bigint NOT NULL,
    updated timestamptz NOT NULL DEFAULT now()
);
