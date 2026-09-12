import { formatUnits } from "viem";
import type { Asset } from "@/data";
import type { RebateRecord } from "@/data/rebates";
import {
  count,
  percent,
  tokenWithSymbol,
  truncateAddress,
  truncateHash,
} from "@/lib/format";
import { relativeTime } from "./explorer";

function amount(value: bigint, asset: Asset | undefined): string {
  // Without decimals the base-unit integer is the only honest thing to print.
  if (!asset) return `${count(Number(value))} raw`;
  return tokenWithSymbol(formatUnits(value, asset.decimals), asset.symbol);
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
    pair: `${tokenIn?.symbol ?? truncateAddress(rebate.tokenIn)}/${tokenOut?.symbol ?? truncateAddress(rebate.tokenOut)}`,
    strategy: `strategy ${truncateHash(rebate.strategyHash)}`,
    deposit: amount(rebate.amountIn + rebate.makerRebate, tokenIn),
    output: amount(rebate.amountOut, tokenOut),
    makerRebate: amount(rebate.makerRebate, tokenIn),
    executorProfit: amount(rebate.executorProfit, tokenIn),
    deviation: `${percent(rebate.deviationBps / 100)} moved`,
    status: rebate.status,
    deadlineBlock: rebate.deadlineBlock,
    statusDetail:
      rebate.status === "executed"
        ? `${truncateHash(rebate.transactionHash)} · ${relativeTime(at)}`
        : `${rebate.deadlineBlock == null ? "deadline pending" : `before blk ${count(rebate.deadlineBlock)}`} · ${relativeTime(at)}`,
  };
}
