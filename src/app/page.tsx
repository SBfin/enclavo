import Link from "next/link";
import GridField from "@/components/GridField";
import { Mark, Wordmark } from "@/components/Logo";

export default function Home() {
  return (
    <main className="relative flex min-h-dvh flex-col overflow-hidden">
      <GridField />
      <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(ellipse_at_center,transparent_45%,var(--background)_95%)]" />

      <header className="relative z-10 flex items-center justify-between px-6 py-5 sm:px-10">
        <Wordmark />
        <span className="label rounded-full border border-line px-3 py-1">NEAR testnet</span>
      </header>

      <section className="relative z-10 flex flex-1 flex-col items-center justify-center px-6 text-center">
        <div className="mb-6 text-accent">
          <Mark size={44} />
        </div>
        <h1 className="text-6xl font-light tracking-tight sm:text-8xl">enclavo</h1>
        <p className="mt-5 text-lg text-muted sm:text-xl">
          Private inference, traded in <span className="text-foreground">one unit</span>.
        </p>

        <div className="mt-12 flex flex-col items-center gap-6 sm:flex-row sm:gap-10">
          <div className="text-left">
            <div className="text-4xl font-medium tracking-tight tnum">1 IU</div>
            <div className="label mt-1">= 1M tokens / day</div>
          </div>
          <Link
            href="/dashboard/market"
            className="group inline-flex items-center gap-3 rounded-md bg-foreground px-6 py-3.5 text-[15px] font-medium text-background transition hover:bg-accent"
          >
            Enter market
            <span className="transition group-hover:translate-x-0.5" aria-hidden="true">
              →
            </span>
          </Link>
        </div>
      </section>

      <footer className="relative z-10 flex justify-center px-6 pb-8">
        <span className="label">attested · metered · settled onchain</span>
      </footer>
    </main>
  );
}
