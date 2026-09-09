import { act, fireEvent, screen } from "@testing-library/react";
import { Route, useLocation, useParams } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TransitionRoutes } from "@/components/TransitionRoutes";
import type { CreatePair, CreatedPosition } from "@/ports/positions";
import { INITIAL_STATE } from "@/state";
import { useAppStore } from "@/store";
import { renderWithServices } from "@/test/harness";
import { CreatePoolPage } from "./CreatePoolPage";

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

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  useAccount: () => ({
    address: "0x1111111111111111111111111111111111111111",
    chainId: 31337,
  }),
  useClient: () => ({}),
  useConnectorClient: () => ({ data: {} }),
}));

function StrategyDestination() {
  const location = useLocation();
  return (
    <>
      <p>Created strategy {useParams().strategyHash}</p>
      <p>Wait for index {String(location.state?.waitForStrategyIndex)}</p>
    </>
  );
}

beforeEach(() => {
  useAppStore.setState({
    ...INITIAL_STATE,
    step: 4,
    strategy: "Concentrated",
    amtA: "1",
    amtB: "2500",
    bandMin: -5,
    bandMax: 5,
  });
});

describe("CreatePoolPage", () => {
  it("changes orientation beside the price chart without reassigning deposit amounts", async () => {
    useAppStore.setState({ step: 1, flipped: false });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockResolvedValue([pair]),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/weth-usdc/new",
    );

    expect(await screen.findByText("WETH / USDC")).toBeVisible();
    expect(
      screen.queryByRole("button", { name: "WETH ⇄ USDC" }),
    ).not.toBeInTheDocument();

    act(() => {
      useAppStore.setState({ step: 2, amtA: "0.75", amtB: "1875" });
    });
    fireEvent.click(screen.getByRole("button", { name: "WETH ⇄ USDC" }));

    expect(useAppStore.getState()).toMatchObject({
      flipped: true,
      amtA: "1875",
      amtB: "0.75",
    });
    expect(screen.getByRole("button", { name: "USDC ⇄ WETH" })).toBeVisible();
  });

  it("applies price presets without losing custom bounds or the pair curve", async () => {
    const stablePair = { ...pair, type: "Stable" as const };
    useAppStore.setState({
      step: 2,
      strategy: "Concentrated",
      createPreset: "Market",
      bandMax: 0.08,
      bandMin: -0.03,
    });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockResolvedValue([stablePair]),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/weth-usdc/new",
    );

    expect((await screen.findAllByText("2,500.00")).at(0)).toBeVisible();
    act(() => {
      useAppStore.setState({
        strategy: "Concentrated",
        createPreset: "Market",
        bandMax: 0.08,
        bandMin: -0.03,
      });
    });

    fireEvent.click(screen.getByRole("button", { name: "Custom" }));
    expect(useAppStore.getState()).toMatchObject({
      strategy: "Concentrated",
      createPreset: "Custom",
      bandMax: 0.08,
      bandMin: -0.03,
    });

    fireEvent.click(
      screen.getAllByRole("button", { name: "Full range" }).at(-1)!,
    );
    fireEvent.click(screen.getByRole("button", { name: "Market" }));
    expect(useAppStore.getState()).toMatchObject({
      strategy: "Pegged",
      createPreset: "Market",
      pegSym: true,
      bandMax: 5,
      bandMin: -5,
    });

    fireEvent.click(screen.getByRole("button", { name: "±0.04%" }));
    expect(useAppStore.getState()).toMatchObject({
      strategy: "Pegged",
      createPreset: "±0.04%",
      pegSym: true,
      bandMax: 0.04,
      bandMin: -0.04,
    });
  });

  it("keeps a pegged strategy inside the SwapVM width domain when selected", async () => {
    const stablePair = {
      ...pair,
      type: "Stable" as const,
      defaultBandPct: 0.004,
    };
    useAppStore.setState({
      step: 2,
      strategy: "Concentrated",
      pegSym: false,
    });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockResolvedValue([stablePair]),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/weth-usdc/new",
    );

    expect((await screen.findAllByText("2,500.00")).at(0)).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Pegged" }));

    expect(useAppStore.getState()).toMatchObject({
      strategy: "Pegged",
      pegSym: true,
      bandMax: 0.01,
      bandMin: -0.01,
    });
  });

  it("pairs deposits with the live-price curve allocation", async () => {
    const stablePair = {
      ...pair,
      base: {
        ...pair.base,
        decimals: 6,
        symbol: "USDT",
        balance: 99.49,
        balanceRaw: 99_490_000n,
      },
      quote: {
        ...pair.quote,
        decimals: 6,
        symbol: "USDC",
        balance: 10_096.73,
        balanceRaw: 10_096_730_000n,
      },
      mid: 0.9992,
      type: "Stable" as const,
    };
    useAppStore.setState({
      step: 3,
      strategy: "Pegged",
      pegSym: true,
      bandMin: -0.1,
      bandMax: 0.1,
    });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockResolvedValue([stablePair]),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/usdt-usdc/new",
    );

    expect(
      await screen.findByText(
        /1 USDT pairs with 0\.9992 USDC at the live market price/,
      ),
    ).toBeVisible();
    const [baseAmount, quoteAmount] = screen.getAllByRole("textbox");
    fireEvent.change(baseAmount, { target: { value: "99.49" } });

    expect(baseAmount).toHaveValue("99.49");
    expect(quoteAmount).toHaveValue("99.410408");

    fireEvent.click(screen.getByRole("button", { name: "Use full balances" }));
    expect(baseAmount).toHaveValue("99.49");
    expect(quoteAmount).toHaveValue("99.410408");
  });

  it("recalculates paired deposits when the curve changes", async () => {
    useAppStore.setState({
      step: 3,
      strategy: "Concentrated",
      amtA: "1",
      amtB: "123",
    });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockResolvedValue([pair]),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/weth-usdc/new",
    );

    expect(
      await screen.findByText(
        /1 WETH pairs with 2,626\.62 USDC at the live market price/,
      ),
    ).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Full range" }));

    expect(useAppStore.getState()).toMatchObject({
      strategy: "Full range",
      amtA: "1",
      amtB: "2500",
    });
  });

  it("shows live pairs and routes only after Aqua confirms the position", async () => {
    useAppStore.setState({ step: 3 });
    let loadPairs!: (pairs: CreatePair[]) => void;
    let confirm!: (position: CreatedPosition) => void;
    const submit = vi.fn().mockReturnValue(
      new Promise<CreatedPosition>((resolve) => {
        confirm = resolve;
      }),
    );
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
        <Route
          path="/explorer/strategies/:strategyHash"
          element={<StrategyDestination />}
        />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockReturnValue(
            new Promise<CreatePair[]>((resolve) => {
              loadPairs = resolve;
            }),
          ),
          history: vi.fn().mockResolvedValue([]),
          createIntent: () => ({ submit }),
        },
      },
      "/pools/weth-usdc/new",
      "browser",
    );

    await act(async () => {
      loadPairs([pair]);
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    expect(
      await screen.findByText(
        /1 WETH pairs with 2,626\.62 USDC at the live market price/,
      ),
    ).toBeVisible();
    const [baseAmount] = screen.getAllByRole("textbox");
    fireEvent.change(baseAmount, { target: { value: "1" } });
    fireEvent.click(screen.getByRole("button", { name: "Review" }));
    await act(async () => {
      fireEvent.click(
        screen.getByRole("button", { name: /Approve WETH & USDC/ }),
      );
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(screen.queryByText(/Created strategy/)).not.toBeInTheDocument();

    vi.useFakeTimers();
    await act(async () => {
      confirm({ strategyHash: "strategy-123", transactionHash: "tx-123" });
      await vi.advanceTimersByTimeAsync(1);
    });
    expect(screen.getByTestId("route-transition")).toBeInTheDocument();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(210);
    });

    expect(screen.getByText("Created strategy strategy-123")).toBeVisible();
    expect(screen.getByText("Wait for index true")).toBeVisible();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(310);
    });
    expect(screen.queryByTestId("route-transition")).not.toBeInTheDocument();
    vi.useRealTimers();
  });
});
