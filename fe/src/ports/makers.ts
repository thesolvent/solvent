import type { DepthCurve, PairRef } from "@/data";
import type { RecordPage } from "@/data/explorer";
import type {
  InventoryAsset,
  MakerDashboard,
  MakerPeriod,
  MakerSettlement,
  MakerSummary,
  Position,
  PositionHistory,
} from "@/data/makers";

export type StrategyReadSource = "direct";

export interface MakersPort {
  list(): Promise<MakerSummary[]>;
  dashboard(address: string, period: MakerPeriod): Promise<MakerDashboard>;
  inventory(address: string, period: MakerPeriod): Promise<InventoryAsset[]>;
  positions(address: string, period: MakerPeriod): Promise<Position[]>;
  position(
    hash: string,
    chainId?: number,
    source?: StrategyReadSource,
  ): Promise<Position>;
  depth(
    hash: string,
    pair: PairRef,
    chainId?: number,
    source?: StrategyReadSource,
  ): Promise<DepthCurve>;
  history(
    hash: string,
    chainId?: number,
    source?: StrategyReadSource,
  ): Promise<PositionHistory>;
  settlements(
    address: string,
    period: MakerPeriod,
    cursor?: string,
  ): Promise<RecordPage<MakerSettlement>>;
}
