"use client";

import PageHeader from "@/components/PageHeader";
import { useAccount } from "@/components/AccountProvider";
import { useWallet } from "@/components/WalletProvider";

export default function SettingsPage() {
  const { session, network, walletAccount, signOut } = useWallet();
  const { account } = useAccount();

  return (
    <div className="mx-auto w-full max-w-3xl px-6 py-8 sm:px-10">
      <PageHeader title="Settings" />

      <section>
        <h2 className="mb-3 text-lg font-medium">NEAR wallet</h2>
        <ul className="border border-line">
          <Item label="Signed in as" value={session} />
          <Item label="Network" value={`NEAR ${network}`} />
          <Item label="Connected wallet account" value={walletAccount ?? "Not connected"} />
        </ul>
        <p className="mt-3 text-xs leading-5 text-muted">
          Your NEAR account is your Enclavo account: credit, API keys, bids and capacity all belong to it. Signing in only
          signs a message; it never sends a transaction. To use another account, sign out and connect it.
        </p>
      </section>

      <section className="mt-10">
        <h2 className="mb-3 text-lg font-medium">Account</h2>
        <ul className="border border-line">
          <Item label="Enclavo account ID" value={account?.id ?? "Created with your first API key"} />
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
