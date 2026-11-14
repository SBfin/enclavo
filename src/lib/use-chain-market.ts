"use client";
import { useCallback, useEffect, useState } from "react";
import { useWallet } from "@/components/WalletProvider";
import { PLAIN_CALL, TOKEN_CALL, USD, view, type ChainAuction, type ChainBid, type ChainEntitlement, type ChainInfo } from "@/lib/near";

export const FAUCET_AMOUNT = 1_000 * USD;

type Snapshot = {
  token: string;
  auctions: ChainAuction[];
  bids: ChainBid[];
  entitlements: ChainEntitlement[];
  /** Escrow released to the user inside the contract, in token atomic units (micro-USD). */
  unlocked: number;
  /** Stablecoin in the user's wallet, in token atomic units (micro-USD). */
  wallet: number;
  loaded: boolean;
};
const EMPTY: Snapshot = { token: "", auctions: [], bids: [], entitlements: [], unlocked: 0, wallet: 0, loaded: false };

// The NEAR auction contract as seen by the connected wallet: public auctions plus the wallet's
// own bids, entitlements and balances, and the transactions the Market and Account pages send.
export function useChainMarket(chain: ChainInfo | null) {
  const { walletAccount, call } = useWallet();
  const contract = chain?.auction_contract ?? "";
  const [data, setData] = useState<Snapshot>(EMPTY);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async (signal?: AbortSignal): Promise<Snapshot> => {
    if (!chain) return EMPTY;
    const read = <T,>(target: string, method: string, args: object = {}) => view<T>(chain, target, method, args, signal);
    const [config, auctions] = await Promise.all([read<{ token: string }>(contract, "get_config"), read<ChainAuction[]>(contract, "list_auctions", { limit: 50 })]);
    if (!walletAccount) return { ...EMPTY, token: config.token, auctions, loaded: true };
    const owner = { account_id: walletAccount };
    const [bids, entitlements, unlocked, wallet] = await Promise.all([
      read<ChainBid[]>(contract, "bids_of", owner), read<ChainEntitlement[]>(contract, "entitlements_of", owner),
      read<string>(contract, "balance_of", owner), read<string>(config.token, "ft_balance_of", owner),
    ]);
    return { token: config.token, auctions, bids, entitlements, unlocked: Number(unlocked), wallet: Number(wallet), loaded: true };
  }, [chain, contract, walletAccount]);

  const refresh = useCallback(async () => {
    try { setData(await load()); setError(null); }
    catch (cause) { setError(cause instanceof Error ? cause.message : "Could not read the auction contract."); }
  }, [load]);

  useEffect(() => {
    const controller = new AbortController();
    const update = () => {
      void load(controller.signal)
        .then((result) => { if (!controller.signal.aborted) { setData(result); setError(null); } })
        .catch((cause) => { if (!controller.signal.aborted) setError(cause.message); });
    };
    update();
    const timer = setInterval(update, 8_000);
    return () => { controller.abort(); clearInterval(timer); };
  }, [load]);

  /** Escrows units × days × maxPrice. Funds already unlocked in the contract are used first. */
  const placeBid = useCallback(async (auction: ChainAuction, units: number, maxPrice: number) => {
    const escrow = units * auction.term_days * maxPrice;
    const bid = { auction_id: auction.id, units, max_price: String(maxPrice) };
    if (data.unlocked >= escrow) await call(contract, { method: "place_bid", args: bid, ...PLAIN_CALL });
    else await call(data.token, { method: "ft_transfer_call", args: { receiver_id: contract, amount: String(escrow), msg: JSON.stringify(bid) }, ...TOKEN_CALL });
  }, [call, contract, data.token, data.unlocked]);
  const cancelBid = useCallback((bid: ChainBid) => call(contract, { method: "cancel_bid", args: { auction_id: bid.auction_id, index: bid.index }, ...PLAIN_CALL }), [call, contract]);
  const settle = useCallback((id: number) => call(contract, { method: "clear_auction", args: { auction_id: id }, ...PLAIN_CALL }), [call, contract]);
  const withdraw = useCallback(() => call(contract, { method: "withdraw", args: {}, ...PLAIN_CALL }), [call, contract]);
  const faucet = useCallback(() => call(data.token, { method: "mint", args: { account_id: walletAccount, amount: String(FAUCET_AMOUNT) }, ...PLAIN_CALL }), [call, data.token, walletAccount]);

  return { ...data, contract, error, refresh, placeBid, cancelBid, settle, withdraw, faucet };
}
