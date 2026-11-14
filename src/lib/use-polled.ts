"use client";
// One shared poller per key. Every component asking for the same key shares a single loop and
// its last result, so the Market page, the Account page and the sidebar don't each hit the RPC.
// A loop backs off while reads fail, pauses while the tab is hidden, and keeps the last good
// data on screen; it stops when nothing on the page uses it.
import { useCallback, useSyncExternalStore } from "react";
import { BALANCES_EVENT } from "@/components/WalletProvider";
import { describeError, nextDelay } from "@/lib/poll-policy";

export type Polled<T> = {
  data: T | undefined;
  /** Why the latest read failed, or null after a success. */
  error: string | null;
  /** When data was last read successfully (ms since epoch), or 0. */
  updatedAt: number;
  failures: number;
};

type Entry = {
  state: Polled<unknown>;
  listeners: Set<() => void>;
  load: (signal: AbortSignal) => Promise<unknown>;
  intervalMs: number;
  timer?: ReturnType<typeof setTimeout>;
  controller?: AbortController;
  inflight?: Promise<void>;
};

const EMPTY: Polled<never> = { data: undefined, error: null, updatedAt: 0, failures: 0 };
const entries = new Map<string, Entry>();

function schedule(entry: Entry, key: string) {
  clearTimeout(entry.timer);
  if (!entry.listeners.size || document.hidden) return;
  entry.timer = setTimeout(() => void run(key), nextDelay(entry.intervalMs, entry.state.failures));
}

function run(key: string): Promise<void> {
  const entry = entries.get(key);
  if (!entry) return Promise.resolve();
  if (entry.inflight) return entry.inflight;
  clearTimeout(entry.timer);
  const controller = new AbortController();
  entry.controller = controller;
  entry.inflight = entry.load(controller.signal).then(
    (data) => { entry.state = { data, error: null, updatedAt: Date.now(), failures: 0 }; },
    (error) => {
      // Stopped because nothing uses this key any more: not a failure.
      if (!controller.signal.aborted) entry.state = { ...entry.state, error: describeError(error), failures: entry.state.failures + 1 };
    },
  ).then(() => {
    entry.inflight = undefined;
    if (controller.signal.aborted) return;
    entry.listeners.forEach((notify) => notify());
    schedule(entry, key);
  });
  return entry.inflight;
}

function due(entry: Entry) {
  return Date.now() - entry.state.updatedAt >= nextDelay(entry.intervalMs, entry.state.failures);
}

function subscribe(key: string, load: Entry["load"], intervalMs: number, notify: () => void) {
  let entry = entries.get(key);
  if (!entry) entries.set(key, (entry = { state: EMPTY, listeners: new Set(), load, intervalMs }));
  entry.load = load;
  entry.intervalMs = intervalMs;
  entry.listeners.add(notify);
  if (entry.listeners.size === 1) {
    if (due(entry)) void run(key);
    else schedule(entry, key);
  }
  return () => {
    entry.listeners.delete(notify);
    if (entry.listeners.size) return;
    clearTimeout(entry.timer);
    entry.controller?.abort();
  };
}

/** Reads every active key again now, e.g. after a wallet transaction. */
function refreshAll() {
  for (const [key, entry] of entries) if (entry.listeners.size) void run(key);
}

if (typeof window !== "undefined") {
  document.addEventListener("visibilitychange", () => {
    for (const [key, entry] of entries) {
      if (!entry.listeners.size) continue;
      if (document.hidden) clearTimeout(entry.timer);
      else if (due(entry)) void run(key);
      else schedule(entry, key);
    }
  });
  window.addEventListener(BALANCES_EVENT, refreshAll);
}

/**
 * Shared, polled data for `key` (null disables it). `load` must be stable for a given key,
 * e.g. from useCallback over the same values the key is built from.
 */
export function usePolled<T>(key: string | null, load: (signal: AbortSignal) => Promise<T>, intervalMs: number) {
  const subscribeKey = useCallback(
    (notify: () => void) => (key ? subscribe(key, load, intervalMs, notify) : () => {}),
    [key, load, intervalMs],
  );
  const state = useSyncExternalStore(
    subscribeKey,
    () => (key ? (entries.get(key)?.state ?? EMPTY) : EMPTY),
    () => EMPTY,
  ) as Polled<T>;
  const refresh = useCallback(async () => {
    if (!key) return;
    const entry = entries.get(key);
    if (entry?.inflight) await entry.inflight;
    await run(key);
  }, [key]);
  return { ...state, refresh };
}
