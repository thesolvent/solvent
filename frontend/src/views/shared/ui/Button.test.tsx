import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { Button } from "./Button";

describe("Button", () => {
  it("renders its label and defaults to the primary (lime) variant", () => {
    render(<Button>Swap</Button>);
    const el = screen.getByRole("button", { name: "Swap" });
    expect(el).toBeInTheDocument();
    expect(el).toHaveClass("bg-lime");
  });

  it("applies the secondary variant", () => {
    render(<Button variant="secondary">Send</Button>);
    expect(screen.getByRole("button", { name: "Send" })).toHaveClass("bg-ink");
  });
});
