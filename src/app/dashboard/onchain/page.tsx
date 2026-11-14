"use client";
import { useCallback, useEffect, useState } from "react";
import PageHeader from "@/components/PageHeader";
import { useCatalog } from "@/lib/use-catalog";
import { useWallet } from "@/components/WalletProvider";
import { fmtTokens, fmtUSD } from "@/lib/market";
import { PLAIN_CALL, TOKEN_CALL, USD, newApiKey, sha256Hex, usd, view, type ChainAuction, type ChainBid, type ChainEntitlement } from "@/lib/near";

type Snapshot = { token: string; auctions: ChainAuction[]; bids: ChainBid[]; entitlements: ChainEntitlement[]; unlocked: string; wallet: string; keyHash: string | null };
const EMPTY: Snapshot = { token: "", auctions: [], bids: [], entitlements: [], unlocked: "0", wallet: "0", keyHash: null };
const FAUCET_AMOUNT = 1_000 * USD;

export default function OnchainPage() {
  const { models, status, loading, error: catalogError } = useCatalog();
  const chain = status?.chain ?? null;
  const contract = chain?.auction_contract ?? "";
  // The same wallet connection the rest of the app signs in with.
  const { ready, walletAccount: accountId, connect, signOut: disconnect, call } = useWallet();
  const [data, setData] = useState<Snapshot>(EMPTY);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [now, setNow] = useState(0);
  const [selection, setSelection] = useState<number | null>(null);
  const [units, setUnits] = useState(1);
  const [price, setPrice] = useState("");
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [apiKey, setApiKey] = useState<string | null>(null);

  const load = useCallback(async (signal?: AbortSignal): Promise<Snapshot> => {
    if (!chain) return EMPTY;
    const read = <T,>(target: string, method: string, args: object = {}) => view<T>(chain, target, method, args, signal);
    const [config, auctions] = await Promise.all([read<{ token: string }>(contract, "get_config"), read<ChainAuction[]>(contract, "list_auctions", { limit: 50 })]);
    if (!accountId) return { ...EMPTY, token: config.token, auctions };
    const owner = { account_id: accountId };
    const [bids, entitlements, unlocked, wallet, keyHash] = await Promise.all([
      read<ChainBid[]>(contract, "bids_of", owner), read<ChainEntitlement[]>(contract, "entitlements_of", owner),
      read<string>(contract, "balance_of", owner), read<string>(config.token, "ft_balance_of", owner), read<string | null>(contract, "key_hash_of", owner),
    ]);
    return { token: config.token, auctions, bids, entitlements, unlocked, wallet, keyHash };
  }, [chain, contract, accountId]);

  const refresh = useCallback(async () => { try { setData(await load()); setLoadError(null); } catch (error) { setLoadError(error instanceof Error ? error.message : "Could not read the contract."); } }, [load]);
  useEffect(() => {
    const controller = new AbortController();
    const update = () => { setNow(Date.now()); void load(controller.signal).then((result) => { if (!controller.signal.aborted) { setData(result); setLoadError(null); } }).catch((error) => { if (!controller.signal.aborted) setLoadError(error.message); }); };
    update(); const timer = setInterval(update, 8_000);
    return () => { controller.abort(); clearInterval(timer); };
  }, [load]);

  // A key generated in this browser is shown only while its hash is the one authorized on chain.
  useEffect(() => {
    let cancelled = false;
    const stored = accountId ? sessionStorage.getItem(`enclavo-key:${accountId}`) : null;
    void (stored ? sha256Hex(stored) : Promise.resolve(null)).then((hash) => { if (!cancelled) setApiKey(stored && hash === data.keyHash ? stored : null); });
    return () => { cancelled = true; };
  }, [accountId, data.keyHash]);

  const accepting = (auction: ChainAuction) => auction.status === "open" && now < auction.ends_at_ms;
  const open = data.auctions.filter(accepting);
  const auction = data.auctions.find((item) => item.id === selection) ?? open[0] ?? data.auctions[0];
  const modelName = (route: string) => models.find((model) => model.id === route)?.name || route;
  const effectivePrice = price || String(usd(auction?.reserve_price));
  const priceAtomic = Math.round(Number(effectivePrice) * USD);
  const escrow = auction ? units * auction.term_days * priceAtomic : 0;
  const funds = Number(data.unlocked) + Number(data.wallet);
  const belowReserve = !!auction && priceAtomic < Number(auction.reserve_price);

  async function run(done: string, action: () => Promise<unknown>) {
    setBusy(true); setFailure(null); setNotice(null);
    try { await action(); setNotice(done); await refresh(); }
    catch (error) { setFailure(error instanceof Error ? error.message : "The transaction was not completed."); }
    finally { setBusy(false); }
  }
  const placeBid = () => run("Bid placed. Its maximum cost is held in the contract until the auction clears or you cancel.", () => {
    if (!auction) throw new Error("Choose an auction.");
    const bid = { auction_id: auction.id, units, max_price: String(priceAtomic) };
    // Funds already unlocked in the contract are used first; otherwise one token transfer carries the bid.
    if (Number(data.unlocked) >= escrow) return call(contract, { method: "place_bid", args: bid, ...PLAIN_CALL });
    return call(data.token, { method: "ft_transfer_call", args: { receiver_id: contract, amount: String(escrow), msg: JSON.stringify(bid) }, ...TOKEN_CALL });
  });
  const cancel = (bid: ChainBid) => run("Bid cancelled. Its escrow is unlocked.", () => call(contract, { method: "cancel_bid", args: { auction_id: bid.auction_id, index: bid.index }, ...PLAIN_CALL }));
  const settle = (id: number) => run("Auction settled. Winners hold entitlements and unspent escrow is unlocked.", () => call(contract, { method: "clear_auction", args: { auction_id: id }, ...PLAIN_CALL }));
  const withdraw = () => run("Unlocked balance sent to your wallet.", () => call(contract, { method: "withdraw", args: {}, ...PLAIN_CALL }));
  const faucet = () => run("Test tokens minted to your wallet.", () => call(data.token, { method: "mint", args: { account_id: accountId, amount: String(FAUCET_AMOUNT) }, ...PLAIN_CALL }));
  const authorize = () => run("Key authorized. The gateway accepts it within a few seconds.", async () => {
    const key = newApiKey();
    // Stored before signing so a wallet that redirects away does not lose it.
    sessionStorage.setItem(`enclavo-key:${accountId}`, key);
    await call(contract, { method: "authorize_key", args: { key_hash: await sha256Hex(key) }, ...PLAIN_CALL });
  });

  if (!loading && !chain) return <div className="mx-auto w-full max-w-7xl px-6 py-8 sm:px-10"><PageHeader title="On-chain market" /><p className="border border-line p-5 text-sm text-muted">{catalogError || "This gateway is not connected to an auction contract."}</p></div>;

  return <div className="mx-auto w-full max-w-7xl px-6 py-8 sm:px-10">
    <PageHeader title="On-chain market" note={chain ? `NEAR ${chain.network} · ${contract}` : "NEAR"} />
    {(loadError || failure) && <p role="alert" className="mb-5 border border-warn p-4 text-sm text-warn">{failure || loadError}</p>}
    {notice && <p role="status" className="mb-5 border border-line p-4 text-sm text-good">{notice}</p>}
    <section className="grid grid-cols-2 border border-line md:grid-cols-4">
      <Tile label="Open auctions" value={String(open.length)} hint={`${fmtTokens(open.reduce((sum, item) => sum + item.units, 0))} IU / day offered`} />
      <Tile label="Wallet balance" value={fmtUSD(usd(data.wallet), 2)} hint={accountId ? "Stablecoin in your wallet" : "Connect a wallet"} />
      <Tile label="Unlocked in contract" value={fmtUSD(usd(data.unlocked), 2)} hint="Refunds ready to withdraw" />
      <div className="p-4"><div className="label">NEAR wallet</div>{accountId
        ? <><div className="mt-3 truncate text-sm" title={accountId}>{accountId}</div><button onClick={() => void disconnect()} className="mt-1 text-xs text-muted hover:text-foreground">Disconnect</button></>
        : <button disabled={!ready} onClick={() => void connect()} className="mt-3 rounded-md bg-foreground px-4 py-2 text-sm font-medium text-background disabled:opacity-40">Connect wallet</button>}</div>
    </section>
    {accountId && <div className="mt-3 flex flex-wrap gap-3 text-sm">
      {chain?.network === "testnet" && <button disabled={busy} onClick={faucet} className="rounded-md border border-line px-4 py-2 disabled:opacity-40">Get 1,000 test mUSDC</button>}
      {Number(data.unlocked) > 0 && <button disabled={busy} onClick={withdraw} className="rounded-md border border-line px-4 py-2 disabled:opacity-40">Withdraw {fmtUSD(usd(data.unlocked), 2)}</button>}
    </div>}

    <div className="mt-8 grid gap-8 lg:grid-cols-[minmax(0,1fr)_330px]">
      <section className="min-w-0">
        <h2 className="mb-3 text-lg font-medium">Auctions</h2>
        <div className="overflow-x-auto border border-line"><table className="w-full min-w-[650px] text-left text-sm"><thead><tr className="border-b border-line"><th className="label px-4 py-3">Model / term</th><th className="label px-4 py-3 text-right">Lot</th><th className="label px-4 py-3 text-right">Reserve</th><th className="label px-4 py-3 text-right">Bids</th><th className="label px-4 py-3">State</th><th /></tr></thead><tbody>{data.auctions.map((item) => <tr key={item.id} onClick={() => { setSelection(item.id); setPrice(""); }} className={`cursor-pointer border-b border-line last:border-0 ${auction?.id === item.id ? "bg-white/[0.06]" : "hover:bg-white/[0.03]"}`}>
          <td className="px-4 py-3">{modelName(item.route_id)}<div className="text-xs text-muted">#{item.id} · {item.term_days} days</div></td>
          <td className="px-4 py-3 text-right font-mono">{item.units} IU/day</td>
          <td className="px-4 py-3 text-right font-mono">{fmtUSD(usd(item.reserve_price), 4)}</td>
          <td className="px-4 py-3 text-right font-mono">{item.status === "open" ? `${item.open_bids} · ${item.open_bid_units} IU` : `${item.filled_units} IU filled`}</td>
          <td className="px-4 py-3">{accepting(item) ? <>Open<div className="text-xs text-muted">closes {new Date(item.ends_at_ms).toLocaleString()}</div></> : item.status === "open" ? "Closed · awaiting settlement" : <>Cleared<div className="text-xs text-muted">{item.clearing_price ? `at ${fmtUSD(usd(item.clearing_price), 4)}` : "no fills"}</div></>}</td>
          <td className="px-4 py-3 text-right">{item.status === "open" && !accepting(item) && now > 0 && <button disabled={busy || !accountId} onClick={(event) => { event.stopPropagation(); void settle(item.id); }} className="rounded-md border border-line px-3 py-1 disabled:opacity-40">Settle</button>}</td>
        </tr>)}</tbody></table>{!data.auctions.length && <p className="p-5 text-sm text-muted">{chain ? "No auctions have been published on this contract yet." : "Loading the contract…"}</p>}</div>
        <p className="mt-3 text-xs leading-5 text-muted">Bids are escrowed by the contract, not by Enclavo. After the close anyone can settle: eligible bids fill in price/time order at one clearing price, and whatever a bid did not spend is unlocked for withdrawal. Bids are public on chain.</p>
      </section>
      <aside className="border border-line bg-panel p-5">
        <h2 className="text-lg font-medium">Bid on chain</h2><p className="mt-1 text-sm text-muted">{auction ? `${modelName(auction.route_id)} · ${auction.term_days} days` : "Choose an auction"}</p>
        <label className="label mt-6 block" htmlFor="chain-quantity">Quantity · IU/day</label><input id="chain-quantity" type="number" min={1} max={1000000} step={1} value={units} onChange={(event) => setUnits(Math.max(1, Math.floor(Number(event.target.value) || 1)))} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono" />
        <label className="label mt-5 block" htmlFor="chain-price">Maximum price · $ / IU-day</label><input id="chain-price" type="number" min="0.000001" step="0.001" value={effectivePrice} onChange={(event) => setPrice(event.target.value)} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono" />
        <dl className="mt-6 space-y-3 border-t border-line pt-4 text-sm"><div className="flex justify-between"><dt>Daily capacity</dt><dd>{fmtTokens(units * (auction?.tokens_per_iu || 1e6))} tokens</dd></div><div className="flex justify-between"><dt>Escrowed now</dt><dd className="font-mono">{Number.isFinite(escrow) ? fmtUSD(usd(escrow), 4) : "—"}</dd></div>{auction && <div className="flex justify-between"><dt>Reserve price</dt><dd>{fmtUSD(usd(auction.reserve_price), 4)}</dd></div>}</dl>
        {!accountId ? <button disabled={!ready} onClick={() => void connect()} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background disabled:opacity-40">Connect wallet to bid</button>
          : !auction || !accepting(auction) ? <p className="mt-6 text-sm text-muted">This auction is not accepting bids. Choose an open one.</p>
          : <><button disabled={busy || !(escrow > 0) || belowReserve || funds < escrow} onClick={placeBid} className="mt-6 w-full rounded-md bg-foreground py-3 text-sm font-medium text-background disabled:opacity-40">{busy ? "Waiting for wallet…" : `Place bid · escrow ${fmtUSD(usd(escrow), 2)}`}</button>
            {belowReserve && <p className="mt-3 text-sm text-warn">The contract rejects bids below the reserve price.</p>}
            {!belowReserve && funds < escrow && <p className="mt-3 text-sm text-warn">Your wallet does not hold enough stablecoin for this bid.</p>}</>}
      </aside>
    </div>

    <section className="mt-10"><h2 className="mb-3 text-lg font-medium">Your bids</h2><div className="overflow-x-auto border border-line"><table className="w-full min-w-[650px] text-left text-sm"><thead><tr className="border-b border-line"><th className="label px-4 py-3">Auction</th><th className="label px-4 py-3">Quantity</th><th className="label px-4 py-3">Max / IU-day</th><th className="label px-4 py-3">Status</th><th className="label px-4 py-3">Filled / paid</th><th /></tr></thead><tbody>{data.bids.map((bid) => { const lot = data.auctions.find((item) => item.id === bid.auction_id); return <tr key={`${bid.auction_id}:${bid.index}`} className="border-b border-line last:border-0">
      <td className="px-4 py-3">{lot ? modelName(lot.route_id) : `Auction ${bid.auction_id}`}<div className="text-xs text-muted">#{bid.auction_id}{lot ? ` · ${lot.term_days} days` : ""}</div></td>
      <td className="px-4 py-3 font-mono">{bid.units} IU/day</td><td className="px-4 py-3 font-mono">{fmtUSD(usd(bid.max_price), 4)}</td><td className="px-4 py-3 capitalize">{bid.status}</td>
      <td className="px-4 py-3 font-mono">{bid.filled_units} · {fmtUSD(usd(bid.charged), 4)}</td>
      <td className="px-4 py-3">{bid.status === "open" && lot && accepting(lot) && <button disabled={busy} onClick={() => void cancel(bid)} className="text-muted">Cancel</button>}</td>
    </tr>; })}</tbody></table>{!data.bids.length && <p className="p-5 text-sm text-muted">{accountId ? "No on-chain bids from this wallet yet." : "Connect a wallet to see its bids."}</p>}</div></section>

    <section className="mt-10"><h2 className="mb-3 text-lg font-medium">Your capacity</h2>
      <div className="grid gap-4 md:grid-cols-2">{data.entitlements.map((item) => <article key={item.id} className="border border-line bg-panel p-5"><h3 className="font-medium">{modelName(item.route_id)}</h3><p className="mt-2 text-sm text-muted">{item.units} IU/day · {fmtTokens(item.units * item.tokens_per_iu)} tokens a day · ends {new Date(item.ends_at_ms).toLocaleDateString()}</p><div className="mt-4 flex justify-between text-sm"><span>Entitlement #{item.id}</span><span className="font-mono">paid {fmtUSD(usd(item.paid), 2)}</span></div></article>)}</div>
      {!data.entitlements.length && <p className="border border-line p-5 text-sm text-muted">Capacity won on chain appears here once its auction is settled.</p>}
      {data.entitlements.length > 0 && <div className="mt-4 border border-line p-5 text-sm">
        <h3 className="font-medium">Gateway access</h3>
        <p className="mt-2 leading-6 text-muted">Your wallet authorizes an API key by publishing only its hash. The gateway then serves this wallet&apos;s capacity to that key; no Enclavo account is needed. Authorizing again replaces the previous key.</p>
        {apiKey && <><p className="mt-4 text-good">Copy this key now. It is kept only in this browser tab.</p><pre className="mt-2 overflow-x-auto border border-line bg-background p-3 font-mono text-xs">{apiKey}</pre><p className="mt-3 text-muted">List your capacity with <span className="font-mono">GET /api/v1/contracts</span>, then pass a contract <span className="font-mono">id</span> as <span className="font-mono">capacity_contract_id</span> in a chat request.</p></>}
        {!apiKey && data.keyHash && <p className="mt-4 text-muted">A key is authorized for this wallet <span className="font-mono">(sha256 {data.keyHash.slice(0, 12)}…)</span>.</p>}
        <button disabled={busy} onClick={authorize} className="mt-4 rounded-md border border-line px-4 py-2 disabled:opacity-40">{data.keyHash ? "Replace API key" : "Authorize an API key"}</button>
      </div>}
    </section>
  </div>;
}
function Tile({ label, value, hint }: { label: string; value: string; hint: string }) { return <div className="border-r border-line p-4"><div className="label">{label}</div><div className="mt-3 text-2xl font-medium tracking-tight tnum">{value}</div><div className="mt-1 text-xs text-muted">{hint}</div></div>; }
