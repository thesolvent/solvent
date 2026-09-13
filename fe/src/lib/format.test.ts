import { describe, expect, it } from "vitest";

import { fit } from "./format";

describe("fit", () => {
  it("holds a stable size within a band and steps down at its boundary", () => {
    const firstBand = ["1", "10", "100", "100.", "100.5", "1234567"].map(fit);
    expect(new Set(firstBand).size).toBe(1);
    expect(fit("1234567")).not.toBe(fit("12345678"));
    expect(fit("12345678")).toBe(fit("123456789"));
  });

  it("keeps shrinking after the last fixed band", () => {
    expect(fit("1234567890123")).not.toBe(fit("123456789012345678"));
  });
});
