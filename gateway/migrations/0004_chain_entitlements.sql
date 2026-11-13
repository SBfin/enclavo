-- Capacity won in the on-chain auction contract, mirrored so the gateway can meter it.
ALTER TABLE accounts ADD COLUMN near_account_id TEXT UNIQUE;

-- On-chain entitlements have no off-chain auction or bid behind them.
ALTER TABLE capacity_contracts ALTER COLUMN auction_id DROP NOT NULL;
ALTER TABLE capacity_contracts ALTER COLUMN bid_id DROP NOT NULL;
ALTER TABLE capacity_contracts ADD COLUMN chain_contract TEXT;
ALTER TABLE capacity_contracts ADD COLUMN chain_entitlement_id BIGINT;
ALTER TABLE capacity_contracts ADD CONSTRAINT capacity_source CHECK (
    (chain_contract IS NULL AND chain_entitlement_id IS NULL AND auction_id IS NOT NULL AND bid_id IS NOT NULL)
    OR (chain_contract IS NOT NULL AND chain_entitlement_id IS NOT NULL AND auction_id IS NULL AND bid_id IS NULL)
);
CREATE UNIQUE INDEX capacity_chain_entitlement ON capacity_contracts(chain_contract, chain_entitlement_id);

-- Next entitlement id to read from each auction contract.
CREATE TABLE chain_sync (
    contract            TEXT PRIMARY KEY,
    next_entitlement_id BIGINT NOT NULL DEFAULT 0
);
