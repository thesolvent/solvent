import type { Position, PositionHistory } from "@/data/makers";
import type { TradeRecord } from "@/data/explorer";
import { balanceText, compactAddress, percent, usd } from "./makers";
import { relativeTime, tokenText } from "./explorer";

export function rangeDescription(position: Position | undefined): string {
  if (!position) return "Range information is unavailable.";
  const [base] = position.pair.split(/\s*\/\s*/);
  const units = `${position.quoteSymbol} per ${base}`;
  switch (position.rangeKind) {
    case "full":
      return "Full-range constant product: no configured lower or upper price limit. Executable depth is limited by the position’s reserves and available wallet balance.";
    case "peg":
      return `Peg-relative range: ${position.range}. This describes the concentration around the strategy’s central peg in ${units}; it is not a hard price cutoff. The depth curve includes fees and the available balance.`;
    default:
      return `Configured lower and upper prices: ${position.range}, in ${units}. These bound the concentrated-liquidity curve; they are not dates or slippage settings. Available balances can limit execution before a bound is reached.`;
  }
}

function cell(
  label: string,
  value: string,
  tag: string,
  tone: "ok" | "warn" | "n" = "n",
) {
  return {
    label,
    value,
    tag,
    tagBg:
      tone === "ok"
        ? "var(--lime-wash-soft)"
        : tone === "warn"
          ? "var(--warn-bg)"
          : "var(--line-faint)",
    tagFg:
      tone === "ok"
        ? "var(--green-darkest)"
        : tone === "warn"
          ? "var(--warn-ink)"
          : "var(--text-mid)",
  };
}

export function strategyDetail(
  position: Position | undefined,
  history: PositionHistory | undefined,
  trades: TradeRecord[],
) {
  const docked = position?.state === "docked";
  const full = position?.rangeKind === "full";
  return {
    title: position ? `${position.pair} · ${position.curve}` : "Strategy",
    state: position?.state ?? "—",
    stBg: docked ? "var(--line-faint)" : "var(--lime-wash-soft)",
    stFg: docked ? "var(--text-mid)" : "var(--green-darkest)",
    maker: compactAddress(position?.maker ?? ""),
    pool: position?.pair ?? "—",
    since:
      history?.createdBlock == null
        ? "—"
        : `since blk ${history.createdBlock.toLocaleString("en-US")}`,
    shape: [
      cell(
        "Virtual balance",
        balanceText(position?.committed ?? []),
        "committed",
      ),
      cell(
        "Actual / pullable",
        balanceText(position?.actual ?? []),
        position
          ? position.backed
            ? "fully backed ✓"
            : (position.shortfall ?? "partially backed")
          : "—",
        position ? (position.backed ? "ok" : "warn") : "n",
      ),
      cell("Fee", position ? `${position.feeBps} bps` : "—", "input fee"),
      cell(
        "Range",
        position?.range ?? "—",
        full
          ? "constant product"
          : position?.curve === "Pegged"
            ? "peg-relative bounds"
            : "bounded band",
      ),
    ].map((value, i) => ({ ...value, sep: i ? "var(--line)" : "transparent" })),
    active: [
      {
        label: "Fills (7d)",
        value: position?.fills7d?.toLocaleString("en-US") ?? "—",
        sub: docked ? "docked" : "pulls",
      },
      {
        label: "Volume (7d)",
        value: usd(position?.volume7dUsd),
        sub: "routed",
      },
      {
        label: "Quote uptime",
        value: percent(position?.quoteUptimePct),
        sub: docked ? "not quoting" : "7d",
      },
      {
        label: "Last fill",
        value: position?.lastFillAt ? relativeTime(position.lastFillAt) : "—",
        sub: "",
      },
    ].map((value, i) => ({ ...value, sep: i ? "var(--line)" : "transparent" })),
    fills: trades.map((trade) => ({
      id: trade.id,
      hash: trade.txHash ? compactAddress(trade.txHash) : "—",
      flow: `${tokenText(trade.input)} → ${tokenText(trade.output)}`,
      blk: trade.blockNumber?.toLocaleString("en-US") ?? "—",
      trade: `trade ${trade.id.slice(-6)}`,
    })),
    noFills: trades.length === 0,
  };
}
