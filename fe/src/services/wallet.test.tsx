import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { displayBalance, useWalletAction } from "./wallet";

const wallet = vi.hoisted(() => ({
  address: undefined as `0x${string}` | undefined,
  chainId: undefined as number | undefined,
  isConnected: false,
  connectOrCreateWallet: vi.fn(),
  switchChain: vi.fn(),
}));

vi.mock("@privy-io/react-auth", () => ({
  usePrivy: () => ({
    connectOrCreateWallet: wallet.connectOrCreateWallet,
  }),
}));

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  useAccount: () => ({
    address: wallet.address,
    chainId: wallet.chainId,
    isConnected: wallet.isConnected,
  }),
  useSwitchChain: () => ({ switchChain: wallet.switchChain }),
}));

function Probe() {
  const action = useWalletAction();
  return (
    <button type="button" onClick={action.prepare}>
      {action.switchTo ?? (action.connected ? "Ready" : "Connect")}
    </button>
  );
}

beforeEach(() => {
  wallet.address = undefined;
  wallet.chainId = undefined;
  wallet.isConnected = false;
  wallet.connectOrCreateWallet.mockReset();
  wallet.switchChain.mockReset();
});

it("opens Privy only when no wallet is active", () => {
  render(<Probe />);

  fireEvent.click(screen.getByRole("button", { name: "Connect" }));

  expect(wallet.connectOrCreateWallet).toHaveBeenCalledOnce();
  expect(wallet.switchChain).not.toHaveBeenCalled();
});

it("switches an external wallet before any protocol write", () => {
  wallet.address = "0x1111111111111111111111111111111111111111";
  wallet.chainId = 31338;
  wallet.isConnected = true;
  render(<Probe />);

  fireEvent.click(screen.getByRole("button", { name: "Solvent Devnet" }));

  expect(wallet.switchChain).toHaveBeenCalledExactlyOnceWith({
    chainId: 31337,
  });
  expect(wallet.connectOrCreateWallet).not.toHaveBeenCalled();
});

it("formats an on-chain balance for compact picker display", () => {
  expect(displayBalance(399_300_000_000n, 6)).toBe("399,300");
  expect(displayBalance(1_234_567_890_000_000_000n, 18)).toBe("1.234567");
});
