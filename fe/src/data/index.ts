export type Token = {
  symbol: string;
  name: string;
  price: number;
  change: string;
  balance: string;
  tags: string[];
  net: string;
};

export const TOKENS: Token[] = [
  {
    symbol: "ETH",
    name: "Ether",
    price: 3104.78,
    change: "+0.97%",
    balance: "4.284",
    tags: ["ETH"],
    net: "Ethereum",
  },
  {
    symbol: "SOL",
    name: "Solana",
    price: 136.52,
    change: "+2.14%",
    balance: "112.40",
    tags: ["BTC"],
    net: "Solana",
  },
  {
    symbol: "USDC",
    name: "USD Coin",
    price: 1,
    change: "+0.02%",
    balance: "8,120.00",
    tags: ["Stables", "Aqua"],
    net: "Ethereum",
  },
  {
    symbol: "USDT",
    name: "Tether USD",
    price: 0.999,
    change: "0.00%",
    balance: "2,400.00",
    tags: ["USD"],
    net: "Ethereum",
  },
  {
    symbol: "WBTC",
    name: "Wrapped Bitcoin",
    price: 62140.5,
    change: "+0.56%",
    balance: "0.0412",
    tags: ["ETH"],
    net: "Ethereum",
  },
  {
    symbol: "1INCH",
    name: "1inch Network",
    price: 0.42,
    change: "-1.08%",
    balance: "15,600",
    tags: ["DeFi", "Aqua"],
    net: "Ethereum",
  },
  {
    symbol: "ARB",
    name: "Arbitrum",
    price: 0.78,
    change: "+3.42%",
    balance: "980",
    tags: ["DeFi"],
    net: "Arbitrum",
  },
  {
    symbol: "stETH",
    name: "Lido Staked Ether",
    price: 3098.1,
    change: "+1.12%",
    balance: "1.902",
    tags: ["DeFi", "Majors"],
    net: "Ethereum",
  },
];

export const NETWORKS = [
  "All networks",
  "Ethereum",
  "Solana",
  "Arbitrum",
  "Base",
];
export const TAGS = ["All", "Aqua", "Stables", "Majors", "DeFi"];

/** Baseline band half-width, in the create-pool chart's own units. */
export const BAND_K0 = 46;

export type CorePair = {
  a: string;
  b: string;
  mid: number;
  type: string;
  band: number;
  fee: string;
  vol: number;
  drift: number;
  walA: number;
  walB: number;
};

export const CORE6: CorePair[] = [
  {
    a: "WETH",
    b: "USDC",
    mid: 3104.78,
    type: "Volatile",
    band: 0.35,
    fee: "0.05%",
    vol: 0.34,
    drift: 0.0009,
    walA: 2.41,
    walB: 7480.2,
  },
  {
    a: "WBTC",
    b: "ETH",
    mid: 20.014,
    type: "Correlated",
    band: 0.18,
    fee: "0.01%",
    vol: 0.16,
    drift: -0.0004,
    walA: 0.42,
    walB: 8.36,
  },
  {
    a: "WBTC",
    b: "USDC",
    mid: 62140.5,
    type: "Volatile",
    band: 0.42,
    fee: "0.05%",
    vol: 0.41,
    drift: 0.0015,
    walA: 0.19,
    walB: 11920.0,
  },
  {
    a: "USDC",
    b: "USDT",
    mid: 1.0002,
    type: "Stable",
    band: 0.05,
    fee: "0.01%",
    vol: 0.04,
    drift: 0.00006,
    walA: 253.79,
    walB: 275.4,
  },
  {
    a: "DAI",
    b: "USDC",
    mid: 0.9998,
    type: "Stable",
    band: 0.05,
    fee: "0.01%",
    vol: 0.05,
    drift: -0.00008,
    walA: 640.1,
    walB: 512.66,
  },
  {
    a: "LINK",
    b: "WETH",
    mid: 0.00512,
    type: "Correlated",
    band: 0.26,
    fee: "0.05%",
    vol: 0.28,
    drift: 0.0007,
    walA: 184.5,
    walB: 1.07,
  },
];

export type AssetLeg = {
  pair: string;
  meta: string;
  cur: string;
  op: string;
  usd: number;
  fees: number;
  apy: number;
  vol: number;
  cov: number;
};

export type AssetRow = {
  sym: string;
  tint: string;
  wallet: number;
  walletAmt: string;
  shared: number;
  sharedAmt: string;
  fees: number;
  apy: number;
  legs: AssetLeg[];
};

export const ASSET_ROWS: AssetRow[] = [
  {
    sym: "USDC",
    tint: "#e6f0fb",
    wallet: 271,
    walletAmt: "270.8343",
    shared: 542,
    sharedAmt: "541.6494",
    fees: 186,
    apy: 0,
    legs: [
      {
        pair: "USDC/USDT",
        meta: "Straight · 0.04%",
        cur: "270.8194",
        op: "270.8194",
        usd: 271,
        fees: 92,
        apy: 0,
        vol: 0,
        cov: 1.0,
      },
      {
        pair: "USDC/WBTC",
        meta: "Concentrated · 0.05%",
        cur: "270.8299",
        op: "270.8299",
        usd: 271,
        fees: 94,
        apy: 11.4,
        vol: 1.02e6,
        cov: 0.96,
      },
    ],
  },
  {
    sym: "USDT",
    tint: "#e2f4ef",
    wallet: 167,
    walletAmt: "166.8793",
    shared: 167,
    sharedAmt: "166.8699",
    fees: 74,
    apy: 0,
    legs: [
      {
        pair: "USDC/USDT",
        meta: "Straight · 0.04%",
        cur: "166.8699",
        op: "166.8699",
        usd: 167,
        fees: 74,
        apy: 0,
        vol: 0,
        cov: 1.0,
      },
    ],
  },
  {
    sym: "WBTC",
    tint: "#fbeee0",
    wallet: 247,
    walletAmt: "0.0038236",
    shared: 237,
    sharedAmt: "0.0036711",
    fees: 412,
    apy: 14.2,
    legs: [
      {
        pair: "WBTC/USDC",
        meta: "Concentrated · 0.05%",
        cur: "0.0036711",
        op: "0.0038200",
        usd: 237,
        fees: 412,
        apy: 14.2,
        vol: 1.84e6,
        cov: 0.96,
      },
    ],
  },
  {
    sym: "WETH",
    tint: "#eeeef4",
    wallet: 198,
    walletAmt: "0.0639012",
    shared: 174,
    sharedAmt: "0.0562330",
    fees: 268,
    apy: 11.6,
    legs: [
      {
        pair: "WETH/USDC",
        meta: "Concentrated · 0.05%",
        cur: "0.0562330",
        op: "0.0639012",
        usd: 174,
        fees: 268,
        apy: 11.6,
        vol: 1.02e6,
        cov: 0.88,
      },
    ],
  },
];

export const LAT_SERIES = [96, 62, 78, 104, 44, 70, 22];

export const SHARE_SEGS = [
  { label: "Solvent fills", pct: 61, color: "#c8f24e" },
  { label: "Other resolvers", pct: 19, color: "#63c31c" },
  { label: "Unfilled quotes", pct: 20, color: "#e7e7e4" },
];

export type WalletToken = {
  sym: string;
  name: string;
  usd: number;
  bal: string;
  addr: string;
  chg: number;
  tags: string[];
  net: string;
  tint: string;
};

export const WALLET_TOKENS: WalletToken[] = [
  {
    sym: "USDC",
    name: "USD Coin",
    usd: 263.63,
    bal: "263.339635",
    addr: "0xa0b8…eb48",
    chg: -0.02,
    tags: ["USD"],
    net: "Ethereum",
    tint: "#e6f0fb",
  },
  {
    sym: "USDT",
    name: "Tether USD",
    usd: 254.47,
    bal: "254.466499",
    addr: "0xdac1…1ec7",
    chg: 0,
    tags: ["USD"],
    net: "Ethereum",
    tint: "#e2f4ef",
  },
  {
    sym: "WBTC",
    name: "Wrapped Bitcoin",
    usd: 246.65,
    bal: "0.00382362",
    addr: "0x2260…c599",
    chg: 1.85,
    tags: ["BTC"],
    net: "Ethereum",
    tint: "#fbeee0",
  },
  {
    sym: "stETH",
    name: "Liquid staked Ether 2.0",
    usd: 222.87,
    bal: "0.11638896",
    addr: "0xae7a…fe84",
    chg: 2.02,
    tags: ["ETH", "DeFi"],
    net: "Ethereum",
    tint: "#eaeefb",
  },
  {
    sym: "WETH",
    name: "Wrapped Ether",
    usd: 198.4,
    bal: "0.06390121",
    addr: "0xc02a…6cc2",
    chg: 1.24,
    tags: ["ETH"],
    net: "Ethereum",
    tint: "#eeeef4",
  },
  {
    sym: "ETH",
    name: "Ether",
    usd: 3104.78,
    bal: "0.9412008",
    addr: "native",
    chg: 0.97,
    tags: ["ETH"],
    net: "Arbitrum",
    tint: "#eeeef4",
  },
  {
    sym: "DAI",
    name: "Dai Stablecoin",
    usd: 140.02,
    bal: "140.041209",
    addr: "0x6b17…1d0f",
    chg: 0.01,
    tags: ["USD", "DeFi"],
    net: "Base",
    tint: "#fbf2e0",
  },
  {
    sym: "LINK",
    name: "Chainlink",
    usd: 96.18,
    bal: "6.4120044",
    addr: "0x5149…ff2ca",
    chg: -0.64,
    tags: ["DeFi"],
    net: "Ethereum",
    tint: "#e6ecfb",
  },
];

export const TIME_LABELS: Record<string, string[]> = {
  "7d": ["Aug 28", "Sep 1", "Sep 4"],
  "3m": ["Jun 2026", "Jul 2026", "Sep 2026"],
  All: ["2024", "2025", "2026"],
};

export const DEFAULT_BAND: Record<string, number> = {
  Stable: 0.05,
  Volatile: 0.35,
  Incentivised: 0.9,
};

export const STABLES = ["USDC", "USDT", "DAI"];

export type Pool = {
  pair: string;
  type: string;
  venue: string;
  range: string;
  tvl: string;
  vol: string;
  fills: string;
  fee: string;
  apr: string;
};

export const POOLS: Pool[] = [
  {
    pair: "ETH / USDC",
    type: "Volatile",
    venue: "Aqua core · 4 makers",
    range: "0.02% spread",
    tvl: "$18.4M",
    vol: "$6.2M",
    fills: "2,140",
    fee: "0.05% · v4",
    apr: "12.8%",
  },
  {
    pair: "SOL / USDC",
    type: "Volatile",
    venue: "Aqua core · 6 makers",
    range: "0.04% spread",
    tvl: "$9.7M",
    vol: "$4.1M",
    fills: "1,806",
    fee: "0.05% · v4",
    apr: "18.2%",
  },
  {
    pair: "WBTC / ETH",
    type: "Volatile",
    venue: "Aqua extended · 3 makers",
    range: "0.01% spread",
    tvl: "$24.1M",
    vol: "$3.4M",
    fills: "912",
    fee: "0.01% · v4",
    apr: "7.4%",
  },
  {
    pair: "1INCH / USDC",
    type: "Incentivised",
    venue: "Aqua incentive · 5 makers",
    range: "0.09% spread",
    tvl: "$3.2M",
    vol: "$1.1M",
    fills: "604",
    fee: "0.30% · v4",
    apr: "31.6%",
  },
  {
    pair: "ARB / USDC",
    type: "Incentivised",
    venue: "Aqua extended · 4 makers",
    range: "0.06% spread",
    tvl: "$5.8M",
    vol: "$2.0M",
    fills: "1,142",
    fee: "0.05% · v4",
    apr: "16.1%",
  },
  {
    pair: "stETH / ETH",
    type: "Stable",
    venue: "Aqua core · 3 makers",
    range: "0.01% spread",
    tvl: "$31.6M",
    vol: "$2.8M",
    fills: "486",
    fee: "0.01% · v4",
    apr: "5.2%",
  },
  {
    pair: "USDC / USDT",
    type: "Stable",
    venue: "Aqua core · 8 makers",
    range: "0.005% spread",
    tvl: "$42.3M",
    vol: "$11.4M",
    fills: "5,318",
    fee: "0.01% · v4",
    apr: "4.1%",
  },
  {
    pair: "SOL / ETH",
    type: "Volatile",
    venue: "Aqua extended · 5 makers",
    range: "0.05% spread",
    tvl: "$7.1M",
    vol: "$1.9M",
    fills: "820",
    fee: "0.05% · v4",
    apr: "14.7%",
  },
];

export const NAV = ["Home", "Swap", "Pools", "Makers", "Explorer"] as const;
export type NavPage = (typeof NAV)[number];

export type ActivityRow = {
  kind: string;
  who: string;
  text: string;
  flow: string;
  blk: string;
  ago: string;
  tx: string;
  ent: string;
  link: string;
  trade?: string;
};

export const ACTIVITY: ActivityRow[] = [
  {
    kind: "register",
    who: "MM-07",
    text: "registered ETH/USDC · XYC · 5bps",
    flow: "strategy 0x3f9a…c210",
    blk: "41,310",
    ago: "2s",
    tx: "0x3f9a…c210",
    ent: "maker",
    link: "strategy",
  },
  {
    kind: "pull",
    who: "MM-01",
    text: "pull on ETH/USDC",
    flow: "1.2 ETH → 3,724 USDC",
    blk: "41,309",
    ago: "6s",
    tx: "0x8f21…7d44",
    ent: "maker",
    link: "trade",
    trade: "#a91c",
  },
  {
    kind: "pull",
    who: "MM-04",
    text: "pull on ETH/USDC",
    flow: "1.3 ETH → 4,018 USDC",
    blk: "41,309",
    ago: "6s",
    tx: "0x8f2b…91c0",
    ent: "maker",
    link: "trade",
    trade: "#a91c",
  },
  {
    kind: "dock",
    who: "MM-03",
    text: "docked WBTC/USDC",
    flow: "position closed",
    blk: "41,301",
    ago: "30s",
    tx: "0x1a05…b8e1",
    ent: "maker",
    link: "strategy",
  },
  {
    kind: "push",
    who: "MM-04",
    text: "push into ETH/USDC",
    flow: "8,400 USDC committed",
    blk: "41,298",
    ago: "48s",
    tx: "0x55cc…2f90",
    ent: "maker",
    link: "strategy",
  },
  {
    kind: "pull",
    who: "MM-02",
    text: "pull on WBTC/USDC",
    flow: "0.18 WBTC → 7,314 USDC",
    blk: "41,297",
    ago: "4m",
    tx: "0x2b64…0ac7",
    ent: "maker",
    link: "trade",
    trade: "#b7e2",
  },
  {
    kind: "register",
    who: "MM-11",
    text: "registered USDC/USDT · Pegged · 1bps",
    flow: "strategy 0x9d12…44be",
    blk: "41,291",
    ago: "6m",
    tx: "0x9d12…44be",
    ent: "maker",
    link: "strategy",
  },
  {
    kind: "push",
    who: "MM-07",
    text: "push into USDC/USDT",
    flow: "42,000 USDC committed",
    blk: "41,286",
    ago: "9m",
    tx: "0x63aa…7e11",
    ent: "maker",
    link: "strategy",
  },
  {
    kind: "dock",
    who: "MM-09",
    text: "docked SOL/USDC",
    flow: "position closed",
    blk: "41,271",
    ago: "14m",
    tx: "0xd410…2b77",
    ent: "maker",
    link: "strategy",
  },
];

export type Trade = {
  id: string;
  blk: string;
  pair: string;
  inn: string;
  out: string;
  makers: number;
  impact: string;
  status: string;
  tx: string;
  taker: string;
  spread: number;
  unit: string;
  dl: string;
};

export const TRADES: Trade[] = [
  {
    id: "a91c",
    blk: "41,309",
    pair: "ETH/USDC",
    inn: "2.5 ETH",
    out: "7,742 USDC",
    makers: 3,
    impact: "0.04%",
    status: "confirmed",
    tx: "0x8f21…7d44",
    taker: "0xC0A8…B505",
    spread: 3.1,
    unit: "USDC",
    dl: "41,340",
  },
  {
    id: "b7e2",
    blk: "41,297",
    pair: "WBTC/USDC",
    inn: "0.3 WBTC",
    out: "12,190 USDC",
    makers: 2,
    impact: "0.06%",
    status: "confirmed",
    tx: "0x2b64…0ac7",
    taker: "0x4Fd1…22A9",
    spread: 7.4,
    unit: "USDC",
    dl: "41,326",
  },
  {
    id: "c033",
    blk: "—",
    pair: "SOL/USDC",
    inn: "120 SOL",
    out: "—",
    makers: 0,
    impact: "—",
    status: "declined",
    tx: "—",
    taker: "0x91bE…7C40",
    spread: 0,
    unit: "USDC",
    dl: "41,318",
  },
  {
    id: "d5aa",
    blk: "41,280",
    pair: "ETH/USDC",
    inn: "5.0 ETH",
    out: "15,470 USDC",
    makers: 4,
    impact: "0.09%",
    status: "reorg-open",
    tx: "0x77ab…19f3",
    taker: "0x2E77…1D08",
    spread: 6.2,
    unit: "USDC",
    dl: "41,311",
  },
  {
    id: "e18f",
    blk: "41,274",
    pair: "USDC/USDT",
    inn: "24,000 USDC",
    out: "23,988 USDT",
    makers: 5,
    impact: "0.01%",
    status: "confirmed",
    tx: "0x4e61…20aa",
    taker: "0xAb03…9E61",
    spread: 2.4,
    unit: "USDT",
    dl: "41,300",
  },
  {
    id: "f240",
    blk: "41,260",
    pair: "WETH/USDC",
    inn: "1.4 WETH",
    out: "4,331 USDC",
    makers: 2,
    impact: "0.05%",
    status: "failed",
    tx: "0xbb92…c401",
    taker: "0x7C55…04Bd",
    spread: 0,
    unit: "USDC",
    dl: "41,288",
  },
  {
    id: "a7c1",
    blk: "—",
    pair: "ETH/USDC",
    inn: "0.9 ETH",
    out: "2,790 USDC",
    makers: 2,
    impact: "0.03%",
    status: "pending",
    tx: "0x51d0…8ea2",
    taker: "0xE1b4…33F7",
    spread: 1.2,
    unit: "USDC",
    dl: "41,352",
  },
  {
    id: "b904",
    blk: "41,251",
    pair: "WBTC/ETH",
    inn: "0.4 WBTC",
    out: "7.6 ETH",
    makers: 3,
    impact: "0.07%",
    status: "partial",
    tx: "0x0cc7…5b18",
    taker: "0x88Da…41Ac",
    spread: 2.9,
    unit: "ETH",
    dl: "41,279",
  },
];

export const DAY_LABELS = ["S", "M", "T", "W", "T", "F", "S"];
export const EARN_VALUES = [42, 96, 68, 100, 54, 76, 88];
export const EARN_TIPS = [
  "$41.20",
  "$92.60",
  "$66.10",
  "$155.20",
  "$52.40",
  "$74.80",
  "$86.30",
];
export const MAKER_BARS = [46, 72, 58, 96, 64, 84, 70];
export const HEALTH = [38, 58, 26, 72, 44, 90, 34, 62, 48, 80, 30, 66, 52, 88];

export type Strategy = {
  pair: string;
  curve: string;
  maker: string;
  state: string;
  since: string;
  virt: string;
  act: string;
  backed: boolean;
  short: string;
  fee: string;
  range: string;
  rangeTag: string;
  mid: string;
  lo: number;
  hi: number;
  ref: number;
  fills: string;
  vol: string;
  up: string;
  last: string;
  /** [txHash, flow, block, tradeId] */
  tx: [string, string, string, string][];
};

export const STRATS: Strategy[] = [
  {
    pair: "ETH / USDC",
    curve: "Concentrated",
    maker: "0x9f3c…MM-04",
    state: "active",
    since: "40,110",
    virt: "4.0 ETH · 12,000 USDC",
    act: "4.0 ETH · 12,000 USDC",
    backed: true,
    short: "",
    fee: "5 bps",
    range: "2,900 – 3,300 USDC",
    rangeTag: "±6% around 3,104",
    mid: "3,104 USDC",
    lo: 2900,
    hi: 3300,
    ref: 3104,
    fills: "1,204",
    vol: "$2.1M",
    up: "99.3%",
    last: "6s ago",
    tx: [
      ["0x8f…a1", "1.6 ETH → 4,955 USDC", "41,309", "#a91c"],
      ["0x2b…c9", "0.8 ETH → 2,470 USDC", "41,297", "#a0e2"],
      ["0x71…f4", "2.4 ETH → 7,430 USDC", "41,280", "#9fd1"],
      ["0xd0…3e", "0.5 ETH → 1,548 USDC", "41,266", "#9f04"],
    ],
  },
  {
    pair: "USDC / USDT",
    curve: "Pegged",
    maker: "0x77de…MM-07",
    state: "active",
    since: "40,318",
    virt: "180,000 USDC · 180,000 USDT",
    act: "142,400 USDC · 180,000 USDT",
    backed: false,
    short: "short 37,600 USDC",
    fee: "1 bps",
    range: "Peg 1.0000",
    rangeTag: "+0.29% / −0.45%",
    mid: "1.0000 USDT",
    lo: 0.9955,
    hi: 1.0029,
    ref: 1.0,
    fills: "5,318",
    vol: "$11.4M",
    up: "99.9%",
    last: "2s ago",
    tx: [
      ["0x4c…7b", "40,000 USDC → 39,996 USDT", "41,311", "#b2a7"],
      ["0x93…1d", "12,500 USDC → 12,499 USDT", "41,304", "#b1e0"],
    ],
  },
  {
    pair: "WBTC / ETH",
    curve: "XYC",
    maker: "0xc410…MM-11",
    state: "docked",
    since: "39,742",
    virt: "1.2 WBTC · 19.4 ETH",
    act: "1.2 WBTC · 19.4 ETH",
    backed: true,
    short: "",
    fee: "30 bps",
    range: "Full range",
    rangeTag: "constant product",
    mid: "19.36 ETH",
    lo: 0,
    hi: 0,
    ref: 19.36,
    fills: "0",
    vol: "$0",
    up: "—",
    last: "docked at blk 41,120",
    tx: [],
  },
];
