"use client";
import Link from "next/link";
import { useMemo, useRef, useState, useSyncExternalStore } from "react";
import { usePrivy } from "@privy-io/react-auth";
import PageHeader from "@/components/PageHeader";
import { useAccount } from "@/components/AccountProvider";
import { useCatalog, type GatewayModel } from "@/lib/use-catalog";
import { useMarket, type Auction, type Bid } from "@/lib/use-market";
import { fmtUSD, fmtTokens } from "@/lib/market";

const usd = (micro: number) => micro / 1e6;

// A one-second clock for countdowns. It reads 0 during prerendering and hydration, so
// time-dependent text only appears once the page is running in the browser.
let clock = 0;
function subscribeClock(onTick: () => void) {
  clock = Date.now();
  const timer = setInterval(() => { clock = Date.now(); onTick(); }, 1000);
  return () => clearInterval(timer);
}
const useNow = () => useSyncExternalStore(subscribeClock, () => clock, () => 0);

function timeLeft(endsAt: string, now: number): string {
  if (!now) return "…";
  const ms = Date.parse(endsAt) - now;
  if (ms <= 0) return "closing now";
  const s = Math.floor(ms / 1000), m = Math.floor(s / 60), h = Math.floor(m / 60), d = Math.floor(h / 24);
  if (d >= 1) return `${d}d ${h % 24}h`;
  if (h >= 1) return `${h}h ${m % 60}m`;
  if (m >= 1) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

// What the public order book says about the price, without revealing anyone's bid.
function demand(auction: Auction): { tone: "muted" | "good" | "warn"; text: string } {
  if (auction.bid_units === 0) return { tone: "muted", text: "No bids yet. Any bid at or above the reserve would currently win and pay the reserve." };
  if (auction.bid_units <= auction.units)
    return { tone: "good", text: `${auction.units - auction.bid_units} of ${auction.units} IU/day still unclaimed. If it closed now, winners would pay the reserve.` };
  return { tone: "warn", text: `Oversubscribed: ${auction.bid_units} IU/day bid for ${auction.units}. Highest bids win, and everyone pays the lowest winning bid.` };
}

const BID_STATUS: Record<string, { label: string; tone: string; detail: string }> = {
  open: { label: "Waiting", tone: "text-accent", detail: "Clears when the auction closes. You can still cancel." },
  filled: { label: "Won", tone: "text-good", detail: "Filled in full. The capacity is in Your capacity." },
  partial: { label: "Partly won", tone: "text-good", detail: "Part of your quantity filled; the rest of the hold was released." },
  outbid: { label: "Not filled", tone: "text-muted", detail: "Higher bids took the lot, or your price was below the reserve. Hold released." },
  cancelled: { label: "Cancelled", tone: "text-muted", detail: "Hold released." },
};

const STEPS = [
  ["Pick a lot", "The operator offers a fixed amount of one model's capacity, in IU/day, for a set term, with a minimum (reserve) price."],
  ["Place a sealed bid", "Choose how many IU/day you want and the most you'd pay per IU-day. Quantity × max price × days is held from your credit."],
  ["One price at close", "Highest bids fill first. If bids fit the lot, everyone pays the reserve; if not, everyone pays the lowest winning bid. Unused holds are released."],
  ["Use your capacity", "Winners get a daily token allowance for the whole term, in Playground or through an API key. It resets every 24 hours."],
] as const;

export default function MarketPage() {
  const { login } = usePrivy();
  const { account, signedIn, api, refresh: refreshAccount } = useAccount();
  const { models, status, loading, error: catalogError } = useCatalog();
  const { auctions, bids, contracts, error: marketError, refresh } = useMarket();
  const now = useNow();
  const [selectedId, setSelectedId] = useState("");
  const [units, setUnits] = useState(1);
  const [price, setPrice] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [search, setSearch] = useState("");
  const [confirmedQuote, setConfirmedQuote] = useState<string | null>(null);
  const submission = useRef<{ signature: string; id: string } | null>(null);

  const modelFor = (routeId: string): GatewayModel | undefined => models.find((m) => m.id === routeId);
  const nameFor = (routeId: string) => modelFor(routeId)?.name || routeId.replace(/@.*$/, "");
  const open = useMemo(
    () => auctions.filter((a) => a.status === "open" && a.accepting).sort((a, b) => a.ends_at.localeCompare(b.ends_at)),
    [auctions],
  );
  const closed = auctions.filter((a) => !(a.status === "open" && a.accepting));
  const auction = open.find((a) => a.id === selectedId) || open[0];
  const model = auction ? modelFor(auction.route_id) : undefined;

  const reserve = auction ? usd(auction.reserve_price_micro_usd) : 0;
  const effectivePrice = price || (auction ? String(reserve) : "");
  const maxPrice = Number(effectivePrice);
  const qty = auction ? Math.min(units, auction.units) : units;
  const days = auction?.term_days ?? 0;
  const held = qty * days * maxPrice;
  const belowReserve = Boolean(auction) && maxPrice < reserve;
  const fitsAtReserve = auction ? auction.bid_units + qty <= auction.units : false;
  const available = usd(account?.balance_micro_usd || 0);
  const quote = JSON.stringify([auction?.id, qty, effectivePrice]);
  const confirmBid = confirmedQuote === quote;
  const setConfirmBid = (value: boolean) => setConfirmedQuote(value ? quote : null);

  const sortedModels = useMemo(
    () => models.filter((m) => `${m.name} ${m.model}`.toLowerCase().includes(search.toLowerCase())).sort((a, b) => a.price_per_iu_usd - b.price_per_iu_usd),
    [models, search],
  );
  const activeContracts = contracts.filter((contract) => contract.active);

  function select(id: string) {
    setSelectedId(id); setUnits(1); setPrice(""); setConfirmedQuote(null); setNotice(null); setFailure(null);
  }

  async function placeBid() {
    if (!auction) return;
    setBusy(true); setFailure(null); setNotice(null);
    try {
      const signature = JSON.stringify([auction.id, qty, effectivePrice]);
      if (submission.current?.signature !== signature) submission.current = { signature, id: crypto.randomUUID() };
      const response = await api("market/bids", { method: "POST", body: JSON.stringify({ auction_id: auction.id, client_order_id: submission.current.id, units: qty, max_price_usd: effectivePrice }) });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message || "The bid was not accepted.");
      setNotice(body.duplicate ? "This bid was already recorded. Its current status is shown below." : `Bid placed. ${fmtUSD(held, 2)} is held until the auction closes in ${timeLeft(auction.ends_at, Date.now())}.`);
      setConfirmBid(false); submission.current = null;
      await Promise.all([refresh(), refreshAccount()]);
    } catch (error) { setFailure(error instanceof Error ? error.message : "Could not place your bid."); }
    finally { setBusy(false); }
  }

  async function cancel(id: string) {
    setBusy(true); setFailure(null); setNotice(null);
    try {
      const response = await api(`market/bids/${id}`, { method: "DELETE" });
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message || "Could not cancel the bid.");
      setNotice("Bid cancelled. Its hold has been released.");
      await Promise.all([refresh(), refreshAccount()]);
    } catch (error) { setFailure(error instanceof Error ? error.message : "Could not cancel the bid."); }
    finally { setBusy(false); }
  }

  return <div className="mx-auto w-full max-w-7xl px-6 py-8 sm:px-10">
    <PageHeader title="Market" note="Capacity auctions · $ / IU-day" />
    {status?.mode === "test" && <p className="mb-5 text-sm text-warn">Test providers · no real inference or settlement.</p>}
    {(catalogError || marketError || failure) && <p role="alert" className="mb-5 border border-warn p-4 text-sm text-warn">{failure || catalogError || marketError}</p>}
    {notice && <p role="status" className="mb-5 border border-line p-4 text-sm text-good">{notice}</p>}

    <section aria-labelledby="how-it-works" className="border border-line">
      <h2 id="how-it-works" className="sr-only">How auctions work</h2>
      <ol className="grid md:grid-cols-4">
        {STEPS.map(([title, text], i) => <li key={title} className="border-line p-4 max-md:[&:not(:first-child)]:border-t md:[&:not(:first-child)]:border-l">
          <div className="flex items-baseline gap-2"><span className="font-mono text-xs text-accent">{i + 1}</span><span className="text-sm font-medium">{title}</span></div>
          <p className="mt-2 text-xs leading-5 text-muted">{text}</p>
        </li>)}
      </ol>
      <p className="border-t border-line px-4 py-2 text-xs text-muted">1 IU = one million output-token equivalents per day. Input tokens count at the model&apos;s input/output price ratio.</p>
    </section>

    <section className="mt-6 grid grid-cols-2 border border-line md:grid-cols-4">
      <Tile label="Open auctions" value={String(open.length)} hint={`${models.length} live models`} />
      <Tile label="Capacity offered" value={fmtTokens(open.reduce((sum, a) => sum + a.units, 0))} hint="IU / day across open lots" />
      <Tile label="Available credit" value={fmtUSD(available, 2)} hint={signedIn ? "Your account" : "Sign in to trade"} />
      <Tile label="Held for bids" value={fmtUSD(usd(account?.held_micro_usd || 0), 2)} hint="Released if you don't win" />
    </section>

    <div className="mt-8 grid gap-8 lg:grid-cols-[minmax(0,1fr)_340px]">
      <section className="min-w-0">
        <h2 className="mb-3 text-lg font-medium">Open auctions</h2>
        {!open.length && <p className="border border-dashed border-line p-5 text-sm text-muted">{loading ? "Loading auctions…" : "No auctions are open right now. The operator publishes new lots of capacity; check back soon."}</p>}
        <ul className="space-y-3">
          {open.map((a) => {
            const selected = a.id === auction?.id;
            const state = demand(a);
            const filled = Math.min(a.bid_units / a.units, 1);
            const m = modelFor(a.route_id);
            return <li key={a.id}>
              <button type="button" aria-pressed={selected} onClick={() => select(a.id)} className={`w-full border p-4 text-left transition ${selected ? "border-accent bg-white/[0.05]" : "border-line hover:bg-white/[0.03]"}`}>
                <div className="flex flex-wrap items-baseline justify-between gap-2">
                  <span className="font-medium">{nameFor(a.route_id)}</span>
                  <span className="font-mono text-xs text-muted">closes in <span className="text-foreground tnum">{timeLeft(a.ends_at, now)}</span></span>
                </div>
                <div className="mt-3 grid grid-cols-3 gap-3 text-sm">
                  <Fact label="Lot" value={`${a.units} IU/day`} sub={`${fmtTokens(a.units * (m?.tokens_per_iu || 1e6))} tokens/day`} />
                  <Fact label="Term" value={`${a.term_days} days`} sub="starts at close" />
                  <Fact label="Reserve" value={fmtUSD(usd(a.reserve_price_micro_usd), 2)} sub="min $ / IU-day" />
                </div>
                <div className="mt-4">
                  <div className="flex justify-between text-xs text-muted"><span>Demand</span><span className="tnum">{a.bid_units} / {a.units} IU/day · {a.bid_count} {a.bid_count === 1 ? "bid" : "bids"}</span></div>
                  <div className="mt-1 h-1.5 overflow-hidden rounded-full bg-white/[0.08]"><div className={`h-full ${state.tone === "warn" ? "bg-warn" : "bg-good"}`} style={{ width: `${filled * 100}%` }} /></div>
                  <p className={`mt-2 text-xs ${state.tone === "warn" ? "text-warn" : state.tone === "good" ? "text-good" : "text-muted"}`}>{state.text}</p>
                </div>
              </button>
            </li>;
          })}
        </ul>
      </section>

      <aside className="h-fit border border-line bg-panel p-5 lg:sticky lg:top-6">
        {!auction ? <>
          <h2 className="text-lg font-medium">Bid</h2>
          <p className="mt-2 text-sm text-muted">Select an open auction to bid on it.</p>
        </> : <>
          <div className="label">Your bid</div>
          <h2 className="mt-1 text-lg font-medium">{nameFor(auction.route_id)}</h2>
          <p className="mt-1 text-sm text-muted">{auction.units} IU/day lot · {auction.term_days}-day term · closes in {timeLeft(auction.ends_at, now)}</p>

          <label className="mt-6 flex justify-between text-sm" htmlFor="quantity"><span>Quantity</span><span className="label">IU / day · max {auction.units}</span></label>
          <input id="quantity" type="number" min={1} max={auction.units} step={1} value={qty} onChange={(event) => { setUnits(Math.min(auction.units, Math.max(1, Math.floor(Number(event.target.value) || 1)))); setConfirmBid(false); }} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono" />

          <label className="mt-5 flex justify-between text-sm" htmlFor="bid-price"><span>Maximum price</span><span className="label">$ / IU-day</span></label>
          <input id="bid-price" type="number" min={reserve} step="0.01" value={effectivePrice} onChange={(event) => { setPrice(event.target.value); setConfirmBid(false); }} className={`mt-2 w-full rounded-md border bg-background px-3 py-2 font-mono ${belowReserve ? "border-warn" : "border-line"}`} />
          <p className={`mt-2 text-xs ${belowReserve ? "text-warn" : "text-muted"}`}>
            {belowReserve ? `Below the ${fmtUSD(reserve, 2)} reserve: this bid could not win.` : `Reserve is ${fmtUSD(reserve, 2)}. Winners pay the clearing price, never more than their maximum. Bid higher only to beat competing bids.`}
          </p>

          <dl className="mt-6 space-y-3 border-t border-line pt-4 text-sm">
            <Row label="If you win" value={`${fmtTokens(qty * (model?.tokens_per_iu || 1e6))} tokens/day for ${auction.term_days} days`} />
            <Row label="Held from credit now" value={Number.isFinite(held) ? fmtUSD(held, 2) : "—"} mono />
            <Row label="Cost if it closed now" mono value={belowReserve ? "Not filled" : fitsAtReserve ? fmtUSD(qty * days * reserve, 2) : `${fmtUSD(qty * days * reserve, 2)}–${fmtUSD(held, 2)}`} />
          </dl>
          {!belowReserve && !fitsAtReserve && <p className="mt-2 text-xs text-muted">Demand would exceed the lot, so the price depends on other sealed bids. You may also get only part of your quantity.</p>}

          {!signedIn ? <button onClick={login} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background">Sign in to bid</button>
            : !account ? <Link href="/dashboard/api-keys" className="mt-6 block rounded-md bg-foreground py-3 text-center text-sm font-medium text-background">Connect your account</Link>
            : <>
              <button disabled={busy || belowReserve || !(held > 0) || available < held} onClick={() => confirmBid ? placeBid() : setConfirmBid(true)} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background transition hover:bg-accent disabled:opacity-40">
                {busy ? "Submitting…" : confirmBid ? `Confirm · hold ${fmtUSD(held, 2)}` : "Place bid"}
              </button>
              {confirmBid && <button onClick={() => setConfirmBid(false)} className="mt-2 w-full text-sm text-muted">Back</button>}
              {available < held && <p className="mt-3 text-sm text-warn">You have {fmtUSD(available, 2)} of credit; this bid needs {fmtUSD(held, 2)}. Lower the quantity or price, or ask the operator for credit.</p>}
            </>}
          <p className="mt-4 text-xs leading-5 text-muted">You can cancel until the auction closes. At close, anything you don&apos;t pay is released back to your credit.</p>
        </>}
      </aside>
    </div>

    <section className="mt-10">
      <h2 className="mb-3 text-lg font-medium">Your bids</h2>
      <div className="overflow-x-auto border border-line">
        <table className="w-full min-w-[680px] text-left text-sm">
          <thead><tr className="border-b border-line"><th className="label px-4 py-3 font-normal">Auction</th><th className="label px-4 py-3 font-normal">Quantity</th><th className="label px-4 py-3 font-normal">Max / IU-day</th><th className="label px-4 py-3 font-normal">Status</th><th className="label px-4 py-3 font-normal">Won · paid</th><th /></tr></thead>
          <tbody>{bids.map((bid) => <BidRow key={bid.id} bid={bid} name={nameFor(bid.route_id)} now={now} busy={busy} onCancel={cancel} />)}</tbody>
        </table>
        {!bids.length && <p className="p-5 text-sm text-muted">{signedIn ? "No bids yet. Pick an open auction above." : "Sign in to see your bids."}</p>}
      </div>
    </section>

    <section className="mt-10">
      <h2 className="mb-3 text-lg font-medium">Your capacity</h2>
      <div className="grid gap-4 md:grid-cols-2">
        {activeContracts.map((contract) => <article key={contract.id} className="border border-line bg-panel p-5">
          <h3 className="font-medium">{nameFor(contract.route_id)}</h3>
          <p className="mt-2 text-sm text-muted">{contract.units} IU/day · ends {new Date(contract.ends_at).toLocaleDateString()}</p>
          <div className="mt-4 flex justify-between text-sm"><span>Remaining today</span><span className="font-mono">{fmtTokens(contract.remaining_tokens)} of {fmtTokens(contract.daily_tokens)} tokens</span></div>
          <Link href={`/dashboard/playground?model=${encodeURIComponent(contract.route_id)}&contract=${contract.id}`} className="mt-4 inline-block rounded-md border border-line px-4 py-2 text-sm">Use in Playground →</Link>
        </article>)}
      </div>
      {!activeContracts.length && <p className="border border-line p-5 text-sm text-muted">Capacity you win appears here when its auction closes. Use it in Playground or with an API key.</p>}
    </section>

    {closed.length > 0 && <section className="mt-10">
      <h2 className="mb-3 text-lg font-medium">Recently closed</h2>
      <div className="overflow-x-auto border border-line">
        <table className="w-full min-w-[620px] text-left text-sm">
          <thead><tr className="border-b border-line"><th className="label px-4 py-3 font-normal">Auction</th><th className="label px-4 py-3 font-normal">Sold</th><th className="label px-4 py-3 font-normal">Reserve</th><th className="label px-4 py-3 font-normal">Clearing price</th><th className="label px-4 py-3 font-normal">Closed</th></tr></thead>
          <tbody>{closed.map((a) => <tr key={a.id} className="border-b border-line last:border-0">
            <td className="px-4 py-3">{nameFor(a.route_id)}<div className="text-xs text-muted">{a.term_days} days</div></td>
            <td className="px-4 py-3 font-mono">{a.status === "open" ? "clearing…" : `${a.filled_units} / ${a.units} IU/day`}</td>
            <td className="px-4 py-3 font-mono">{fmtUSD(usd(a.reserve_price_micro_usd), 2)}</td>
            <td className="px-4 py-3 font-mono">{a.clearing_price_micro_usd != null ? fmtUSD(usd(a.clearing_price_micro_usd), 2) : <span className="font-sans text-muted">{a.status === "open" ? "—" : "No winning bids"}</span>}</td>
            <td className="px-4 py-3 text-muted">{new Date(a.ends_at).toLocaleString()}</td>
          </tr>)}</tbody>
        </table>
      </div>
    </section>}

    <details className="mt-10 border border-line">
      <summary className="cursor-pointer px-4 py-3 text-sm">All models and token prices <span className="text-muted">· {models.length} live</span></summary>
      <div className="border-t border-line p-4">
        <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search models" aria-label="Search models" className="mb-3 w-full max-w-xs rounded-md border border-line bg-background px-3 py-2 text-sm" />
        <div className="overflow-x-auto">
          <table className="w-full min-w-[680px] text-left text-sm">
            <thead><tr className="border-b border-line"><th className="label px-3 py-2 font-normal">Model</th><th className="label px-3 py-2 font-normal">Context</th><th className="label px-3 py-2 text-right font-normal">Input / 1M</th><th className="label px-3 py-2 text-right font-normal">Output / 1M</th><th className="label px-3 py-2 text-right font-normal">Open auctions</th></tr></thead>
            <tbody>{sortedModels.map((item) => {
              const first = open.find((a) => a.route_id === item.id);
              const count = open.filter((a) => a.route_id === item.id).length;
              return <tr key={item.id} className="border-b border-line last:border-0">
                <td className="px-3 py-2">{item.name}<div className="text-xs text-muted">{item.model} · NEAR AI · {item.provider_type === "vllm" ? "Confidential TEE" : "Partner TEE"}</div></td>
                <td className="px-3 py-2 font-mono text-xs text-muted">{item.context_length ? fmtTokens(item.context_length) : "—"}</td>
                <td className="px-3 py-2 text-right font-mono">{fmtUSD(item.input_price_per_million_tokens_usd, 2)}</td>
                <td className="px-3 py-2 text-right font-mono">{fmtUSD(item.price_per_million_tokens_usd, 2)}</td>
                <td className="px-3 py-2 text-right">{first ? <button onClick={() => select(first.id)} className="text-accent">{count} · bid →</button> : <span className="text-muted">—</span>}</td>
              </tr>;
            })}</tbody>
          </table>
        </div>
        <p className="mt-3 text-xs text-muted">Hardware attestation is reported by the provider; proof verification is not yet enabled.</p>
      </div>
    </details>
  </div>;
}

function BidRow({ bid, name, now, busy, onCancel }: { bid: Bid; name: string; now: number; busy: boolean; onCancel: (id: string) => void }) {
  const info = BID_STATUS[bid.status] || { label: bid.status, tone: "text-muted", detail: "" };
  return <tr className="border-b border-line align-top last:border-0">
    <td className="px-4 py-3">{name}<div className="text-xs text-muted">{bid.term_days} days{bid.status === "open" ? ` · closes in ${timeLeft(bid.ends_at, now)}` : ""}</div></td>
    <td className="px-4 py-3 font-mono">{bid.units} IU/day</td>
    <td className="px-4 py-3 font-mono">{fmtUSD(usd(bid.max_price_micro_usd), 2)}</td>
    <td className="px-4 py-3"><span className={info.tone}>{info.label}</span><div className="max-w-56 text-xs text-muted">{info.detail}</div></td>
    <td className="px-4 py-3 font-mono">{bid.status === "open" ? "—" : `${bid.filled_units} IU · ${fmtUSD(usd(bid.charged_micro_usd), 2)}`}</td>
    <td className="px-4 py-3 text-right">{bid.status === "open" && <button disabled={busy} onClick={() => onCancel(bid.id)} className="text-muted transition hover:text-foreground">Cancel</button>}</td>
  </tr>;
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
