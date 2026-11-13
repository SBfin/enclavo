"use client";
import { useCallback, useEffect, useRef, useState } from "react";
import type { NearConnector } from "@hot-labs/near-connect";

type Call = { method: string; args: object; gas: string; deposit: string };

/// A NEAR wallet session, separate from the Privy sign-in: Privy does not provision NEAR wallets.
export function useNearWallet(network: "testnet" | "mainnet" | undefined) {
  const connector = useRef<NearConnector | null>(null);
  const [accountId, setAccountId] = useState<string | null>(null);
  const [ready, setReady] = useState(false);

  useEffect(() => {
    if (!network) return;
    let cancelled = false;
    // The connector touches window and storage, so it is loaded in the browser only.
    void import("@hot-labs/near-connect").then(async ({ NearConnector }) => {
      if (cancelled) return;
      const instance = new NearConnector({ network, features: network === "testnet" ? { testnet: true } : undefined });
      instance.on("wallet:signIn", (event) => setAccountId(event.accounts[0]?.accountId ?? null));
      instance.on("wallet:signOut", () => setAccountId(null));
      connector.current = instance;
      try {
        const wallet = await instance.wallet();
        const accounts = await wallet.getAccounts();
        if (!cancelled) setAccountId(accounts[0]?.accountId ?? null);
      } catch {
        // No wallet connected yet.
      }
      if (!cancelled) setReady(true);
    });
    return () => { cancelled = true; };
  }, [network]);

  const connect = useCallback(async () => { await connector.current?.connect(); }, []);
  const disconnect = useCallback(async () => { await connector.current?.disconnect(); setAccountId(null); }, []);
  const call = useCallback(async (receiverId: string, { method, args, gas, deposit }: Call) => {
    if (!connector.current) throw new Error("Connect a NEAR wallet first.");
    const wallet = await connector.current.wallet();
    return wallet.signAndSendTransaction({ receiverId, actions: [{ type: "FunctionCall", params: { methodName: method, args, gas, deposit } }] });
  }, []);

  return { ready, accountId, connect, disconnect, call };
}
