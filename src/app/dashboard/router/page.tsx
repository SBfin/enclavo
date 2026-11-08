"use client";

import { useState, useSyncExternalStore } from "react";
import PageHeader from "@/components/PageHeader";
import { useCatalog } from "@/lib/use-catalog";
import { readChatStream } from "@/lib/chat-stream";
import { fmtUSD } from "@/lib/market";

const noSubscribe = () => () => {};
type Account = { balance_micro_usd: number; requests: number; spent_micro_usd: number };

export default function RouterPage() {
  const origin = useSyncExternalStore(noSubscribe, () => window.location.origin, () => "");
  const { models, status, loading, error } = useCatalog();
  const [selected, setSelected] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [prompt, setPrompt] = useState("Explain what a confidential computing enclave does in two sentences.");
  const [answer, setAnswer] = useState("");
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [cost, setCost] = useState<number | null>(null);
  const [account, setAccount] = useState<Account | null>(null);
  const [copied, setCopied] = useState(false);
  const model = models.find((item) => item.id === selected) || models[0];
  const baseUrl = `${origin}/api/v1`;

  async function send() {
    if (!model || !prompt.trim() || !apiKey.trim()) return;
    setBusy(true); setAnswer(""); setFailure(null); setCost(null); setAccount(null);
    const headers = { "content-type": "application/json", authorization: `Bearer ${apiKey.trim()}` };
    try {
      const response = await fetch("/api/v1/chat/completions", {
        method: "POST", headers,
        body: JSON.stringify({ model: model.id, messages: [{ role: "user", content: prompt }],
          max_tokens: 512, stream: true,
          ...(model.model.includes("glm-5.3-flash") ? { reasoning_effort: "low" } : {}),
        }),
      });
      if (!response.ok) {
        const data = await response.json();
        throw new Error(data.error?.message || (response.status === 402 ? "The account needs more credit." : `Request failed (${response.status}).`));
      }
      const charged = await readChatStream(response, (text) => setAnswer((current) => current + text));
      setCost(charged);
      const accountResponse = await fetch("/api/v1/account", { headers, cache: "no-store" });
      if (accountResponse.ok) setAccount(await accountResponse.json());
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
      <PageHeader title="Router" note={status ? (status.mode === "live" ? "Live inference" : "Test providers · simulated inference") : "Connecting to gateway"} />
      {error && <p role="alert" className="mb-6 border border-warn p-4 text-sm text-warn">{error}</p>}
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
          <label className="label mt-5 block" htmlFor="apiKey">Enclavo API key</label>
          <input id="apiKey" type="password" autoComplete="off" spellCheck={false} value={apiKey}
            onChange={(event) => setApiKey(event.target.value)} placeholder="ek_…"
            className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 font-mono text-sm" />
          <p className="mt-2 text-xs text-muted">Use a funded gateway key. It stays in this page&apos;s memory; your provider key stays on the server.</p>
          <label className="label mt-5 block" htmlFor="prompt">Prompt</label>
          <textarea id="prompt" rows={5} value={prompt} onChange={(event) => setPrompt(event.target.value)}
            className="mt-2 w-full rounded-md border border-line bg-background px-3 py-2 text-sm" />
          <button type="button" onClick={send} disabled={busy || !model || !apiKey.trim() || !prompt.trim()}
            className="mt-4 rounded-md bg-foreground px-5 py-3 text-sm font-medium text-background disabled:opacity-40">
            {busy ? "Generating…" : "Send request"}
          </button>
          <p className="mt-3 text-xs text-muted">Sending uses the account&apos;s prepaid credit and, in live mode, incurs a real provider charge.</p>
        </div>
        <div className="border border-line bg-panel p-5" aria-live="polite" aria-busy={busy}>
          <h2 className="text-lg font-medium">Response</h2>
          <div className="mt-5 whitespace-pre-wrap text-sm leading-7">{answer || (busy ? "Waiting for the provider…" : "Your model response will appear here.")}</div>
          {failure && <p role="alert" className="mt-4 text-sm text-warn">{failure}</p>}
          {cost !== null && <p className="mt-5 border-t border-line pt-4 text-sm text-muted">Charged: {fmtUSD(cost / 1_000_000, 6)}</p>}
          {account && <dl className="mt-4 space-y-2 text-sm">
            <div className="flex justify-between"><dt>Remaining credit</dt><dd>{fmtUSD(account.balance_micro_usd / 1_000_000, 6)}</dd></div>
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
