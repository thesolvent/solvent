import {
  Strategy,
  linearWidthFromSymmetricRangePercent,
  strategyAllocator,
  type AllocationCurve,
} from "@solvent/sdk/construction";
import { createPositionClient } from "@solvent/sdk/positions";
import { MAX_UINT248, parseTokenAmount } from "@solvent/sdk/validation";
import type { Address, Hex } from "viem";

import type { Asset, PairInfo } from "@solvent/sdk/client";
import type {
  CreatePair,
  CreateToken,
  PositionInput,
  PositionsPort,
} from "@/ports/positions";
import { solventApi } from "./client";

const TOKEN_TINTS: Record<string, string> = {
  DAI: "#f5f0df",
  LINK: "#e7ecfb",
  USDC: "#e6f0fb",
  USDT: "#e2f4ef",
  WBTC: "#fbeee0",
  WETH: "#eee9fb",
};

export const positionsAdapter: PositionsPort = {
  async pairs(wallet) {
    const [catalog, assets, pools] = await Promise.all([
      solventApi.pairs(wallet ? { wallet } : undefined),
      solventApi.assets(),
      solventApi.pools(),
    ]);
    const metadata = new Map(
      assets.items.map((asset) => [asset.address.toLowerCase(), asset]),
    );
    const tvlByPair = new Map(
      pools.items.map((pool) => [
        pairKey(pool.base.address, pool.quote.address),
        pool.tvl_usd ?? undefined,
      ]),
    );
    return catalog.items.flatMap((pair) =>
      pair.mid && Number.isFinite(pair.mid) && pair.mid > 0
        ? [
            toCreatePair(
              pair,
              metadata,
              tvlByPair.get(pairKey(pair.base.address, pair.quote.address)),
            ),
          ]
        : [],
    );
  },

  async history(pair, period) {
    const history = await solventApi.pairHistory({
      base: pair.base.address,
      quote: pair.quote.address,
      period,
    });
    return history.points.map((point) => ({
      timestampMs: point.timestamp_ms,
      price: point.price,
      volumeUsd: point.volume_usd ?? undefined,
    }));
  },

  createIntent(input, clients) {
    const sdk = createPositionClient({ api: solventApi, ...clients });
    let intent: ReturnType<typeof sdk.createIntent> | undefined;
    return {
      async submit() {
        intent ??= sdk.createIntent(buildRequest(input));
        return intent.submit();
      },
    };
  },

  pushIntent(input, clients) {
    const sdk = createPositionClient({ api: solventApi, ...clients });
    let intent: ReturnType<typeof sdk.pushIntent> | undefined;
    return {
      async submit() {
        intent ??= sdk.pushIntent({
          maker: input.maker as Address,
          strategyHash: input.strategyHash as Hex,
          token: input.token.address as Address,
          amount: parseTokenAmount(
            input.amount,
            input.token.decimals,
            `${input.token.symbol} amount`,
          ),
        });
        return intent.submit();
      },
    };
  },

  dockIntent(input, clients) {
    const sdk = createPositionClient({ api: solventApi, ...clients });
    let intent: ReturnType<typeof sdk.dockIntent> | undefined;
    return {
      async submit() {
        intent ??= sdk.dockIntent({
          maker: input.maker as Address,
          strategyHash: input.strategyHash as Hex,
          tokens: input.tokens.map((token) => token as Address),
        });
        return intent.submit();
      },
    };
  },
};

function toCreatePair(
  pair: PairInfo,
  metadata: ReadonlyMap<string, Asset>,
  tvlUsd: number | undefined,
): CreatePair {
  return {
    base: toCreateToken(
      pair.base,
      pair.wallet?.base ?? 0,
      pair.wallet?.base_raw ?? "0",
      metadata.get(pair.base.address.toLowerCase()),
    ),
    quote: toCreateToken(
      pair.quote,
      pair.wallet?.quote ?? 0,
      pair.wallet?.quote_raw ?? "0",
      metadata.get(pair.quote.address.toLowerCase()),
    ),
    mid: pair.mid as number,
    tvlUsd,
    type: pair.type === "stable" ? "Stable" : "Volatile",
    defaultBandPct: pair.default_band_pct,
    defaultFeeBps: pair.default_fee_bps,
  };
}

function pairKey(left: string, right: string): string {
  return [left.toLowerCase(), right.toLowerCase()].sort().join(":");
}

function toCreateToken(
  token: PairInfo["base"],
  balance: number,
  balanceRaw: string,
  asset: Asset | undefined,
): CreateToken {
  return {
    address: token.address as Address,
    decimals: token.decimals,
    symbol: token.symbol,
    name: asset?.name ?? token.symbol,
    tags: asset?.tags ?? [],
    balance,
    balanceRaw: BigInt(balanceRaw),
    valueUsd: asset?.price_usd == null ? undefined : asset.price_usd * balance,
    changePct: asset?.change_24h_pct ?? undefined,
    tint: TOKEN_TINTS[token.symbol] ?? "#f1f1ee",
  };
}

function buildRequest(input: PositionInput) {
  const { base, quote } = orientedPair(input);
  const amountBase = parseTokenAmount(
    input.amountBase,
    base.decimals,
    `${base.symbol} amount`,
    MAX_UINT248,
  );
  const amountQuote = parseTokenAmount(
    input.amountQuote,
    quote.decimals,
    `${quote.symbol} amount`,
    MAX_UINT248,
  );
  validateAllocation(input, base, quote, amountBase, amountQuote);
  const strategy = buildStrategy(input, base, quote, amountBase, amountQuote)
    .fee(input.feeBps)
    .salt(secureSalt())
    .build(input.maker);
  return {
    maker: input.maker,
    strategy,
    amounts: [
      { token: base.address, amount: amountBase },
      { token: quote.address, amount: amountQuote },
    ],
  };
}

function validateAllocation(
  input: PositionInput,
  base: CreateToken,
  quote: CreateToken,
  amountBase: bigint,
  amountQuote: bigint,
) {
  const curve: AllocationCurve =
    input.curve === "Full range"
      ? { kind: "fullRange" }
      : input.curve === "Pegged"
        ? { kind: "pegged" }
        : {
            kind: "concentrated",
            priceMin: input.priceMin,
            priceMax: input.priceMax,
          };
  const allocator = strategyAllocator({
    base: { address: base.address, decimals: base.decimals },
    quote: { address: quote.address, decimals: quote.decimals },
    spotPrice: input.spotPrice,
    curve,
  });
  if (
    amountBase <= 0n ||
    amountQuote <= 0n ||
    !allocator.matches({ base: amountBase, quote: amountQuote })
  ) {
    throw new Error(
      "Deposit amounts do not match the selected curve at the live market price",
    );
  }
}

function orientedPair(input: PositionInput) {
  return input.flipped
    ? { base: input.pair.quote, quote: input.pair.base }
    : { base: input.pair.base, quote: input.pair.quote };
}

function buildStrategy(
  input: PositionInput,
  base: CreateToken,
  quote: CreateToken,
  amountBase: bigint,
  amountQuote: bigint,
) {
  const baseRef = { address: base.address, decimals: base.decimals };
  const quoteRef = { address: quote.address, decimals: quote.decimals };
  switch (input.curve) {
    case "Full range":
      return Strategy.fullRange();
    case "Concentrated":
      return Strategy.concentrated({
        base: baseRef,
        quote: quoteRef,
        priceMin: input.priceMin,
        priceMax: input.priceMax,
      });
    case "Pegged":
      if (!input.peggedSymmetric) {
        throw new Error("Pegged positions require a symmetric range");
      }
      return Strategy.pegged({
        tokenA: { ...baseRef, reserve: amountBase },
        tokenB: { ...quoteRef, reserve: amountQuote },
        linearWidth: linearWidthFromSymmetricRangePercent(input.halfWidthPct),
      });
  }
}

function secureSalt(): bigint {
  const words = crypto.getRandomValues(new Uint32Array(2));
  const salt = words.reduce((value, word) => (value << 32n) | BigInt(word), 0n);
  return salt || 1n;
}
