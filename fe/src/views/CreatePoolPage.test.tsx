import { act, fireEvent, screen } from "@testing-library/react";
import { Route, useLocation, useParams } from "react-router-dom";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { TransitionRoutes } from "@/components/TransitionRoutes";
import type {
  CreatePair,
  CreatedPosition,
  PositionSubmissionOptions,
} from "@/ports/positions";
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

/** What 1,875 USDC pairs with on the ±5% concentrated curve at a 2,500 mid. */
const FLIPPED_WETH = "0.7879867893524485";

const wallet = vi.hoisted(() => ({ connected: true }));
const connectOrCreateWallet = vi.hoisted(() => vi.fn());

vi.mock("@privy-io/react-auth", () => ({
  usePrivy: () => ({ connectOrCreateWallet }),
}));

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  // A disconnected wallet reports no address either: `useWalletAction` reads a known address as
  // connected, so leaving one here would make every wallet look attached.
  useAccount: () => ({
    isConnected: wallet.connected,
    address: wallet.connected
      ? "0x1111111111111111111111111111111111111111"
      : undefined,
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
  wallet.connected = true;
  connectOrCreateWallet.mockClear();
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
  it("re-derives both deposits when the orientation flips", async () => {
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

    // The 1,875 USDC keeps its token; the WETH side is re-quoted against it, not handed the 0.75.
    const flipped = useAppStore.getState();
    expect(flipped.flipped).toBe(true);
    expect(flipped.amtA).toBe("1875");
    expect(flipped.amtB).not.toBe("0.75");
    expect(flipped.amtB).toBe(FLIPPED_WETH);
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
    // By name, not by position: the boxes carry no visible label, so the name is all a maker
    // filling this in without sight has to tell them apart.
    const baseAmount = screen.getByRole("textbox", {
      name: "USDT deposit amount",
    });
    const quoteAmount = screen.getByRole("textbox", {
      name: "USDC deposit amount",
    });
    fireEvent.change(baseAmount, { target: { value: "99.49" } });

    expect(baseAmount).toHaveValue("99.49");
    expect(quoteAmount).toHaveValue("99.410408");

    fireEvent.click(screen.getByRole("button", { name: "Use full balances" }));
    expect(baseAmount).toHaveValue("99.49");
    expect(quoteAmount).toHaveValue("99.410408");
  });

  it("recalculates paired deposits when the curve changes", async () => {
    useAppStore.setState({ step: 3, strategy: "Concentrated" });
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
    fireEvent.change(screen.getAllByRole("textbox")[0], {
      target: { value: "1" },
    });
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
    const submit = vi
      .fn()
      .mockImplementation((options?: PositionSubmissionOptions) => {
        options?.onStatus?.({
          kind: "approving",
          token: pair.base.address,
          index: 0,
          total: 1,
        });
        return new Promise<CreatedPosition>((resolve) => {
          confirm = resolve;
        });
      });
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
      fireEvent.click(screen.getByRole("button", { name: "Create position" }));
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    await vi.waitFor(() => expect(submit).toHaveBeenCalledOnce());
    expect(
      await screen.findByRole("button", {
        name: "Approve WETH — step 1 of 2",
      }),
    ).toBeDisabled();
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
  it("names the pool it came from in the breadcrumb trail", async () => {
    useAppStore.setState({ step: 1 });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
        <Route path="/pools" element={<p>Pools index</p>} />
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
      screen.getByRole("button", { name: "WETH/USDC" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Pools" }));
    expect(await screen.findByText("Pools index")).toBeVisible();
  });

  it("refuses a pair slug the deployment does not serve", async () => {
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
      "/pools/aave-usdc/new",
    );

    expect(
      await screen.findByText("AAVE/USDC isn’t an available pair."),
    ).toBeVisible();
    expect(screen.queryByText("WETH / USDC")).not.toBeInTheDocument();
    expect(screen.queryByText("In range")).not.toBeInTheDocument();
  });

  it("draws no price until the pair catalog has answered", async () => {
    useAppStore.setState({ step: 2 });
    let loadPairs!: (pairs: CreatePair[]) => void;
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockReturnValue(
            new Promise<CreatePair[]>((resolve) => {
              loadPairs = resolve;
            }),
          ),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/weth-usdc/new",
    );

    expect(screen.getByText("Loading supported pairs…")).toBeVisible();
    expect(screen.queryByText("In range")).not.toBeInTheDocument();
    expect(screen.queryByText("1.00")).not.toBeInTheDocument();

    await act(async () => {
      loadPairs([pair]);
      await new Promise((resolve) => window.setTimeout(resolve, 0));
    });
    expect(screen.getByText("In range")).toBeVisible();
  });

  it("offers a retry when the pair catalog fails", async () => {
    useAppStore.setState({ step: 2 });
    const pairs = vi
      .fn()
      .mockRejectedValueOnce(new Error("down"))
      .mockResolvedValue([pair]);
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      { positions: { pairs, history: vi.fn().mockResolvedValue([]) } },
      "/pools/weth-usdc/new",
    );

    expect(
      await screen.findByText("Couldn’t load supported pairs."),
    ).toBeVisible();
    expect(screen.queryByText("In range")).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByText("In range")).toBeVisible();
    expect(pairs).toHaveBeenCalledTimes(2);
  });

  it("opens with no deposit the maker did not type", async () => {
    useAppStore.setState({ step: 2 });
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

    expect(await screen.findByText("In range")).toBeVisible();
    fireEvent.click(screen.getByRole("button", { name: "Next step" }));

    await screen.findByText(/1 WETH pairs with/);
    const [baseAmount, quoteAmount] = screen.getAllByRole("textbox");
    expect(baseAmount).toHaveValue("");
    expect(quoteAmount).toHaveValue("");
    expect(screen.queryByText(/uses 50% of balance/)).not.toBeInTheDocument();
  });

  it("renders every wallet balance through one formatter", async () => {
    const raw = {
      ...pair,
      base: { ...pair.base, balanceRaw: 1_234_567_812_345_678_901n },
    };
    useAppStore.setState({ step: 3 });
    renderWithServices(
      <TransitionRoutes>
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
      </TransitionRoutes>,
      {
        positions: {
          pairs: vi.fn().mockResolvedValue([raw]),
          history: vi.fn().mockResolvedValue([]),
        },
      },
      "/pools/weth-usdc/new",
    );

    expect(await screen.findByText("bal 1.2345")).toBeVisible();
    expect(screen.queryByText(/1\.234567812345678901/)).not.toBeInTheDocument();

    act(() => useAppStore.setState({ step: 1 }));
    expect(screen.getByText(/^1\.2345 WETH ·/)).toBeVisible();
  });

  it("asks for a wallet before it asks for a signature", async () => {
    wallet.connected = false;
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

    const cta = await screen.findByRole("button", { name: "Connect wallet" });
    expect(cta).toBeEnabled();
    fireEvent.click(cta);
    expect(connectOrCreateWallet).toHaveBeenCalledOnce();
  });

  it("promises only the signatures it will ask for", async () => {
    useAppStore.setState({ step: 3 });
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

    await screen.findByText(/1 WETH pairs with/);
    fireEvent.change(screen.getAllByRole("textbox")[0], {
      target: { value: "1" },
    });
    fireEvent.click(screen.getByRole("button", { name: "Review" }));

    expect(
      screen.getByRole("button", { name: "Create position" }),
    ).toBeVisible();
    expect(screen.queryByText(/step 1 of 2/)).not.toBeInTheDocument();
    expect(
      screen.getByText(/an approval for each of WETH and USDC/),
    ).toBeVisible();
  });
});
