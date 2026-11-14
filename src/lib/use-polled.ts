"use client";
// React access to the shared pollers in poll-store: one loop per key for the whole page, so the
// Market page, the Account page and the sidebar don't each hit the RPC.
import { useCallback, useSyncExternalStore } from "react";
import { BALANCES_EVENT } from "@/components/WalletProvider";
import { describeError, nextDelay } from "@/lib/poll-policy";
import { EMPTY, createPoller, type Polled } from "@/lib/poll-store";

export type { Polled } from "@/lib/poll-store";

const poller = createPoller({ delay: nextDelay, describe: describeError, hidden: () => document.hidden });

if (typeof window !== "undefined") {
  document.addEventListener("visibilitychange", poller.visibilityChanged);
  window.addEventListener(BALANCES_EVENT, poller.refreshAll);
}

/**
 * Shared, polled data for `key` (null disables it). `load` must be stable for a given key,
 * e.g. from useCallback over the same values the key is built from.
 */
export function usePolled<T>(key: string | null, load: (signal: AbortSignal) => Promise<T>, intervalMs: number) {
  const subscribe = useCallback(
    (notify: () => void) => (key ? poller.subscribe(key, load, intervalMs, notify) : () => {}),
    [key, load, intervalMs],
  );
  const state = useSyncExternalStore(subscribe, () => (key ? poller.state(key) : EMPTY), () => EMPTY) as Polled<T>;
  const refresh = useCallback(() => (key ? poller.refresh(key) : Promise.resolve()), [key]);
  return { ...state, refresh };
}
