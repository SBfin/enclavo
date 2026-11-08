"use client";

import Link from "next/link";
import PageHeader from "@/components/PageHeader";
import { useCatalog } from "@/lib/use-catalog";
import { fmtUSD } from "@/lib/market";

export default function MarketPage() {
  const { models, status, loading, error } = useCatalog();
  return (
    <div className="mx-auto w-full max-w-6xl px-6 py-8 sm:px-10">
      <PageHeader title="Market" note={status?.mode === "test" ? "Test catalog" : "Gateway catalog"} />
      <p className="mb-6 text-sm text-muted">Available inference routes and their current token prices. Purchase capacity by consuming prepaid credit through the router.</p>
      {loading && <p role="status" className="text-sm text-muted">Loading routes from the gateway…</p>}
      {error && <p role="alert" className="border border-warn p-4 text-sm text-warn">{error}</p>}
      {status?.mode === "test" && <p className="mb-6 text-sm text-warn">These are test providers. Switch to live configuration for real inference.</p>}
      <div className="grid gap-5 md:grid-cols-2">
        {models.map((model) => <article key={model.id} className="border border-line bg-panel p-5">
          <div className="label">{model.owned_by}</div>
          <h2 className="mt-2 text-xl font-medium">{model.name}</h2>
          <p className="mt-2 break-all font-mono text-xs text-muted">{model.id}</p>
          <dl className="mt-6 space-y-3 text-sm">
            <div className="flex justify-between"><dt>Input / 1M tokens</dt><dd className="font-mono">{fmtUSD(model.input_price_per_million_tokens_usd, 4)}</dd></div>
            <div className="flex justify-between"><dt>Output / 1M tokens</dt><dd className="font-mono">{fmtUSD(model.price_per_million_tokens_usd, 4)}</dd></div>
            <div className="flex justify-between gap-4"><dt>Attestation</dt><dd className="text-right text-muted">{model.attestation}</dd></div>
            <div className="flex justify-between"><dt>Route circuit</dt><dd>{model.available ? "Closed" : "Open · temporarily unavailable"}</dd></div>
          </dl>
          <Link href="/dashboard/router" className="mt-6 inline-block rounded-md bg-foreground px-5 py-3 text-sm font-medium text-background">Use inference</Link>
        </article>)}
      </div>
      <p className="mt-8 text-xs text-muted">Long-term capacity auctions are not available. No orders or future-delivery commitments are created here.</p>
    </div>
  );
}
