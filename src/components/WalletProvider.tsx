"use client";

import { createContext, useCallback, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { NearConnector, NearWalletBase } from "@hot-labs/near-connect";

/** Dispatched after each wallet transaction so balance displays refresh straight away. */
export const BALANCES_EVENT = "enclavo:balances";

export const NEAR_NETWORK: "testnet" | "mainnet" = process.env.NEXT_PUBLIC_NEAR_NETWORK === "mainnet" ? "mainnet" : "testnet";

type Call = { method: string; args: object; gas: string; deposit: string };
type WalletContextValue = {
  network: "testnet" | "mainnet";
  /** The connector has loaded and restored any previous wallet connection. */
  ready: boolean;
  /** The NEAR account currently connected in the wallet (no signature needed). */
  walletAccount: string | null;
  /** The NEAR account the server has verified for this browser session. */
  session: string | null;
  sessionReady: boolean;
  busy: boolean;
  error: string | null;
  /** Connects a wallet if needed, then signs a sign-in message and starts a session. */
  signIn: () => Promise<void>;
  /** Ends the session and disconnects the wallet. */
  signOut: () => Promise<void>;
  /** Connects a wallet without signing in, for reading and sending on-chain transactions. */
  connect: () => Promise<void>;
  call: (receiverId: string, call: Call) => Promise<unknown>;
};

const WalletContext = createContext<WalletContextValue | null>(null);

async function post(path: string, body?: object) {
  const response = await fetch(path, { method: "POST", headers: { "content-type": "application/json" }, body: body ? JSON.stringify(body) : undefined, cache: "no-store" });
  const data = response.status === 204 ? {} : await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error?.message || "The request failed.");
  return data;
}

// One NEAR wallet connection for the whole app. The wallet account is the user's identity.
export function WalletProvider({ children }: { children: ReactNode }) {
  const connector = useRef<NearConnector | null>(null);
  const [ready, setReady] = useState(false);
  const [walletAccount, setWalletAccount] = useState<string | null>(null);
  const [session, setSession] = useState<string | null>(null);
  const [sessionReady, setSessionReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Latest values for the wallet's event callbacks, which outlive any single render.
  const sessionRef = useRef<string | null>(null);
  const walletRef = useRef<string | null>(null);

  const endSession = useCallback(async () => {
    sessionRef.current = null;
    setSession(null);
    await post("/api/auth/logout").catch(() => undefined);
  }, []);

  // A session belongs to one account: a wallet now showing a different account ends it.
  const reconcile = useCallback(() => {
    if (sessionRef.current && walletRef.current && walletRef.current !== sessionRef.current) void endSession();
  }, [endSession]);
  const updateWallet = useCallback((account: string | null) => { walletRef.current = account; setWalletAccount(account); reconcile(); }, [reconcile]);
  const updateSession = useCallback((account: string | null) => { sessionRef.current = account; setSession(account); reconcile(); }, [reconcile]);

  useEffect(() => {
    let cancelled = false;
    void fetch("/api/auth/session", { cache: "no-store" })
      .then((response) => response.json())
      .then((data) => { if (!cancelled) { updateSession(data.accountId ?? null); setSessionReady(true); } })
      .catch(() => { if (!cancelled) setSessionReady(true); });
    // The connector touches window and storage, so it is loaded in the browser only.
    void import("@hot-labs/near-connect").then(async ({ NearConnector }) => {
      if (cancelled) return;
      const instance = new NearConnector({ network: NEAR_NETWORK, features: NEAR_NETWORK === "testnet" ? { testnet: true } : undefined });
      instance.on("wallet:signIn", (event) => updateWallet(event.accounts[0]?.accountId ?? null));
      instance.on("wallet:signOut", () => updateWallet(null));
      connector.current = instance;
      try {
        const accounts = await (await instance.wallet()).getAccounts();
        if (!cancelled) updateWallet(accounts[0]?.accountId ?? null);
      } catch {
        // No wallet connected yet.
      }
      if (!cancelled) setReady(true);
    });
    return () => { cancelled = true; };
  }, [updateSession, updateWallet]);

  const connectWallet = useCallback(async (): Promise<NearWalletBase> => {
    const instance = connector.current;
    if (!instance) throw new Error("The NEAR wallet connector is still loading.");
    try {
      const wallet = await instance.wallet();
      if ((await wallet.getAccounts()).length) return wallet;
    } catch {
      // Not connected; fall through to the wallet picker.
    }
    return instance.connect();
  }, []);

  const connect = useCallback(async () => {
    setError(null);
    try { await connectWallet(); } catch (cause) { setError(cause instanceof Error ? cause.message : "Could not connect a wallet."); }
  }, [connectWallet]);

  const signIn = useCallback(async () => {
    setBusy(true); setError(null);
    try {
      const wallet = await connectWallet();
      const challenge = await post("/api/auth/challenge");
      const nonce = Uint8Array.from(atob(challenge.nonce), (char) => char.charCodeAt(0));
      const signed = await wallet.signMessage({ message: challenge.message, recipient: challenge.recipient, nonce });
      if (!signed?.signature) throw new Error("The wallet did not sign the message.");
      const result = await post("/api/auth/verify", { accountId: signed.accountId, publicKey: signed.publicKey, signature: signed.signature, nonce: challenge.nonce });
      updateSession(result.accountId);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Sign-in failed.");
    } finally {
      setBusy(false);
    }
  }, [connectWallet, updateSession]);

  const signOut = useCallback(async () => {
    await endSession();
    await connector.current?.disconnect().catch(() => undefined);
    updateWallet(null);
  }, [endSession, updateWallet]);

  const call = useCallback(async (receiverId: string, { method, args, gas, deposit }: Call) => {
    if (!connector.current) throw new Error("Connect a NEAR wallet first.");
    const wallet = await connector.current.wallet();
    const outcome = await wallet.signAndSendTransaction({ receiverId, actions: [{ type: "FunctionCall", params: { methodName: method, args, gas, deposit } }] });
    window.dispatchEvent(new Event(BALANCES_EVENT));
    return outcome;
  }, []);

  return (
    <WalletContext.Provider value={{ network: NEAR_NETWORK, ready, walletAccount, session, sessionReady, busy, error, signIn, signOut, connect, call }}>
      {children}
    </WalletContext.Provider>
  );
}

export function useWallet() {
  const context = useContext(WalletContext);
  if (!context) throw new Error("useWallet requires WalletProvider");
  return context;
}
