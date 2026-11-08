ALTER TABLE accounts ADD COLUMN owner_user_id TEXT UNIQUE;

CREATE TABLE api_keys (
    id UUID PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    name TEXT NOT NULL,
    prefix TEXT NOT NULL,
    key_hash BYTEA NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ
);
CREATE INDEX api_keys_account ON api_keys(account_id, created_at DESC);
