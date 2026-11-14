"use client";

import Link from "next/link";
import { useEffect, useRef, useSyncExternalStore } from "react";
import { useWallet } from "@/components/WalletProvider";
import { Mark } from "@/components/Logo";
import { IU_NOTE, STEPS } from "@/lib/how-it-works";

// Shown once per wallet, the first time it signs in on this browser. The "seen" flag is a
// per-browser convenience in localStorage; if storage is unavailable the dialog just shows.
const key = (account: string) => `enclavo:welcomed:${account}`;
const EVENT = "enclavo:welcome";

function subscribe(onChange: () => void) {
  window.addEventListener("storage", onChange);
  window.addEventListener(EVENT, onChange);
  return () => { window.removeEventListener("storage", onChange); window.removeEventListener(EVENT, onChange); };
}
function seen(account: string | null): boolean {
  if (!account) return true;
  try { return window.localStorage.getItem(key(account)) === "1"; } catch { return false; }
}

export default function Welcome() {
  const { session } = useWallet();
  // The server snapshot says "seen" so nothing flashes before the browser can check.
  const welcomed = useSyncExternalStore(subscribe, () => seen(session), () => true);
  const start = useRef<HTMLButtonElement>(null);

  function dismiss() {
    if (!session) return;
    try { window.localStorage.setItem(key(session), "1"); } catch { /* storage blocked: closes for this view only */ }
    window.dispatchEvent(new Event(EVENT));
  }

  useEffect(() => {
    if (welcomed) return;
    start.current?.focus();
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape") dismiss(); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (welcomed || !session) return null;
  return (
    <div className="fixed inset-0 z-30 flex items-center justify-center bg-background/80 p-4 backdrop-blur-sm">
      <div role="dialog" aria-modal="true" aria-labelledby="welcome-title" className="max-h-full w-full max-w-3xl overflow-y-auto border border-line bg-panel p-6 sm:p-8">
        <div className="flex items-center gap-3 text-accent"><Mark size={28} /><span className="label">Welcome, {session}</span></div>
        <h2 id="welcome-title" className="mt-4 text-2xl font-light tracking-tight sm:text-3xl">How Enclavo works</h2>
        <p className="mt-2 text-sm text-muted">Private AI capacity, bought at auction on NEAR and used like any OpenAI-compatible API.</p>
        <ol className="mt-6 grid border border-line sm:grid-cols-2">
          {STEPS.map((step, i) => (
            <li key={step.title} className="border-line p-4 max-sm:[&:not(:first-child)]:border-t sm:[&:nth-child(even)]:border-l sm:[&:nth-child(n+3)]:border-t">
              <div className="flex items-baseline gap-2"><span className="font-mono text-xs text-accent">{i + 1}</span><span className="text-sm font-medium">{step.title}</span>{i === 0 && <span className="text-xs text-good">· done</span>}</div>
              <p className="mt-2 text-xs leading-5 text-muted">{step.text}</p>
            </li>
          ))}
        </ol>
        <p className="mt-3 text-xs text-muted">{IU_NOTE}</p>
        <div className="mt-6 flex flex-wrap items-center gap-4">
          <button ref={start} type="button" onClick={dismiss} className="rounded-md bg-foreground px-5 py-3 text-sm font-medium text-background transition hover:bg-accent">Get started</button>
          <Link href="/dashboard/how-it-works" onClick={dismiss} className="text-sm text-accent">Read the full guide →</Link>
        </div>
        <p className="mt-4 text-xs text-muted">You can come back to this any time from <span className="text-foreground">How it works</span> in the sidebar.</p>
      </div>
    </div>
  );
}
