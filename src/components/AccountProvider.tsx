"use client";

import { createContext, useCallback, useContext, useEffect, useState, type ReactNode } from "react";
import { usePrivy } from "@privy-io/react-auth";

export type CustomerAccount = { id: string; balance_micro_usd: number; held_micro_usd: number; requests: number; spent_micro_usd: number };
type AccountContextValue = { account: CustomerAccount | null; loading: boolean; error: string | null; signedIn: boolean; api: (path: string, init?: RequestInit) => Promise<Response>; refresh: () => Promise<void> };
const AccountContext = createContext<AccountContextValue | null>(null);

export function AccountProvider({ children }: { children: ReactNode }) {
  const { authenticated, user, getAccessToken } = usePrivy();
  const identity = authenticated ? user?.id : undefined;
  const [state, setState] = useState<{ identity?: string; account: CustomerAccount | null; error: string | null }>({ account: null, error: null });
  const api = useCallback(async (path: string, init?: RequestInit) => {
    const token = await getAccessToken();
    if (!token) throw new Error("Sign in again to access your account.");
    const headers = new Headers(init?.headers);
    headers.set("authorization", `Bearer ${token}`);
    if (typeof init?.body === "string") headers.set("content-type", "application/json");
    return fetch(`/api/dashboard/${path}`, { ...init, headers, cache: "no-store" });
  }, [getAccessToken]);
  const refresh = useCallback(async () => {
    if (!identity) return;
    try {
      const response = await api("account");
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message || "Could not load your account.");
      setState({ identity, account: body.account, error: null });
    } catch (error) {
      setState({ identity, account: null, error: error instanceof Error ? error.message : "Could not load your account." });
    }
  }, [identity, api]);
  useEffect(() => {
    if (!identity) return;
    let cancelled = false;
    const update = () => { void api("account").then(async (response) => {
      const body = await response.json();
      if (!response.ok) throw new Error(body.error?.message || "Could not load your account.");
      if (!cancelled) setState({ identity, account: body.account, error: null });
    }).catch((error) => { if (!cancelled) setState({ identity, account: null, error: error.message }); }); };
    update(); const timer = setInterval(update, 15_000);
    return () => { cancelled = true; clearInterval(timer); };
  }, [identity, api]);
  const current = state.identity === identity;
  return <AccountContext.Provider key={identity || "guest"} value={{ account: current ? state.account : null, loading: Boolean(identity && !current), error: current ? state.error : null, signedIn: authenticated, api, refresh }}>{children}</AccountContext.Provider>;
}

export function useAccount() {
  const context = useContext(AccountContext);
  if (!context) throw new Error("useAccount requires AccountProvider");
  return context;
}
