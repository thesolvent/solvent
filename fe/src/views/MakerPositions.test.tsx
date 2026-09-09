import { fireEvent, render, screen, within } from "@testing-library/react";
import { expect, it, vi } from "vitest";
import { MakerPositions } from "./MakerPositions";

const position = {
  hash: "position-a",
  pair: "LINK/USDC",
  meta: "Concentrated · 0.3% · Volatile",
  cov: "1.00× cov",
  covNum: "1.00×",
  width: "Bounded",
  widthBg: "var(--lime-wash-soft)",
  widthFg: "var(--green-darkest)",
  splitA: "50%",
  labelA: "50% LINK",
  labelB: "50% USDC",
  stats: [{ label: "Current balance", value: "10 LINK", sep: "transparent" }],
};

const ignoreOpen = () => undefined;

it("keeps the first loaded pair and position selected when earlier identities arrive", () => {
  const { rerender } = render(
    <MakerPositions positions={[]} onOpenPosition={ignoreOpen} />,
  );
  rerender(
    <MakerPositions positions={[position]} onOpenPosition={ignoreOpen} />,
  );
  expect(
    screen.getByRole("region", { name: "Position position-a details" }),
  ).toBeInTheDocument();

  rerender(
    <MakerPositions
      onOpenPosition={ignoreOpen}
      positions={[
        { ...position, hash: "new-pair-position", pair: "AAVE/USDC" },
        { ...position, hash: "position-0" },
        position,
      ]}
    />,
  );
  expect(screen.getByRole("button", { name: "LINK/USDC" })).toHaveAttribute(
    "aria-expanded",
    "true",
  );
  expect(
    screen.getByRole("region", { name: "Position position-a details" }),
  ).toBeInTheDocument();
  expect(screen.getAllByRole("region")).toHaveLength(1);
});

it("groups identical pairs without merging positions and preserves the selected hash on refresh", () => {
  const duplicate = {
    ...position,
    hash: "position-b",
    stats: [{ label: "Current balance", value: "20 LINK", sep: "transparent" }],
  };
  const other = { ...position, hash: "position-c", pair: "WBTC/USDC" };
  const { rerender } = render(
    <MakerPositions
      positions={[position, duplicate, other]}
      onOpenPosition={ignoreOpen}
    />,
  );

  const pair = screen.getByRole("button", { name: "LINK/USDC" });
  const secondPair = screen.getByRole("button", { name: "WBTC/USDC" });
  expect(pair).toHaveAttribute("aria-expanded", "true");
  expect(secondPair).toHaveAttribute("aria-expanded", "false");
  expect(
    screen.getByRole("region", { name: "Position position-a details" }),
  ).toBeInTheDocument();

  fireEvent.click(
    screen.getByRole("button", {
      name: "Expand LINK/USDC position position-b",
    }),
  );
  expect(
    screen.queryByRole("region", { name: "Position position-a details" }),
  ).not.toBeInTheDocument();
  expect(
    within(
      screen.getByRole("region", { name: "Position position-b details" }),
    ).getByText("20 LINK"),
  ).toBeInTheDocument();

  rerender(<MakerPositions positions={[]} onOpenPosition={ignoreOpen} />);
  expect(screen.queryAllByRole("region")).toHaveLength(0);

  rerender(
    <MakerPositions
      onOpenPosition={ignoreOpen}
      positions={[
        other,
        { ...position, hash: "new-pair-position", pair: "AAVE/USDC" },
        { ...position, hash: "position-0" },
        duplicate,
        position,
      ]}
    />,
  );
  expect(
    screen.getByRole("button", {
      name: "Collapse LINK/USDC position position-b",
    }),
  ).toHaveAttribute("aria-expanded", "true");
  expect(screen.getAllByRole("region")).toHaveLength(1);

  const refreshedPair = screen.getByRole("button", { name: "LINK/USDC" });
  const refreshedSecondPair = screen.getByRole("button", { name: "WBTC/USDC" });
  fireEvent.click(refreshedSecondPair);
  expect(
    screen.getByRole("button", {
      name: "Collapse WBTC/USDC position position-c",
    }),
  ).toHaveAttribute("aria-expanded", "true");
  expect(refreshedPair).toHaveAttribute("aria-expanded", "false");
  fireEvent.click(refreshedSecondPair);
  expect(screen.queryAllByRole("region")).toHaveLength(0);
});

it("opens an individual position while keeping its disclosure and actions independent", () => {
  const onOpenPosition = vi.fn();
  render(
    <MakerPositions positions={[position]} onOpenPosition={onOpenPosition} />,
  );

  fireEvent.click(
    screen.getByRole("button", {
      name: "Open LINK/USDC position position-a",
    }),
  );
  expect(onOpenPosition).toHaveBeenCalledExactlyOnceWith("position-a");

  fireEvent.click(
    screen.getByRole("button", {
      name: "Collapse LINK/USDC position position-a",
    }),
  );
  expect(
    screen.queryByRole("region", { name: "Position position-a details" }),
  ).not.toBeInTheDocument();
  expect(onOpenPosition).toHaveBeenCalledTimes(1);

  fireEvent.click(screen.getByRole("button", { name: "Push" }));
  expect(onOpenPosition).toHaveBeenCalledTimes(1);
});
