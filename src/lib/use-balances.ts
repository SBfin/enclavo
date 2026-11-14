"use client";
import { useCallback, useEffect, useState } from "react";
import { useAccount } from "@/components/AccountProvider";
import { BALANCES_EVENT, useWallet } from "@/components/WalletProvider";
import { view, type ChainBid, type ChainInfo } from "@/lib/near";
import type { CapacityContract } from "@/lib/use-market";

export type Balances = {
  /** Token atomic units (micro-USD). */
  wallet: number;
  unlocked: number;
  escrow: number;
  /** Today's token allowance across active capacity. */
  tokensLeft: number;
  tokensDaily: number;
};

// What the signed-in user can spend: stablecoin in the wallet, funds unlocked in the auction
// contract, escrow held by open bids, and today's remaining capacity on the gateway.
export function useBalances() {
  const { session } = useWallet();
  const { account, api } = useAccount();
  const [state, setState] = useState<{ owner: string | null; balances: Balances | null }>({ owner: null, balances: null });

  const load = useCallback(async (signal?: AbortSignal): Promise<Balances | null> => {
    if (!session) return null;
    const status = await fetch("/api/v1/status", { cache: "no-store", signal }).then((r) => r.json());
    const chain: ChainInfo | null = status.chain ?? null;
    const owner = { account_id: session };
    const capacity = api("market/contracts", { signal }).then((r) => (r.ok ? r.json() : { contracts: [] })).then((b) => (b.contracts ?? []) as CapacityContract[]).catch(() => []);
    let wallet = 0, unlocked = 0, escrow = 0;
    if (chain) {
      const read = <T,>(target: string, method: string, args: object = {}) => view<T>(chain, target, method, args, signal);
      const { token } = await read<{ token: string }>(chain.auction_contract, "get_config");
      const [w, u, bids] = await Promise.all([read<string>(token, "ft_balance_of", owner), read<string>(chain.auction_contract, "balance_of", owner), read<ChainBid[]>(chain.auction_contract, "bids_of", owner)]);
      wallet = Number(w); unlocked = Number(u);
      escrow = bids.filter((b) => b.status === "open").reduce((sum, b) => sum + Number(b.escrow), 0);
    }
    const active = (await capacity).filter((c) => c.active);
    return { wallet, unlocked, escrow, tokensLeft: active.reduce((s, c) => s + c.remaining_tokens, 0), tokensDaily: active.reduce((s, c) => s + c.daily_tokens, 0) };
  }, [session, api]);

  useEffect(() => {
    const controller = new AbortController();
    const update = () => { void load(controller.signal).then((balances) => { if (!controller.signal.aborted) setState({ owner: session, balances }); }).catch(() => { /* keep the last reading; the next poll retries */ }); };
    update();
    const timer = setInterval(update, 15_000);
    window.addEventListener(BALANCES_EVENT, update);
    return () => { controller.abort(); clearInterval(timer); window.removeEventListener(BALANCES_EVENT, update); };
  }, [load, session]);

  return { balances: state.owner === session ? state.balances : null, credit: account?.balance_micro_usd ?? 0 };
}
