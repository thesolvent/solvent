import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { TokenIcon } from "./TokenIcon";

describe("TokenIcon", () => {
  it("falls back to initials for a token the list gave no icon", () => {
    render(<TokenIcon logoUri={null} symbol="usdc" />);

    expect(screen.getByText("US")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  it("falls back to initials when the icon fails to load", () => {
    render(<TokenIcon logoUri="https://example.invalid/x.png" symbol="WETH" />);

    const image = document.querySelector("img");
    expect(image).not.toBeNull();
    fireEvent.error(image!);

    expect(screen.getByText("WE")).toBeInTheDocument();
    expect(document.querySelector("img")).toBeNull();
  });

  // A blocked host hangs rather than 404s, so no error ever arrives and nothing tells us to
  // fall back. The initials have to already be on screen.
  it("shows initials while an icon that never answers is still pending", () => {
    render(<TokenIcon logoUri="https://blocked.invalid/x.png" symbol="WBTC" />);

    expect(document.querySelector("img")).not.toBeNull();
    expect(screen.getByText("WB")).toBeInTheDocument();
  });
});
