import { describe, expect, it } from "vitest";

import { fit } from "./format";

describe("fit", () => {
  it("holds one size while a figure grows inside its band", () => {
    // Someone typing "100.5" crosses none of the boundaries, so the type must not move.
    const typing = ["1", "10", "100", "100.", "100.5", "1234567"].map(fit);
    expect(new Set(typing).size).toBe(1);
  });

  it("steps down only at a band edge", () => {
    expect(fit("1234567")).not.toBe(fit("12345678"));
    expect(fit("12345678")).toBe(fit("123456789"));
    expect(fit("123456789")).not.toBe(fit("1234567890"));
  });

  it("keeps shrinking past the last band, where nothing else would fit", () => {
    expect(fit("1234567890123")).not.toBe(fit("123456789012345678"));
    expect(fit("")).toBe(fit("1"));
  });
});
