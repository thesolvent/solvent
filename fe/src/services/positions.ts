import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useAccount, useClient, useConnectorClient } from "wagmi";

import type { WalletClients } from "@solvent/sdk/swap";
import type {
  CreatedPosition,
  CreatePair,
  PairPricePoint,
  PositionForm,
  PositionIntent,
  PositionsPort,
  PriceHistoryPeriod,
} from "@/ports/positions";
import type { CreateSpan } from "@/state";
import { useServices } from "./context";

interface Submission {
  key: string;
  intent: PositionIntent | undefined;
}

export interface PositionSubmission {
  send: () => void;
  submitting: boolean;
  result: CreatedPosition | undefined;
  problem: string | undefined;
}

export function useCreatePairs() {
  const { positions } = useServices();
  const { address } = useAccount();
  return useQuery({
    queryKey: ["create-position", "pairs", address?.toLowerCase()],
    queryFn: () => positions.pairs(address),
  });
}

const HISTORY_PERIOD: Record<CreateSpan, PriceHistoryPeriod> = {
  "7d": "7d",
  "3m": "3m",
  All: "all",
};

export function usePairPriceHistory(
  pair: CreatePair | undefined,
  span: CreateSpan,
) {
  const { positions } = useServices();
  const period = HISTORY_PERIOD[span];
  return useQuery<PairPricePoint[]>({
    queryKey: [
      "create-position",
      "history",
      pair?.base.address.toLowerCase(),
      pair?.quote.address.toLowerCase(),
      period,
    ],
    queryFn: () =>
      pair ? positions.history(pair, period) : Promise.resolve([]),
    enabled: pair !== undefined,
    staleTime: 5 * 60 * 1_000,
  });
}

function submissionKey(
  form: PositionForm | undefined,
  address?: string,
  chainId?: number,
): string {
  return JSON.stringify([form, address, chainId], (_key, value) =>
    typeof value === "bigint" ? `${value}n` : value,
  );
}

function createIntent(
  positions: PositionsPort,
  form: PositionForm | undefined,
  maker: string | undefined,
  { publicClient, walletClient }: Partial<WalletClients>,
): PositionIntent | undefined {
  if (!form || !maker || !publicClient || !walletClient) return undefined;
  return positions.createIntent(
    { ...form, maker: maker as `0x${string}` },
    { publicClient, walletClient },
  );
}

async function submitCurrent(
  attempt: Submission,
  currentKey: string,
): Promise<CreatedPosition> {
  if (attempt.key !== currentKey) throw new Error("Position inputs changed");
  if (!attempt.intent) throw new Error("Connect a wallet to create a position");
  return attempt.intent.submit();
}

/** React owns request state; the SDK intent owns strategy identity and transaction retries. */
export function useCreatePosition(
  form: PositionForm | undefined,
  onCreated?: (result: CreatedPosition) => void,
): PositionSubmission {
  const { positions } = useServices();
  const queryClient = useQueryClient();
  const { address, chainId } = useAccount();
  const publicClient = useClient({ chainId });
  const { data: walletClient } = useConnectorClient();
  const key = submissionKey(form, address, chainId);
  const mutation = useMutation({
    mutationFn: (attempt: Submission) => submitCurrent(attempt, key),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({
          queryKey: ["create-position", "pairs"],
          refetchType: "none",
        }),
        queryClient.invalidateQueries({
          queryKey: ["makers"],
          refetchType: "none",
        }),
        queryClient.invalidateQueries({
          queryKey: ["pools"],
          refetchType: "none",
        }),
      ]);
    },
  });

  function send() {
    const previous = mutation.variables;
    mutation.mutate(
      previous?.key === key && previous.intent
        ? previous
        : {
            key,
            intent: createIntent(positions, form, address, {
              publicClient,
              walletClient,
            }),
          },
      { onSuccess: onCreated },
    );
  }

  const current = mutation.variables?.key === key;
  return {
    send,
    submitting: mutation.isPending,
    result: current ? mutation.data : undefined,
    problem:
      current && mutation.error instanceof Error
        ? mutation.error.message
        : undefined,
  };
}
