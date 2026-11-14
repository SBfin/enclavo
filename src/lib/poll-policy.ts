// How chain reads are retried and described. Pure, so it can be tested without a browser.

/** Longest wait between attempts while the RPC keeps failing. */
export const MAX_DELAY_MS = 120_000;

/** Wait before the next read: the normal interval, doubled after each consecutive failure. */
export function nextDelay(intervalMs: number, failures: number): number {
  if (failures <= 0) return intervalMs;
  return Math.min(intervalMs * 2 ** Math.min(failures, 16), MAX_DELAY_MS);
}

const UNREACHABLE = "The NEAR RPC isn't answering right now. It may be rate-limiting this network.";

/**
 * A short, user-facing reason for a failed read. Public RPCs answer rate limits (429) without
 * CORS headers, so the browser only reports an opaque network error; both mean "try later".
 */
export function describeError(error: unknown): string {
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  if (error instanceof TypeError || /failed to fetch|networkerror|load failed|\b429\b|too many requests/i.test(message)) return UNREACHABLE;
  return message || "Could not read the chain.";
}

/** "just now", "40s ago", "3 min ago", "2 h ago". */
export function ago(thenMs: number, nowMs: number): string {
  const s = Math.max(0, Math.round((nowMs - thenMs) / 1000));
  if (s < 5) return "just now";
  if (s < 60) return `${s}s ago`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  return `${Math.round(m / 60)} h ago`;
}
