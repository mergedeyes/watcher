-- Nodes join with a one-time token and are then known by their certificate
-- (cert_fp). Rows also exist for every node label a host names, joined or
-- not, since hosts.node_id refers to them.
ALTER TABLE nodes
    ADD COLUMN token_hash    bytea,
    ADD COLUMN token_expires timestamptz,
    ADD COLUMN joined        timestamptz,
    ADD COLUMN hostname      text,
    ADD COLUMN address       text;

CREATE UNIQUE INDEX nodes_cert_fp ON nodes (cert_fp);
CREATE UNIQUE INDEX nodes_token_hash ON nodes (token_hash);
