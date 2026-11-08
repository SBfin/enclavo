ALTER TABLE holds DROP CONSTRAINT holds_kind_check;
ALTER TABLE holds ADD CONSTRAINT holds_kind_check CHECK(kind IN ('request','batch','auction'));
CREATE TABLE market_auctions (
 id UUID PRIMARY KEY, route_id TEXT NOT NULL, units BIGINT NOT NULL CHECK(units>0),
 term_days INT NOT NULL CHECK(term_days IN (7,30,90)), reserve_price_micro BIGINT NOT NULL CHECK(reserve_price_micro>0),
 tokens_per_iu BIGINT NOT NULL CHECK(tokens_per_iu>0), prompt_weight_bps INT NOT NULL CHECK(prompt_weight_bps>=0),
 ends_at TIMESTAMPTZ NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','cleared')),
 cleared_at TIMESTAMPTZ, clearing_price_micro BIGINT, filled_units BIGINT NOT NULL DEFAULT 0
);
CREATE INDEX market_due ON market_auctions(ends_at) WHERE status='open';
CREATE TABLE market_bids (
 id UUID PRIMARY KEY, auction_id UUID NOT NULL REFERENCES market_auctions(id), account_id UUID NOT NULL REFERENCES accounts(id),
 client_order_id UUID NOT NULL, units BIGINT NOT NULL CHECK(units>0), max_price_micro BIGINT NOT NULL CHECK(max_price_micro>0),
 hold_id UUID NOT NULL REFERENCES holds(id), created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
 status TEXT NOT NULL DEFAULT 'open' CHECK(status IN ('open','filled','partial','outbid','cancelled')),
 filled_units BIGINT NOT NULL DEFAULT 0, charged_micro BIGINT NOT NULL DEFAULT 0,
 UNIQUE(account_id,client_order_id)
);
CREATE INDEX market_bid_priority ON market_bids(auction_id,max_price_micro DESC,created_at,id) WHERE status='open';
CREATE TABLE capacity_contracts (
 id UUID PRIMARY KEY, auction_id UUID NOT NULL REFERENCES market_auctions(id), bid_id UUID NOT NULL UNIQUE REFERENCES market_bids(id),
 account_id UUID NOT NULL REFERENCES accounts(id), route_id TEXT NOT NULL, units BIGINT NOT NULL CHECK(units>0),
 tokens_per_iu BIGINT NOT NULL CHECK(tokens_per_iu>0), prompt_weight_bps INT NOT NULL CHECK(prompt_weight_bps>=0),
 starts_at TIMESTAMPTZ NOT NULL DEFAULT now(), ends_at TIMESTAMPTZ NOT NULL,
 paid_micro BIGINT NOT NULL CHECK(paid_micro>=0)
);
CREATE INDEX capacity_owner ON capacity_contracts(account_id,ends_at);
CREATE TABLE capacity_daily_usage (
 contract_id UUID NOT NULL REFERENCES capacity_contracts(id), day_index INT NOT NULL,
 used_bps BIGINT NOT NULL DEFAULT 0 CHECK(used_bps>=0), held_bps BIGINT NOT NULL DEFAULT 0 CHECK(held_bps>=0),
 PRIMARY KEY(contract_id,day_index)
);
CREATE TABLE capacity_holds (
 id UUID PRIMARY KEY, contract_id UUID NOT NULL REFERENCES capacity_contracts(id), day_index INT NOT NULL,
 amount_bps BIGINT NOT NULL CHECK(amount_bps>=0), status TEXT NOT NULL DEFAULT 'active' CHECK(status IN ('active','closed')),
 expires_at TIMESTAMPTZ NOT NULL DEFAULT now()+interval '30 minutes'
);
CREATE INDEX capacity_hold_expiry ON capacity_holds(expires_at) WHERE status='active';
ALTER TABLE usage_events ADD COLUMN capacity_contract_id UUID REFERENCES capacity_contracts(id);
