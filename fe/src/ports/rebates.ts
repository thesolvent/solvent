import type { WalletClients } from "@solvent/sdk/swap";
import type {
  ExecutedRebate,
  RebateRecord,
  RebateStatus,
} from "@/data/rebates";
import type { RecordPage } from "@/data/explorer";

export interface RebateFilter {
  maker?: string;
  status?: RebateStatus;
}

/** One server-authorized execution, retained while a wallet submission is retried. */
export interface RebateIntent {
  submit(): Promise<ExecutedRebate>;
}

export interface RebatesPort {
  list(
    filter: RebateFilter,
    cursor?: string,
  ): Promise<RecordPage<RebateRecord>>;
  createIntent(
    input: { rebateId: string; executor: string },
    clients: WalletClients,
  ): RebateIntent;
}
