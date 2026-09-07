import { useConfig, useStats } from "@/application/system";

export function HomePage() {
  const config = useConfig();
  const stats = useStats();
  return (
    <section className="flex min-h-[70vh] flex-col justify-between bg-lime px-8 py-16">
      <div>
        <h1 className="font-display text-6xl leading-[0.95] text-ink uppercase sm:text-8xl">
          Every intent.
          <br />
          One liquidity layer.
        </h1>
        <p className="mt-6 max-w-xl font-sans text-lg text-ink/80">
          Solvent fills UniswapX, ERC-7683 and more from shared Aqua depth.
          Resolvers execute without ever holding inventory.
        </p>
      </div>
      <dl className="mt-12 flex flex-wrap gap-x-10 gap-y-4">
        <Stat
          label="Chain"
          value={config.data ? `#${config.data.chain_id}` : "—"}
        />
        <Stat
          label="Block"
          value={stats.data ? String(stats.data.block_height) : "—"}
        />
        <Stat label="Active makers" value={fmt(stats.data?.active_makers)} />
        <Stat label="Trades settled" value={fmt(stats.data?.trades_settled)} />
      </dl>
    </section>
  );
}

function Stat({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex flex-col gap-1 font-mono text-xs tracking-wide uppercase">
      <dt className="text-ink/50">{label}</dt>
      <dd className="text-sm text-ink">{value}</dd>
    </div>
  );
}

function fmt(n?: number | null): string {
  return n == null ? "—" : String(n);
}
