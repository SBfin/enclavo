"use client";

import { usePrivy } from "@privy-io/react-auth";
import PageHeader from "@/components/PageHeader";

export default function SettingsPage() {
  const { user, linkEmail, linkGoogle, linkWallet, logout } = usePrivy();

  const accounts = [
    { label: "Email", value: user?.email?.address, link: linkEmail },
    { label: "Google", value: user?.google?.email, link: linkGoogle },
    { label: "Wallet", value: user?.wallet?.address, link: () => linkWallet() },
  ];

  return (
    <div className="mx-auto w-full max-w-3xl px-6 py-8 sm:px-10">
      <PageHeader title="Settings" />

      <section>
        <h2 className="mb-3 text-lg font-medium">Sign-in methods</h2>
        <ul className="border border-line">
          {accounts.map((a) => (
            <li key={a.label} className="flex items-center justify-between gap-4 border-b border-line px-4 py-4 last:border-0">
              <div className="min-w-0">
                <div className="text-sm">{a.label}</div>
                <div className="truncate font-mono text-xs text-muted">{a.value ?? "Not linked"}</div>
              </div>
              {!a.value && (
                <button
                  type="button"
                  onClick={a.link}
                  className="shrink-0 rounded-md border border-line px-4 py-2 text-sm text-muted transition hover:text-foreground"
                >
                  Link
                </button>
              )}
            </li>
          ))}
        </ul>
      </section>

      <section className="mt-10">
        <h2 className="mb-3 text-lg font-medium">Account</h2>
        <div className="border border-line px-4 py-4">
          <div className="label">User ID</div>
          <div className="mt-1 truncate font-mono text-xs text-muted">{user?.id}</div>
        </div>
        <button
          type="button"
          onClick={logout}
          className="mt-6 rounded-md border border-line px-4 py-2 text-sm text-muted transition hover:text-foreground"
        >
          Sign out
        </button>
      </section>
    </div>
  );
}
