import type { OrderTerms } from "@solvent/sdk/orders";
import { createSwapClient, type SwapIntent } from "@solvent/sdk/swap";
import {
  InputValidationError,
  MAX_UINT256,
  parseTokenAmount,
  validatedUint,
} from "@solvent/sdk/validation";
import type { SwapInput, SwapPort } from "@/ports/swap";
import { toQuote } from "../mappers/quote";
import { solventApi } from "./client";

const ORDER_TTL_SECS = 600;
const BPS = 10_000n;

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
    const amountInRaw = parseTokenAmount(amount, from.decimals, "Swap amount");
    const priced = await solventApi.quote({
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
    const swaps = createSwapClient({ api: solventApi, ...clients });
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
