import type { OrderTerms } from "@solvent/sdk/orders";
import { createSwapClient, type SwapIntent } from "@solvent/sdk/swap";
import { parseUnits } from "viem";
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
  if (!Number.isFinite(slippagePct) || slippagePct < 0 || slippagePct >= 100) {
    throw new Error("Slippage must be between 0 and 100 percent");
  }
  const tolerance = BigInt(Math.round(slippagePct * 100));
  return {
    swapper,
    tokenIn: from.address,
    tokenOut: to.address,
    amountIn: parseUnits(amount, from.decimals),
    minAmountOut: (quote.amountOutRaw * (BPS - tolerance)) / BPS,
    deadline: Math.floor(Date.now() / 1000) + ORDER_TTL_SECS,
  };
}

export const swapAdapter: SwapPort = {
  async quote({ from, to, amount }) {
    const priced = await solventApi.quote({
      token_in: from.address,
      token_out: to.address,
      amount_in: parseUnits(amount, from.decimals).toString(),
    });
    return toQuote(priced, to.decimals);
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
