"use client";

import Link from "next/link";
import { usePathname } from "next/navigation";
import { usePrivy } from "@privy-io/react-auth";
import { Wordmark } from "@/components/Logo";

const NAV = [
  { href: "/dashboard/market", label: "Market", icon: MarketIcon },
  { href: "/dashboard/onchain", label: "On-chain", icon: ChainIcon },
  { href: "/dashboard/playground", label: "Playground", icon: RouterIcon },
  { href: "/dashboard/api-keys", label: "API Keys", icon: KeyIcon },
  { href: "/dashboard/settings", label: "Settings", icon: SettingsIcon },
];

export default function Sidebar() {
  const pathname = usePathname();
  const { authenticated, user, logout, login } = usePrivy();
  const identity = user?.email?.address ?? user?.google?.email ?? shortAddress(user?.wallet?.address) ?? "Signed in";

  return (
    <aside className="flex shrink-0 flex-col border-line max-md:border-b md:sticky md:top-0 md:h-dvh md:w-60 md:border-r">
      <div className="flex items-center justify-between px-5 py-4 md:py-6">
        <Link href="/" aria-label="Enclavo home">
          <Wordmark />
        </Link>
      </div>

      <nav aria-label="Dashboard" className="flex gap-1 overflow-x-auto px-3 pb-3 md:overflow-visible md:flex-1 md:flex-col md:pb-0">
        {NAV.map(({ href, label, icon: Icon }) => {
          const active = pathname === href || pathname.startsWith(`${href}/`);
          return (
            <Link
              key={href}
              href={href}
              aria-current={active ? "page" : undefined}
              className={`flex items-center gap-3 rounded-md px-3 py-2 text-sm transition ${
                active ? "bg-white/[0.07] text-foreground" : "text-muted hover:bg-white/[0.03] hover:text-foreground"
              }`}
            >
              <Icon />
              <span className="whitespace-nowrap">{label}</span>
            </Link>
          );
        })}
      </nav>

      {!authenticated && <button type="button" onClick={login} className="m-4 rounded-md bg-foreground px-4 py-2 text-sm text-background">Sign in</button>}

      <div className={`border-t border-line px-5 py-4 ${authenticated ? "hidden md:block" : "hidden"}`}>
        <div className="truncate text-sm" title={identity}>
          {identity}
        </div>
        <button type="button" onClick={logout} className="mt-1 text-sm text-muted transition hover:text-foreground">
          Sign out
        </button>
      </div>
    </aside>
  );
}

function KeyIcon() {
  return <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true"><circle cx="8" cy="8" r="4"/><path d="m11 11 9 9m-3-3 3-3m-6 0 3-3"/></svg>;
}

function shortAddress(address?: string) {
  return address ? `${address.slice(0, 6)}…${address.slice(-4)}` : undefined;
}

function RouterIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
      <circle cx="5" cy="12" r="2" />
      <circle cx="19" cy="5" r="2" />
      <circle cx="19" cy="19" r="2" />
      <path d="M7 12h4l6-6M11 12l6 6" />
    </svg>
  );
}

function MarketIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
      <path d="M4 19V5M4 19h16" />
      <path d="M8 15l3-4 3 2 5-6" />
    </svg>
  );
}

function ChainIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
      <path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1" />
      <path d="M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1" />
    </svg>
  );
}

function SettingsIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" aria-hidden="true">
      <circle cx="12" cy="12" r="3" />
      <path d="M12 3v3M12 18v3M3 12h3M18 12h3M5.6 5.6l2.1 2.1M16.3 16.3l2.1 2.1M5.6 18.4l2.1-2.1M16.3 7.7l2.1-2.1" />
    </svg>
  );
}
