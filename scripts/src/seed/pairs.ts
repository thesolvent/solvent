export interface PairSpec {
  base: string;
  quote: string;
  pegged?: boolean;
  widthPct: number;
  feeBps: number;
  /** Base-token inventory; quote inventory is sized at the API's mid price. */
  size: number;
}

export const PAIRS: readonly PairSpec[] = [
  { base: "WETH", quote: "USDC", widthPct: 8, feeBps: 5, size: 30 },
  { base: "WBTC", quote: "USDC", widthPct: 10, feeBps: 30, size: 2 },
  { base: "LINK", quote: "USDC", widthPct: 12, feeBps: 30, size: 20_000 },
  {
    base: "DAI",
    quote: "USDC",
    pegged: true,
    widthPct: 1,
    feeBps: 1,
    size: 250_000,
  },
  { base: "WETH", quote: "DAI", widthPct: 8, feeBps: 5, size: 30 },
  { base: "WBTC", quote: "DAI", widthPct: 10, feeBps: 30, size: 2 },
  { base: "LINK", quote: "DAI", widthPct: 12, feeBps: 30, size: 20_000 },
  {
    base: "USDT",
    quote: "USDC",
    pegged: true,
    widthPct: 1,
    feeBps: 1,
    size: 250_000,
  },
];
