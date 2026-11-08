"use client";
import Link from "next/link";
import { useMemo, useRef, useState } from "react";
import { usePrivy } from "@privy-io/react-auth";
import PageHeader from "@/components/PageHeader";
import { useAccount } from "@/components/AccountProvider";
import { useCatalog } from "@/lib/use-catalog";
import { useMarket } from "@/lib/use-market";
import { fmtUSD, fmtTokens } from "@/lib/market";
const TERMS = [7, 30, 90];

export default function MarketPage() {
  const { login } = usePrivy();
  const { account, signedIn, api, refresh: refreshAccount } = useAccount();
  const { models, status, loading, error: catalogError } = useCatalog();
  const { auctions, bids, contracts, error: marketError, refresh } = useMarket();
  const [selection, setSelection] = useState("");
  const [days, setDays] = useState(30);
  const [units, setUnits] = useState(10);
  const [price, setPrice] = useState("");
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [search, setSearch] = useState("");
  const [confirmBid, setConfirmBid] = useState(false);
  const submission = useRef<{ signature: string; id: string } | null>(null);
  const model = models.find((model) => model.id === selection) || models[0];
  const openAuctions = auctions.filter((auction) => auction.status === "open" && auction.accepting);
  const auction = openAuctions.filter((auction) => auction.route_id === model?.id && auction.term_days === days).sort((a,b) => a.ends_at.localeCompare(b.ends_at))[0];
  const effectivePrice = price || String((auction?.reserve_price_micro_usd ?? (model?.price_per_iu_usd || 0) * 1e6) / 1e6);
  const notional = units * days * Number(effectivePrice);
  const available = (account?.balance_micro_usd || 0) / 1e6;
  const sorted = useMemo(() => [...models].filter((model) => `${model.name} ${model.model}`.toLowerCase().includes(search.toLowerCase())).sort((a,b) => a.price_per_iu_usd-b.price_per_iu_usd), [models, search]);
  const activeContracts = contracts.filter((contract) => contract.active);
  function select(id: string) { setSelection(id); setPrice(""); setConfirmBid(false); setNotice(null); setFailure(null); }
  async function placeBid() {
    if (!auction) return;
    setBusy(true); setFailure(null); setNotice(null);
    try {
      const signature = JSON.stringify([auction.id, units, effectivePrice]);
      if (submission.current?.signature !== signature) submission.current = { signature, id: crypto.randomUUID() };
      const response = await api("market/bids", { method: "POST", body: JSON.stringify({ auction_id: auction.id, client_order_id: submission.current.id, units, max_price_usd: effectivePrice }) });
      const body = await response.json(); if (!response.ok) throw new Error(body.error?.message || "The bid was not accepted.");
      setNotice(body.duplicate ? "This bid was already recorded. Its current status is shown below." : "Bid accepted. Its maximum cost is reserved until the auction clears or you cancel it."); setConfirmBid(false); submission.current = null;
      await Promise.all([refresh(), refreshAccount()]);
    } catch (error) { setFailure(error instanceof Error ? error.message : "Could not place your bid."); }
    finally { setBusy(false); }
  }
  async function cancel(id: string) {
    setBusy(true); setFailure(null); setNotice(null);
    try { const response = await api(`market/bids/${id}`, { method: "DELETE" }); const body = await response.json(); if (!response.ok) throw new Error(body.error?.message || "Could not cancel the bid."); setNotice("Bid cancelled. Reserved credit has been released."); await Promise.all([refresh(), refreshAccount()]); }
    catch (error) { setFailure(error instanceof Error ? error.message : "Could not cancel the bid."); } finally { setBusy(false); }
  }
  return <div className="mx-auto w-full max-w-7xl px-6 py-8 sm:px-10">
    <PageHeader title="Market" note="Capacity auctions · $ / IU-day" />
    {status?.mode === "test" && <p className="mb-5 text-sm text-warn">Test providers · no real inference or settlement.</p>}
    {(catalogError || marketError || failure) && <p role="alert" className="mb-5 border border-warn p-4 text-sm text-warn">{failure || catalogError || marketError}</p>}
    {notice && <p role="status" className="mb-5 border border-line p-4 text-sm text-good">{notice}</p>}
    <section className="grid grid-cols-2 border border-line md:grid-cols-4">
      <Tile label="Open auctions" value={String(openAuctions.length)} hint={`${models.length} live models`} />
      <Tile label="Capacity offered" value={fmtTokens(openAuctions.reduce((sum,a) => sum+a.units,0))} hint="IU / day" />
      <Tile label="Available credit" value={fmtUSD(available, 4)} hint={signedIn ? "Your account" : "Sign in to trade"} />
      <Tile label="Reserved credit" value={fmtUSD((account?.held_micro_usd || 0) / 1e6, 4)} hint="Open bids and requests" />
    </section>
    <div className="mt-8 grid gap-8 lg:grid-cols-[minmax(0,1fr)_330px]">
      <section className="min-w-0">
        <div className="mb-3 flex flex-wrap items-center justify-between gap-3"><h2 className="text-lg font-medium">Model capacity</h2><input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search models" aria-label="Search models" className="rounded-md border border-line bg-background px-3 py-2 text-sm" /></div>
        {loading && <p className="p-5 text-sm text-muted">Loading current model offers…</p>}
        <div className="overflow-x-auto border border-line"><table className="w-full min-w-[650px] text-left text-sm"><thead><tr className="border-b border-line"><th className="label px-4 py-3">Model</th><th className="label px-4 py-3">Context</th><th className="label px-4 py-3 text-right">Input / 1M</th><th className="label px-4 py-3 text-right">Output / 1M</th><th className="label px-4 py-3 text-right">Auctions</th></tr></thead><tbody>{sorted.map((item) => <tr key={item.id} onClick={() => select(item.id)} className={`cursor-pointer border-b border-line last:border-0 ${model?.id === item.id ? "bg-white/[0.06]" : "hover:bg-white/[0.03]"}`}><td className="px-4 py-3"><button onClick={() => select(item.id)} className="text-left">{item.name}</button><div className="mt-1 text-xs text-muted">{item.model}<br/>NEAR AI · {item.provider_type === "vllm" ? "Confidential TEE" : "Partner TEE"}</div></td><td className="px-4 py-3 font-mono text-xs text-muted">{item.context_length ? fmtTokens(item.context_length) : "—"}</td><td className="px-4 py-3 text-right font-mono">{fmtUSD(item.input_price_per_million_tokens_usd, 4)}</td><td className="px-4 py-3 text-right font-mono">{fmtUSD(item.price_per_million_tokens_usd, 4)}</td><td className="px-4 py-3 text-right font-mono">{openAuctions.filter((a) => a.route_id === item.id).length}</td></tr>)}</tbody></table></div>
        {model && <div className="mt-4 border border-line bg-panel p-5"><h3 className="font-medium">{model.name}</h3><p className="mt-2 text-sm leading-6 text-muted">{model.description}</p><div className="mt-3 flex flex-wrap gap-2">{model.supported_features?.map((feature) => <span key={feature} className="rounded-full border border-line px-3 py-1 text-xs text-muted">{feature.replaceAll("_", " ")}</span>)}</div><p className="mt-3 text-xs text-muted">Hardware attestation is reported by the provider; proof verification is not yet enabled.</p></div>}
      </section>
      <aside className="border border-line bg-panel p-5">
        <h2 className="text-lg font-medium">Bid for capacity</h2><p className="mt-1 text-sm text-muted">{model?.name || "Choose a model"}</p>
        <label className="label mt-6 block" htmlFor="quantity">Quantity · IU/day</label><input id="quantity" type="number" min={1} max={1000000} step={1} value={units} onChange={(event) => { setUnits(Math.max(1,Math.floor(Number(event.target.value) || 1))); setConfirmBid(false); }} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono" />
        <div className="label mt-5">Term</div><div className="mt-2 grid grid-cols-3 overflow-hidden rounded-md border border-line">{TERMS.map((term) => <button key={term} aria-pressed={days === term} onClick={() => { setDays(term); setPrice(""); setConfirmBid(false); }} className={`py-2 text-sm ${days === term ? "bg-foreground text-background" : "text-muted"}`}>{term}d</button>)}</div>
        <label className="label mt-5 block" htmlFor="bid-price">Maximum price · $ / IU-day</label><input id="bid-price" type="number" min="0.000001" step="0.001" value={effectivePrice} onChange={(event) => { setPrice(event.target.value); setConfirmBid(false); }} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono" />
        <dl className="mt-6 space-y-3 border-t border-line pt-4 text-sm"><div className="flex justify-between"><dt>Daily capacity</dt><dd>{fmtTokens(units*(model?.tokens_per_iu || 1e6))} tokens*</dd></div><div className="flex justify-between"><dt>Maximum reserved</dt><dd className="font-mono">{Number.isFinite(notional) ? fmtUSD(notional, 4) : "—"}</dd></div>{auction && <><div className="flex justify-between"><dt>Lot size</dt><dd>{auction.units} IU/day</dd></div><div className="flex justify-between"><dt>Reserve price</dt><dd>{fmtUSD(auction.reserve_price_micro_usd/1e6, 4)}</dd></div><div className="flex justify-between gap-4"><dt>Closes</dt><dd className="text-right">{new Date(auction.ends_at).toLocaleString()}</dd></div></>}</dl>
        {!signedIn ? <button onClick={login} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background">Sign in to bid</button> : !account ? <Link href="/dashboard/api-keys" className="mt-6 block rounded-md bg-foreground py-3 text-center text-sm font-medium text-background">Connect your account</Link> : !auction ? <p className="mt-6 text-sm text-muted">No {days}-day auction is open for this model. Choose another model or term.</p> : <><button disabled={busy || !Number.isFinite(notional) || notional <= 0 || available < notional} onClick={() => confirmBid ? placeBid() : setConfirmBid(true)} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background disabled:opacity-40">{busy ? "Submitting…" : confirmBid ? `Confirm bid · reserve ${fmtUSD(notional,4)}` : "Place bid"}</button>{confirmBid && <button onClick={() => setConfirmBid(false)} className="mt-2 w-full text-sm text-muted">Cancel</button>}{available < notional && <p className="mt-3 text-sm text-warn">Your available credit does not cover this bid. Add funds to your account before bidding.</p>}</>}
        <p className="mt-4 text-xs leading-5 text-muted">Bids reserve their maximum cost. At close, eligible bids fill in price/time order at one clearing price. Unspent reserves are released.</p>
      </aside>
    </div>
    <p className="mt-4 text-xs text-muted">* One IU is one million output-token equivalents per day. Input and output consume capacity at the model&apos;s contracted ratio. Daily capacity resets every 24 hours from the contract start; unused capacity does not roll over.</p>
    <section className="mt-10"><h2 className="mb-3 text-lg font-medium">Your bids</h2><div className="overflow-x-auto border border-line"><table className="w-full min-w-[650px] text-left text-sm"><thead><tr className="border-b border-line"><th className="label px-4 py-3">Model / term</th><th className="label px-4 py-3">Quantity</th><th className="label px-4 py-3">Max / IU-day</th><th className="label px-4 py-3">Status</th><th className="label px-4 py-3">Filled / paid</th><th /></tr></thead><tbody>{bids.map((bid) => <tr key={bid.id} className="border-b border-line last:border-0"><td className="px-4 py-3">{models.find((m) => m.id === bid.route_id)?.name || bid.route_id}<div className="text-xs text-muted">{bid.term_days} days</div></td><td className="px-4 py-3 font-mono">{bid.units} IU/day</td><td className="px-4 py-3 font-mono">{fmtUSD(bid.max_price_micro_usd/1e6,4)}</td><td className="px-4 py-3 capitalize">{bid.status}</td><td className="px-4 py-3 font-mono">{bid.filled_units} · {fmtUSD(bid.charged_micro_usd/1e6,4)}</td><td className="px-4 py-3">{bid.status === "open" && <button disabled={busy} onClick={() => cancel(bid.id)} className="text-muted">Cancel</button>}</td></tr>)}</tbody></table>{!bids.length && <p className="p-5 text-sm text-muted">{signedIn ? "No bids yet. Select an open auction to place your first bid." : "Sign in to see your bids."}</p>}</div></section>
    <section className="mt-10"><h2 className="mb-3 text-lg font-medium">Your capacity</h2><div className="grid gap-4 md:grid-cols-2">{activeContracts.map((contract) => <article key={contract.id} className="border border-line bg-panel p-5"><h3 className="font-medium">{models.find((m) => m.id === contract.route_id)?.name || contract.route_id}</h3><p className="mt-2 text-sm text-muted">{contract.units} IU/day · ends {new Date(contract.ends_at).toLocaleDateString()}</p><div className="mt-4 flex justify-between text-sm"><span>Remaining today</span><span className="font-mono">{fmtTokens(contract.remaining_tokens)} tokens</span></div><Link href={`/dashboard/playground?model=${encodeURIComponent(contract.route_id)}&contract=${contract.id}`} className="mt-4 inline-block rounded-md border border-line px-4 py-2 text-sm">Use in Playground →</Link></article>)}</div>{!activeContracts.length && <p className="border border-line p-5 text-sm text-muted">Won capacity will appear here after auction clearing. You can consume it in Playground or through an API key.</p>}</section>
  </div>;
}
function Tile({ label, value, hint }: { label: string; value: string; hint: string }) { return <div className="border-r border-line p-4 last:border-0"><div className="label">{label}</div><div className="mt-3 text-2xl font-medium tracking-tight tnum">{value}</div><div className="mt-1 text-xs text-muted">{hint}</div></div>; }
