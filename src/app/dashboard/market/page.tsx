"use client";

import { useCallback, useEffect, useMemo, useState } from "react";
import { usePrivy } from "@privy-io/react-auth";
import PageHeader from "@/components/PageHeader";
import { FLOOR, INDEX, ROUTES, TERMS, findRoute, fmtTokens, fmtUSD } from "@/lib/market";
import type { Bid } from "@/app/api/bids/route";

export default function MarketPage() {
  const { authenticated, getAccessToken } = usePrivy();
  const [routeId, setRouteId] = useState(ROUTES[0].id);
  const [iu, setIu] = useState(1000);
  const [days, setDays] = useState<number>(30);
  const [maxPrice, setMaxPrice] = useState(ROUTES[0].pricePerIUDay);
  const [bids, setBids] = useState<Bid[]>([]);
  const [status, setStatus] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const [busy, setBusy] = useState(false);

  const route = findRoute(routeId) ?? ROUTES[0];
  const tokensPerDay = iu * route.tokensPerIU;
  const notional = iu * days * maxPrice;

  const authedFetch = useCallback(
    async (init?: RequestInit) => {
      const token = await getAccessToken();
      return fetch("/api/bids", {
        ...init,
        headers: { "content-type": "application/json", authorization: `Bearer ${token ?? ""}`, ...init?.headers },
      });
    },
    [getAccessToken],
  );

  useEffect(() => {
    if (!authenticated) return;
    authedFetch()
      .then((r) => r.json())
      .then((d) => setBids(d.bids ?? []))
      .catch(() => setBids([]));
  }, [authenticated, authedFetch]);

  const selectRoute = (id: string) => {
    const r = findRoute(id);
    if (!r) return;
    setRouteId(id);
    setMaxPrice(r.pricePerIUDay);
    setStatus(null);
  };

  const placeBid = async () => {
    setBusy(true);
    setStatus(null);
    try {
      const res = await authedFetch({ method: "POST", body: JSON.stringify({ routeId, iu, days, maxPrice }) });
      const data = await res.json();
      if (!res.ok) {
        setStatus({ kind: "error", text: data.error ?? "The bid was not placed." });
        return;
      }
      setBids((b) => [data.bid, ...b]);
      setStatus({ kind: "ok", text: "Bid placed. It enters the next daily auction." });
    } catch {
      setStatus({ kind: "error", text: "Couldn't reach the server. Try again." });
    } finally {
      setBusy(false);
    }
  };

  const sorted = useMemo(() => [...ROUTES].sort((a, b) => a.pricePerIUDay - b.pricePerIUDay), []);

  return (
    <div className="mx-auto w-full max-w-6xl px-6 py-8 sm:px-10">
      <PageHeader title="Market" note="30-day blocks · $ / IU-day" />

      <section className="grid grid-cols-2 border border-line md:grid-cols-4">
        <Tile label="Private index" value={fmtUSD(INDEX.privateIndex)} accent />
        <Tile label="Private cheapest" value={fmtUSD(INDEX.privateCheapest)} />
        <Tile label="Reference" value={fmtUSD(INDEX.reference)} />
        <Tile label="Attested routes" value={String(ROUTES.length)} unit="" />
      </section>

      <div className="mt-8 grid gap-8 lg:grid-cols-[minmax(0,1fr)_320px]">
        <section className="min-w-0">
          <h2 className="mb-3 text-lg font-medium">Routes</h2>
          <div className="overflow-x-auto border border-line">
            <table className="w-full min-w-[560px] text-sm">
              <thead>
                <tr className="border-b border-line text-left">
                  <th className="label px-4 py-3 font-normal">Model</th>
                  <th className="label px-4 py-3 font-normal">Provider</th>
                  <th className="label px-4 py-3 text-right font-normal">Score</th>
                  <th className="label px-4 py-3 text-right font-normal">Tokens / IU</th>
                  <th className="label px-4 py-3 text-right font-normal">Price</th>
                </tr>
              </thead>
              <tbody>
                {sorted.map((r) => {
                  const selected = r.id === routeId;
                  return (
                    <tr
                      key={r.id}
                      onClick={() => selectRoute(r.id)}
                      className={`cursor-pointer border-b border-line last:border-0 transition ${selected ? "bg-white/[0.06]" : "hover:bg-white/[0.03]"}`}
                    >
                      <td className="px-4 py-3">
                        <button type="button" onClick={() => selectRoute(r.id)} className="text-left">
                          {r.model}
                        </button>
                      </td>
                      <td className="px-4 py-3 text-muted">
                        {r.provider} <span className="text-xs">· {r.attestation}</span>
                      </td>
                      <td className={`px-4 py-3 text-right tnum ${r.score < FLOOR ? "text-warn" : ""}`}>{r.score}</td>
                      <td className="px-4 py-3 text-right font-mono tnum">{fmtTokens(r.tokensPerIU)}</td>
                      <td className="px-4 py-3 text-right font-mono tnum">{fmtUSD(r.pricePerIUDay)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          <p className="mt-3 text-xs text-muted">
            Scores below {FLOOR} can&apos;t deliver into floor-{FLOOR} contracts.
          </p>
        </section>

        <aside className="border border-line bg-panel p-5">
          <h2 className="text-lg font-medium">Bid</h2>
          <p className="mt-1 text-sm text-muted">
            {route.model} · {route.provider}
          </p>

          <div className="mt-6 space-y-5">
            <Field label="Quantity" hint="IU">
              <input
                id="iu"
                type="number"
                min={10}
                step={10}
                value={iu}
                onChange={(e) => setIu(Math.max(0, Math.round(Number(e.target.value) || 0)))}
                className="w-full rounded-md border border-line bg-background px-3 py-2 font-mono tnum"
              />
            </Field>

            <Field label="Term">
              <div className="grid grid-cols-3 overflow-hidden rounded-md border border-line">
                {TERMS.map((d) => (
                  <button
                    key={d}
                    type="button"
                    aria-pressed={days === d}
                    onClick={() => setDays(d)}
                    className={`py-2 text-sm transition ${days === d ? "bg-foreground text-background" : "text-muted hover:text-foreground"}`}
                  >
                    {d}d
                  </button>
                ))}
              </div>
            </Field>

            <Field label="Max price" hint="$ / IU-day">
              <input
                id="maxPrice"
                type="number"
                min={0}
                step={0.001}
                value={maxPrice}
                onChange={(e) => setMaxPrice(Number(e.target.value) || 0)}
                className="w-full rounded-md border border-line bg-background px-3 py-2 font-mono tnum"
              />
            </Field>
          </div>

          <dl className="mt-6 space-y-2 border-t border-line pt-4 text-sm">
            <Row label="Tokens / day" value={fmtTokens(tokensPerDay)} />
            <Row label="Max cost" value={fmtUSD(notional)} />
          </dl>

          <button
            type="button"
            onClick={placeBid}
            disabled={busy}
            className="mt-6 w-full rounded-md bg-foreground py-3 text-[15px] font-medium text-background transition hover:bg-accent disabled:opacity-50"
          >
            {busy ? "Placing…" : "Place bid"}
          </button>
          {status && (
            <p role="status" className={`mt-3 text-sm ${status.kind === "ok" ? "text-good" : "text-warn"}`}>
              {status.text}
            </p>
          )}
        </aside>
      </div>

      <section className="mt-10">
        <h2 className="mb-3 text-lg font-medium">Your bids</h2>
        {bids.length === 0 ? (
          <p className="border border-dashed border-line px-4 py-6 text-sm text-muted">
            No bids yet. Pick a route and place one above.
          </p>
        ) : (
          <ul className="border border-line">
            {bids.map((b) => {
              const r = findRoute(b.routeId);
              return (
                <li key={b.id} className="flex flex-wrap items-center justify-between gap-2 border-b border-line px-4 py-3 text-sm last:border-0">
                  <span>
                    {r?.model} <span className="text-muted">· {r?.provider}</span>
                  </span>
                  <span className="font-mono tnum text-muted">
                    {b.iu.toLocaleString("en-US")} IU · {b.days}d · ≤ {fmtUSD(b.maxPrice)}
                  </span>
                </li>
              );
            })}
          </ul>
        )}
      </section>
    </div>
  );
}

function Tile({ label, value, unit = "/ IU-day", accent }: { label: string; value: string; unit?: string; accent?: boolean }) {
  return (
    <div className="border-line p-5 [&:not(:first-child)]:border-l max-md:[&:nth-child(3)]:border-l-0 max-md:[&:nth-child(n+3)]:border-t">
      <div className="label">{label}</div>
      <div className={`mt-2 text-3xl font-light tracking-tight tnum ${accent ? "text-accent" : ""}`}>
        {value} {unit && <span className="text-sm text-muted">{unit}</span>}
      </div>
    </div>
  );
}

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <div>
      <div className="mb-2 flex justify-between">
        <span className="text-sm">{label}</span>
        {hint && <span className="label">{hint}</span>}
      </div>
      {children}
    </div>
  );
}

function Row({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex justify-between">
      <dt className="text-muted">{label}</dt>
      <dd className="font-mono tnum">{value}</dd>
    </div>
  );
}
