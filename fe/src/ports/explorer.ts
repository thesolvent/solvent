import type {
  ActivityRecord,
  ObservedOrder,
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

export interface ExplorerPort {
  /** Every order the feed showed us, newest first. */
  orders(limit?: number): Promise<ObservedOrder[]>;
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
