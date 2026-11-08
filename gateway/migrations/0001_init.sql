-- Money is integer micro-USD everywhere (1 micro-USD = 1 USDC atomic unit).

CREATE TABLE accounts (
    id            UUID PRIMARY KEY,
    api_key_hash  BYTEA NOT NULL UNIQUE,
    balance_micro BIGINT NOT NULL DEFAULT 0 CHECK (balance_micro >= 0),
    held_micro    BIGINT NOT NULL DEFAULT 0 CHECK (held_micro >= 0),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Funds set aside from a balance while a request or batch runs.
CREATE TABLE holds (
    id           UUID PRIMARY KEY,
    account_id   UUID NOT NULL REFERENCES accounts(id),
    kind         TEXT NOT NULL CHECK (kind IN ('request', 'batch')),
    amount_micro BIGINT NOT NULL CHECK (amount_micro >= 0),
    status       TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'closed')),
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at   TIMESTAMPTZ NOT NULL
);
CREATE INDEX holds_active_expiry ON holds (expires_at) WHERE status = 'active';

-- Append-only journal of every balance movement.
CREATE TABLE ledger_entries (
    id           BIGSERIAL PRIMARY KEY,
    account_id   UUID NOT NULL REFERENCES accounts(id),
    kind         TEXT NOT NULL CHECK (kind IN ('topup', 'charge')),
    amount_micro BIGINT NOT NULL,
    reference    TEXT NOT NULL,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX ledger_entries_account ON ledger_entries (account_id, created_at);

-- Every x402 payment the gateway has seen, per request or top-up.
CREATE TABLE payments (
    id                UUID PRIMARY KEY,
    signature_hash    BYTEA NOT NULL UNIQUE,
    purpose           TEXT NOT NULL CHECK (purpose IN ('request', 'topup')),
    scheme            TEXT NOT NULL,
    network           TEXT NOT NULL,
    payer             TEXT,
    account_id        UUID REFERENCES accounts(id),
    authorized_atomic NUMERIC(78, 0) NOT NULL,
    settle_atomic     NUMERIC(78, 0),
    charged_micro     BIGINT,
    status            TEXT NOT NULL CHECK (status IN
                        ('authorized', 'settling', 'settled', 'settle_pending', 'failed', 'released', 'needs_review')),
    transaction       TEXT,
    error             TEXT,
    settle_attempts   INT NOT NULL DEFAULT 0,
    payload           JSONB NOT NULL,
    requirements      JSONB NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX payments_open ON payments (updated_at) WHERE status IN ('settle_pending', 'settling');

-- One row per answered request, whoever paid for it.
CREATE TABLE usage_events (
    id                BIGSERIAL PRIMARY KEY,
    request_id        TEXT NOT NULL,
    account_id        UUID REFERENCES accounts(id),
    payment_id        UUID REFERENCES payments(id),
    route_id          TEXT NOT NULL,
    tier              TEXT NOT NULL CHECK (tier IN ('live', 'batch')),
    prompt_tokens     BIGINT NOT NULL,
    completion_tokens BIGINT NOT NULL,
    usage_estimated   BOOLEAN NOT NULL DEFAULT false,
    cost_micro        BIGINT NOT NULL,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX usage_events_account ON usage_events (account_id, created_at);

CREATE TABLE files (
    id         TEXT PRIMARY KEY,
    account_id UUID NOT NULL REFERENCES accounts(id),
    purpose    TEXT NOT NULL,
    filename   TEXT NOT NULL,
    bytes      BIGINT NOT NULL,
    content    BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE batches (
    id                TEXT PRIMARY KEY,
    account_id        UUID NOT NULL REFERENCES accounts(id),
    input_file_id     TEXT NOT NULL REFERENCES files(id),
    endpoint          TEXT NOT NULL,
    completion_window TEXT NOT NULL,
    status            TEXT NOT NULL CHECK (status IN
                        ('in_progress', 'finalizing', 'completed', 'expired', 'cancelling', 'cancelled')),
    hold_id           UUID NOT NULL REFERENCES holds(id),
    ceiling_micro     BIGINT NOT NULL,
    output_file_id    TEXT REFERENCES files(id),
    error_file_id     TEXT REFERENCES files(id),
    metadata          JSONB,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at        TIMESTAMPTZ NOT NULL,
    finalizing_at     TIMESTAMPTZ,
    completed_at      TIMESTAMPTZ,
    expired_at        TIMESTAMPTZ,
    cancelling_at     TIMESTAMPTZ,
    cancelled_at      TIMESTAMPTZ
);
CREATE INDEX batches_account ON batches (account_id, created_at DESC);

CREATE TABLE batch_items (
    id              BIGSERIAL PRIMARY KEY,
    batch_id        TEXT NOT NULL REFERENCES batches(id),
    line            INT NOT NULL,
    custom_id       TEXT NOT NULL,
    body            JSONB NOT NULL,
    status          TEXT NOT NULL DEFAULT 'pending' CHECK (status IN
                      ('pending', 'running', 'succeeded', 'failed', 'expired', 'cancelled')),
    attempts        INT NOT NULL DEFAULT 0,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_until    TIMESTAMPTZ,
    route_id        TEXT,
    cost_micro      BIGINT,
    response        JSONB,
    error           JSONB,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (batch_id, custom_id)
);
CREATE INDEX batch_items_runnable ON batch_items (next_attempt_at) WHERE status = 'pending';
CREATE INDEX batch_items_running ON batch_items (locked_until) WHERE status = 'running';
