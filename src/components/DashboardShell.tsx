"use client";

import type { ReactNode } from "react";
import { usePrivy } from "@privy-io/react-auth";
import { usePathname } from "next/navigation";
import { Mark } from "@/components/Logo";
import Sidebar from "@/components/Sidebar";

// Frames every dashboard page with the sidebar. Pages always render so Next.js can
// prerender and validate them; until the visitor signs in, a prompt covers the page
// and the page underneath is inert.
export default function DashboardShell({ children }: { children: ReactNode }) {
  const { ready, authenticated, login } = usePrivy();
  const pathname = usePathname();
  // Market is public, and the on-chain market is used with a NEAR wallet rather than a sign-in.
  const open = pathname === "/dashboard/market" || pathname === "/dashboard/onchain";
  const locked = !open && (!ready || !authenticated);

  return (
    <div className="relative flex min-h-dvh flex-col md:flex-row">
      <Sidebar />
      <main className={`min-w-0 flex-1 transition ${locked ? "pointer-events-none select-none blur-sm" : ""}`} inert={locked}>
        {children}
      </main>

      {locked && (
        <div className="fixed inset-0 z-20 flex flex-col items-center justify-center gap-6 bg-background/70 px-6 text-center">
          {ready ? (
            <>
              <span className="text-accent">
                <Mark size={40} />
              </span>
              <h1 className="text-3xl font-light tracking-tight">Sign in to Enclavo</h1>
              <button
                type="button"
                onClick={login}
                className="rounded-md bg-foreground px-6 py-3 text-[15px] font-medium text-background transition hover:bg-accent"
              >
                Sign in
              </button>
            </>
          ) : (
            <span className="label">Loading</span>
          )}
        </div>
      )}
    </div>
  );
}
