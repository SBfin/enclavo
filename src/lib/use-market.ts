"use client";
import { useCallback, useEffect, useState } from "react";
import { useAccount } from "@/components/AccountProvider";

export type Auction = { id: string; route_id: string; units: number; term_days: number; reserve_price_micro_usd: number; ends_at: string; status: string; accepting: boolean; bid_units: number; bid_count: number; filled_units: number; clearing_price_micro_usd: number | null };
export type Bid = { id: string; auction_id: string; route_id: string; units: number; term_days: number; max_price_micro_usd: number; status: string; filled_units: number; charged_micro_usd: number; ends_at: string };
export type CapacityContract = { id: string; auction_id: string; route_id: string; units: number; starts_at: string; ends_at: string; active: boolean; paid_micro_usd: number; daily_tokens: number; used_tokens: number; held_tokens: number; remaining_tokens: number };

export function useMarket() {
  const { api, signedIn } = useAccount();
  const [data, setData] = useState<{ auctions: Auction[]; bids: Bid[]; contracts: CapacityContract[]; error: string | null }>({ auctions: [], bids: [], contracts: [], error: null });
  const load = useCallback(async (signal?: AbortSignal) => {
    const read = async (response: Response) => { const body = await response.json(); if (!response.ok) throw new Error(body.error?.message || "Could not load the market."); return body; };
    const auctions = await fetch("/api/v1/market/auctions", { cache: "no-store", signal }).then(read);
    let bids: Bid[] = [], contracts: CapacityContract[] = [];
    if (signedIn) {
      const [orders, capacity] = await Promise.all([api("market/bids", { signal }).then(read), api("market/contracts", { signal }).then(read)]);
      bids = orders.bids; contracts = capacity.contracts;
    }
    return { auctions: auctions.auctions as Auction[], bids, contracts, error: null };
  }, [api, signedIn]);
  const refresh = useCallback(async () => { try { setData(await load()); } catch (error) { setData((old) => ({ ...old, error: error instanceof Error ? error.message : "Could not load the market." })); } }, [load]);
  useEffect(() => {
    const controller = new AbortController();
    const update = () => { void load(controller.signal).then((result) => { if (!controller.signal.aborted) setData(result); }).catch((error) => { if (!controller.signal.aborted) setData((old) => ({ ...old, error: error.message })); }); };
    update(); const timer = setInterval(update, 10_000);
    return () => { controller.abort(); clearInterval(timer); };
  }, [load]);
  return { ...data, refresh };
}
