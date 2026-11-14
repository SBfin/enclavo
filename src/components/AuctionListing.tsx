"use client";
import Link from "next/link";
import type { ReactNode } from "react";
import { DEFAULT_FILTERS, SORTS, demandRatio, narrowed, phase, timeLeft, type Filters, type StatusFilter } from "@/lib/auction-listing";
import { fmtTokens, fmtUSD } from "@/lib/market";
import { usd, type ChainAuction } from "@/lib/near";

const STATUSES: { key: StatusFilter; label: string }[] = [
  { key: "open", label: "Open" },
  { key: "closed", label: "Ready to settle" },
  { key: "settled", label: "Settled" },
  { key: "all", label: "All" },
];

const EMPTY: Record<StatusFilter, string> = {
  open: "No auctions are open right now. New lots of capacity are published on chain; check back soon.",
  closed: "Nothing is waiting to be settled.",
  settled: "No auction has been settled yet.",
  all: "No auctions have been published yet.",
};

const COLUMNS = "md:grid-cols-[minmax(0,1.5fr)_minmax(0,1fr)_minmax(0,0.75fr)_minmax(0,1.2fr)_minmax(0,1.15fr)]";
const control = "w-full rounded-md border border-line bg-background px-3 py-2 text-sm text-foreground";

type Props = {
  /** Every auction loaded from the contract, before filtering. */
  auctions: ChainAuction[];
  /** The auctions to show, already filtered and sorted. */
  listed: ChainAuction[];
  counts: Record<StatusFilter, number>;
  now: number;
  loaded: boolean;
  nameOf: (routeId: string) => string;
  filters: Filters;
  onFilters: (filters: Filters) => void;
  selectedId: number | null;
  onSelect: (id: number) => void;
  /** Shown under the selected open auction: its order book. */
  expanded: ReactNode;
  onSettle: (id: number) => void;
  canSettle: boolean;
  /** Ends bidding early and settles at once. Only the operator's wallet can do this. */
  onClose: (id: number) => void;
  canClose: boolean;
  busy: boolean;
};

export default function AuctionListing({ auctions, listed, counts, now, loaded, nameOf, filters, onFilters, selectedId, onSelect, expanded, onSettle, canSettle, onClose, canClose, busy }: Props) {
  const set = (patch: Partial<Filters>) => onFilters({ ...filters, ...patch });
  const clear = () => onFilters({ ...DEFAULT_FILTERS, status: filters.status, sort: filters.sort });
  const routes = [...new Set(auctions.map((a) => a.route_id))].sort((a, b) => nameOf(a).localeCompare(nameOf(b)));
  const terms = [...new Set(auctions.map((a) => a.term_days))].sort((a, b) => a - b);
  const filtered = narrowed(filters);

  return <section aria-labelledby="auctions-heading" className="min-w-0">
    <h2 id="auctions-heading" className="sr-only">Auctions</h2>
    <div role="group" aria-label="Auction status" className="flex flex-wrap gap-2">
      {STATUSES.map(({ key, label }) => {
        const pressed = filters.status === key;
        return <button key={key} type="button" aria-pressed={pressed} onClick={() => set({ status: key })}
          className={`rounded-full border px-3 py-1.5 text-sm transition ${pressed ? "border-accent bg-accent/10 text-foreground" : "border-line text-muted hover:text-foreground"}`}>
          {label}<span className={`ml-2 font-mono text-xs tnum ${key === "closed" && counts.closed > 0 ? "text-warn" : ""}`}>{counts[key]}</span>
        </button>;
      })}
    </div>

    <div className="mt-3 grid grid-cols-2 gap-2 xl:grid-cols-[minmax(0,1.5fr)_minmax(0,1.2fr)_minmax(0,0.8fr)_minmax(0,1.1fr)]">
      <input type="search" aria-label="Search models" placeholder="Search models" value={filters.query} onChange={(event) => set({ query: event.target.value })} className={`${control} col-span-2 xl:col-span-1`} />
      <select aria-label="Model" value={filters.route} onChange={(event) => set({ route: event.target.value })} className={control}>
        <option value="">All models</option>
        {routes.map((route) => <option key={route} value={route}>{nameOf(route)}</option>)}
      </select>
      <select aria-label="Term" value={filters.term} onChange={(event) => set({ term: Number(event.target.value) })} className={control}>
        <option value={0}>Any term</option>
        {terms.map((days) => <option key={days} value={days}>{days} days</option>)}
      </select>
      <select aria-label="Sort by" value={filters.sort} onChange={(event) => set({ sort: event.target.value as Filters["sort"] })} className={`${control} col-span-2 xl:col-span-1`}>
        {SORTS.map(({ key, label }) => <option key={key} value={key}>{label}</option>)}
      </select>
    </div>
    <div className="mt-3 flex flex-wrap items-center justify-between gap-x-4 gap-y-2 text-xs text-muted">
      <label className="flex items-center gap-2">
        <input type="checkbox" checked={filters.unclaimed} onChange={(event) => set({ unclaimed: event.target.checked })} className="accent-[var(--accent)]" />
        Unclaimed capacity only
      </label>
      <span className="flex flex-wrap items-center gap-x-4 gap-y-1">
        <span className="tnum" aria-live="polite">{listed.length} of {auctions.length} {auctions.length === 1 ? "auction" : "auctions"}</span>
        {filtered && <button type="button" onClick={clear} className="text-accent">Clear filters</button>}
        <Link href="/dashboard/how-it-works" className="text-accent">How auctions work →</Link>
      </span>
    </div>

    <div className="mt-3 border border-line">
      <div className={`hidden gap-4 border-b border-line px-4 py-2 md:grid ${COLUMNS}`}>
        {["Model", "Lot", "Reserve", "Demand", "Status"].map((label) => <span key={label} className="label">{label}</span>)}
      </div>
      {!loaded && !auctions.length ? <p className="p-5 text-sm text-muted">Reading the auction contract…</p>
        : !listed.length ? <p className="p-5 text-sm text-muted">
          {filtered ? <>No auctions match these filters. <button type="button" onClick={clear} className="text-accent">Clear filters</button></> : EMPTY[filters.status]}
        </p>
        : <ul>{listed.map((a) => <AuctionRow key={a.id} auction={a} now={now} name={nameOf(a.route_id)} selected={a.id === selectedId && phase(a, now) === "open"}
          expanded={expanded} onSelect={onSelect} onSettle={onSettle} canSettle={canSettle} onClose={onClose} canClose={canClose} busy={busy} />)}</ul>}
    </div>
    {auctions.length >= 50 && <p className="mt-2 text-xs text-muted">Showing the 50 most recent auctions.</p>}
  </section>;
}

function AuctionRow({ auction: a, now, name, selected, expanded, onSelect, onSettle, canSettle, onClose, canClose, busy }: {
  auction: ChainAuction; now: number; name: string; selected: boolean; expanded: ReactNode;
  onSelect: (id: number) => void; onSettle: (id: number) => void; canSettle: boolean;
  onClose: (id: number) => void; canClose: boolean; busy: boolean;
}) {
  const state = phase(a, now);
  const ratio = demandRatio(a);
  const taken = state === "settled" ? a.filled_units : a.open_bid_units;
  const cells = <>
    <Cell label="Model">
      <div className="font-medium">{name}</div>
      <div className="text-xs text-muted">#{a.id} · {a.term_days}-day term</div>
    </Cell>
    <Cell label="Lot">
      <div className="whitespace-nowrap font-mono tnum">{a.units} IU/day</div>
      <div className="text-xs text-muted">{fmtTokens(a.units * a.tokens_per_iu)} tokens/day</div>
    </Cell>
    <Cell label="Reserve">
      <div className="font-mono tnum">{fmtUSD(usd(a.reserve_price), 2)}</div>
      <div className="text-xs text-muted">per IU-day</div>
    </Cell>
    <Cell label="Demand">
      <div className="h-1.5 overflow-hidden rounded-full bg-white/[0.08]"><div className={`h-full ${ratio > 1 ? "bg-warn" : "bg-good"}`} style={{ width: `${Math.min(ratio, 1) * 100}%` }} /></div>
      <div className="mt-1 text-xs text-muted tnum">
        {state === "settled" ? `${taken} / ${a.units} sold` : `${taken} / ${a.units} bid · ${a.open_bids} ${a.open_bids === 1 ? "bid" : "bids"}`}
      </div>
    </Cell>
    <Cell label="Status">
      {state === "open" && <div className="whitespace-nowrap text-sm">closes in <span className={`font-mono tnum ${a.ends_at_ms - now < 3_600_000 ? "text-warn" : ""}`}>{timeLeft(a.ends_at_ms, now)}</span></div>}
      {state === "closed" && <div className="text-sm text-warn">Closed · awaiting settlement</div>}
      {state === "settled" && <>
        <div className="text-sm">{a.clearing_price ? <>Cleared at <span className="font-mono tnum">{fmtUSD(usd(a.clearing_price), 2)}</span></> : <span className="text-muted">No winning bids</span>}</div>
        <div className="text-xs text-muted">{a.cleared_at_ms ? new Date(a.cleared_at_ms).toLocaleDateString() : ""}</div>
      </>}
    </Cell>
  </>;
  const grid = `grid gap-3 px-4 py-3 text-left max-md:grid-cols-2 md:items-center md:gap-4 ${COLUMNS}`;

  return <li className="border-b border-line last:border-0">
    {state === "open"
      ? <button type="button" aria-expanded={selected} onClick={() => onSelect(a.id)} className={`w-full transition ${grid} ${selected ? "bg-white/[0.05] shadow-[inset_2px_0_0_var(--accent)]" : "hover:bg-white/[0.03]"}`}>{cells}</button>
      : <div className={grid}>{cells}</div>}
    {state === "closed" && <div className="flex flex-wrap items-center justify-between gap-3 border-t border-line px-4 py-3 text-xs text-muted">
      <span>Settling runs the clearing on chain. Anyone with a connected wallet can do it, for a small gas fee.</span>
      <button type="button" disabled={busy || !canSettle} onClick={() => onSettle(a.id)} className="rounded-md bg-foreground px-4 py-2 text-sm font-medium text-background disabled:opacity-40">Settle now</button>
    </div>}
    {state === "open" && canClose && <div className="flex flex-wrap items-center justify-between gap-3 border-t border-line px-4 py-3 text-xs text-muted">
      <span>You are the operator. Closing ends bidding now and settles the auction in the same transaction.</span>
      <button type="button" disabled={busy} onClick={() => onClose(a.id)} className="rounded-md border border-line px-4 py-2 text-sm font-medium text-foreground disabled:opacity-40">Close and settle now</button>
    </div>}
    {selected && <div className="border-t border-line p-3">
      <p className={`mb-3 text-xs ${ratio > 1 ? "text-warn" : ratio > 0 ? "text-good" : "text-muted"}`}>{demandNote(a)}</p>
      {expanded}
    </div>}
  </li>;
}

function demandNote(a: ChainAuction): string {
  if (a.open_bid_units === 0) return "No bids yet. Any bid at or above the reserve would currently win and pay the reserve.";
  if (a.open_bid_units <= a.units) return `${a.units - a.open_bid_units} of ${a.units} IU/day still unclaimed. If it closed now, winners would pay the reserve.`;
  return `Oversubscribed: ${a.open_bid_units} IU/day bid for ${a.units}. Highest bids win, and everyone pays the lowest winning bid.`;
}

function Cell({ label, children }: { label: string; children: ReactNode }) {
  return <div className="min-w-0 first:max-md:col-span-2"><div className="label mb-1 md:hidden">{label}</div>{children}</div>;
}
