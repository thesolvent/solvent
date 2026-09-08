import { screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Pool } from "@/data";
import { renderWithServices } from "@/test/harness";

import { PoolsPage } from "./PoolsPage";
import styles from "./PoolsPage.module.css";

const POOL: Pool = {
  pair: "WETH / USDC",
  type: "Volatile",
  venue: "Aqua core · 1 maker",
  range: "0.05% spread",
  tvl: "$164.2K",
  vol: "—",
  fills: "0",
  fee: "0.05%",
  apr: "—",
};

/** The filter cells only read symbols, so the rest of an asset is left off here. */
const SYMBOLS = ["WETH", "USDC"].map((symbol) => ({ symbol }));

describe("PoolsPage", () => {
  it("renders the pools the port returns", async () => {
    const list = vi.fn().mockResolvedValue([POOL]);
    renderWithServices(<PoolsPage />, {
      pools: { list },
      assets: { list: vi.fn().mockResolvedValue(SYMBOLS) },
    });

    expect(await screen.findAllByText("WETH / USDC")).not.toHaveLength(0);
    expect(await screen.findByText("0.05% spread")).toBeInTheDocument();
  });

  it("keeps the recommendation bar in place when nothing matches", async () => {
    const list = vi.fn().mockResolvedValue([]);
    const { container } = renderWithServices(<PoolsPage />, {
      pools: { list },
      assets: { list: vi.fn().mockResolvedValue(SYMBOLS) },
    });

    expect(await screen.findByText("Filters")).toBeInTheDocument();
    // The bar holds its row so filtering to nothing does not shift the page...
    expect(container.querySelector(`.${styles.recommend}`)).toBeInTheDocument();
    // ...but recommends nothing, and must not assume pools[0] exists.
    expect(screen.queryByText("Recommended")).not.toBeInTheDocument();
  });
});
