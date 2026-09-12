import { formatUnits } from "viem";
import type { Asset } from "@/data";
import type { RebateRecord } from "@/data/rebates";
import { relativeTime, shortHash } from "./explorer";

function amount(value: bigint, asset: Asset | undefined): string {
  if (!asset) return `${value.toLocaleString("en-US")} raw`;
  const display = Number(formatUnits(value, asset.decimals)).toLocaleString(
    "en-US",
    { maximumSignificantDigits: 8 },
  );
  return `${display} ${asset.symbol}`;
}

export function rebateRow(rebate: RebateRecord, assets: Asset[]) {
  const byAddress = new Map(
    assets.map((asset) => [asset.address.toLowerCase(), asset]),
  );
  const tokenIn = byAddress.get(rebate.tokenIn.toLowerCase());
  const tokenOut = byAddress.get(rebate.tokenOut.toLowerCase());
  const at = rebate.executedAt ?? rebate.publishedAt;
  return {
    id: rebate.id,
    originTradeId: rebate.originTradeId,
    pair: `${tokenIn?.symbol ?? shortHash(rebate.tokenIn)}/${tokenOut?.symbol ?? shortHash(rebate.tokenOut)}`,
    inputAsset: tokenIn,
    outputAsset: tokenOut,
    strategy: `strategy ${shortHash(rebate.strategyHash)}`,
    deposit: amount(rebate.amountIn + rebate.makerRebate, tokenIn),
    output: amount(rebate.amountOut, tokenOut),
    makerRebate: amount(rebate.makerRebate, tokenIn),
    executorProfit: amount(rebate.executorProfit, tokenIn),
    deviation: `${(rebate.deviationBps / 100).toFixed(2)}% moved`,
    status: rebate.status,
    deadlineBlock: rebate.deadlineBlock,
    statusDetail:
      rebate.status === "executed"
        ? `${shortHash(rebate.transactionHash)} · ${relativeTime(at)}`
        : `${rebate.deadlineBlock == null ? "deadline pending" : `before blk ${rebate.deadlineBlock.toLocaleString("en-US")}`} · ${relativeTime(at)}`,
  };
}
