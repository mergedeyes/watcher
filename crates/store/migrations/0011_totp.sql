-- Two-factor authentication with time-based one-time codes (TOTP, RFC 6238).
ALTER TABLE users
    -- The shared secret, sealed with secret.key like SSH secrets, so a copy
    -- of the database can't produce codes. NULL: two-factor is off.
    ADD COLUMN totp_sealed bytea,
    -- The last 30-second time step a code was accepted for: each code
    -- works only once.
    ADD COLUMN totp_step bigint;

-- One-time recovery codes for a lost authenticator, as SHA-256 hashes.
CREATE TABLE recovery_codes (
    user_id   int NOT NULL REFERENCES users ON DELETE CASCADE,
    code_hash bytea NOT NULL,
    PRIMARY KEY (user_id, code_hash)
);
