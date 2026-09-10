import type { OrderTerms } from "@solvent/sdk/orders";
import { createSwapClient, type SwapIntent } from "@solvent/sdk/swap";
import {
  InputValidationError,
  MAX_UINT256,
  parseTokenAmount,
  validatedUint,
} from "@solvent/sdk/validation";
import { bytesToHex, toHex } from "viem";
import { chain } from "@/adapters/wallet/config";
import type { SwapInput, SwapPort } from "@/ports/swap";
import { toCrossChainQuote, toQuote } from "../mappers/quote";
import {
  baseApi,
  crossChainApi,
  crossChainOriginApi,
  solventApi,
} from "./client";

const ORDER_TTL_SECS = 600;
const BPS = 10_000n;

function sameChainApi(chainId: number) {
  return chainId === chain.id ? solventApi : baseApi;
}

function requestId(): `0x${string}` {
  const bytes = new Uint8Array(32);
  globalThis.crypto.getRandomValues(bytes);
  return bytesToHex(bytes);
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
      `Cross-chain quoting is configured from ${originConfig.networks[0] ?? "the origin chain"} to ${destinationConfig.networks[0] ?? "the destination chain"}`,
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
  const minAmountOut = (quote.amountOutRaw * (BPS - tolerance)) / BPS;
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
    const swaps = createSwapClient({
      api: sameChainApi(input.from.chainId),
      ...clients,
    });
    let intent: SwapIntent | undefined;
    return {
      async submit() {
        // Validate display units inside the async operation so failures reach mutation state.
        intent ??= swaps.createIntent(orderTerms(input));
        const result = await intent.submit();
        return { tradeId: result.trade_id, status: result.status };
      },
    };
  },
};
