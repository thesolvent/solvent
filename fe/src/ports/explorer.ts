import type {
  ActivityRecord,
  ObservedOrderPage,
  ExplorerStats,
  RecordPage,
  TradeRecord,
} from "@/data/explorer";

export interface TradeFilter {
  strategy_hash?: string;
  status?: string;
  base?: string;
  quote?: string;
}

export interface ActivityFilter {
  kind?: string;
  entity?: string;
}

export interface OrderFeedFilter {
  source?: string;
  tokenIn?: string;
  tokenOut?: string;
  state?: string;
}

export interface ExplorerPort {
  /** One page of every order the feed showed us, newest first, narrowed by the filter fields. */
  orders(
    query?: { limit?: number; offset?: number } & OrderFeedFilter,
  ): Promise<ObservedOrderPage>;
  trades(
    filter: TradeFilter,
    cursor?: string,
  ): Promise<RecordPage<TradeRecord>>;
  trade(id: string): Promise<TradeRecord>;
  activity(
    filter: ActivityFilter,
    cursor?: string,
  ): Promise<RecordPage<ActivityRecord>>;
  stats(): Promise<ExplorerStats>;
}
