import type { WalletClients } from "@solvent/sdk/swap";
import { useMutation } from "@tanstack/react-query";
import { useAccount, useClient, useConnectorClient } from "wagmi";
import type { Asset, Quote, SubmittedSwap } from "@/data";
import { isSwapDeclined, submissionProblem } from "@/lib/swap";
import type { SwapIntent, SwapPort } from "@/ports/swap";
import { useServices } from "./context";

export interface SwapForm {
  from: Asset | undefined;
  to: Asset | undefined;
  amount: string;
  quote: Quote | undefined;
  slippagePct: number;
}

interface Submission {
  key: string;
  intent: SwapIntent | undefined;
}

export interface SwapSubmission {
  send: () => void;
  submitting: boolean;
  result: SubmittedSwap | undefined;
  problem: string | undefined;
}

function submissionKey(
  form: SwapForm,
  address?: string,
  chainId?: number,
): string {
  return JSON.stringify([
    form.from?.address,
    form.to?.address,
    form.amount,
    form.slippagePct,
    address,
    chainId,
  ]);
}

function createIntent(
  swap: SwapPort,
  form: SwapForm,
  swapper: string | undefined,
  { publicClient, walletClient }: Partial<WalletClients>,
): SwapIntent | undefined {
  const { from, to, quote } = form;
  if (!from || !to || !quote || !swapper || !publicClient || !walletClient)
    return undefined;
  return swap.createIntent(
    { ...form, from, to, quote, swapper },
    { publicClient, walletClient },
  );
}

async function submitCurrent(
  attempt: Submission,
  currentKey: string,
): Promise<SubmittedSwap> {
  // A paused mutation may resume after the form or connected wallet has changed.
  if (attempt.key !== currentKey) throw new Error("Swap inputs changed");
  if (!attempt.intent) throw new Error("Connect a wallet to swap");
  return attempt.intent.submit();
}

/** React owns mutation state; the intent owns payment authorization and retry identity. */
export function useSubmitSwap(
  form: SwapForm,
  onSubmitted?: (result: SubmittedSwap) => void,
): SwapSubmission {
  const { swap } = useServices();
  const { address, chainId } = useAccount();
  const publicClient = useClient({ chainId });
  const { data: walletClient } = useConnectorClient();
  const key = submissionKey(form, address, chainId);
  const mutation = useMutation({
    mutationFn: (attempt: Submission) => submitCurrent(attempt, key),
  });

  function send() {
    const previous = mutation.variables;
    const retry =
      previous?.key === key &&
      previous.intent &&
      !isSwapDeclined(mutation.error);
    mutation.mutate(
      retry
        ? previous
        : {
            key,
            intent: createIntent(swap, form, address, {
              publicClient,
              walletClient,
            }),
          },
      // Per-call callbacks stop observing when the page unmounts.
      { onSuccess: onSubmitted },
    );
  }

  const current = mutation.variables?.key === key;
  return {
    send,
    submitting: mutation.isPending,
    result: current ? mutation.data : undefined,
    problem: current ? submissionProblem(mutation.error) : undefined,
  };
}
