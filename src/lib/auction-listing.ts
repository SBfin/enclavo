// Filtering and sorting for the Market page's auction listing. Pure, so it can be tested
// without a browser; `now` is milliseconds since the epoch, or 0 before the page has a clock.
import type { ChainAuction } from "./near";

/** Where an auction is in its life: taking bids, closed but not yet settled, or settled. */
export type Phase = "open" | "closed" | "settled";
export type StatusFilter = Phase | "all";
export type SortKey = "closing" | "newest" | "reserve-asc" | "reserve-desc" | "lot" | "demand";

export type Filters = {
  status: StatusFilter;
  /** Case-insensitive text matched against the model name and route id. */
  query: string;
  /** Exact route id, or "" for any model. */
  route: string;
  /** Term in days, or 0 for any term. */
  term: number;
  /** Only auctions where demand has not yet covered (or did not cover) the lot. */
  unclaimed: boolean;
  sort: SortKey;
};

export const DEFAULT_FILTERS: Filters = { status: "open", query: "", route: "", term: 0, unclaimed: false, sort: "closing" };

export const SORTS: { key: SortKey; label: string }[] = [
  { key: "closing", label: "Closing soonest" },
  { key: "newest", label: "Newest" },
  { key: "reserve-asc", label: "Reserve: low to high" },
  { key: "reserve-desc", label: "Reserve: high to low" },
  { key: "lot", label: "Largest lot" },
  { key: "demand", label: "Most demand" },
];

export function phase(auction: ChainAuction, now: number): Phase {
  if (auction.status === "cleared") return "settled";
  return now > 0 && now >= auction.ends_at_ms ? "closed" : "open";
}

export function countByStatus(auctions: ChainAuction[], now: number): Record<StatusFilter, number> {
  const counts: Record<StatusFilter, number> = { open: 0, closed: 0, settled: 0, all: auctions.length };
  for (const auction of auctions) counts[phase(auction, now)] += 1;
  return counts;
}

/** Share of the lot bid for (open) or sold (settled), from 0 up; above 1 is oversubscribed. */
export function demandRatio(auction: ChainAuction): number {
  const taken = auction.status === "cleared" ? auction.filled_units : auction.open_bid_units;
  return auction.units > 0 ? taken / auction.units : 0;
}

/** True when any filter other than status and sort narrows the list. */
export function narrowed(filters: Filters): boolean {
  return filters.query.trim() !== "" || filters.route !== "" || filters.term !== 0 || filters.unclaimed;
}

const PHASE_ORDER: Record<Phase, number> = { open: 0, closed: 1, settled: 2 };

export function filterAuctions(auctions: ChainAuction[], filters: Filters, now: number, nameOf: (routeId: string) => string): ChainAuction[] {
  const query = filters.query.trim().toLowerCase();
  const matches = auctions.filter((a) => {
    if (filters.status !== "all" && phase(a, now) !== filters.status) return false;
    if (filters.route && a.route_id !== filters.route) return false;
    if (filters.term && a.term_days !== filters.term) return false;
    if (filters.unclaimed && demandRatio(a) >= 1) return false;
    return !query || nameOf(a.route_id).toLowerCase().includes(query) || a.route_id.toLowerCase().includes(query);
  });
  const compare = comparator(filters.sort, now);
  return matches.sort((a, b) => compare(a, b) || b.id - a.id);
}

function comparator(sort: SortKey, now: number): (a: ChainAuction, b: ChainAuction) => number {
  switch (sort) {
    case "newest": return (a, b) => b.created_at_ms - a.created_at_ms;
    case "reserve-asc": return (a, b) => Number(a.reserve_price) - Number(b.reserve_price);
    case "reserve-desc": return (a, b) => Number(b.reserve_price) - Number(a.reserve_price);
    case "lot": return (a, b) => b.units * b.tokens_per_iu - a.units * a.tokens_per_iu;
    case "demand": return (a, b) => demandRatio(b) - demandRatio(a);
    case "closing":
      // Still open first by close time, then those waiting to settle, then the most recently settled.
      return (a, b) => {
        const pa = phase(a, now), pb = phase(b, now);
        if (pa !== pb) return PHASE_ORDER[pa] - PHASE_ORDER[pb];
        return pa === "settled" ? (b.cleared_at_ms ?? 0) - (a.cleared_at_ms ?? 0) : a.ends_at_ms - b.ends_at_ms;
      };
  }
}

export function timeLeft(endsAtMs: number, now: number): string {
  if (!now) return "…";
  const ms = endsAtMs - now;
  if (ms <= 0) return "closed";
  const s = Math.floor(ms / 1000), m = Math.floor(s / 60), h = Math.floor(m / 60), d = Math.floor(h / 24);
  if (d >= 1) return `${d}d ${h % 24}h`;
  if (h >= 1) return `${h}h ${m % 60}m`;
  if (m >= 1) return `${m}m ${s % 60}s`;
  return `${s}s`;
}
