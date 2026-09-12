import type {
  ActivityRecord,
  ExplorerStats,
  RecordPage,
  TradeRecord,
} from "@/data/explorer";

export interface TradeFilter {
  strategy_hash?: string;
  status?: string;
  base?: string;
  quote?: string;
  chainId?: number;
  /** The swapper whose trades to list. */
  taker?: string;
}

export interface ActivityFilter {
  kind?: string;
  entity?: string;
}

export interface ExplorerPort {
  trades(
    filter: TradeFilter,
    cursor?: string,
  ): Promise<RecordPage<TradeRecord>>;
  trade(id: string): Promise<TradeRecord>;
  /** One person's cross-chain orders, which live in the proxy's store rather than either chain's. */
  crossChainOrders(taker: string): Promise<TradeRecord[]>;
  activity(
    filter: ActivityFilter,
    cursor?: string,
  ): Promise<RecordPage<ActivityRecord>>;
  stats(): Promise<ExplorerStats>;
}
