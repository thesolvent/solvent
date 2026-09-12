import {
  createRebateClient,
  type RebateSubmissionOptions,
} from "@solvent/sdk/rebates";
import type { Address } from "viem";
import type { RebatesPort } from "@/ports/rebates";
import { toRebate } from "../mappers/rebate";
import { solventApi } from "./client";

const PAGE_SIZE = 10;

export const rebatesAdapter: RebatesPort = {
  async list(filter, cursor) {
    const page = await solventApi.rebates({
      ...filter,
      cursor,
      limit: PAGE_SIZE,
    });
    return { items: page.items.map(toRebate), nextCursor: page.next_cursor };
  },

  createIntent({ rebateId, executor }, clients) {
    const intent = createRebateClient({
      api: solventApi,
      ...clients,
    }).createIntent({
      rebateId,
      executor: executor as Address,
    });
    return {
      async submit(options?: RebateSubmissionOptions) {
        const result = await intent.submit(options);
        return {
          rebateId: result.rebateId,
          transactionHash: result.transactionHash,
        };
      },
    };
  },
};
