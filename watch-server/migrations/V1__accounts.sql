-- Accounts and their sign-in sessions.
-- Applied once by refinery (see src/auth.rs); never edit a released
-- migration: add a new V<n>__<name>.sql instead.

CREATE TABLE accounts (
    id BIGSERIAL PRIMARY KEY,
    -- Lower-cased, so lookups and the unique index ignore case.
    email TEXT NOT NULL,
    name TEXT NOT NULL,
    -- Argon2id, in PHC string format.
    password_hash TEXT NOT NULL,
    -- Unix seconds.
    created_at BIGINT NOT NULL
);

CREATE UNIQUE INDEX accounts_email ON accounts (email);

CREATE TABLE sessions (
    -- SHA-256 of the session token, hex; the token itself is never stored.
    token_hash TEXT PRIMARY KEY,
    account_id BIGINT NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_at BIGINT NOT NULL,
    last_used BIGINT NOT NULL
);

CREATE INDEX sessions_account ON sessions (account_id);
