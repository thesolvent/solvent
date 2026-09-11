import type { TradeRecord } from "@/data/explorer";

const SAME_CHAIN_STAGES = [
  "Created",
  "Quoted",
  "Reserved",
  "Simulated",
  "Submitted",
  "Confirmed",
];
const CROSS_CHAIN_STAGES = [
  "Quoted",
  "Destination fill",
  "Proof relay",
  "Origin claim",
  "Repayment",
  "Complete",
];

type StageState =
  "recorded" | "not recorded" | "not reached" | "awaiting" | "pending";

export function isTerminalTrade(status: string): boolean {
  return ["confirmed", "declined", "failed"].includes(status);
}

/** Status identifies progress; only lifecycle events establish that a stage was recorded. */
export function tradeLifecycle(trade: TradeRecord) {
  const stages =
    trade.flow === "cross-chain" ? CROSS_CHAIN_STAGES : SAME_CHAIN_STAGES;
  const terminal = isTerminalTrade(trade.status);
  const recorded = new Map(
    trade.lifecycle.map((stage) => [stage.status, stage.at]),
  );
  const latestStage = stages.reduce(
    (latest, label, index) =>
      recorded.has(label.toLowerCase()) || label.toLowerCase() === trade.status
        ? index
        : latest,
    -1,
  );
  const steps = stages.map((label, index) => {
    const at = recorded.get(label.toLowerCase());
    let state: StageState;
    if (at !== undefined) state = "recorded";
    else if (trade.status === "confirmed" || index <= latestStage)
      state = "not recorded";
    else if (terminal) state = "not reached";
    else state = index === latestStage + 1 ? "awaiting" : "pending";

    return {
      label,
      state,
      elapsedSeconds:
        typeof at !== "number" ? null : Math.max(0, at - trade.createdAt),
    };
  });
  return {
    steps,
    recordedCount: steps.filter((step) => step.state === "recorded").length,
    elapsedSeconds:
      trade.settledAt === null
        ? null
        : Math.max(0, trade.settledAt - trade.createdAt),
    phases: [
      trade.flow === "cross-chain"
        ? {
            name: "Destination execution",
            complete: recorded.has("destination fill"),
          }
        : { name: "Quote & reserve", complete: recorded.has("reserved") },
      trade.flow === "cross-chain"
        ? { name: "Origin settlement", complete: recorded.has("complete") }
        : { name: "Simulate & settle", complete: recorded.has("confirmed") },
    ],
  };
}
