import { focusManager, onlineManager } from "@tanstack/react-query";
import { act, fireEvent, screen, waitFor } from "@testing-library/react";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Asset, Quote, SubmittedSwap } from "@/data";
import { renderWithServices } from "@/test/harness";
import { useQuote } from "./quote";
import { useSubmitSwap } from "./swap";

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  useAccount: () => ({
    address: "0x1111111111111111111111111111111111111111",
    chainId: 31337,
  }),
  useClient: () => ({}),
  useConnectorClient: () => ({ data: {} }),
}));
const ASSET = {
  chainId: 31337,
  address: "0x2222222222222222222222222222222222222222",
  symbol: "WETH",
  name: "Wrapped Ether",
  decimals: 18,
  price: 2_500,
  change: "0%",
  tags: ["ETH"],
  net: "Devnet",
  pairs: ["WETH/USDC"],
} satisfies Asset;
const QUOTE = {
  tokenIn: ASSET.address,
  tokenOut: ASSET.address,
  amountInRaw: 1n,
  amountOut: "100",
  amountOutUsd: 100,
  priceImpact: "0%",
  makersSourced: 1,
  amountOutRaw: 100n,
  expiresAt: Date.now() + 60_000,
} satisfies Quote;
const SUBMITTED = {
  tradeId: "trade",
  status: "submitted",
} satisfies SubmittedSwap;

function Probe() {
  const [amount, setAmount] = useState("1");
  const swap = useSubmitSwap({
    from: ASSET,
    to: ASSET,
    amount,
    quote: QUOTE,
    slippagePct: 1,
  });
  return (
    <>
      <button disabled={swap.submitting} onClick={swap.send}>
        send
      </button>
      <button onClick={() => setAmount("2")}>edit</button>
      <p>{swap.problem}</p>
      <p>{swap.result?.status}</p>
    </>
  );
}

function setup(submit = vi.fn().mockResolvedValue(SUBMITTED)) {
  const createIntent = vi.fn().mockReturnValue({ submit });
  renderWithServices(<Probe />, { swap: { createIntent } });
  fireEvent.click(screen.getByRole("button", { name: "send" }));
  return { createIntent, submit };
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("swap submission", () => {
  it("does not apply a completed submission to an edited trade", async () => {
    let finish!: (value: SubmittedSwap) => void;
    const submit = vi.fn().mockReturnValue(
      new Promise((resolve) => {
        finish = resolve;
      }),
    );
    setup(submit);
    await waitFor(() => expect(submit).toHaveBeenCalledOnce());
    fireEvent.click(screen.getByRole("button", { name: "edit" }));
    finish(SUBMITTED);
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "send" })).toBeEnabled(),
    );
    expect(screen.queryByText("submitted")).not.toBeInTheDocument();
  });

  it("keeps wallet failure details out of logs and allows a retry", async () => {
    const submit = vi
      .fn()
      .mockRejectedValueOnce(
        Object.assign(new Error("sensitive signed payload"), {
          cause: { code: 4001 },
        }),
      )
      .mockResolvedValue(SUBMITTED);
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => {});
    try {
      setup(submit);
      expect(
        await screen.findByText("Wallet request rejected"),
      ).toBeInTheDocument();
      expect(consoleError).not.toHaveBeenCalled();
      fireEvent.click(screen.getByRole("button", { name: "send" }));
      expect(await screen.findByText("submitted")).toBeInTheDocument();
    } finally {
      consoleError.mockRestore();
    }
  });

  it("does not submit edited inputs when an offline submission resumes", async () => {
    onlineManager.setOnline(false);
    try {
      const { submit } = setup();
      await waitFor(() =>
        expect(screen.getByRole("button", { name: "send" })).toBeDisabled(),
      );
      fireEvent.click(screen.getByRole("button", { name: "edit" }));
      act(() => onlineManager.setOnline(true));
      await waitFor(() =>
        expect(screen.getByRole("button", { name: "send" })).toBeEnabled(),
      );
      expect(submit).not.toHaveBeenCalled();
    } finally {
      onlineManager.setOnline(true);
    }
  });

  it("retains the same intent when the submission response is lost", async () => {
    const submit = vi
      .fn()
      .mockRejectedValueOnce(new Error("Connection lost"))
      .mockResolvedValue(SUBMITTED);
    const { createIntent } = setup(submit);
    expect(
      await screen.findByText("Could not submit the swap"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "send" }));
    expect(await screen.findByText("submitted")).toBeInTheDocument();
    expect(createIntent).toHaveBeenCalledOnce();
    expect(submit).toHaveBeenCalledTimes(2);
  });

  it("creates a fresh intent after a known decline", async () => {
    const declined = Object.assign(new Error("declined"), {
      name: "SwapDeclinedError",
    });
    const { createIntent } = setup(
      vi.fn().mockRejectedValueOnce(declined).mockResolvedValue(SUBMITTED),
    );
    expect(
      await screen.findByText("The resolver declined this swap"),
    ).toBeInTheDocument();
    expect(screen.queryByText("submitted")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "send" }));
    expect(await screen.findByText("submitted")).toBeInTheDocument();
    expect(createIntent).toHaveBeenCalledTimes(2);
  });
});

it("hides the old quote immediately while a new amount is being debounced", async () => {
  function PriceProbe() {
    const [amount, setAmount] = useState("1");
    const { quote } = useQuote(
      ASSET,
      {
        ...ASSET,
        address: "0x3333333333333333333333333333333333333333",
      },
      amount,
    );
    return (
      <>
        <button onClick={() => setAmount("2")}>change amount</button>
        <p>{quote?.amountOut ?? "pending"}</p>
      </>
    );
  }
  renderWithServices(<PriceProbe />, {
    swap: {
      quote: vi.fn().mockResolvedValue({ ...QUOTE, amountOut: "old price" }),
    },
  });
  expect(await screen.findByText("old price")).toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "change amount" }));
  expect(screen.queryByText("old price")).not.toBeInTheDocument();
});

it("does not query a value that exceeds the input token precision", async () => {
  function PriceProbe() {
    const { problem } = useQuote(
      ASSET,
      {
        ...ASSET,
        address: "0x3333333333333333333333333333333333333333",
      },
      "1.0000000000000000001",
    );
    return <p>{problem}</p>;
  }
  const quoteRequest = vi.fn();
  renderWithServices(<PriceProbe />, { swap: { quote: quoteRequest } });

  expect(
    await screen.findByText("Swap amount supports at most 18 decimal places"),
  ).toBeInTheDocument();
  expect(quoteRequest).not.toHaveBeenCalled();
});

it("quotes matching token addresses when their chains differ", async () => {
  function PriceProbe() {
    const { quote } = useQuote(
      ASSET,
      { ...ASSET, chainId: 31338, net: "Base" },
      "1",
    );
    return <p>{quote?.amountOut ?? "pending"}</p>;
  }
  const quoteRequest = vi.fn().mockResolvedValue(QUOTE);
  renderWithServices(<PriceProbe />, { swap: { quote: quoteRequest } });

  expect(await screen.findByText("100")).toBeInTheDocument();
  expect(quoteRequest).toHaveBeenCalledWith(
    expect.objectContaining({
      from: expect.objectContaining({ chainId: 31337 }),
      to: expect.objectContaining({ chainId: 31338 }),
    }),
  );
});

function StateProbe() {
  const { quote, pricing, stale } = useQuote(
    ASSET,
    { ...ASSET, address: "0x3333333333333333333333333333333333333333" },
    "1",
  );
  return (
    <>
      <p>{quote?.amountOut ?? "no price"}</p>
      <p>{pricing ? "pricing" : "actionable"}</p>
      <p>{stale ? "stale" : "fresh"}</p>
    </>
  );
}

/** Refetch the way a returning tab does, rather than waiting out the refresh interval. */
async function refresh(request: ReturnType<typeof vi.fn>) {
  const calls = request.mock.calls.length;
  act(() => {
    focusManager.setFocused(false);
    focusManager.setFocused(true);
  });
  await waitFor(() => expect(request.mock.calls.length).toBeGreaterThan(calls));
}

it("leaves a live price actionable while it is refreshing", async () => {
  const request = vi
    .fn()
    .mockResolvedValueOnce(QUOTE)
    .mockReturnValue(new Promise(() => {}));
  renderWithServices(<StateProbe />, { swap: { quote: request } });
  expect(await screen.findByText("actionable")).toBeInTheDocument();

  await refresh(request);

  expect(screen.getByText("100")).toBeInTheDocument();
  expect(screen.getByText("actionable")).toBeInTheDocument();
});

it("keeps the last unexpired price when a refresh fails, and says it is stale", async () => {
  const request = vi
    .fn()
    .mockResolvedValueOnce(QUOTE)
    .mockRejectedValue(new Error("resolver unavailable"));
  renderWithServices(<StateProbe />, { swap: { quote: request } });
  expect(await screen.findByText("100")).toBeInTheDocument();

  await refresh(request);

  expect(await screen.findByText("stale")).toBeInTheDocument();
  expect(screen.getByText("100")).toBeInTheDocument();
});
