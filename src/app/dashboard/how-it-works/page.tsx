"use client";
import Link from "next/link";
import PageHeader from "@/components/PageHeader";
import { useWallet } from "@/components/WalletProvider";
import { IU_NOTE, STEPS } from "@/lib/how-it-works";

// A worked example of the uniform-price rule the contract applies at settlement.
const EXAMPLE = [
  { bidder: "A", units: 6, max: 0.9, won: 6 },
  { bidder: "B", units: 6, max: 0.7, won: 4 },
  { bidder: "C", units: 4, max: 0.6, won: 0 },
];
const CLEARING = 0.7;

const FAQ = [
  ["What is an IU?", "An Inference Unit: one million output-token equivalents per day on the model you bought. Input tokens count at the model's input/output price ratio. Your allowance resets every 24 hours from the start of your capacity; unused tokens don't roll over."],
  ["Who holds my money?", "The auction contract on NEAR, not Enclavo. It escrows your bid's maximum cost and, at settlement, charges only the clearing price. Anything unspent is unlocked for your next bid or for withdrawal to your wallet."],
  ["Can I change my mind?", "Yes. You can cancel a bid at any time before the auction closes, and its escrow is unlocked straight away."],
  ["Who settles an auction?", "Anyone with a connected wallet, once the auction has closed. Settling runs the clearing on chain for a small gas fee; the result is the same whoever does it."],
  ["What makes it private?", "Every model runs on its provider's confidential-computing hardware (TEEs). Today the attestation is reported by the provider; Enclavo does not yet verify the hardware proofs itself."],
  ["Is this real money?", "Not on testnet. Test USDC has no value and you can mint 1,000 at a time from the Account page or the bid panel."],
] as const;

export default function HowItWorksPage() {
  const { session, signIn, busy } = useWallet();
  return (
    <div className="mx-auto w-full max-w-5xl px-6 py-8 sm:px-10">
      <PageHeader title="How it works" note="Private inference, bought at auction" />
      <p className="max-w-3xl text-lg font-light leading-8">
        Enclavo sells private AI capacity as a <span className="text-accent">daily token allowance</span>. You buy it once at an
        auction settled on NEAR, then use it like any OpenAI-compatible API, from Playground or your own app.
      </p>

      <ol className="mt-10 grid gap-4 md:grid-cols-2">
        {STEPS.map((step, i) => (
          <li key={step.title} className="flex flex-col border border-line bg-panel p-5">
            <div className="flex items-baseline gap-3">
              <span className="font-mono text-sm text-accent">{i + 1}</span>
              <h2 className="text-lg font-medium">{step.title}</h2>
            </div>
            <p className="mt-3 flex-1 text-sm leading-6 text-muted">{step.text}</p>
            {i === 0 && !session ? (
              <button type="button" disabled={busy} onClick={() => void signIn()} className="mt-4 self-start rounded-md bg-foreground px-4 py-2 text-sm font-medium text-background disabled:opacity-50">
                {busy ? "Waiting for wallet…" : "Connect NEAR wallet"}
              </button>
            ) : i === 0 ? (
              <p className="mt-4 text-sm text-good">Connected as <span className="font-mono">{session}</span></p>
            ) : (
              <Link href={step.href} className="mt-4 self-start text-sm text-accent">{step.action} →</Link>
            )}
          </li>
        ))}
      </ol>
      <p className="mt-3 text-xs text-muted">{IU_NOTE}</p>

      <section className="mt-12">
        <h2 className="text-lg font-medium">How the price is set</h2>
        <p className="mt-2 max-w-3xl text-sm leading-6 text-muted">
          Everyone who wins pays the same price per IU-day. Bids fill from the highest maximum price down. If the bids fit in
          the lot, the price is the reserve; if there are more bids than capacity, the price is the lowest bid that still won.
        </p>
        <div className="mt-5 overflow-x-auto border border-line">
          <div className="border-b border-line px-4 py-3 text-sm">Example: a lot of <span className="font-mono">10 IU/day</span> with a <span className="font-mono">$0.50</span> reserve receives three bids.</div>
          <table className="w-full min-w-[560px] text-left text-sm">
            <thead><tr className="border-b border-line"><th className="label px-4 py-2 font-normal">Bidder</th><th className="label px-4 py-2 font-normal">Wants</th><th className="label px-4 py-2 font-normal">Max price</th><th className="label px-4 py-2 font-normal">Gets</th><th className="label px-4 py-2 font-normal">Pays per IU-day</th></tr></thead>
            <tbody>{EXAMPLE.map((row) => (
              <tr key={row.bidder} className="border-b border-line last:border-0">
                <td className="px-4 py-2">{row.bidder}</td>
                <td className="px-4 py-2 font-mono">{row.units} IU/day</td>
                <td className="px-4 py-2 font-mono">${row.max.toFixed(2)}</td>
                <td className={`px-4 py-2 font-mono ${row.won ? "text-good" : "text-muted"}`}>{row.won ? `${row.won} IU/day${row.won < row.units ? " (partial)" : ""}` : "nothing"}</td>
                <td className="px-4 py-2 font-mono">{row.won ? `$${CLEARING.toFixed(2)}` : "— (refunded)"}</td>
              </tr>
            ))}</tbody>
          </table>
          <p className="border-t border-line px-4 py-3 text-xs leading-5 text-muted">
            A fills first, then B takes the last 4 IU. B is the lowest bid that won, so everyone pays $0.70, even A, who was
            willing to pay $0.90. C is outbid and all of C&apos;s escrow is unlocked.
          </p>
        </div>
      </section>

      <section className="mt-12">
        <h2 className="text-lg font-medium">Questions</h2>
        <dl className="mt-4 grid gap-4 md:grid-cols-2">
          {FAQ.map(([question, answer]) => (
            <div key={question} className="border border-line p-5">
              <dt className="font-medium">{question}</dt>
              <dd className="mt-2 text-sm leading-6 text-muted">{answer}</dd>
            </div>
          ))}
        </dl>
      </section>

      <div className="mt-12 flex flex-wrap gap-3">
        <Link href="/dashboard/market" className="rounded-md bg-foreground px-5 py-3 text-sm font-medium text-background">Browse auctions</Link>
        <Link href="/dashboard/account" className="rounded-md border border-line px-5 py-3 text-sm">Get test USDC</Link>
      </div>
    </div>
  );
}
