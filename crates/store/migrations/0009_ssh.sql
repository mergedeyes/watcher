-- Credentials for checks over SSH, sealed by the server (the database
-- never sees them in plain text), and host keys pinned on first use.
CREATE TABLE secrets (
    name text PRIMARY KEY,
    kind text NOT NULL CHECK (kind IN ('key', 'password')),
    -- Keys: the public key, as a line for authorized_keys.
    public_key text,
    sealed bytea NOT NULL,
    created timestamptz NOT NULL DEFAULT now(),
    updated timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE ssh_host_keys (
    address text NOT NULL,
    port int NOT NULL,
    -- The pinned key, as an OpenSSH public key line.
    key text NOT NULL,
    pinned timestamptz NOT NULL DEFAULT now(),
    -- A different key the host presented since: refused until accepted.
    offered text,
    offered_at timestamptz,
    PRIMARY KEY (address, port)
);
