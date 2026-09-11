import { render, screen } from "@testing-library/react";
import { MemoryRouter, Routes } from "react-router-dom";
import { describe, expect, it } from "vitest";

import { routes } from "./route-table";

function renderAt(path: string) {
  return render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>{routes}</Routes>
    </MemoryRouter>,
  );
}

describe("route table", () => {
  it("answers an unknown path with not-found instead of redirecting home", () => {
    renderAt("/pols?from=email");
    expect(
      screen.getByRole("heading", { level: 1, name: "No such page" }),
    ).toBeInTheDocument();
    expect(screen.getByText("/pols")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to home" })).toHaveAttribute(
      "href",
      "/",
    );
  });

  it("serves no documentation, rather than a swap widget", () => {
    renderAt("/docs");
    expect(screen.getByText("No such page")).toBeInTheDocument();
  });
});
