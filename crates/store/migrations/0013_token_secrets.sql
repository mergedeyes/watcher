-- API tokens (e.g. Proxmox VE) as a third kind of secret. The token's ID
-- isn't secret and is kept in the clear for lists; the sealed value holds
-- the ID and the secret together.
ALTER TABLE secrets DROP CONSTRAINT secrets_kind_check;
ALTER TABLE secrets ADD CONSTRAINT secrets_kind_check
    CHECK (kind IN ('key', 'password', 'token'));
ALTER TABLE secrets ADD COLUMN token_id text;
