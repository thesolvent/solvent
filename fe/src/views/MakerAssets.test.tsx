import { fireEvent, render, screen } from "@testing-library/react";
import { expect, it, vi } from "vitest";

import { MakerAssets } from "./MakerAssets";

const asset = {
  address: "0xdai",
  sym: "DAI",
  tint: "#fff",
  open: true,
  across: "Across 1 position",
  wallet: "$10.75K",
  walletAmt: "10,750.98 DAI",
  shared: "$10.75K",
  sharedAmt: "10,750.98 DAI",
  fees: "$0.00",
  apy: "0.0%",
  ratio: "1.00×",
  legs: [
    {
      hash: "position-a",
      pair: "WBTC/DAI",
      meta: "Concentrated · 0.05%",
      cur: "10,750.98 DAI",
      curUsd: "$10.75K",
      op: "10,750.98 DAI",
      fees: "$0.00",
      apy: "0.0%",
      cov: "1.00×",
    },
  ],
};

it("opens an asset's individual position and keeps the asset disclosure separate", () => {
  const onToggle = vi.fn();
  const onOpenPosition = vi.fn();
  render(
    <MakerAssets
      assets={[asset]}
      onToggle={onToggle}
      onOpenPosition={onOpenPosition}
    />,
  );

  fireEvent.click(
    screen.getByRole("button", {
      name: "Open WBTC/DAI position position-a",
    }),
  );
  expect(onOpenPosition).toHaveBeenCalledExactlyOnceWith("position-a");
  expect(onToggle).not.toHaveBeenCalled();

  fireEvent.click(
    screen.getByRole("button", { name: "Collapse DAI positions" }),
  );
  expect(onToggle).toHaveBeenCalledExactlyOnceWith(0);
  expect(onOpenPosition).toHaveBeenCalledTimes(1);
});
