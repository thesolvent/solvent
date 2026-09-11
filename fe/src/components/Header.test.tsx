import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { INITIAL_STATE } from "@/state";
import { useAppStore } from "@/store";
import { renderWithServices } from "@/test/harness";

import { Header } from "./Header";

const wallet = vi.hoisted(() => ({
  address: undefined as `0x${string}` | undefined,
}));
const privy = vi.hoisted(() => ({
  ready: true,
  authenticated: false,
  connectOrCreateWallet: vi.fn(),
  logout: vi.fn(),
}));
const wagmi = vi.hoisted(() => ({
  disconnect: vi.fn(),
}));
const wallets = vi.hoisted(() => ({
  value: [] as Array<{ address: string; walletClientType: string }>,
}));
const exportWallet = vi.hoisted(() => vi.fn());

vi.mock("@privy-io/react-auth", () => ({
  usePrivy: () => privy,
  useWallets: () => ({ wallets: wallets.value }),
  useExportWallet: () => ({ exportWallet }),
}));

vi.mock("wagmi", async (original) => ({
  ...(await original<typeof import("wagmi")>()),
  useAccount: () => ({ address: wallet.address }),
  useDisconnect: () => wagmi,
}));

beforeEach(() => {
  useAppStore.setState(INITIAL_STATE);
  wallet.address = undefined;
  privy.authenticated = false;
  privy.connectOrCreateWallet.mockReset();
  privy.logout.mockReset().mockResolvedValue(undefined);
  wagmi.disconnect.mockReset();
  wallets.value = [];
  exportWallet.mockReset().mockResolvedValue(undefined);
});

it("offers key export only for the active Privy embedded wallet", () => {
  wallet.address = "0x1111111111111111111111111111111111111111";
  privy.authenticated = true;
  wallets.value = [{ address: wallet.address, walletClientType: "privy" }];
  renderWithServices(<Header />);

  fireEvent.click(screen.getByRole("button", { name: "Wallet account" }));
  fireEvent.click(screen.getByRole("menuitem", { name: "Export wallet" }));

  expect(exportWallet).toHaveBeenCalledWith({ address: wallet.address });
});

it("does not offer key export for an external wallet", () => {
  wallet.address = "0x1111111111111111111111111111111111111111";
  wallets.value = [{ address: wallet.address, walletClientType: "metamask" }];
  renderWithServices(<Header />);

  fireEvent.click(screen.getByRole("button", { name: "Wallet account" }));

  expect(
    screen.queryByRole("menuitem", { name: "Export wallet" }),
  ).not.toBeInTheDocument();
});

it("keeps the selected product active across the application shell", () => {
  renderWithServices(<Header />);

  fireEvent.click(screen.getByRole("button", { name: "Solvent" }));
  expect(screen.queryByText("Product")).not.toBeInTheDocument();
  expect(
    screen.queryByText("Same account, same balances"),
  ).not.toBeInTheDocument();
  expect(
    screen.getByText("Same-chain intent swaps, powered by Aqua."),
  ).toBeInTheDocument();
  expect(
    screen.getByText(
      "Cross-chain intent swaps, built on Aqua + Compact + CCTP + CCIP",
    ),
  ).toBeInTheDocument();
  const solventX = screen.getByRole("menuitemradio", { name: /SolventX/ });
  expect(solventX).toHaveAttribute("aria-checked", "false");

  fireEvent.click(solventX);

  expect(useAppStore.getState().productMode).toBe("SolventX");
  expect(screen.getByRole("button", { name: "SolventX" })).toHaveAttribute(
    "aria-expanded",
    "false",
  );
});

it("opens wallet actions for the active wallet before disconnecting", () => {
  wallet.address = "0x1111111111111111111111111111111111111111";
  renderWithServices(<Header />);

  fireEvent.click(screen.getByRole("button", { name: "Wallet account" }));

  expect(screen.getByRole("menu", { name: "Wallet account" })).toBeVisible();
  expect(privy.logout).not.toHaveBeenCalled();

  fireEvent.click(screen.getByRole("menuitem", { name: "Disconnect" }));

  expect(wagmi.disconnect).toHaveBeenCalledOnce();
  expect(privy.logout).toHaveBeenCalledOnce();
});
