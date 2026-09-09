import { act, screen } from "@testing-library/react";
import { SolventApiError } from "@solvent/sdk/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import type { DepthCurve } from "@/data";
import type { Position } from "@/data/makers";
import { renderWithServices } from "@/test/harness";
import { usePosition, usePositionDepth } from "./makers";

const position = {
  hash: "new-strategy",
  pair: "DAI/USDC",
  ref: {
    base: "0x0000000000000000000000000000000000000001",
    quote: "0x0000000000000000000000000000000000000002",
    baseDecimals: 18,
    quoteDecimals: 6,
  },
} as Position;

const emptyDepth: DepthCurve = {
  axisTitle: "DAI in",
  bestPrice: null,
  levels: [],
};

const populatedDepth: DepthCurve = {
  axisTitle: "DAI in",
  bestPrice: 1,
  levels: [
    {
      sizeIn: 1,
      output: 1,
      price: 1,
      impactPct: 0,
      makersUsed: 1,
    },
  ],
};

function PositionProbe({ waitForIndex = false }: { waitForIndex?: boolean }) {
  const query = usePosition(position.hash, { waitForIndex });
  return <p>{query.data?.pair ?? (query.isError ? "missing" : "loading")}</p>;
}

function DepthProbe({
  waitForLiquidity = false,
}: {
  waitForLiquidity?: boolean;
}) {
  const query = usePositionDepth(position, { waitForLiquidity });
  return <p>{query.data?.levels.length ?? "loading"}</p>;
}

afterEach(() => {
  vi.useRealTimers();
});

describe("usePosition", () => {
  it("retries a newly confirmed strategy while the indexer catches up", async () => {
    vi.useFakeTimers();
    const read = vi
      .fn()
      .mockRejectedValueOnce(new SolventApiError(404, "not indexed"))
      .mockResolvedValue(position);
    renderWithServices(<PositionProbe waitForIndex />, {
      makers: { position: read },
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_001);
    });

    expect(screen.getByText("DAI/USDC")).toBeVisible();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it("does not poll a missing strategy opened directly", async () => {
    vi.useFakeTimers();
    const read = vi
      .fn()
      .mockRejectedValue(new SolventApiError(404, "not found"));
    renderWithServices(<PositionProbe />, { makers: { position: read } });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(30_000);
    });

    expect(screen.getByText("missing")).toBeVisible();
    expect(read).toHaveBeenCalledOnce();
  });
});

describe("usePositionDepth", () => {
  it("rechecks empty depth quickly while a new strategy's budgets synchronize", async () => {
    vi.useFakeTimers();
    const read = vi
      .fn()
      .mockResolvedValueOnce(emptyDepth)
      .mockResolvedValue(populatedDepth);
    renderWithServices(<DepthProbe waitForLiquidity />, {
      makers: { depth: read },
    });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_001);
    });

    expect(screen.getByText("1")).toBeVisible();
    expect(read).toHaveBeenCalledTimes(2);
  });

  it("keeps the normal refresh interval for an established empty strategy", async () => {
    vi.useFakeTimers();
    const read = vi.fn().mockResolvedValue(emptyDepth);
    renderWithServices(<DepthProbe />, { makers: { depth: read } });

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_001);
    });

    expect(screen.getByText("0")).toBeVisible();
    expect(read).toHaveBeenCalledOnce();
  });
});
