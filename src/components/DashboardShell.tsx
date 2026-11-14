"use client";

import type { ReactNode } from "react";
import Link from "next/link";
import { useWallet } from "@/components/WalletProvider";
import { usePathname } from "next/navigation";
import { Mark } from "@/components/Logo";
import Sidebar from "@/components/Sidebar";
import Welcome from "@/components/Welcome";

// Frames every dashboard page with the sidebar. Pages always render so Next.js can
// prerender and validate them; until the visitor signs in, a prompt covers the page
// and the page underneath is inert.
export default function DashboardShell({ children }: { children: ReactNode }) {
  const { sessionReady, session, signIn, busy, error } = useWallet();
  const pathname = usePathname();
  // Anyone can browse the Market and the guide; everything else belongs to a signed-in wallet.
  const open = pathname === "/dashboard/market" || pathname === "/dashboard/how-it-works";
  const locked = !open && (!sessionReady || !session);

  return (
    <div className="relative flex min-h-dvh flex-col md:flex-row">
      <Sidebar />
      <main className={`min-w-0 flex-1 transition ${locked ? "pointer-events-none select-none blur-sm" : ""}`} inert={locked}>
        {children}
      </main>

      <Welcome />

      {locked && (
        <div className="fixed inset-0 z-20 flex flex-col items-center justify-center gap-6 bg-background/70 px-6 text-center">
          {sessionReady ? (
            <>
              <span className="text-accent">
                <Mark size={40} />
              </span>
              <h1 className="text-3xl font-light tracking-tight">Connect your NEAR wallet</h1>
              <p className="max-w-sm text-sm text-muted">Your NEAR account is your Enclavo account. You sign one message to prove it; no transaction is sent.</p>
              <Link href="/dashboard/how-it-works" className="text-sm text-accent">New here? See how it works →</Link>
              <button
                type="button"
                onClick={() => void signIn()}
                disabled={busy}
                className="rounded-md bg-foreground px-6 py-3 text-[15px] font-medium text-background transition hover:bg-accent disabled:opacity-50"
              >
                {busy ? "Waiting for wallet…" : "Connect NEAR wallet"}
              </button>
              {error && <p role="alert" className="max-w-sm text-sm text-warn">{error}</p>}
            </>
          ) : (
            <span className="label">Loading</span>
          )}
        </div>
      )}
    </div>
  );
}
