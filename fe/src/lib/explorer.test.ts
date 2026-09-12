import { describe, expect, it } from "vitest";
import detail from "@/data/fixtures/trade-detail.json";
import { toTrade } from "@/adapters/mappers/explorer";
import { tradeLifecycle } from "./trade-lifecycle";
import { explorerStats, tradeDetail } from "./explorer";

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
