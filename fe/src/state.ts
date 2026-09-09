import { createContext, useContext } from "react";

export type Page = "Home" | "Swap" | "Pools" | "Makers" | "Explorer" | "Docs";

/** Explorer strategy selector — identifies a maker's curve on a pair. */
export type StratSel = { maker: string; curve: string; pair: string };

export type PoolQuery = {
  ptype: string;
  sell: string;
  buy: string;
  fee: string;
  apr: string;
};

/** The subset of state a breadcrumb restores when you step back to it. */
export type Snap = {
  page: Page;
  detail: number | null;
  create: boolean;
  xpStrat: StratSel | null;
  maker: string | null;
};

export type Crumb = { snap: Snap; label: string };
export type CreateSpan = "7d" | "3m" | "All";

export type AppState = {
  page: Page | null;
  trail: Crumb[];

  swapTab: string;
  fromToken: string;
  toToken: string;
  amount: string;
  openCell: string | null;

  poolQuery: PoolQuery;
  poolSort: string;
  poolPage: number;

  detail: number | null;

  create: boolean;
  createFee: string;
  customFeePct: string;
  createPreset: string;
  corePair: number;
  flipped: boolean;
  strategy: string;
  pegSym: boolean;
  dragging: string | null;
  chartHover: { x: number; y: number } | null;
  chartZoom: number;
  volHover: number | null;
  tokenTag: string | null;
  pickerSlot: number;
  slotA: string | null;
  slotB: string | null;
  q1: string;
  q2: string;

  step: number;
  stepDirty: Record<number, boolean>;
  createSpan: CreateSpan;
  bandMax: number;
  bandMin: number;
  amtA: string;
  amtB: string;
  hoverFrac: number | null;

  makerSort: string;
  filterPick: Record<number, number>;
  tvlMin: number;

  picker: "from" | "to" | null;
  pQuery: string;
  pTag: string;
  pNet: string;

  xpStrat: StratSel | null;
  maker: string | null;

  mkSel: string | null;
  mkSpan: string;
  mkTab: string;
  mkAsset: number | null;
  mkTip: number | null;
  mkBar: number | null;
  mkLat: number | null;

  xpTab: string;
  xpType: string;
  xpEnt: string;
  xpStatus: string;
  xpPair: string;
  xpOpen: string | null;
};

type CreatePositionState = Pick<
  AppState,
  | "create"
  | "createFee"
  | "customFeePct"
  | "createPreset"
  | "corePair"
  | "flipped"
  | "strategy"
  | "pegSym"
  | "dragging"
  | "chartHover"
  | "chartZoom"
  | "volHover"
  | "tokenTag"
  | "pickerSlot"
  | "slotA"
  | "slotB"
  | "q1"
  | "q2"
  | "step"
  | "stepDirty"
  | "createSpan"
  | "bandMax"
  | "bandMin"
  | "amtA"
  | "amtB"
  | "hoverFrac"
>;

export const INITIAL_CREATE_POSITION_STATE: CreatePositionState = {
  create: false,
  createFee: "Auto 0.01%",
  customFeePct: "0.10",
  createPreset: "Market",
  corePair: 3,
  flipped: false,
  strategy: "Pegged",
  pegSym: false,
  dragging: null,
  chartHover: null,
  chartZoom: 1,
  volHover: null,
  tokenTag: null,
  pickerSlot: 1,
  slotA: null,
  slotB: null,
  q1: "",
  q2: "",
  step: 1,
  stepDirty: {},
  createSpan: "3m",
  bandMax: 0.05,
  bandMin: -0.05,
  amtA: "",
  amtB: "",
  hoverFrac: null,
};

export const INITIAL_STATE: AppState = {
  page: null,
  trail: [],

  swapTab: "Swap",
  fromToken: "ETH",
  toToken: "SOL",
  amount: "2.500",
  openCell: null,

  poolQuery: {
    ptype: "All pools",
    sell: "Any",
    buy: "Any",
    fee: "Any",
    apr: "Any",
  },
  poolSort: "Best",
  poolPage: 0,

  detail: null,

  ...INITIAL_CREATE_POSITION_STATE,

  makerSort: "Virtual",
  filterPick: { 0: 0 },
  tvlMin: 0,

  picker: null,
  pQuery: "",
  pTag: "All",
  pNet: "All networks",

  xpStrat: null,
  maker: null,

  mkSel: null,
  mkSpan: "1M",
  mkTab: "Positions",
  mkAsset: null,
  mkTip: null,
  mkBar: null,
  mkLat: null,

  xpTab: "Trades",
  xpType: "All types",
  xpEnt: "All entities",
  xpStatus: "All status",
  xpPair: "All pairs",
  xpOpen: null,
};

/** Canvas-level knobs the design exposed as editor props. */
export type AppConfig = {
  landingPage: Page;
  showFaucet: boolean;
  showResolverRoute: boolean;
  slippage: number;
};

export const DEFAULT_CONFIG: AppConfig = {
  landingPage: "Home",
  showFaucet: true,
  showResolverRoute: true,
  slippage: 0.5,
};

export type AppApi = {
  state: AppState;
  config: AppConfig;
  page: Page;
  set: (patch: Partial<AppState>) => void;
  navTo: (page: Page) => void;
  go: (page: Page) => () => void;
  push: (view: Partial<AppState>, label: string) => void;
  pop: () => void;
  crumbs: (
    current: string,
  ) => { label: string; sep: string; fg: string; go: () => void }[];
};

export const AppCtx = createContext<AppApi | null>(null);

/** Everything on {@link AppApi} except the state itself. Chrome that only navigates reads this,
 *  so a pointer-driven state change does not re-render it. */
export type AppActions = Omit<AppApi, "state">;

export const AppActionsCtx = createContext<AppActions | null>(null);

export function useAppActions(): AppActions {
  const actions = useContext(AppActionsCtx);
  if (!actions)
    throw new Error("useAppActions must be used inside <AppProvider>");
  return actions;
}

export function useApp(): AppApi {
  const ctx = useContext(AppCtx);
  if (!ctx) throw new Error("useApp must be used inside <AppProvider>");
  return ctx;
}
