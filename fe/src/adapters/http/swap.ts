import type { OrderTerms } from "@solvent/sdk/orders";
import {
  createSwapClient,
  type SwapIntent as SdkSwapIntent,
  type SwapSubmissionOptions,
} from "@solvent/sdk/swap";
import {
  approveCompact,
  compactBalance,
  depositCompact,
  signCompactMandate,
} from "@solvent/sdk/cross-chain";
import {
  InputValidationError,
  MAX_UINT256,
  parseTokenAmount,
  validatedUint,
} from "@solvent/sdk/validation";
import { bytesToHex, toHex, type Address } from "viem";
import { chain } from "@/adapters/wallet/config";
import { minimumOutput } from "@/lib/swap";
import type { SwapInput, SwapPort } from "@/ports/swap";
import { toCrossChainQuote, toQuote } from "../mappers/quote";
import {
  baseApi,
  crossChainApi,
  crossChainOriginApi,
  solventApi,
} from "./client";

const ORDER_TTL_SECS = 600;
const COMPACT_TTL_SECS = 900;

function sameChainApi(chainId: number) {
  return chainId === chain.id ? solventApi : baseApi;
}

function requestId(): `0x${string}` {
  const bytes = new Uint8Array(32);
  globalThis.crypto.getRandomValues(bytes);
  return bytesToHex(bytes);
}

function nonce(): `0x${string}` {
  return requestId();
}

function directIntent(
  input: SwapInput,
  clients: Parameters<SwapPort["createIntent"]>[1],
) {
  const aggregate = input.quote.crossChain;
  if (!aggregate)
    throw new Error("The cross-chain quote is missing settlement terms");
  const request = {
    quote: aggregate,
    sponsor: input.swapper as Address,
    recipient: input.swapper as Address,
    order_nonce: nonce(),
    compact_nonce: nonce(),
    compact_expires_unix: aggregate.expires_at_unix + COMPACT_TTL_SECS,
  };
  let pending: Promise<{ tradeId: string; status: string }> | undefined;

  async function execute(options?: SwapSubmissionOptions) {
    options?.onStatus?.({ kind: "preparing" });
    const draft = await crossChainApi.draft(request);
    const compactId = BigInt(draft.order.compact_id);
    const amount = BigInt(draft.commitment.amount);
    const balance = await compactBalance(
      clients.publicClient,
      draft.compact,
      request.sponsor,
      compactId,
    );
    if (balance < amount) {
      const depositAmount = amount - balance;
      const onBroadcast = () => options?.onStatus?.({ kind: "confirming" });
      options?.onStatus?.({ kind: "approving", token: draft.commitment.token });
      await approveCompact(
        clients.publicClient,
        clients.walletClient,
        {
          compact: draft.compact,
          token: draft.commitment.token,
          amount: depositAmount,
          sponsor: request.sponsor,
        },
        { onBroadcast },
      );
      options?.onStatus?.({ kind: "submitting" });
      await depositCompact(
        clients.publicClient,
        clients.walletClient,
        {
          compact: draft.compact,
          token: draft.commitment.token,
          lockTag: draft.commitment.lock_tag,
          amount: depositAmount,
          sponsor: request.sponsor,
        },
        { onBroadcast },
      );
    }
    const terms = draft.commitment;
    options?.onStatus?.({ kind: "signing" });
    const signature = await signCompactMandate(
      clients.walletClient,
      draft.compact,
      draft.order.origin_chain_id,
      {
        arbiter: terms.arbiter,
        sponsor: terms.sponsor,
        nonce: BigInt(terms.nonce),
        expires: BigInt(terms.expires),
        lockTag: terms.lock_tag,
        token: terms.token,
        amount: BigInt(terms.amount),
        mandate: {
          orderId: terms.mandate.order_id,
          destinationChainId: BigInt(terms.mandate.destination_chain_id),
          destinationSettler: terms.mandate.destination_settler,
          fillProofVerifier: terms.mandate.fill_proof_verifier,
          outputToken: terms.mandate.output_token,
          minimumOutputAmount: BigInt(terms.mandate.minimum_output_amount),
          recipient: terms.mandate.recipient,
          fillDeadline: terms.mandate.fill_deadline,
          exclusiveFiller: terms.mandate.exclusive_filler,
          routeKind: terms.mandate.route_kind,
        },
      },
    );
    options?.onStatus?.({ kind: "submitting" });
    const order = await crossChainApi.submitDirect({
      draft: request,
      sponsor_signature: signature,
    });
    return { tradeId: order.order_id, status: order.state };
  }

  return {
    submit(options?: SwapSubmissionOptions) {
      pending ??= execute(options).finally(() => {
        pending = undefined;
      });
      return pending;
    },
  };
}

/** A deployment's own chain name, for a message that has to name it. */
function chainName(
  config: {
    chain_id: number;
    chains?: { chain_id: number; name: string }[];
  },
  fallback: string,
): string {
  return (
    config.chains?.find((c) => c.chain_id === config.chain_id)?.name ??
    config.chains?.[0]?.name ??
    fallback
  );
}

async function crossChainQuote({
  from,
  to,
  amount,
}: Parameters<SwapPort["quote"]>[0]) {
  const [originConfig, destinationConfig, originAssets, destinationAssets] =
    await Promise.all([
      crossChainOriginApi.config(),
      baseApi.config(),
      crossChainOriginApi.assets(),
      baseApi.assets(),
    ]);
  if (
    from.chainId !== originConfig.chain_id ||
    to.chainId !== destinationConfig.chain_id
  ) {
    throw new Error(
      `Cross-chain quoting is configured from ${chainName(originConfig, "the origin chain")} to ${chainName(destinationConfig, "the destination chain")}`,
    );
  }

  const originAsset = originAssets.items.find(
    (asset) => asset.symbol === from.symbol,
  );
  const destinationInput = destinationAssets.items.find(
    (asset) => asset.symbol === from.symbol,
  );
  if (!originAsset || !destinationInput) {
    throw new Error(`${from.symbol} has no cross-chain representation`);
  }

  const amountInRaw = parseTokenAmount(amount, from.decimals, "Swap amount");
  const destinationAmountIn = parseTokenAmount(
    amount,
    destinationInput.decimals,
    "Destination swap amount",
  );
  const aggregate = await crossChainApi.quote({
    request_id: requestId(),
    origin_chain_id: originConfig.chain_id,
    destination_chain_id: destinationConfig.chain_id,
    origin_token_in: originAsset.address as `0x${string}`,
    origin_token_out: originAsset.address as `0x${string}`,
    destination_token_in: destinationInput.address as `0x${string}`,
    destination_token_out: to.address,
    amount_in: toHex(amountInRaw),
    destination_amount_in: toHex(destinationAmountIn),
    deadline_unix: Math.floor(Date.now() / 1_000) + ORDER_TTL_SECS,
    route: "direct",
  });
  return toCrossChainQuote(aggregate, from, to, amountInRaw);
}

function orderTerms({
  from,
  to,
  amount,
  quote,
  swapper,
  slippagePct,
}: SwapInput): OrderTerms {
  if (!Number.isFinite(slippagePct)) {
    throw new InputValidationError(
      "slippage",
      "invalid_decimal",
      "Slippage must be a finite percentage",
    );
  }
  const tolerance = BigInt(Math.round(slippagePct * 100));
  if (tolerance < 0n || tolerance > 9_999n) {
    throw new InputValidationError(
      "slippage",
      "out_of_range",
      "Slippage must resolve to 0 through 9,999 basis points",
    );
  }
  const amountIn = parseTokenAmount(amount, from.decimals, "Swap amount");
  if (
    quote.tokenIn.toLowerCase() !== from.address.toLowerCase() ||
    quote.tokenOut.toLowerCase() !== to.address.toLowerCase() ||
    quote.amountInRaw !== amountIn
  ) {
    throw new InputValidationError(
      "quote",
      "out_of_range",
      "The quote does not match the current swap inputs",
    );
  }
  if (!Number.isFinite(quote.expiresAt) || quote.expiresAt <= Date.now()) {
    throw new InputValidationError(
      "quote",
      "expired",
      "The quote expired; request a fresh price",
    );
  }
  validatedUint(quote.amountOutRaw, 256, "quoted output", { positive: true });
  const minAmountOut = minimumOutput(quote.amountOutRaw, slippagePct);
  validatedUint(minAmountOut, 256, "minimum output", { positive: true });
  return {
    swapper,
    tokenIn: from.address,
    tokenOut: to.address,
    amountIn,
    minAmountOut,
    deadline: Math.floor(Date.now() / 1000) + ORDER_TTL_SECS,
  };
}

export const swapAdapter: SwapPort = {
  async quote({ from, to, amount }) {
    if (from.chainId !== to.chainId) {
      return crossChainQuote({ from, to, amount });
    }
    const amountInRaw = parseTokenAmount(amount, from.decimals, "Swap amount");
    const priced = await sameChainApi(from.chainId).quote({
      token_in: from.address,
      token_out: to.address,
      amount_in: amountInRaw.toString(),
    });
    const amountOutRaw = BigInt(priced.amount_out.raw);
    if (amountOutRaw > MAX_UINT256 || amountOutRaw <= 0n) {
      throw new Error("The resolver returned an invalid output amount");
    }
    return toQuote(priced, to.decimals, {
      tokenIn: from.address,
      tokenOut: to.address,
      amountInRaw,
    });
  },

  createIntent(input, clients) {
    if (input.from.chainId !== input.to.chainId) {
      return directIntent(input, clients);
    }
    const swaps = createSwapClient({
      api: sameChainApi(input.from.chainId),
      ...clients,
    });
    let intent: SdkSwapIntent | undefined;
    return {
      async submit(options?: SwapSubmissionOptions) {
        // Validate display units inside the async operation so failures reach mutation state.
        intent ??= swaps.createIntent(orderTerms(input));
        const result = await intent.submit(options);
        return { tradeId: result.trade_id, status: result.status };
      },
    };
  },
};
