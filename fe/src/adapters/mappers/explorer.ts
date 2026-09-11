import type { ActivityEvent, Stats, Trade } from "@solvent/sdk/client";
import type { CrossChainOrder, SagaState } from "@solvent/sdk/cross-chain";
import { formatUnits } from "viem";
import type { Asset } from "@/data";
import type {
  ActivityRecord,
  ExplorerStats,
  TradeRecord,
} from "@/data/explorer";

export function toTrade(api: Trade): TradeRecord {
  const legs = api.legs ?? [];
  const total = legs.reduce((sum, leg) => sum + BigInt(leg.amount_out.raw), 0n);
  return {
    flow: "same-chain",
    signaturePresent: api.signature_present ?? null,
    id: api.id,
    status: api.status,
    taker: api.taker,
    input: {
      symbol: api.input.token.symbol,
      display: api.input.amount.display,
    },
    output: {
      symbol: api.output.token.symbol,
      display: api.output.amount.display,
    },
    surplus: api.surplus
      ? { symbol: api.input.token.symbol, display: api.surplus.display }
      : null,
    priceImpactPct: api.price_impact_pct ?? null,
    makers:
      api.legs == null
        ? null
        : new Set(legs.map((leg) => leg.maker.toLowerCase())).size,
    txHash: api.tx_hash ?? null,
    blockNumber: api.block_number ?? null,
    createdAt: api.created_at,
    settledAt: api.settled_at ?? null,
    deadlineAt: api.deadline_block ?? null,
    orderHash: api.order_hash ?? null,
    lifecycle: api.lifecycle ?? [],
    legs: legs.map((leg) => ({
      maker: leg.maker,
      strategyHash: leg.strategy_hash,
      curve: leg.curve ?? null,
      input: { symbol: api.input.token.symbol, display: leg.amount_in.display },
      output: {
        symbol: api.output.token.symbol,
        display: leg.amount_out.display,
      },
      sharePct:
        total === 0n
          ? 0
          : Number((BigInt(leg.amount_out.raw) * 10_000n) / total) / 100,
    })),
  };
}

function crossChainStatus(state: SagaState): string {
  if (state === "complete") return "confirmed";
  if (state === "failed_before_delivery") return "failed";
  if (state === "quoted") return "quoted";
  if (["preparing", "prepared"].includes(state)) return "reserved";
  return "submitted";
}

function crossChainLifecycle(order: CrossChainOrder) {
  const recordedAt = new Map(
    (order.lifecycle ?? []).map((event) => [event.stage, event.at]),
  );
  const stages = [
    ["quoted", "quoted", true],
    ["destination fill", "destination_fill", order.destination != null],
    ["proof relay", "proof_relay", order.fill_proof != null],
    ["origin claim", "origin_claim", order.origin != null],
    ["repayment", "repayment", order.repayment != null],
    ["complete", "complete", order.state === "complete"],
  ] as const;
  return stages
    .filter(([, , recorded]) => recorded)
    .map(([status, stage]) => ({ status, at: recordedAt.get(stage) ?? null }));
}

function servedAsset(assets: Asset[], address: string): Asset | undefined {
  return assets.find(
    (asset) => asset.address.toLowerCase() === address.toLowerCase(),
  );
}

/** Adapt a durable SolventX saga to the already-reviewed trade details layout. */
export function toCrossChainTrade(
  order: CrossChainOrder,
  originAssets: Asset[],
  destinationAssets: Asset[],
): TradeRecord {
  const input = servedAsset(originAssets, order.quote.origin.input_token);
  const destinationInput = servedAsset(
    destinationAssets,
    order.quote.destination.input_token,
  );
  const output = servedAsset(
    destinationAssets,
    order.quote.destination.output_token,
  );
  const inputDecimals = input?.decimals ?? 18;
  const destinationInputDecimals = destinationInput?.decimals ?? 18;
  const outputDecimals = output?.decimals ?? 18;
  const sources = order.quote.destination.sources;
  const total = sources.reduce(
    (sum, source) => sum + BigInt(source.amount),
    0n,
  );
  const amountOut = BigInt(order.quote.amount_out);
  // Browser-authored quotes expire ten minutes after creation; the saga currently stores no clock.
  const createdAt = Math.max(0, order.quote.expires_at_unix - 600);
  const status = crossChainStatus(order.state);
  const evidence = order.origin ?? order.destination;
  const lifecycle = crossChainLifecycle(order);
  const quotedAt = lifecycle.find((stage) => stage.status === "quoted")?.at;
  const completedAt = lifecycle.find(
    (stage) => stage.status === "complete",
  )?.at;

  return {
    flow: "cross-chain",
    signaturePresent: true,
    id: order.order_id,
    status,
    taker: order.taker ?? "",
    input: {
      symbol: input?.symbol ?? "TOKEN",
      display: formatUnits(BigInt(order.quote.amount_in), inputDecimals),
      net: input?.net ?? `Chain ${order.quote.origin.local_chain}`,
      logoUri: input?.logoUri,
    },
    output: {
      symbol: output?.symbol ?? "TOKEN",
      display: formatUnits(amountOut, outputDecimals),
      net: output?.net ?? `Chain ${order.quote.destination.local_chain}`,
      logoUri: output?.logoUri,
    },
    surplus: null,
    priceImpactPct: null,
    makers: new Set(sources.map((source) => source.maker.toLowerCase())).size,
    txHash: evidence?.transaction_hash ?? null,
    blockNumber: evidence?.block_number ?? null,
    createdAt: quotedAt ?? createdAt,
    settledAt: completedAt ?? null,
    deadlineAt: order.quote.expires_at_unix,
    orderHash: order.order_id,
    lifecycle,
    legs: sources.map((source) => {
      const sourceOutput = BigInt(source.amount);
      const destinationAmountIn = BigInt(order.quote.destination.amount_in);
      const sourceInput =
        total === 0n ? 0n : (destinationAmountIn * sourceOutput) / total;
      return {
        maker: source.maker,
        strategyHash: source.strategy_hash,
        curve: null,
        input: {
          symbol: destinationInput?.symbol ?? "TOKEN",
          display: formatUnits(sourceInput, destinationInputDecimals),
        },
        output: {
          symbol: output?.symbol ?? "TOKEN",
          display: formatUnits(sourceOutput, outputDecimals),
        },
        sharePct:
          total === 0n ? 0 : Number((sourceOutput * 10_000n) / total) / 100,
      };
    }),
  };
}

export function toActivity(api: ActivityEvent): ActivityRecord {
  return {
    id: [
      api.tx_hash,
      api.log_index,
      api.block_number,
      api.kind,
      api.maker,
      api.strategy_hash,
      api.at,
    ].join(":"),
    kind: api.kind,
    maker: api.maker,
    strategyHash: api.strategy_hash,
    amount: api.token
      ? { symbol: api.token.token.symbol, display: api.token.amount.display }
      : null,
    at: api.at,
    blockNumber: api.block_number ?? null,
    txHash: api.tx_hash ?? null,
  };
}

export function toStats(api: Stats): ExplorerStats {
  return {
    blockHeight: api.block_height,
    events24h: api.events_24h ?? null,
    tradesSettled: api.trades_settled ?? null,
    confirmedPct: api.confirmed_pct ?? null,
    medianImpactPct: api.median_impact_pct ?? null,
    activeMakers: api.active_makers ?? null,
    quotingNow: api.quoting_now ?? null,
  };
}
