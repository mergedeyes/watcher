-- Users, login sessions and API tokens. Sessions and tokens are stored as
-- SHA-256 hashes only, so a copy of the database grants no access.

CREATE TABLE users (
    id            int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    username      text NOT NULL,
    -- Argon2id PHC string.
    password_hash text NOT NULL,
    admin         boolean NOT NULL DEFAULT false,
    disabled      boolean NOT NULL DEFAULT false,
    created       timestamptz NOT NULL DEFAULT now(),
    last_login    timestamptz
);
-- Usernames are unique regardless of case.
CREATE UNIQUE INDEX users_username ON users (lower(username));

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY,
    user_id    int NOT NULL REFERENCES users ON DELETE CASCADE,
    created    timestamptz NOT NULL DEFAULT now(),
    expires    timestamptz NOT NULL
);
CREATE INDEX sessions_user ON sessions (user_id);

CREATE TABLE api_tokens (
    id         int GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    user_id    int NOT NULL REFERENCES users ON DELETE CASCADE,
    name       text NOT NULL,
    token_hash bytea NOT NULL UNIQUE,
    created    timestamptz NOT NULL DEFAULT now(),
    last_used  timestamptz,
    UNIQUE (user_id, name)
);
