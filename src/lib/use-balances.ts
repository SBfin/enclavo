"use client";
import { useCallback } from "react";
import { useAccount } from "@/components/AccountProvider";
import { useWallet } from "@/components/WalletProvider";
import type { ChainInfo } from "@/lib/near";
import { useChainSnapshot } from "@/lib/use-chain-market";
import type { CapacityContract } from "@/lib/use-market";
import { usePolled } from "@/lib/use-polled";

export type Balances = {
  /** Token atomic units (micro-USD). */
  wallet: number;
  unlocked: number;
  escrow: number;
  /** Today's token allowance across active capacity. */
  tokensLeft: number;
  tokensDaily: number;
};

const loadStatus = (signal: AbortSignal): Promise<{ chain?: ChainInfo | null }> =>
  fetch("/api/v1/status", { cache: "no-store", signal }).then((response) => response.json());

// What the signed-in user can spend: stablecoin in the wallet, funds unlocked in the auction
// contract, escrow held by open bids, and today's remaining capacity on the gateway. The chain
// part is the same shared snapshot the Market and Account pages read, so it costs no extra RPC calls.
export function useBalances() {
  const { session } = useWallet();
  const { account, api } = useAccount();
  // Which contract and RPC to read barely ever changes.
  const status = usePolled(session ? "gateway-status" : null, loadStatus, 300_000);
  const chain = status.data?.chain ?? null;
  const snapshot = useChainSnapshot(session ? chain : null, session);
  const loadCapacity = useCallback(
    (signal: AbortSignal) => api("market/contracts", { signal })
      .then((r) => (r.ok ? r.json() : { contracts: [] }))
      .then((body) => (body.contracts ?? []) as CapacityContract[]),
    [api],
  );
  const capacity = usePolled(session ? `capacity|${session}` : null, loadCapacity, 15_000);

  const contracts = capacity.data ?? (capacity.error ? [] : undefined);
  const chainReady = status.data !== undefined && (!chain || snapshot.data !== undefined);
  let balances: Balances | null = null;
  if (session && chainReady && contracts) {
    const snap = snapshot.data;
    const active = contracts.filter((c) => c.active);
    balances = {
      wallet: snap?.wallet ?? 0,
      unlocked: snap?.unlocked ?? 0,
      escrow: (snap?.bids ?? []).filter((b) => b.status === "open").reduce((sum, b) => sum + Number(b.escrow), 0),
      tokensLeft: active.reduce((sum, c) => sum + c.remaining_tokens, 0),
      tokensDaily: active.reduce((sum, c) => sum + c.daily_tokens, 0),
    };
  }
  return { balances, credit: account?.balance_micro_usd ?? 0 };
}
