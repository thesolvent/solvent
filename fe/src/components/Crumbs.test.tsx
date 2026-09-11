import { screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { renderWithServices } from "@/test/harness";

import { Crumbs } from "./Crumbs";

describe("Crumbs", () => {
  it("marks the page you are on rather than offering it as a control", () => {
    renderWithServices(
      <Crumbs
        current="Trade 0x8f"
        trail={[{ label: "Explorer", to: "/explorer" }]}
      />,
    );
    const nav = screen.getByRole("navigation", { name: "Breadcrumb" });
    expect(nav).toContainElement(
      screen.getByRole("button", { name: "Explorer" }),
    );
    expect(screen.queryByRole("button", { name: "Trade 0x8f" })).toBeNull();
    const current = screen.getByText("Trade 0x8f");
    expect(current).toHaveAttribute("aria-current", "page");
    expect(current.tagName).toBe("SPAN");
  });
});
