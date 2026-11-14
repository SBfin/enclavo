"use client";
import { useCallback } from "react";
import { useWallet } from "@/components/WalletProvider";
import { PLAIN_CALL, TOKEN_CALL, USD, view, type ChainAuction, type ChainBid, type ChainEntitlement, type ChainInfo } from "@/lib/near";
import { usePolled } from "@/lib/use-polled";

export const FAUCET_AMOUNT = 1_000 * USD;

export type Snapshot = {
  token: string;
  /** The account that publishes auctions and may close one early. */
  operator: string;
  auctions: ChainAuction[];
  bids: ChainBid[];
  entitlements: ChainEntitlement[];
  /** Escrow released to the user inside the contract, in token atomic units (micro-USD). */
  unlocked: number;
  /** Stablecoin in the user's wallet, in token atomic units (micro-USD). */
  wallet: number;
  loaded: boolean;
};
const EMPTY: Snapshot = { token: "", operator: "", auctions: [], bids: [], entitlements: [], unlocked: 0, wallet: 0, loaded: false };

/** Market data changes slowly; a bid or settlement refreshes it at once through the wallet event. */
const MARKET_INTERVAL_MS = 20_000;
const BOOK_INTERVAL_MS = 20_000;

// The settlement token and operator never change for a contract, so they are read once per page load.
type Config = { token: string; owner: string };
const configs = new Map<string, Promise<Config>>();
function configOf(chain: ChainInfo): Promise<Config> {
  const key = `${chain.rpc_url}|${chain.auction_contract}`;
  let config = configs.get(key);
  if (!config) {
    config = view<Config>(chain, chain.auction_contract, "get_config");
    config.catch(() => configs.delete(key));
    configs.set(key, config);
  }
  return config;
}

/**
 * Public auctions plus one account's bids, entitlements and balances, polled once for the whole
 * page however many components read it (Market, Account, sidebar).
 */
export function useChainSnapshot(chain: ChainInfo | null, account: string | null) {
  const contract = chain?.auction_contract ?? "";
  const load = useCallback(async (signal: AbortSignal): Promise<Snapshot> => {
    if (!chain) return EMPTY;
    const read = <T,>(target: string, method: string, args: object = {}) => view<T>(chain, target, method, args, signal);
    const [{ token, owner: operator }, auctions] = await Promise.all([configOf(chain), read<ChainAuction[]>(contract, "list_auctions", { limit: 50 })]);
    if (!account) return { ...EMPTY, token, operator, auctions, loaded: true };
    const owner = { account_id: account };
    const [bids, entitlements, unlocked, wallet] = await Promise.all([
      read<ChainBid[]>(contract, "bids_of", owner), read<ChainEntitlement[]>(contract, "entitlements_of", owner),
      read<string>(contract, "balance_of", owner), read<string>(token, "ft_balance_of", owner),
    ]);
    return { token, operator, auctions, bids, entitlements, unlocked: Number(unlocked), wallet: Number(wallet), loaded: true };
  }, [chain, contract, account]);
  return usePolled(chain ? `market|${chain.rpc_url}|${contract}|${account ?? ""}` : null, load, MARKET_INTERVAL_MS);
}

// The NEAR auction contract as seen by the connected wallet: public auctions plus the wallet's
// own bids, entitlements and balances, and the transactions the Market and Account pages send.
export function useChainMarket(chain: ChainInfo | null) {
  const { walletAccount, call } = useWallet();
  const contract = chain?.auction_contract ?? "";
  const polled = useChainSnapshot(chain, walletAccount);
  const data = polled.data ?? EMPTY;

  /** Escrows units × days × maxPrice. Funds already unlocked in the contract are used first. */
  const placeBid = useCallback(async (auction: ChainAuction, units: number, maxPrice: number) => {
    const escrow = units * auction.term_days * maxPrice;
    const bid = { auction_id: auction.id, units, max_price: String(maxPrice) };
    if (data.unlocked >= escrow) await call(contract, { method: "place_bid", args: bid, ...PLAIN_CALL });
    else await call(data.token, { method: "ft_transfer_call", args: { receiver_id: contract, amount: String(escrow), msg: JSON.stringify(bid) }, ...TOKEN_CALL });
  }, [call, contract, data.token, data.unlocked]);
  const cancelBid = useCallback((bid: ChainBid) => call(contract, { method: "cancel_bid", args: { auction_id: bid.auction_id, index: bid.index }, ...PLAIN_CALL }), [call, contract]);
  const settle = useCallback((id: number) => call(contract, { method: "clear_auction", args: { auction_id: id }, ...PLAIN_CALL }), [call, contract]);
  /** Operator only: ends bidding now and settles in the same transaction. */
  const closeEarly = useCallback((id: number) => call(contract, { method: "close_auction", args: { auction_id: id }, ...PLAIN_CALL }), [call, contract]);
  const withdraw = useCallback(() => call(contract, { method: "withdraw", args: {}, ...PLAIN_CALL }), [call, contract]);
  const faucet = useCallback(() => call(data.token, { method: "mint", args: { account_id: walletAccount, amount: String(FAUCET_AMOUNT) }, ...PLAIN_CALL }), [call, data.token, walletAccount]);

  return { ...data, contract, error: polled.error, updatedAt: polled.updatedAt, refresh: polled.refresh, placeBid, cancelBid, settle, closeEarly, withdraw, faucet };
}

/** Every bid on one auction, read from the contract and shared like the market snapshot. */
export function useAuctionBids(chain: ChainInfo | null, auctionId: number | null) {
  const load = useCallback(
    (signal: AbortSignal) => (chain && auctionId != null ? view<ChainBid[]>(chain, chain.auction_contract, "get_bids", { auction_id: auctionId }, signal) : Promise.resolve([] as ChainBid[])),
    [chain, auctionId],
  );
  const key = chain && auctionId != null ? `bids|${chain.rpc_url}|${chain.auction_contract}|${auctionId}` : null;
  const polled = usePolled(key, load, BOOK_INTERVAL_MS);
  return { bids: polled.data ?? [], loaded: polled.data !== undefined, refresh: polled.refresh };
}
