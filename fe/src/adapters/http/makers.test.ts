import { beforeEach, describe, expect, it, vi } from "vitest";

const api = vi.hoisted(() => ({
  primary: {
    position: vi.fn(),
    positionDepth: vi.fn(),
    positionHistory: vi.fn(),
  },
  destination: {
    position: vi.fn(),
    positionDepth: vi.fn(),
    positionHistory: vi.fn(),
  },
  direct: {
    position: vi.fn(),
    positionDepth: vi.fn(),
    positionHistory: vi.fn(),
  },
}));

vi.mock("./client", () => ({
  solventApi: api.primary,
  baseApi: api.destination,
  directDestinationApi: api.direct,
}));

beforeEach(() => vi.resetAllMocks());

describe("maker HTTP adapter", () => {
  it.each([
    [
      "position",
      () =>
        import("./makers").then(({ makersAdapter }) =>
          makersAdapter.position("0x01", 31338),
        ),
    ],
    [
      "positionDepth",
      () =>
        import("./makers").then(({ makersAdapter }) =>
          makersAdapter.depth(
            "0x01",
            {
              base: "0x0000000000000000000000000000000000000001",
              quote: "0x0000000000000000000000000000000000000002",
              baseDecimals: 18,
              quoteDecimals: 6,
            },
            31338,
          ),
        ),
    ],
    [
      "positionHistory",
      () =>
        import("./makers").then(({ makersAdapter }) =>
          makersAdapter.history("0x01", 31338),
        ),
    ],
  ] as const)("reads destination %s data from Base", async (method, read) => {
    const failure = new Error("destination request reached");
    api.destination[method].mockRejectedValue(failure);

    await expect(read()).rejects.toBe(failure);
    expect(api.destination[method]).toHaveBeenCalled();
    expect(api.primary[method]).not.toHaveBeenCalled();
  });
});

it("reads a direct-route strategy from the direct destination index", async () => {
  const failure = new Error("direct destination request reached");
  api.direct.position.mockRejectedValue(failure);

  const { makersAdapter } = await import("./makers");
  await expect(makersAdapter.position("0x01", 31338, "direct")).rejects.toBe(
    failure,
  );
  expect(api.direct.position).toHaveBeenCalledWith("0x01");
  expect(api.destination.position).not.toHaveBeenCalled();
});
