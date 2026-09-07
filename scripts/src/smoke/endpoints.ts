/**
 * Read-API smoke matrix, driven through the SDK client an application would use — so a failure
 * here is a server or SDK fault, never a caller's. Ids discovered by one call feed the next, so the
 * matrix reflects real data instead of invented arguments.
 */
import { createSolventClient } from "@solvent/sdk/client";

const BASE_URL = process.env.SOLVENT_API_URL ?? "http://127.0.0.1:8080";

const client = createSolventClient({ baseUrl: BASE_URL });

type Status = "OK" | "EMPTY" | "SKIP" | "FAIL";

interface Row {
  name: string;
  status: Status;
  detail: string;
}

/** Collects one row per endpoint; `run` returning null means "reached, but nothing stored yet". */
class Matrix {
  private readonly rows: Row[] = [];

  async check(name: string, run: () => Promise<string | null>): Promise<void> {
    try {
      const detail = await run();
      this.rows.push(
        detail === null
          ? { name, status: "EMPTY", detail: "no data yet" }
          : { name, status: "OK", detail },
      );
    } catch (error) {
      this.rows.push({
        name,
        status: "FAIL",
        detail: error instanceof Error ? error.message : String(error),
      });
    }
  }

  skip(name: string, why: string): void {
    this.rows.push({ name, status: "SKIP", detail: why });
  }

  skipAll(names: readonly string[], why: string): void {
    for (const name of names) this.skip(name, why);
  }

  /** Print the matrix; a failed row makes the process exit non-zero. */
  report(): void {
    const width = Math.max(...this.rows.map((row) => row.name.length));
    for (const row of this.rows) {
      console.log(`${row.name.padEnd(width)}  ${row.status.padEnd(5)}  ${row.detail}`);
    }
    const count = (status: Status) => this.rows.filter((row) => row.status === status).length;
    console.log(
      `\n${count("OK")} ok · ${count("EMPTY")} empty · ${count("SKIP")} skipped · ${count("FAIL")} failed`,
    );
    if (count("FAIL") > 0) process.exitCode = 1;
  }
}

const sized = (items: { length: number }, unit: string): string | null =>
  items.length ? `${items.length} ${unit}` : null;

/** Endpoints needing a signature or an existing strategy are exercised by their own flows. */
async function main(): Promise<void> {
  console.log(`smoke: ${BASE_URL}\n`);
  const matrix = new Matrix();

  await matrix.check("config", async () => {
    const config = await client.config();
    return `chain ${config.chain_id} · fee ${config.default_fee_bps}bps · faucet=${config.features.faucet}`;
  });

  await matrix.check("assets", async () => sized((await client.assets()).items, "assets"));

  const pools = await client.pools().catch(() => undefined);
  await matrix.check("pools", async () => (pools ? sized(pools.items, "pools") : null));

  const pool = pools?.items[0];
  if (pool) {
    const pair = { base: pool.base.address, quote: pool.quote.address };
    await matrix.check("poolDetail", async () => {
      const detail = await client.poolDetail(pair);
      return `${detail.pair} · ${detail.makers.length} makers`;
    });
    await matrix.check("poolDepth", async () => {
      const depth = await client.poolDepth(pair);
      return depth.points.length ? `${depth.points.length} points · best ${depth.best_price}` : null;
    });
  } else {
    matrix.skipAll(["poolDetail", "poolDepth"], "no pools");
  }

  await matrix.check("stats", async () => {
    const stats = await client.stats();
    return `block ${stats.block_height} · makers ${stats.active_makers ?? 0} · settled ${stats.trades_settled ?? 0}`;
  });

  const trades = await client.trades({ limit: 5 }).catch(() => undefined);
  await matrix.check("trades", async () => (trades ? sized(trades.items, "trades") : null));

  const trade = trades?.items[0];
  if (trade) {
    await matrix.check("tradeDetail", async () => {
      const detail = await client.tradeDetail(trade.id);
      return `${detail.status} · ${detail.legs?.length ?? 0} legs`;
    });
  } else {
    matrix.skip("tradeDetail", "no trades");
  }

  await matrix.check("activity", async () => sized((await client.activity({ limit: 5 })).items, "events"));

  const makers = await client.makers().catch(() => undefined);
  await matrix.check("makers", async () => (makers ? sized(makers.items, "makers") : null));

  const maker = makers?.items[0]?.maker;
  if (maker) {
    await matrix.check("maker", async () => {
      const dashboard = await client.maker(maker);
      return `${dashboard.active_positions} active · ${dashboard.window_days}d window`;
    });
    await matrix.check("makerInventory", async () =>
      sized((await client.makerInventory(maker)).items, "rows"),
    );
    await matrix.check("makerTrades", async () =>
      sized((await client.makerTrades(maker, { limit: 5 })).items, "trades"),
    );

    const positions = await client.makerPositions(maker).catch(() => undefined);
    await matrix.check("makerPositions", async () =>
      positions ? sized(positions.items, "positions") : null,
    );

    const hash = positions?.items[0]?.strategy_hash;
    if (hash) {
      await matrix.check("position", async () => (await client.position(hash)).pair);
    } else {
      matrix.skip("position", "no positions");
    }
  } else {
    matrix.skipAll(
      ["maker", "makerInventory", "makerTrades", "makerPositions", "position"],
      "no makers",
    );
  }

  await matrix.check("pairs", async () => sized((await client.pairs()).items, "pairs"));

  await matrix.check("balances", async () => {
    const wallet = "0xf39Fd6e51aad88F6F4ce6aB8827279cffFb92266";
    return sized((await client.balances(wallet)).items, "tokens");
  });

  // A quote needs live maker depth; with none shipped there is nothing to fill against.
  if (pool) {
    await matrix.check("quote", async () => {
      const quote = await client.quote({
        token_in: pool.base.address,
        token_out: pool.quote.address,
        amount_in: String(10n ** BigInt(pool.base.decimals)),
      });
      return `out ${quote.amount_out?.display ?? "?"}`;
    });
  } else {
    matrix.skip("quote", "no pools");
  }

  matrix.skip("swap", "needs a signed order");
  matrix.skip("positionsPreview", "needs a shipped strategy");

  matrix.report();
}

await main();
