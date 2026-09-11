import type { WalletClients } from "@solvent/sdk/swap";
import {
  type InfiniteData,
  skipToken,
  useInfiniteQuery,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { useLayoutEffect, useState } from "react";
import { useAccount, useClient, useConnectorClient } from "wagmi";
import type { RecordPage } from "@/data/explorer";
import type { ExecutedRebate, RebateRecord } from "@/data/rebates";
import type { RebateFilter, RebateIntent, RebatesPort } from "@/ports/rebates";
import { LIVE_QUERY_OPTIONS } from "./live";
import { useServices } from "./context";

interface Submission {
  key: string;
  rebateId: string;
  intent: RebateIntent | undefined;
}

type RebatePages = InfiniteData<RecordPage<RebateRecord>, unknown>;

interface ActiveRebateSnapshot {
  source?: RebatePages;
  pages: RebateRecord[][];
  availableIds: ReadonlySet<string>;
  missedPolls: ReadonlyMap<string, number>;
}

const EMPTY_ACTIVE_REBATES: ActiveRebateSnapshot = {
  pages: [],
  availableIds: new Set(),
  missedPolls: new Map(),
};

function currentPages(data: RebatePages): RebateRecord[][] {
  return data.pages.map(({ items }) =>
    items.filter(({ status }) => status === "ready"),
  );
}

function snapshot(data: RebatePages): ActiveRebateSnapshot {
  const pages = currentPages(data);
  const ids = pages.flatMap((page) => page.map(({ id }) => id));
  return {
    source: data,
    pages,
    availableIds: new Set(ids),
    missedPolls: new Map(ids.map((id) => [id, 0])),
  };
}

function pageRows(
  rows: RebateRecord[],
  previous: RebateRecord[][],
  current: RebateRecord[][],
): RebateRecord[][] {
  if (rows.length === 0) return current.length ? current : [];
  const pageSize = Math.max(
    1,
    ...previous.map((page) => page.length),
    ...current.map((page) => page.length),
  );
  return Array.from({ length: Math.ceil(rows.length / pageSize) }, (_, page) =>
    rows.slice(page * pageSize, (page + 1) * pageSize),
  );
}

function reconcileActiveRebates(
  previous: ActiveRebateSnapshot,
  data: RebatePages,
): ActiveRebateSnapshot {
  if (!previous.source) return snapshot(data);
  const current = currentPages(data);
  const currentById = new Map(
    current.flatMap((page) =>
      page.map((rebate) => [rebate.id, rebate] as const),
    ),
  );
  const availableIds = new Set(currentById.keys());
  const missedPolls = new Map<string, number>();
  const rows: RebateRecord[] = [];

  for (const previousRebate of previous.pages.flat()) {
    const currentRebate = currentById.get(previousRebate.id);
    if (currentRebate) {
      rows.push(currentRebate);
      currentById.delete(previousRebate.id);
      missedPolls.set(previousRebate.id, 0);
      continue;
    }
    const misses = (previous.missedPolls.get(previousRebate.id) ?? 0) + 1;
    if (misses <= 1) {
      rows.push(previousRebate);
      missedPolls.set(previousRebate.id, misses);
    }
  }

  for (const rebate of current.flat()) {
    if (!currentById.has(rebate.id)) continue;
    rows.push(rebate);
    currentById.delete(rebate.id);
    missedPolls.set(rebate.id, 0);
  }

  return {
    source: data,
    pages: pageRows(rows, previous.pages, current),
    availableIds,
    missedPolls,
  };
}

/** Keep a published opportunity stationary while the next worker pass renews its authorization. */
export function useActiveRebatePages(
  data: RebatePages | undefined,
  dataUpdatedAt: number,
) {
  const [stable, setStable] = useState(EMPTY_ACTIVE_REBATES);

  useLayoutEffect(() => {
    if (data) setStable((previous) => reconcileActiveRebates(previous, data));
  }, [data, dataUpdatedAt]);

  return !stable.source && data ? snapshot(data) : stable;
}

function createIntent(
  rebates: RebatesPort,
  rebateId: string,
  executor: string | undefined,
  { publicClient, walletClient }: Partial<WalletClients>,
): RebateIntent | undefined {
  if (!executor || !publicClient || !walletClient) return undefined;
  return rebates.createIntent(
    { rebateId, executor },
    { publicClient, walletClient },
  );
}

async function submit(attempt: Submission): Promise<ExecutedRebate> {
  if (!attempt.intent)
    throw new Error("Connect a wallet to execute this rebate");
  return attempt.intent.submit();
}

function markExecuted(
  data: RebatePages | undefined,
  result: ExecutedRebate,
): RebatePages | undefined {
  if (!data) return data;
  return {
    ...data,
    pages: data.pages.map((page) => ({
      ...page,
      items: page.items.map((rebate) =>
        rebate.id.toLowerCase() === result.rebateId.toLowerCase()
          ? {
              ...rebate,
              status: "executed",
              deadlineBlock: null,
              executedAt: Math.floor(Date.now() / 1_000),
              transactionHash: result.transactionHash,
            }
          : rebate,
      ),
    })),
  };
}

function executionProblem(error: Error | null): string | undefined {
  if (!error) return undefined;
  if (["InputValidationError", "RebateUnavailableError"].includes(error.name))
    return error.message;
  if (error.message === "Insufficient token balance")
    return "Not enough input tokens in this wallet";
  return "Could not execute this rebate";
}

function submissionKey(
  rebateId: string,
  executor: string | undefined,
  chainId: number | undefined,
): string {
  return JSON.stringify([rebateId, executor, chainId]);
}

export function useRebates(filter: RebateFilter | undefined) {
  const { rebates } = useServices();
  return useInfiniteQuery({
    ...LIVE_QUERY_OPTIONS,
    queryKey: ["rebates", filter],
    initialPageParam: undefined as string | undefined,
    queryFn: filter
      ? ({ pageParam }) => rebates.list(filter, pageParam)
      : skipToken,
    getNextPageParam: (page) => page.nextCursor,
  });
}

export function useExecuteRebate() {
  const { rebates } = useServices();
  const queryClient = useQueryClient();
  const { address, chainId } = useAccount();
  const publicClient = useClient({ chainId });
  const { data: walletClient } = useConnectorClient();
  const mutation = useMutation({
    mutationFn: submit,
    onSuccess: (result) =>
      queryClient.setQueriesData<RebatePages>(
        { queryKey: ["rebates"] },
        (data) => markExecuted(data, result),
      ),
    onError: () => queryClient.invalidateQueries({ queryKey: ["rebates"] }),
  });

  function execute(rebateId: string) {
    const key = submissionKey(rebateId, address, chainId);
    const previous = mutation.variables;
    mutation.mutate(
      previous?.key === key && previous.intent
        ? previous
        : {
            key,
            rebateId,
            intent: createIntent(rebates, rebateId, address, {
              publicClient,
              walletClient,
            }),
          },
    );
  }

  return {
    execute,
    pendingId: mutation.isPending ? mutation.variables?.rebateId : undefined,
    completedId: mutation.isSuccess ? mutation.data.rebateId : undefined,
    failedId: mutation.isError ? mutation.variables?.rebateId : undefined,
    problem: executionProblem(mutation.error),
  };
}
