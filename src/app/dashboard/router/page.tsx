"use client";

import { useState, useSyncExternalStore } from "react";
import PageHeader from "@/components/PageHeader";
import { FLOOR, ROUTES, fmtTokens } from "@/lib/market";

// The origin never changes while the page is open, so there is nothing to subscribe to.
const noSubscribe = () => () => {};

export default function RouterPage() {
  const origin = useSyncExternalStore(noSubscribe, () => window.location.origin, () => "");
  const [copied, setCopied] = useState(false);

  const baseUrl = `${origin}/api/v1`;
  const example = `curl ${baseUrl}/models`;

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(baseUrl);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      setCopied(false);
    }
  };

  return (
    <div className="mx-auto w-full max-w-6xl px-6 py-8 sm:px-10">
      <PageHeader title="Router" note="OpenAI-compatible · testnet" />

      <section className="border border-line bg-panel p-5">
        <div className="label">Endpoint</div>
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <code className="min-w-0 flex-1 truncate rounded-md border border-line bg-background px-3 py-2 font-mono text-sm">
            {origin ? baseUrl : "…"}
          </code>
          <button
            type="button"
            onClick={copy}
            className="rounded-md border border-line px-4 py-2 text-sm text-muted transition hover:text-foreground"
          >
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
        <pre className="mt-4 overflow-x-auto rounded-md border border-line bg-background px-3 py-2 font-mono text-xs text-muted">
          {origin ? example : "…"}
        </pre>
        <p className="mt-3 text-xs text-muted">
          Model listing is live. Chat completions through attested routes come next.
        </p>
      </section>

      <section className="mt-8">
        <h2 className="mb-3 text-lg font-medium">Routes</h2>
        <div className="overflow-x-auto border border-line">
          <table className="w-full min-w-[620px] text-sm">
            <thead>
              <tr className="border-b border-line text-left">
                <th className="label px-4 py-3 font-normal">Model ID</th>
                <th className="label px-4 py-3 font-normal">Provider</th>
                <th className="label px-4 py-3 font-normal">Attestation</th>
                <th className="label px-4 py-3 text-right font-normal">Score</th>
                <th className="label px-4 py-3 text-right font-normal">Tokens / IU</th>
              </tr>
            </thead>
            <tbody>
              {ROUTES.map((r) => (
                <tr key={r.id} className="border-b border-line last:border-0">
                  <td className="px-4 py-3 font-mono text-xs">{r.id}</td>
                  <td className="px-4 py-3">{r.provider}</td>
                  <td className="px-4 py-3 text-muted">{r.attestation}</td>
                  <td className={`px-4 py-3 text-right tnum ${r.score < FLOOR ? "text-warn" : ""}`}>{r.score}</td>
                  <td className="px-4 py-3 text-right font-mono tnum">{fmtTokens(r.tokensPerIU)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </section>
    </div>
  );
}
