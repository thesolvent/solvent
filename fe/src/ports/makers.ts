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

export interface MakersPort {
  list(): Promise<MakerSummary[]>;
  dashboard(address: string, period: MakerPeriod): Promise<MakerDashboard>;
  inventory(address: string, period: MakerPeriod): Promise<InventoryAsset[]>;
  positions(address: string, period: MakerPeriod): Promise<Position[]>;
  position(hash: string): Promise<Position>;
  history(hash: string): Promise<PositionHistory>;
  settlements(
    address: string,
    period: MakerPeriod,
    cursor?: string,
  ): Promise<RecordPage<MakerSettlement>>;
}
