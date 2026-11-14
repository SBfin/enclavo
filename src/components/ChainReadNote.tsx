"use client";
import { ago } from "@/lib/poll-policy";
import { useNow } from "@/lib/use-now";

/** A quiet note while chain reads fail: the last good data stays on screen and reads retry on their own. */
export default function ChainReadNote({ error, loaded, updatedAt }: { error: string | null; loaded: boolean; updatedAt: number }) {
  const now = useNow();
  if (!error) return null;
  return <p role="status" className="mb-5 border border-line px-4 py-3 text-xs text-muted">
    {error} {loaded ? `Showing chain data from ${ago(updatedAt, now || updatedAt)}; ` : ""}retrying automatically.
  </p>;
}
