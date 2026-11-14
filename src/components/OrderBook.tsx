"use client";
import { Fragment } from "react";
import { fmtUSD } from "@/lib/market";
import { usd } from "@/lib/near";
import type { Book } from "@/lib/order-book";

// One auction's bids as an order book: price levels from the highest maximum down, with a depth
// bar of cumulative demand against the lot size. Rows above the lot line win if it closed now.
export default function OrderBook({ book, lot, reserve, loaded }: { book: Book; lot: number; reserve: number; loaded: boolean }) {
  const scale = Math.max(book.demand, lot) || 1;
  const lotAt = `${(lot / scale) * 100}%`;
  const lastWinning = book.levels.findLastIndex((level) => level.filled > 0);
  const price = book.price == null ? null : fmtUSD(usd(book.price), 2);

  return <div className="border border-line bg-panel">
    <div className="flex flex-wrap items-baseline justify-between gap-2 border-b border-line px-4 py-3">
      <h3 className="text-sm font-medium">Order book</h3>
      <span className="text-xs text-muted">
        {!loaded ? "Reading bids…" : price ? <>{book.levels.some((l) => l.draft) ? "With your draft, if" : "If"} it closed now:<span className="text-foreground tnum">{book.filled} / {lot} IU/day</span> filled at <span className="font-mono text-foreground">{price}</span></> : "No bids yet"}
      </span>
    </div>

    {book.levels.length > 0 && <div className="overflow-x-auto">
      <table className="w-full min-w-[520px] text-left text-sm">
        <thead><tr className="border-b border-line">
          <th className="label px-4 py-2 font-normal">Max price</th>
          <th className="label px-4 py-2 text-right font-normal">IU/day</th>
          <th className="label w-2/5 px-4 py-2 font-normal">Depth <span className="normal-case tracking-normal">· lot {lot}</span></th>
          <th className="label px-4 py-2 font-normal">If it closed now</th>
        </tr></thead>
        <tbody>{book.levels.map((level, i) => {
          const won = level.filled > 0;
          const within = Math.min(level.cumulative, lot), beyond = level.cumulative - within;
          return <Fragment key={level.price}>
            <tr className={`border-b border-line ${level.draft ? "bg-accent/[0.08]" : ""}`}>
              <td className="px-4 py-2 font-mono tnum">
                {fmtUSD(usd(level.price), 2)}
                {level.mine && <span className="ml-2 rounded bg-white/[0.08] px-1.5 py-0.5 font-sans text-[10px] uppercase tracking-wide text-accent">you</span>}
                {level.draft && <span className="ml-2 rounded border border-dashed border-accent px-1.5 py-0.5 font-sans text-[10px] uppercase tracking-wide text-accent">draft</span>}
              </td>
              <td className="px-4 py-2 text-right font-mono tnum">{level.units}{level.bids > 1 && <span className="ml-1 font-sans text-xs text-muted">· {level.bids} bids</span>}</td>
              <td className="px-4 py-2">
                <div aria-hidden className="relative h-3 bg-white/[0.04]">
                  <div className="absolute inset-y-0 left-0 bg-good/70" style={{ width: `${(within / scale) * 100}%` }} />
                  {beyond > 0 && <div className="absolute inset-y-0 bg-warn/40" style={{ left: lotAt, width: `${(beyond / scale) * 100}%` }} />}
                  <div className="absolute -inset-y-1 w-px bg-foreground/70" style={{ left: lotAt }} />
                </div>
                <div className="mt-0.5 text-[11px] text-muted tnum">{level.cumulative} cumulative</div>
              </td>
              <td className={`px-4 py-2 ${won ? "text-good" : "text-muted"}`}>
                {!won ? "Outbid · refunded" : level.filled === level.units ? "Wins in full" : `Wins ${level.filled} of ${level.units}`}
              </td>
            </tr>
            {i === lastWinning && book.oversubscribed && <tr className="border-b border-line">
              <td colSpan={4} className="bg-warn/[0.06] px-4 py-1.5 text-xs text-warn">
                Lot fully bid · everyone above pays <span className="font-mono">{price}</span>, the lowest winning price
              </td>
            </tr>}
          </Fragment>;
        })}</tbody>
      </table>
    </div>}

    <p className="px-4 py-3 text-xs leading-5 text-muted">
      {book.levels.length === 0 ? `Any bid at or above the ${fmtUSD(usd(reserve), 2)} reserve would win and pay the reserve.`
        : book.oversubscribed ? "Bids fill from the top. The white line marks the lot size; demand past it (amber) goes unfilled."
          : `${lot - book.filled} IU/day still unclaimed, so winners pay the ${fmtUSD(usd(reserve), 2)} reserve. The white line marks the lot size.`}
      {" "}Bids below the reserve are rejected; earlier bids win ties.
    </p>
  </div>;
}
