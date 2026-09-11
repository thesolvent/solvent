import { fireEvent, screen } from "@testing-library/react";
import { beforeEach, expect, it, vi } from "vitest";

import { INITIAL_STATE } from "@/state";
import { useAppStore } from "@/store";
import { renderWithServices } from "@/test/harness";

import { Header } from "./Header";

vi.mock("@rainbow-me/rainbowkit", () => ({
  ConnectButton: {
    Custom: ({ children }: { children: (state: object) => React.ReactNode }) =>
      children({ mounted: true }),
  },
}));

beforeEach(() => useAppStore.setState(INITIAL_STATE));

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
