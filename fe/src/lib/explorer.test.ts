import { describe, expect, it } from "vitest";
import detail from "@/data/fixtures/trade-detail.json";
import { toTrade } from "@/adapters/mappers/explorer";
import { GLOSSARY } from "./glossary";
import { tradeLifecycle } from "./trade-lifecycle";
import { explorerStats, tradeDetail, tradeRow } from "./explorer";

/** A trade that stopped at `quoted`, the shape the failure states are read from. */
function declined(status: "declined" | "failed" = "declined") {
  return toTrade({
    ...detail,
    status,
    tx_hash: undefined,
    block_number: undefined,
    legs: [],
    lifecycle: [
      { status: "created", at: 100 },
      { status: "quoted", at: 101 },
    ],
    created_at: 100,
    settled_at: undefined,
  });
}

describe("Explorer presentation", () => {
  it("awaits the next stage after the latest known progress without backfilling gaps", () => {
    const view = tradeLifecycle(
      toTrade({
        ...detail,
        status: "submitted",
        settled_at: undefined,
        lifecycle: [detail.lifecycle[0]],
      }),
    );
    expect(view.recordedCount).toBe(1);
    expect(
      view.steps
        .filter((step) => step.state === "awaiting")
        .map((step) => step.label),
    ).toEqual(["Confirmed"]);
    expect(view.steps[1].state).toBe("not recorded");
    expect(view.steps[4].state).toBe("not recorded");
    expect(view.steps[5].state).toBe("awaiting");
  });

  it("uses recorded lifecycle times without filling gaps from status alone", () => {
    const trade = toTrade({
      ...detail,
      lifecycle: [
        { status: "created", at: 100 },
        { status: "confirmed", at: 107 },
      ],
      created_at: 100,
      settled_at: 107,
    });
    const view = tradeLifecycle(trade);
    expect(view.recordedCount).toBe(2);
    expect(view.steps.map((step) => step.elapsedSeconds)).toEqual([
      0,
      null,
      null,
      null,
      null,
      7,
    ]);
    expect(
      view.steps.slice(1, 5).every((step) => step.state === "not recorded"),
    ).toBe(true);
    expect(view.elapsedSeconds).toBe(7);
  });

  it("projects SolventX evidence into cross-chain lifecycle labels", () => {
    const trade = {
      ...toTrade(detail),
      flow: "cross-chain" as const,
      lifecycle: [
        { status: "quoted", at: 100 },
        { status: "destination fill", at: 103 },
        { status: "proof relay", at: 105 },
        { status: "origin claim", at: 107 },
        { status: "repayment", at: 109 },
        { status: "complete", at: 111 },
      ],
      createdAt: 100,
      settledAt: 111,
    };

    const view = tradeLifecycle(trade);

    expect(view.steps.map((step) => step.label)).toEqual([
      "Quoted",
      "Destination fill",
      "Proof relay",
      "Origin claim",
      "Repayment",
      "Complete",
    ]);
    expect(view.recordedCount).toBe(6);
    expect(view.elapsedSeconds).toBe(11);
    expect(view.phases).toEqual([
      { name: "Destination execution", complete: true },
      { name: "Origin settlement", complete: true },
    ]);
  });

  it("distinguishes a signed output floor and a stopped lifecycle from settlement", () => {
    const trade = toTrade({
      ...detail,
      status: "declined",
      tx_hash: undefined,
      block_number: undefined,
      legs: [],
      lifecycle: [
        { status: "created", at: 100 },
        { status: "declined", at: 103 },
      ],
      created_at: 100,
      settled_at: 103,
    });
    const view = tradeDetail(trade);
    const lifecycle = tradeLifecycle(trade);
    expect(view.summary[0].label).toBe("In → out");
    expect(view.summary[0].value).toContain("min. ");
    expect(lifecycle.steps[5].state).toBe("not reached");
    expect(lifecycle.recordedCount).toBe(1);
    expect(view.empty).toBe(true);
    expect(view.facts.find((fact) => fact.label === "Signature")).toEqual({
      label: "Signature",
      value: "—",
    });
  });

  it.each(["declined", "failed"] as const)(
    "reads a %s trade as stopped where it stopped, not as partial progress",
    (status) => {
      const view = tradeLifecycle(declined(status));
      expect(view.halted).toBe(true);
      expect(view.stoppedAt).toBe("Quoted");
      expect(view.steps[2].state).toBe("not reached");
    },
  );

  it("leaves a trade still in flight unhalted, with a stage awaited", () => {
    const view = tradeLifecycle(
      toTrade({
        ...detail,
        status: "submitted",
        settled_at: undefined,
        lifecycle: detail.lifecycle.slice(0, 2),
      }),
    );
    expect(view.halted).toBe(false);
    expect(view.stoppedAt).toBeNull();
  });

  it("says what a stopped trade earned instead of what it might have", () => {
    const view = tradeDetail(declined());
    expect(view.profitLabel).toBe("Not earned");
    expect(view.profit).toBe("—");
    expect(view.profitTag).toBe(GLOSSARY.statusDeclined);

    const live = tradeDetail(toTrade(detail));
    expect(live.profitLabel).toBe("Expected profit");
    expect(live.profitTag).toBe("route estimate · net of estimated gas");
  });

  it("counts a fillable order's deadline down and marks it once it passes", () => {
    const deadline = (seconds: number) =>
      tradeDetail(
        { ...declined(), status: "quoted" },
        (detail.deadline_block - seconds) * 1000,
      ).facts.find((fact) => fact.label === "Deadline");

    expect(deadline(24)?.value).toBe("in 24s");
    expect(deadline(-240)?.value).toBe("expired 4m ago");
    // A settled order's countdown is history; the signed time is what reconciles against a log.
    expect(
      tradeDetail(toTrade(detail)).facts.find(
        (fact) => fact.label === "Deadline",
      )?.value,
    ).toMatch(/2026/);
  });

  it("prints block numbers as identifiers, ungrouped", () => {
    expect(tradeRow(toTrade(detail)).blockLabel).toBe("blk 39064");
    expect(
      explorerStats({
        blockHeight: 39064,
        events24h: null,
        tradesSettled: null,
        confirmedPct: null,
        medianImpactPct: null,
        activeMakers: null,
        quotingNow: null,
      })[0].value,
    ).toBe("39064");
  });

  it("keeps zero-valued metrics visible and unavailable metrics unknown", () => {
    const stats = explorerStats({
      blockHeight: 0,
      events24h: 0,
      tradesSettled: 0,
      confirmedPct: null,
      medianImpactPct: 0,
      activeMakers: 0,
      quotingNow: null,
    });
    expect(stats.map((stat) => stat.value)).toEqual([
      "0",
      "0",
      "0",
      "0.00%",
      "0",
    ]);
    expect(stats[2].sub).toBe("");
    expect(stats[4].sub).toBe("");
  });
});
