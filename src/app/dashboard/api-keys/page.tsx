"use client";

import Link from "next/link";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import PageHeader from "@/components/PageHeader";
import { useAccount } from "@/components/AccountProvider";

type Key = { id: string; name: string; prefix: string; created_at: string; revoked_at: string | null };
const noSubscribe = () => () => {};

export default function ApiKeysPage() {
  const { account, api, refresh, signedIn, loading, error: accountError } = useAccount();
  const origin = useSyncExternalStore(noSubscribe, () => window.location.origin, () => "");
  const [keys, setKeys] = useState<Key[]>([]);
  const [name, setName] = useState("My application");
  const [secret, setSecret] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirmRevoke, setConfirmRevoke] = useState<string | null>(null);
  const load = useCallback(async () => {
    const response = await api("keys"); const body = await response.json();
    if (!response.ok) throw new Error(body.error?.message || "Could not load API keys.");
    setKeys(body.keys);
  }, [api]);
  useEffect(() => {
    if (!signedIn) return;
    let cancelled = false;
    void api("keys").then(async (response) => {
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message || "Could not load API keys.");
      if (!cancelled) setKeys(body.keys);
    }).catch((error) => { if (!cancelled) setError(error.message); });
    return () => { cancelled = true; };
  }, [signedIn, api]);
  async function action(path: string, method: string, body?: object) {
    setBusy(true); setError(null); setNotice(null);
    try {
      const response = await api(path, { method, ...(body ? { body: JSON.stringify(body) } : {}) });
      const result = await response.json();
      if (!response.ok) throw new Error(result.error?.message || "The operation failed.");
      if (result.api_key) setSecret(result.api_key);
      if (result.revoked) { setConfirmRevoke(null); setSecret(null); setNotice("API key revoked. New requests using it will be rejected."); }
      await Promise.all([load(), refresh()]);
    } catch (error) { setError(error instanceof Error ? error.message : "The operation failed."); }
    finally { setBusy(false); }
  }
  async function copy(text: string) {
    try { await navigator.clipboard.writeText(text); setNotice("Copied to clipboard."); }
    catch { setError("Clipboard access failed. Select and copy the value manually."); }
  }
  return <div className="mx-auto w-full max-w-5xl px-6 py-8 sm:px-10">
    <PageHeader title="API Keys" note="Your account · your applications" />
    <section className="mb-8 grid gap-4 border border-line bg-panel p-5 sm:grid-cols-3">
      <div><div className="label">Active keys</div><div className="mt-2 font-mono text-xl">{loading ? "…" : keys.filter((key) => !key.revoked_at).length}</div></div>
      <div><div className="label">Requests served</div><div className="mt-2 font-mono text-xl">{account?.requests || 0}</div></div>
      <div><div className="label">API endpoint</div><button onClick={() => copy(`${origin}/api/v1`)} className="mt-2 break-all text-left font-mono text-xs">{origin ? `${origin}/api/v1` : "…"}</button></div>
    </section>
    {(error || accountError) && <p role="alert" className="mb-5 text-sm text-warn">{error || accountError}</p>}
    {notice && <p role="status" className="mb-5 text-sm text-good">{notice}</p>}
    {secret && <section className="mb-8 border border-accent bg-panel p-5">
      <h2 className="font-medium">Copy your new key</h2><p className="mt-2 text-sm text-muted">This is the only time the full key is shown. Store it securely.</p>
      <div className="mt-4 flex flex-wrap gap-3"><code className="min-w-0 flex-1 break-all rounded-md border border-line p-3 text-xs">{secret}</code><button onClick={() => copy(secret)} className="rounded-md border border-line px-4 py-2 text-sm">Copy key</button></div>
      <button onClick={() => setSecret(null)} className="mt-3 text-sm text-muted">I saved it · hide key</button>
    </section>}
    <section className="border border-line bg-panel p-5">
      <h2 className="text-lg font-medium">Create an API key</h2><p className="mt-2 text-sm text-muted">A key lets an application use the capacity your wallet owns. Every key draws on the same capacity; revoke one at any time.</p>
      <div className="mt-4 flex flex-wrap gap-3"><input value={name} maxLength={80} onChange={(event) => setName(event.target.value)} aria-label="Key name" className="min-w-0 flex-1 rounded-md border border-line bg-background px-3 py-2 text-sm" /><button disabled={busy || !name.trim()} onClick={() => action("keys", "POST", { name: name.trim() })} className="rounded-md bg-foreground px-4 py-2 text-sm font-medium text-background disabled:opacity-40">Create key</button></div>
    </section>
    <section className="mt-8 overflow-x-auto border border-line"><table className="w-full text-left text-sm">
      <thead><tr className="border-b border-line"><th className="label px-4 py-3">Name</th><th className="label px-4 py-3">Key</th><th className="label px-4 py-3">Created</th><th className="label px-4 py-3">Status</th><th className="px-4 py-3" /></tr></thead>
      <tbody>{keys.map((key) => <tr key={key.id} className="border-b border-line last:border-0"><td className="px-4 py-4">{key.name}</td><td className="px-4 py-4 font-mono text-xs">{key.prefix}…</td><td className="px-4 py-4 text-muted">{new Date(key.created_at).toLocaleDateString()}</td><td className="px-4 py-4">{key.revoked_at ? "Revoked" : "Active"}</td><td className="px-4 py-4 text-right">{!key.revoked_at && (confirmRevoke === key.id ? <span className="flex justify-end gap-3"><button disabled={busy} onClick={() => action(`keys/${key.id}`, "DELETE")} className="text-warn">Confirm revoke</button><button onClick={() => setConfirmRevoke(null)} className="text-muted">Cancel</button></span> : <button onClick={() => setConfirmRevoke(key.id)} className="text-muted">Revoke</button>)}</td></tr>)}</tbody>
    </table>{!keys.length && <p className="p-5 text-sm text-muted">No API keys yet. Create one to connect an application.</p>}</section>
    <section className="mt-8"><h2 className="text-lg font-medium">Use your key</h2><pre className="mt-3 overflow-x-auto border border-line bg-panel p-4 font-mono text-xs leading-6">{`curl ${origin || "http://localhost:3001"}/api/v1/chat/completions \\\n  -H 'Authorization: Bearer YOUR_ENCLAVO_KEY' \\\n  -H 'Content-Type: application/json' \\\n  -d '{"model":"MODEL_ID","capacity_contract_id":"YOUR_CAPACITY_ID","messages":[{"role":"user","content":"Hello"}],"max_tokens":128}'`}</pre><p className="mt-3 text-sm text-muted">List the capacity you own, with its model and ID, using <code>GET /api/v1/contracts</code> and the same key. You can also use <Link href="/dashboard/playground" className="text-accent">Playground</Link> while signed in, without copying a key into the page.</p></section>
  </div>;
}
