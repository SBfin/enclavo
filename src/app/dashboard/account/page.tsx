"use client";

import { useState } from "react";
import PageHeader from "@/components/PageHeader";
import { useAccount } from "@/components/AccountProvider";
import { useWallet } from "@/components/WalletProvider";
import { useCatalog } from "@/lib/use-catalog";
import { useChainMarket } from "@/lib/use-chain-market";
import { usd } from "@/lib/near";
import { fmtUSD } from "@/lib/market";

export default function AccountPage() {
  const { session, network, walletAccount, signOut } = useWallet();
  const { account } = useAccount();
  const { status } = useCatalog();
  const market = useChainMarket(status?.chain ?? null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const escrowed = market.bids.filter((b) => b.status === "open").reduce((sum, b) => sum + Number(b.escrow), 0);
  const credit = usd(account?.balance_micro_usd ?? 0);

  async function run(done: string, action: () => Promise<unknown>) {
    setBusy(true); setFailure(null); setNotice(null);
    try { await action(); setNotice(done); await market.refresh(); }
    catch (error) { setFailure(error instanceof Error ? error.message : "The transaction was not completed."); }
    finally { setBusy(false); }
  }

  return (
    <div className="mx-auto w-full max-w-3xl px-6 py-8 sm:px-10">
      <PageHeader title="Account" note={`NEAR ${network}`} />
      {failure && <p role="alert" className="mb-5 border border-warn p-4 text-sm text-warn">{failure}</p>}
      {notice && <p role="status" className="mb-5 border border-line p-4 text-sm text-good">{notice}</p>}

      <section>
        <h2 className="mb-3 text-lg font-medium">Wallet</h2>
        <ul className="border border-line">
          <Item label="Signed in as" value={session} />
          <Item label="Connected wallet account" value={walletAccount ?? "Not connected"} />
        </ul>
        <p className="mt-3 text-xs leading-5 text-muted">
          Your NEAR account is your Enclavo account: bids, capacity and API keys all belong to it. Signing in only signs a
          message; it never sends a transaction. To use another account, sign out and connect it.
        </p>
      </section>

      <section className="mt-10">
        <h2 className="mb-3 text-lg font-medium">Balances</h2>
        <div className="grid grid-cols-1 border border-line sm:grid-cols-3">
          <Balance label="USDC in wallet" value={fmtUSD(usd(market.wallet), 2)} hint="Available to bid" />
          <Balance label="In escrow" value={fmtUSD(usd(escrowed), 2)} hint="Held by open bids" />
          <Balance label="Unlocked in contract" value={fmtUSD(usd(market.unlocked), 2)} hint="Refunds; used first for new bids" />
        </div>
        <div className="mt-3 flex flex-wrap gap-3 text-sm">
          {network === "testnet" && <button disabled={busy || !walletAccount} onClick={() => void run("1,000 test USDC added to your wallet.", market.faucet)} className="rounded-md bg-foreground px-4 py-2 font-medium text-background disabled:opacity-40">Get 1,000 test USDC</button>}
          {market.unlocked > 0 && <button disabled={busy} onClick={() => void run("Unlocked balance sent to your wallet.", market.withdraw)} className="rounded-md border border-line px-4 py-2 disabled:opacity-40">Withdraw {fmtUSD(usd(market.unlocked), 2)} to wallet</button>}
        </div>
        {network === "testnet" && <p className="mt-3 text-xs text-muted">Test USDC is a testnet token with no value. Each request mints 1,000 to your wallet; your wallet asks you to confirm it.</p>}
        {credit > 0 && <p className="mt-3 text-sm text-muted">You also have {fmtUSD(credit, 2)} of Enclavo credit, usable in Playground and with your API keys.</p>}
      </section>

      <section className="mt-10">
        <h2 className="mb-3 text-lg font-medium">Enclavo account</h2>
        <ul className="border border-line">
          <Item label="Account ID" value={account?.id ?? "Created when you win capacity or create an API key"} />
        </ul>
        <button
          type="button"
          onClick={() => void signOut()}
          className="mt-6 rounded-md border border-line px-4 py-2 text-sm text-muted transition hover:text-foreground"
        >
          Sign out and disconnect wallet
        </button>
      </section>
    </div>
  );
}

function Item({ label, value }: { label: string; value: string | null }) {
  return (
    <li className="border-b border-line px-4 py-4 last:border-0">
      <div className="label">{label}</div>
      <div className="mt-1 truncate font-mono text-xs text-muted">{value ?? "—"}</div>
    </li>
  );
}

function Balance({ label, value, hint }: { label: string; value: string; hint: string }) {
  return <div className="border-line p-4 max-sm:[&:not(:first-child)]:border-t sm:[&:not(:first-child)]:border-l"><div className="label">{label}</div><div className="mt-3 text-2xl font-medium tracking-tight tnum">{value}</div><div className="mt-1 text-xs text-muted">{hint}</div></div>;
}
