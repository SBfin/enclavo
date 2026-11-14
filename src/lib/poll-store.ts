// Shared polling loops, one per key. Every subscriber to a key shares a single loop and its last
// result. A loop backs off while reads fail, pauses while the page is hidden, keeps the last good
// data, and stops when its last subscriber leaves. Framework-free so Node tests can drive it.

export type Polled<T> = {
  data: T | undefined;
  /** Why the latest read failed, or null after a success. */
  error: string | null;
  /** When data was last read successfully (ms since epoch), or 0. */
  updatedAt: number;
  failures: number;
};

export type Load = (signal: AbortSignal) => Promise<unknown>;

export type PollerOptions = {
  /** Wait between reads, given the normal interval and the consecutive failures so far. */
  delay: (intervalMs: number, failures: number) => number;
  /** The user-facing reason for a failed read. */
  describe: (error: unknown) => string;
  /** True while the page is hidden; loops don't read then. */
  hidden: () => boolean;
  now?: () => number;
};

type Entry = {
  state: Polled<unknown>;
  listeners: Set<() => void>;
  load: Load;
  intervalMs: number;
  /** When the latest read finished, or 0. The next read is due a delay after it. */
  settledAt: number;
  timer?: ReturnType<typeof setTimeout>;
  controller?: AbortController;
  inflight?: Promise<void>;
};

export const EMPTY: Polled<never> = { data: undefined, error: null, updatedAt: 0, failures: 0 };

export function createPoller({ delay, describe, hidden, now = Date.now }: PollerOptions) {
  const entries = new Map<string, Entry>();

  /** Arms the next read for when it is due, or not at all while unused or hidden. */
  function schedule(key: string, entry: Entry) {
    clearTimeout(entry.timer);
    entry.timer = undefined;
    if (!entry.listeners.size || hidden()) return;
    const wait = Math.max(0, entry.settledAt + delay(entry.intervalMs, entry.state.failures) - now());
    entry.timer = setTimeout(() => void run(key), wait);
  }

  function run(key: string): Promise<void> {
    const entry = entries.get(key);
    if (!entry) return Promise.resolve();
    // Join a read in progress, unless it was cancelled when its last subscriber left: a cancelled
    // read publishes nothing and schedules nothing, so joining it would stall the loop.
    if (entry.inflight && !entry.controller?.signal.aborted) return entry.inflight;
    clearTimeout(entry.timer);
    entry.timer = undefined;
    const controller = new AbortController();
    entry.controller = controller;
    const read: Promise<void> = entry.load(controller.signal).then(
      (data) => {
        if (!controller.signal.aborted) entry.state = { data, error: null, updatedAt: now(), failures: 0 };
      },
      (error) => {
        if (!controller.signal.aborted) entry.state = { ...entry.state, error: describe(error), failures: entry.state.failures + 1 };
      },
    ).then(() => {
      if (entry.inflight === read) entry.inflight = undefined;
      if (controller.signal.aborted) return;
      entry.settledAt = now();
      entry.listeners.forEach((notify) => notify());
      schedule(key, entry);
    });
    entry.inflight = read;
    return read;
  }

  return {
    state: (key: string): Polled<unknown> => entries.get(key)?.state ?? EMPTY,

    /** Joins the loop for `key`, starting it if this is the first subscriber. Returns the unsubscribe. */
    subscribe(key: string, load: Load, intervalMs: number, notify: () => void): () => void {
      let entry = entries.get(key);
      if (!entry) entries.set(key, (entry = { state: EMPTY, listeners: new Set(), load, intervalMs, settledAt: 0 }));
      entry.load = load;
      entry.intervalMs = intervalMs;
      entry.listeners.add(notify);
      if (entry.listeners.size === 1) schedule(key, entry);
      const joined = entry;
      return () => {
        joined.listeners.delete(notify);
        if (joined.listeners.size) return;
        clearTimeout(joined.timer);
        joined.timer = undefined;
        joined.controller?.abort();
      };
    },

    /** Reads `key` again now, after any read already in progress. */
    async refresh(key: string): Promise<void> {
      const inflight = entries.get(key)?.inflight;
      if (inflight) await inflight;
      await run(key);
    },

    /** Reads every key in use again now, e.g. after a wallet transaction. */
    refreshAll() {
      for (const [key, entry] of entries) if (entry.listeners.size) void run(key);
    },

    /** Pauses loops while the page is hidden; on return, reads at once whatever is overdue. */
    visibilityChanged() {
      for (const [key, entry] of entries) if (entry.listeners.size) schedule(key, entry);
    },
  };
}
