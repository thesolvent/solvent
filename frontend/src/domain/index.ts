// The frontend's domain vocabulary: the wire types the app reads/writes, owned by @solvent/sdk.
// Views and application code import data types from here (never from the SDK directly), so the
// SDK stays a single swappable seam. FE-only derived types/formatters also live in this layer.
//
// Sourced from the SDK's `/client` subpath (not the barrel) so P1 stays free of the construction/
// positions modules, whose @1inch deps pull Node built-ins that need a browser polyfill (added
// when the write paths first import them).
export type { Address, Hex } from "viem";
export type {
  AppConfig,
  Asset,
  Pool,
  PoolDetail,
  PoolDepth,
  Stats,
  QuoteRequest,
  QuoteResponse,
  SwapRequest,
  SwapResponse,
  Trade,
  ActivityEvent,
  MakerSummary,
  MakerDashboard,
  MakerTrade,
  InventoryRow,
  Position,
  TokenBalance,
  PairInfo,
  PreviewRequest,
  PreviewResponse,
  List,
} from "@solvent/sdk/client";
