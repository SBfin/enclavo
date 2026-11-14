"use client";

import { Suspense, useState, useSyncExternalStore } from "react";
import Link from "next/link";
import { useSearchParams } from "next/navigation";
import { useAccount } from "@/components/AccountProvider";
import PageHeader from "@/components/PageHeader";
import { useCatalog } from "@/lib/use-catalog";
import { useMarket } from "@/lib/use-market";
import { readChatStream } from "@/lib/chat-stream";
import { fmtUSD } from "@/lib/market";

const noSubscribe = () => () => {};

export default function PlaygroundPage() {
  return <Suspense fallback={<div className="p-8 text-sm text-muted">Loading Playground…</div>}><Playground /></Suspense>;
}

function Playground() {
  const { account, api, refresh, error: accountError } = useAccount();
  const params = useSearchParams();
  const { contracts, refresh: refreshMarket } = useMarket();
  const [paymentSource, setPaymentSource] = useState<string | null>(null);
  const origin = useSyncExternalStore(noSubscribe, () => window.location.origin, () => "");
  const { models, status, loading, error } = useCatalog();
  const [selected, setSelected] = useState("");
  const [prompt, setPrompt] = useState("Explain what a confidential computing enclave does in two sentences.");
  const [answer, setAnswer] = useState("");
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [cost, setCost] = useState<number | null>(null);
  const [copied, setCopied] = useState(false);
  // Default to a model the user owns capacity on, and to that capacity rather than credit.
  const owned = (id: string) => contracts.some((contract) => contract.active && contract.route_id === id);
  const model = models.find((item) => item.id === (selected || params.get("model"))) || models.find((item) => owned(item.id)) || models[0];
  const modelContracts = contracts.filter((contract) => contract.active && contract.route_id === model?.id);
  const wanted = paymentSource ?? params.get("contract");
  const selectedContract = wanted === "" ? undefined : modelContracts.find((contract) => contract.id === wanted) ?? modelContracts[0];
  const credit = (account?.balance_micro_usd || 0) / 1_000_000;
  const canSend = Boolean(account) && (Boolean(selectedContract) || credit > 0);
  const baseUrl = `${origin}/api/v1`;

  async function send() {
    if (!model || !prompt.trim() || !account) return;
    setBusy(true); setAnswer(""); setFailure(null); setCost(null);
    try {
      const response = await api("chat/completions", {
        method: "POST",
        body: JSON.stringify({ model: model.id, messages: [{ role: "user", content: prompt }],
          max_tokens: 512, stream: true,
          ...(selectedContract ? { capacity_contract_id: selectedContract.id } : {}),
          ...(model.model.includes("glm-5.3-flash") ? { reasoning_effort: "low" } : {}),
        }),
      });
      if (!response.ok) {
        const data = await response.json();
        throw new Error(data.error?.message || (response.status === 402 ? "The account needs more credit." : `Request failed (${response.status}).`));
      }
      const charged = await readChatStream(response, (text) => setAnswer((current) => current + text));
      setCost(charged);
      await Promise.all([refresh(), refreshMarket()]);
    } catch (error) {
      setFailure(error instanceof Error ? error.message : "The inference request failed.");
    } finally {
      setBusy(false);
    }
  }

  async function copy() {
    try { await navigator.clipboard.writeText(baseUrl); setCopied(true); }
    catch { setCopied(false); }
  }

  return (
    <div className="mx-auto w-full max-w-6xl px-6 py-8 sm:px-10">
      <PageHeader title="Playground" note={status ? (status.mode === "live" ? "Live inference" : "Test providers · simulated inference") : "Connecting to gateway"} />
      {(error || accountError) && <p role="alert" className="mb-6 border border-warn p-4 text-sm text-warn">{error || accountError}</p>}
      {!contracts.some((contract) => contract.active) && credit <= 0 && <p className="mb-6 border border-line p-4 text-sm text-muted">You don&apos;t own any capacity yet. <Link href="/dashboard/market" className="text-accent">Win an auction on the Market</Link>, then try it here.</p>}
      <section className="border border-line bg-panel p-5">
        <div className="label">API endpoint</div>
        <div className="mt-3 flex gap-3">
          <code className="min-w-0 flex-1 truncate rounded-md border border-line bg-background px-3 py-2 text-sm">{origin ? baseUrl : "…"}</code>
          <button type="button" onClick={copy} className="rounded-md border border-line px-4 py-2 text-sm">{copied ? "Copied" : "Copy"}</button>
        </div>
        <p className="mt-3 text-sm text-muted">Requests reach the inference gateway. Model availability and prices below come from its current configuration.</p>
        {status?.mode === "test" && <p className="mt-3 text-sm text-warn">This stack uses test providers. Responses and payment receipts are simulated.</p>}
      </section>

      <section className="mt-8 grid gap-6 lg:grid-cols-2">
        <div className="border border-line bg-panel p-5">
          <h2 className="text-lg font-medium">Run inference</h2>
          <label className="label mt-5 block" htmlFor="model">Model</label>
          <select id="model" value={model?.id || ""} onChange={(event) => setSelected(event.target.value)} disabled={!models.length || busy}
            className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 text-sm">
            {loading && <option>Loading routes…</option>}
            {models.map((item) => <option key={item.id} value={item.id}>{item.name} · {item.owned_by}</option>)}
          </select>
          <label className="label mt-5 block" htmlFor="payment-source">Spend from</label>
          <select id="payment-source" value={selectedContract?.id || ""} onChange={(event) => setPaymentSource(event.target.value)} disabled={!modelContracts.length && credit <= 0} className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 text-sm">
            {modelContracts.map((contract) => <option key={contract.id} value={contract.id}>Your capacity · {Math.floor(contract.remaining_tokens).toLocaleString()} tokens left today</option>)}
            {credit > 0 && <option value="">Enclavo credit · {fmtUSD(credit, 2)}</option>}
            {!modelContracts.length && credit <= 0 && <option value="">No capacity for this model</option>}
          </select>
          {selectedContract && <p className="mt-2 text-xs text-muted">Won at auction and paid on chain. Requests use its daily allowance at no extra charge.</p>}
          <label className="label mt-5 block" htmlFor="prompt">Prompt</label>
          <textarea id="prompt" rows={5} value={prompt} onChange={(event) => setPrompt(event.target.value)}
            className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 text-sm" />
          <button type="button" onClick={send} disabled={busy || !model || !canSend || !prompt.trim()}
            className="mt-4 rounded-md bg-foreground px-5 py-3 text-sm font-medium text-background disabled:opacity-40">
            {busy ? "Generating…" : "Send request"}
          </button>
          <p className="mt-3 text-xs text-muted">Requests run on the model provider&apos;s confidential hardware and use the selected capacity.</p>
        </div>
        <div className="border border-line bg-panel p-5" aria-live="polite" aria-busy={busy}>
          <h2 className="text-lg font-medium">Response</h2>
          <div className="mt-5 whitespace-pre-wrap text-sm leading-7">{answer || (busy ? "Waiting for the provider…" : "Your model response will appear here.")}</div>
          {failure && <p role="alert" className="mt-4 text-sm text-warn">{failure}</p>}
          {cost !== null && <p className="mt-5 border-t border-line pt-4 text-sm text-muted">{selectedContract ? "Consumed purchased capacity · no additional charge" : `Charged: ${fmtUSD(cost / 1_000_000, 6)}`}</p>}
          {account && <dl className="mt-4 space-y-2 text-sm">
            {selectedContract && <div className="flex justify-between"><dt>Capacity left today</dt><dd>{Math.floor(selectedContract.remaining_tokens).toLocaleString()} tokens</dd></div>}
            {credit > 0 && <div className="flex justify-between"><dt>Remaining credit</dt><dd>{fmtUSD(credit, 6)}</dd></div>}
            <div className="flex justify-between"><dt>Account requests</dt><dd>{account.requests}</dd></div>
          </dl>}
        </div>
      </section>

      <section className="mt-8 overflow-x-auto border border-line">
        <table className="w-full min-w-[620px] text-left text-sm">
          <thead><tr className="border-b border-line"><th className="label px-4 py-3">Route</th><th className="label px-4 py-3">Input / 1M tokens</th><th className="label px-4 py-3">Output / 1M tokens</th><th className="label px-4 py-3">Proof verification</th></tr></thead>
          <tbody>{models.map((item) => <tr key={item.id} className="border-b border-line last:border-0">
            <td className="px-4 py-3">{item.name}<div className="mt-1 font-mono text-xs text-muted">{item.id}</div></td>
            <td className="px-4 py-3 font-mono">{fmtUSD(item.input_price_per_million_tokens_usd, 4)}</td>
            <td className="px-4 py-3 font-mono">{fmtUSD(item.price_per_million_tokens_usd, 4)}</td>
            <td className="px-4 py-3 text-muted">Not verified</td>
          </tr>)}</tbody>
        </table>
      </section>
    </div>
  );
}
