"use client";
import Link from "next/link";
import { useEffect, useMemo, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import PageHeader from "@/components/PageHeader";
import { useWallet } from "@/components/WalletProvider";
import { useCatalog, type GatewayModel } from "@/lib/use-catalog";
import { useAuctionBids, useChainMarket } from "@/lib/use-chain-market";
import { clearBook, type BookBid } from "@/lib/order-book";
import OrderBook from "@/components/OrderBook";
import { useMarket } from "@/lib/use-market";
import { usd, type ChainAuction, type ChainBid } from "@/lib/near";
import { fmtUSD, fmtTokens } from "@/lib/market";
import { IU_NOTE, STEPS } from "@/lib/how-it-works";

// A one-second clock for countdowns. It reads 0 during prerendering and hydration, so
// time-dependent text only appears once the page is running in the browser.
let clock = 0;
function subscribeClock(onTick: () => void) {
  clock = Date.now();
  const timer = setInterval(() => { clock = Date.now(); onTick(); }, 1000);
  return () => clearInterval(timer);
}
const useNow = () => useSyncExternalStore(subscribeClock, () => clock, () => 0);

// The open tab lives in the URL hash, so a link or a reload lands on the same tab.
const TABS = ["auctions", "bids", "capacity", "settled", "models"] as const;
type Tab = (typeof TABS)[number];
const tabListeners = new Set<() => void>();
function subscribeTab(onChange: () => void) {
  tabListeners.add(onChange);
  window.addEventListener("hashchange", onChange);
  return () => { tabListeners.delete(onChange); window.removeEventListener("hashchange", onChange); };
}
function readTab(): Tab {
  const hash = window.location.hash.slice(1);
  return (TABS as readonly string[]).includes(hash) ? (hash as Tab) : "auctions";
}
function showTab(tab: Tab) {
  const { pathname, search } = window.location;
  window.history.replaceState(null, "", tab === "auctions" ? pathname + search : `${pathname}${search}#${tab}`);
  tabListeners.forEach((notify) => notify());
}
const useTab = () => useSyncExternalStore(subscribeTab, readTab, (): Tab => "auctions");

function timeLeft(endsAtMs: number, now: number): string {
  if (!now) return "…";
  const ms = endsAtMs - now;
  if (ms <= 0) return "closed";
  const s = Math.floor(ms / 1000), m = Math.floor(s / 60), h = Math.floor(m / 60), d = Math.floor(h / 24);
  if (d >= 1) return `${d}d ${h % 24}h`;
  if (h >= 1) return `${h}h ${m % 60}m`;
  if (m >= 1) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

function demand(auction: ChainAuction): { tone: "muted" | "good" | "warn"; text: string } {
  if (auction.open_bid_units === 0) return { tone: "muted", text: "No bids yet. Any bid at or above the reserve would currently win and pay the reserve." };
  if (auction.open_bid_units <= auction.units)
    return { tone: "good", text: `${auction.units - auction.open_bid_units} of ${auction.units} IU/day still unclaimed. If it closed now, winners would pay the reserve.` };
  return { tone: "warn", text: `Oversubscribed: ${auction.open_bid_units} IU/day bid for ${auction.units}. Highest bids win, and everyone pays the lowest winning bid.` };
}

const BID_STATUS: Record<string, { label: string; tone: string; detail: string }> = {
  open: { label: "Waiting", tone: "text-accent", detail: "Escrowed until the auction is settled. You can cancel while it's open." },
  filled: { label: "Won", tone: "text-good", detail: "Filled in full. See the Your capacity tab." },
  partial: { label: "Partly won", tone: "text-good", detail: "Part of your quantity filled; the rest was unlocked." },
  outbid: { label: "Not filled", tone: "text-muted", detail: "Higher bids took the lot. Your escrow was unlocked." },
  cancelled: { label: "Cancelled", tone: "text-muted", detail: "Escrow unlocked." },
};

export default function MarketPage() {
  const { session, signIn, busy: signingIn, network } = useWallet();
  const { models, status, loading, error: catalogError } = useCatalog();
  const chain = status?.chain ?? null;
  const market = useChainMarket(chain);
  const { contracts } = useMarket();
  const now = useNow();
  const tab = useTab();
  const [selectedId, setSelectedId] = useState<number | null>(null);
  const [units, setUnits] = useState(1);
  const [price, setPrice] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);

  const modelFor = (routeId: string): GatewayModel | undefined => models.find((m) => m.id === routeId);
  const nameFor = (routeId: string) => modelFor(routeId)?.name || routeId.replace(/@.*$/, "");
  const accepting = (a: ChainAuction) => a.status === "open" && (!now || now < a.ends_at_ms);
  const open = useMemo(() => market.auctions.filter((a) => a.status === "open" && (!now || now < a.ends_at_ms)).sort((a, b) => a.ends_at_ms - b.ends_at_ms), [market.auctions, now]);
  const awaiting = market.auctions.filter((a) => a.status === "open" && now > 0 && now >= a.ends_at_ms);
  const cleared = market.auctions.filter((a) => a.status === "cleared").sort((a, b) => (b.cleared_at_ms ?? 0) - (a.cleared_at_ms ?? 0));
  const auction = open.find((a) => a.id === selectedId) || open[0];

  const reserve = auction ? usd(auction.reserve_price) : 0;
  const effectivePrice = price || (auction ? String(reserve) : "");
  const priceAtomic = Math.round(Number(effectivePrice) * 1e6);
  const qty = auction ? Math.min(units, auction.units) : units;
  const escrow = auction ? qty * auction.term_days * priceAtomic : 0;
  const belowReserve = Boolean(auction) && priceAtomic < Number(auction?.reserve_price);
  const funds = market.wallet + market.unlocked;
  const short = Boolean(session) && market.loaded && funds < escrow;
  const auctionBids = useAuctionBids(chain, auction?.id ?? null);
  // The selected auction's book with the bid being drafted added last (so it loses price ties).
  const book = useMemo(() => {
    const bids: BookBid[] = auctionBids.bids.filter((b) => b.status === "open").map((b) => ({ index: b.index, units: b.units, price: Number(b.max_price), mine: b.bidder === session }));
    if (auction && !belowReserve && priceAtomic > 0) bids.push({ index: Infinity, units: qty, price: priceAtomic, mine: false, draft: true });
    return clearBook(bids, auction?.units ?? 0, Number(auction?.reserve_price ?? 0));
  }, [auctionBids.bids, auction, belowReserve, priceAtomic, qty, session]);
  const draftCost = book.price == null ? 0 : book.draftFill * (auction?.term_days ?? 0) * usd(book.price);
  const escrowed = market.bids.filter((b) => b.status === "open").reduce((sum, b) => sum + Number(b.escrow), 0);

  const syncedIds = new Set(contracts.map((c) => c.chain_entitlement_id).filter((id) => id != null));
  const activeContracts = contracts.filter((c) => c.active);
  const syncing = market.entitlements.filter((e) => !syncedIds.has(e.id) && e.ends_at_ms > (now || 0));

  function select(id: number) { setSelectedId(id); setUnits(1); setPrice(""); setNotice(null); setFailure(null); showTab("auctions"); }

  async function run(done: string, action: () => Promise<unknown>) {
    setBusy(true); setFailure(null); setNotice(null);
    try { await action(); setNotice(done); await Promise.all([market.refresh(), auctionBids.refresh()]); }
    catch (error) { setFailure(error instanceof Error ? error.message : "The transaction was not completed."); }
    finally { setBusy(false); }
  }

  if (!loading && !chain) return <div className="mx-auto w-full max-w-7xl px-6 py-8 sm:px-10">
    <PageHeader title="Market" />
    <p className="border border-line p-5 text-sm text-muted">{catalogError || "This deployment is not connected to an auction contract yet."}</p>
  </div>;

  return <div className="mx-auto w-full max-w-7xl px-6 py-8 sm:px-10">
    <PageHeader title="Market" note={chain ? `Settled on NEAR ${chain.network} · ${market.contract}` : "Settled on NEAR"} />
    {(market.error || failure) && <p role="alert" className="mb-5 border border-warn p-4 text-sm text-warn">{failure || market.error}</p>}
    {notice && <p role="status" className="mb-5 border border-line p-4 text-sm text-good">{notice}</p>}

    <section className="grid grid-cols-2 border border-line md:grid-cols-4">
      <Tile label="Open auctions" value={String(open.length)} hint={`${fmtTokens(open.reduce((sum, a) => sum + a.units, 0))} IU/day offered`} />
      <Tile label="USDC in wallet" value={session ? fmtUSD(usd(market.wallet), 2) : "—"} hint={session ? "Available to bid" : "Connect a wallet"} />
      <Tile label="In escrow" value={session ? fmtUSD(usd(escrowed), 2) : "—"} hint="Held by your open bids" />
      <Tile label="Unlocked" value={session ? fmtUSD(usd(market.unlocked), 2) : "—"} hint={market.unlocked > 0 ? "Used first for bids · withdraw in Account" : "Refunds land here"} />
    </section>

    <TabBar current={tab} onSelect={showTab} tabs={[
      { id: "auctions", label: "Auctions", count: open.length, flag: awaiting.length ? `${awaiting.length} closed, ready to settle` : undefined },
      { id: "bids", label: "Your bids", count: session ? market.bids.filter((b) => b.status === "open").length : undefined },
      { id: "capacity", label: "Your capacity", count: session ? activeContracts.length + syncing.length : undefined, flag: syncing.length ? "won capacity syncing" : undefined },
      { id: "settled", label: "Settled", count: cleared.length },
      { id: "models", label: "Models", count: models.length },
    ]} />

    <div role="tabpanel" id={`panel-${tab}`} aria-labelledby={`tab-${tab}`} className="mt-6">
      {tab === "auctions" && <>
        <section aria-labelledby="how-it-works" className="border border-line">
          <h2 id="how-it-works" className="sr-only">How auctions work</h2>
          <ol className="grid md:grid-cols-4">
            {STEPS.map(({ title, text }, i) => <li key={title} className="border-line p-4 max-md:[&:not(:first-child)]:border-t md:[&:not(:first-child)]:border-l">
              <div className="flex items-baseline gap-2"><span className="font-mono text-xs text-accent">{i + 1}</span><span className="text-sm font-medium">{title}</span></div>
              <p className="mt-2 text-xs leading-5 text-muted">{text}</p>
            </li>)}
          </ol>
          <p className="flex flex-wrap justify-between gap-2 border-t border-line px-4 py-2 text-xs text-muted"><span>{IU_NOTE}</span><Link href="/dashboard/how-it-works" className="text-accent">How pricing works →</Link></p>
        </section>

        <div className="mt-6 grid gap-8 lg:grid-cols-[minmax(0,1fr)_340px]">
          <section className="min-w-0">
            <h2 className="mb-3 text-lg font-medium">Open auctions</h2>
            {!open.length && <p className="border border-dashed border-line p-5 text-sm text-muted">{!market.loaded ? "Reading the auction contract…" : "No auctions are open right now. New lots of capacity are published on chain; check back soon."}</p>}
            <ul className="space-y-3">
              {open.map((a) => {
                const selected = a.id === auction?.id;
                const state = demand(a);
                return <li key={a.id}>
                  <button type="button" aria-pressed={selected} onClick={() => select(a.id)} className={`w-full border p-4 text-left transition ${selected ? "border-accent bg-white/[0.05]" : "border-line hover:bg-white/[0.03]"}`}>
                    <div className="flex flex-wrap items-baseline justify-between gap-2">
                      <span className="font-medium">{nameFor(a.route_id)}</span>
                      <span className="font-mono text-xs text-muted">closes in <span className="text-foreground tnum">{timeLeft(a.ends_at_ms, now)}</span></span>
                    </div>
                    <div className="mt-3 grid grid-cols-3 gap-3 text-sm">
                      <Fact label="Lot" value={`${a.units} IU/day`} sub={`${fmtTokens(a.units * a.tokens_per_iu)} tokens/day`} />
                      <Fact label="Term" value={`${a.term_days} days`} sub="starts when settled" />
                      <Fact label="Reserve" value={fmtUSD(usd(a.reserve_price), 2)} sub="min $ / IU-day" />
                    </div>
                    <div className="mt-4">
                      <div className="flex justify-between text-xs text-muted"><span>Demand</span><span className="tnum">{a.open_bid_units} / {a.units} IU/day · {a.open_bids} {a.open_bids === 1 ? "bid" : "bids"}</span></div>
                      <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-white/[0.08]"><div className={`h-full ${state.tone === "warn" ? "bg-warn" : "bg-good"}`} style={{ width: `${Math.min(a.open_bid_units / a.units, 1) * 100}%` }} /></div>
                      <p className={`mt-2 text-xs ${state.tone === "warn" ? "text-warn" : state.tone === "good" ? "text-good" : "text-muted"}`}>{state.text}</p>
                    </div>
                  </button>
                  {selected && <div className="mt-2"><OrderBook book={book} lot={a.units} reserve={Number(a.reserve_price)} loaded={auctionBids.loaded} /></div>}
                </li>;
              })}
            </ul>

            {awaiting.length > 0 && <div className="mt-8">
              <h2 className="mb-3 text-lg font-medium">Closed · ready to settle</h2>
              <ul className="space-y-3">{awaiting.map((a) => <li key={a.id} className="flex flex-wrap items-center justify-between gap-3 border border-line p-4">
                <div><div className="font-medium">{nameFor(a.route_id)}</div><div className="text-xs text-muted">{a.units} IU/day · {a.term_days} days · {a.open_bid_units} IU/day bid by {a.open_bids} {a.open_bids === 1 ? "bidder" : "bidders"}</div></div>
                <button disabled={busy || !session} onClick={() => void run("Auction settled. Winners now hold capacity; unspent escrow is unlocked.", () => market.settle(a.id))} className="rounded-md bg-foreground px-4 py-2 text-sm font-medium text-background disabled:opacity-40">Settle now</button>
              </li>)}</ul>
              <p className="mt-2 text-xs text-muted">Settling runs the clearing on chain. Anyone with a connected wallet can do it, for a small gas fee.</p>
            </div>}
          </section>

          <aside className="h-fit border border-line bg-panel p-5 lg:sticky lg:top-6">
            {!auction ? <>
              <h2 className="text-lg font-medium">Bid</h2>
              <p className="mt-2 text-sm text-muted">Select an open auction to bid on it.</p>
            </> : <>
              <div className="label">Your bid</div>
              <h2 className="mt-1 text-lg font-medium">{nameFor(auction.route_id)}</h2>
              <p className="mt-1 text-sm text-muted">{auction.units} IU/day lot · {auction.term_days}-day term · closes in {timeLeft(auction.ends_at_ms, now)}</p>

              <label className="mt-6 flex justify-between text-sm" htmlFor="quantity"><span>Quantity</span><span className="label">IU / day · max {auction.units}</span></label>
              <input id="quantity" type="number" min={1} max={auction.units} step={1} value={qty} onChange={(event) => setUnits(Math.min(auction.units, Math.max(1, Math.floor(Number(event.target.value) || 1))))} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono" />

              <label className="mt-5 flex justify-between text-sm" htmlFor="bid-price"><span>Maximum price</span><span className="label">USDC / IU-day</span></label>
              <input id="bid-price" type="number" min={reserve} step="0.01" value={effectivePrice} onChange={(event) => setPrice(event.target.value)} className={`mt-2 w-full rounded-md border bg-background px-3 py-2 font-mono ${belowReserve ? "border-warn" : "border-line"}`} />
              <p className={`mt-2 text-xs ${belowReserve ? "text-warn" : "text-muted"}`}>
                {belowReserve ? `Below the ${fmtUSD(reserve, 2)} reserve: the contract rejects this bid.` : `Reserve is ${fmtUSD(reserve, 2)}. Winners pay the clearing price, never more than their maximum.`}
              </p>

              <dl className="mt-6 space-y-3 border-t border-line pt-4 text-sm">
                <Row label="If you win" value={`${fmtTokens(qty * auction.tokens_per_iu)} tokens/day for ${auction.term_days} days`} />
                <Row label="Escrowed now" value={Number.isFinite(escrow) ? fmtUSD(usd(escrow), 2) : "—"} mono />
                <Row label="If it closed now" mono value={belowReserve ? "Rejected" : book.draftFill === 0 ? "Outbid" : `${book.draftFill === qty ? "" : `${book.draftFill} of ${qty} IU · `}${fmtUSD(draftCost, 2)}`} />
              </dl>

              {!session ? <button disabled={signingIn} onClick={() => void signIn()} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background disabled:opacity-40">{signingIn ? "Waiting for wallet…" : "Connect NEAR wallet to bid"}</button>
                : <>
                  <button disabled={busy || belowReserve || !(escrow > 0) || short} onClick={() => void run(`Bid placed. ${fmtUSD(usd(escrow), 2)} is escrowed until the auction is settled.`, () => market.placeBid(auction, qty, priceAtomic))} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background transition hover:bg-accent disabled:opacity-40">
                    {busy ? "Confirm in your wallet…" : `Place bid · escrow ${fmtUSD(usd(escrow), 2)}`}
                  </button>
                  {short && <div className="mt-3 text-sm text-warn">
                    <p>You have {fmtUSD(usd(funds), 2)} available; this bid escrows {fmtUSD(usd(escrow), 2)}.</p>
                    {network === "testnet" && <button disabled={busy} onClick={() => void run("1,000 test USDC added to your wallet.", market.faucet)} className="mt-2 rounded-md border border-line px-3 py-1.5 text-foreground disabled:opacity-40">Get 1,000 test USDC</button>}
                  </div>}
                </>}
              <p className="mt-4 text-xs leading-5 text-muted">Your wallet asks you to confirm each transaction. You can cancel a bid until the auction closes.</p>
            </>}
          </aside>
        </div>
      </>}
      {tab === "bids" && <>
        <section aria-label="Your bids">
          <div className="overflow-x-auto border border-line">
            <table className="w-full min-w-[680px] text-left text-sm">
              <thead><tr className="border-b border-line"><th className="label px-4 py-3 font-normal">Auction</th><th className="label px-4 py-3 font-normal">Quantity</th><th className="label px-4 py-3 font-normal">Max / IU-day</th><th className="label px-4 py-3 font-normal">Status</th><th className="label px-4 py-3 font-normal">Won · paid</th><th /></tr></thead>
              <tbody>{market.bids.map((bid) => {
                const lot = market.auctions.find((a) => a.id === bid.auction_id);
                return <BidRow key={`${bid.auction_id}:${bid.index}`} bid={bid} lot={lot} name={lot ? nameFor(lot.route_id) : `Auction ${bid.auction_id}`} now={now} busy={busy}
                  canCancel={Boolean(lot && accepting(lot))} onCancel={() => void run("Bid cancelled. Its escrow is unlocked.", () => market.cancelBid(bid))} />;
              })}</tbody>
            </table>
            {!market.bids.length && <p className="p-5 text-sm text-muted">{session ? "No bids yet. Pick one on the Auctions tab." : "Connect your NEAR wallet to see your bids."}</p>}
          </div>
        </section>
      </>}
      {tab === "capacity" && <>
        <section aria-label="Your capacity">
          <div className="grid gap-4 md:grid-cols-2">
            {activeContracts.map((c) => <article key={c.id} className="border border-line bg-panel p-5">
              <h3 className="font-medium">{nameFor(c.route_id)}</h3>
              <p className="mt-2 text-sm text-muted">{c.units} IU/day · ends {new Date(c.ends_at).toLocaleDateString()}</p>
              <div className="mt-4 flex justify-between text-sm"><span>Remaining today</span><span className="font-mono">{fmtTokens(c.remaining_tokens)} of {fmtTokens(c.daily_tokens)} tokens</span></div>
              <Link href={`/dashboard/playground?model=${encodeURIComponent(c.route_id)}&contract=${c.id}`} className="mt-4 inline-block rounded-md bg-foreground px-4 py-2 text-sm font-medium text-background">Try it in Playground →</Link>
            </article>)}
            {syncing.map((e) => <article key={`e${e.id}`} className="border border-dashed border-line p-5">
              <h3 className="font-medium">{nameFor(e.route_id)}</h3>
              <p className="mt-2 text-sm text-muted">{e.units} IU/day · won on chain · syncing to your account (a few seconds)…</p>
            </article>)}
          </div>
          {!activeContracts.length && !syncing.length && <p className="border border-line p-5 text-sm text-muted">{session ? "Capacity you win appears here once its auction is settled." : "Connect your wallet to see the capacity you own."}</p>}
        </section>
      </>}
      {tab === "settled" && <>
        <section aria-label="Settled auctions">
          {!cleared.length && <p className="border border-line p-5 text-sm text-muted">{market.loaded ? "No auction has been settled yet." : "Reading the auction contract…"}</p>}
          {cleared.length > 0 && <>
            <div className="overflow-x-auto border border-line">
              <table className="w-full min-w-[620px] text-left text-sm">
                <thead><tr className="border-b border-line"><th className="label px-4 py-3 font-normal">Auction</th><th className="label px-4 py-3 font-normal">Sold</th><th className="label px-4 py-3 font-normal">Reserve</th><th className="label px-4 py-3 font-normal">Clearing price</th><th className="label px-4 py-3 font-normal">Settled</th></tr></thead>
                <tbody>{cleared.slice(0, 50).map((a) => <tr key={a.id} className="border-b border-line last:border-0">
                  <td className="px-4 py-3">{nameFor(a.route_id)}<div className="text-xs text-muted">#{a.id} · {a.term_days} days</div></td>
                  <td className="px-4 py-3 font-mono">{a.filled_units} / {a.units} IU/day</td>
                  <td className="px-4 py-3 font-mono">{fmtUSD(usd(a.reserve_price), 2)}</td>
                  <td className="px-4 py-3 font-mono">{a.clearing_price ? fmtUSD(usd(a.clearing_price), 2) : <span className="font-sans text-muted">No winning bids</span>}</td>
                  <td className="px-4 py-3 text-muted">{a.cleared_at_ms ? new Date(a.cleared_at_ms).toLocaleString() : "—"}</td>
                </tr>)}</tbody>
              </table>
            </div>
          </>}
        </section>
      </>}
      {tab === "models" && <>
        <section aria-label="Models and token prices" className="border border-line">
          <div className="overflow-x-auto p-4">
            <table className="w-full min-w-[620px] text-left text-sm">
              <thead><tr className="border-b border-line"><th className="label px-3 py-2 font-normal">Model</th><th className="label px-3 py-2 font-normal">Context</th><th className="label px-3 py-2 text-right font-normal">Input / 1M</th><th className="label px-3 py-2 text-right font-normal">Output / 1M</th><th className="label px-3 py-2 text-right font-normal">Open auctions</th></tr></thead>
              <tbody>{[...models].sort((a, b) => a.price_per_iu_usd - b.price_per_iu_usd).map((item) => {
                const first = open.find((a) => a.route_id === item.id);
                return <tr key={item.id} className="border-b border-line last:border-0">
                  <td className="px-3 py-2">{item.name}<div className="text-xs text-muted">{item.model} · NEAR AI · {item.provider_type === "vllm" ? "Confidential TEE" : "Partner TEE"}</div></td>
                  <td className="px-3 py-2 font-mono text-xs text-muted">{item.context_length ? fmtTokens(item.context_length) : "—"}</td>
                  <td className="px-3 py-2 text-right font-mono">{fmtUSD(item.input_price_per_million_tokens_usd, 2)}</td>
                  <td className="px-3 py-2 text-right font-mono">{fmtUSD(item.price_per_million_tokens_usd, 2)}</td>
                  <td className="px-3 py-2 text-right">{first ? <button onClick={() => select(first.id)} className="text-accent">{open.filter((a) => a.route_id === item.id).length} · bid →</button> : <span className="text-muted">—</span>}</td>
                </tr>;
              })}</tbody>
            </table>
            <p className="mt-3 text-xs text-muted">Hardware attestation is reported by the provider; proof verification is not yet enabled.</p>
          </div>
        </section>
      </>}
    </div>
  </div>;
}

function BidRow({ bid, lot, name, now, busy, canCancel, onCancel }: { bid: ChainBid; lot?: ChainAuction; name: string; now: number; busy: boolean; canCancel: boolean; onCancel: () => void }) {
  const info = BID_STATUS[bid.status] || { label: bid.status, tone: "text-muted", detail: "" };
  const closing = bid.status === "open" && lot ? (now && now >= lot.ends_at_ms ? " · closed, awaiting settlement" : ` · closes in ${timeLeft(lot.ends_at_ms, now)}`) : "";
  return <tr className="border-b border-line align-top last:border-0">
    <td className="px-4 py-3">{name}<div className="text-xs text-muted">#{bid.auction_id}{lot ? ` · ${lot.term_days} days` : ""}{closing}</div></td>
    <td className="px-4 py-3 font-mono">{bid.units} IU/day</td>
    <td className="px-4 py-3 font-mono">{fmtUSD(usd(bid.max_price), 2)}</td>
    <td className="px-4 py-3"><span className={info.tone}>{info.label}</span><div className="max-w-56 text-xs text-muted">{info.detail}</div></td>
    <td className="px-4 py-3 font-mono">{bid.status === "open" ? "—" : `${bid.filled_units} IU · ${fmtUSD(usd(bid.charged), 2)}`}</td>
    <td className="px-4 py-3 text-right">{canCancel && <button disabled={busy} onClick={onCancel} className="text-muted transition hover:text-foreground">Cancel</button>}</td>
  </tr>;
}

type TabSpec = { id: Tab; label: string; count?: number; flag?: string };

function TabBar({ tabs, current, onSelect }: { tabs: TabSpec[]; current: Tab; onSelect: (tab: Tab) => void }) {
  const list = useRef<HTMLDivElement>(null);
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const selectedIndex = tabs.findIndex((t) => t.id === current);
  // On narrow screens the strip scrolls sideways; keep the open tab visible, e.g. after a #models link.
  useEffect(() => {
    const strip = list.current, button = buttons.current[selectedIndex];
    if (!strip || !button) return;
    if (button.offsetLeft < strip.scrollLeft || button.offsetLeft + button.offsetWidth > strip.scrollLeft + strip.clientWidth) strip.scrollLeft = button.offsetLeft - 16;
  }, [selectedIndex]);
  // Arrow keys, Home and End move between tabs, as in the WAI-ARIA tabs pattern.
  function onKeyDown(event: KeyboardEvent, index: number) {
    const last = tabs.length - 1;
    const next = { ArrowRight: index === last ? 0 : index + 1, ArrowLeft: index === 0 ? last : index - 1, Home: 0, End: last }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    onSelect(tabs[next].id);
    buttons.current[next]?.focus();
  }
  return <div ref={list} role="tablist" aria-label="Market sections" className="relative mt-8 flex overflow-x-auto border-b border-line [scrollbar-width:none]">
    {tabs.map((t, i) => {
      const selected = t.id === current;
      return <button key={t.id} ref={(el) => { buttons.current[i] = el; }} id={`tab-${t.id}`} type="button" role="tab" aria-selected={selected} aria-controls={`panel-${t.id}`} tabIndex={selected ? 0 : -1}
        onClick={() => onSelect(t.id)} onKeyDown={(event) => onKeyDown(event, i)}
        className={`flex shrink-0 items-center gap-2 px-4 py-3 text-sm transition ${selected ? "text-foreground shadow-[inset_0_-2px_0_var(--accent)]" : "text-muted hover:text-foreground"}`}>
        {t.label}
        {t.count !== undefined && <span className="rounded-full bg-white/[0.08] px-1.5 py-0.5 font-mono text-[11px] leading-none tnum">{t.count}</span>}
        {t.flag && <span className="h-1.5 w-1.5 rounded-full bg-warn" title={t.flag}><span className="sr-only">{t.flag}</span></span>}
      </button>;
    })}
  </div>;
}

function Fact({ label, value, sub }: { label: string; value: string; sub: string }) {
  return <div><div className="label">{label}</div><div className="mt-1 font-mono tnum">{value}</div><div className="text-xs text-muted">{sub}</div></div>;
}

function Row({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return <div className="flex justify-between gap-4"><dt className="text-muted">{label}</dt><dd className={`text-right ${mono ? "font-mono tnum" : ""}`}>{value}</dd></div>;
}

function Tile({ label, value, hint }: { label: string; value: string; hint: string }) {
  return <div className="border-line p-4 [&:not(:last-child)]:border-r max-md:[&:nth-child(2)]:border-r-0 max-md:[&:nth-child(n+3)]:border-t"><div className="label">{label}</div><div className="mt-3 text-2xl font-medium tracking-tight tnum">{value}</div><div className="mt-1 text-xs text-muted">{hint}</div></div>;
}
