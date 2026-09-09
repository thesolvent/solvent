import { act, fireEvent, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { CreatePair, PositionForm } from "@/ports/positions";
import {
  useCreatePairs,
  useCreatePosition,
  usePairPriceHistory,
} from "@/services/positions";
import { renderWithServices } from "@/test/harness";

const ADDRESS = "0x1111111111111111111111111111111111111111";
const pair: CreatePair = {
  base: {
    address: "0x2222222222222222222222222222222222222222",
    decimals: 18,
    symbol: "WETH",
    name: "Wrapped Ether",
    tags: ["ETH"],
    balance: 2,
    balanceRaw: 2_000_000_000_000_000_000n,
    valueUsd: 5_000,
    changePct: 1,
    tint: "#eee",
  },
  quote: {
    address: "0x3333333333333333333333333333333333333333",
    decimals: 6,
    symbol: "USDC",
    name: "USD Coin",
    tags: ["USD"],
    balance: 10_000,
    balanceRaw: 10_000_000_000n,
    valueUsd: 10_000,
    changePct: 0,
    tint: "#eef",
  },
  mid: 2_500,
  tvlUsd: 500_000,
  type: "Volatile",
  defaultBandPct: 5,
  defaultFeeBps: 5,
};
const form: PositionForm = {
  pair,
  curve: "Concentrated",
  feeBps: 5,
  spotPrice: "2500",
  priceMin: "2400",
  priceMax: "2600",
  halfWidthPct: 5,
  amountBase: "1",
  amountQuote: "2500",
};

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  useAccount: () => ({ address: ADDRESS, chainId: 31337 }),
  useClient: () => ({ kind: "public" }),
  useConnectorClient: () => ({ data: { kind: "wallet" } }),
}));

function Probe({ onCreated }: { onCreated: (hash: string) => void }) {
  const pairs = useCreatePairs();
  const creation = useCreatePosition(form, ({ strategyHash }) =>
    onCreated(strategyHash),
  );
  return (
    <>
      <p>{pairs.data?.[0]?.base.symbol ?? "loading"}</p>
      <button onClick={creation.send}>{creation.problem ?? "Create"}</button>
    </>
  );
}

function HistoryProbe() {
  const history = usePairPriceHistory(pair, "All");
  return <p>{history.data?.[0]?.price ?? "loading"}</p>;
}

beforeEach(() => vi.clearAllMocks());

describe("position services", () => {
  it("maps the All chart control to the backend all-history period", async () => {
    const history = vi
      .fn()
      .mockResolvedValue([{ timestampMs: 1, price: 2_450, volumeUsd: 10 }]);
    renderWithServices(<HistoryProbe />, { positions: { history } });

    expect(await screen.findByText("2450")).toBeVisible();
    expect(history).toHaveBeenCalledWith(pair, "all");
  });

  it("loads wallet-aware pairs and submits the retained position intent", async () => {
    const pairs = vi.fn().mockResolvedValue([pair]);
    const submit = vi.fn().mockResolvedValue({
      strategyHash: "0xstrategy",
      transactionHash: "0xtx",
    });
    const createIntent = vi.fn().mockReturnValue({ submit });
    const onCreated = vi.fn();
    renderWithServices(<Probe onCreated={onCreated} />, {
      positions: { pairs, createIntent },
    });

    expect(await screen.findByText("WETH")).toBeVisible();
    expect(pairs).toHaveBeenCalledWith(ADDRESS);
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Create" }));
    });

    await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(createIntent).toHaveBeenCalledWith(
      { ...form, maker: ADDRESS },
      {
        publicClient: { kind: "public" },
        walletClient: { kind: "wallet" },
      },
    );
    await vi.waitFor(() =>
      expect(onCreated).toHaveBeenCalledWith("0xstrategy"),
    );
  });
});
