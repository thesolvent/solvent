import { describe, expect, it } from "vitest";

import {
    MAX_UINT64,
    MAX_UINT248,
    MAX_UINT256,
    InputValidationError,
    parseTokenAmount,
    validatedAddress,
    validatedFeeBps,
    validatedSalt,
    validatedUint,
} from "../src/validation";

describe("protocol input validation", () => {
    it.each([
        [".5", 6, 500_000n],
        ["1.", 6, 1_000_000n],
        [MAX_UINT256.toString(), 0, MAX_UINT256],
    ])("parses exact token amount %s", (value, decimals, expected) => {
        expect(parseTokenAmount(value, decimals)).toBe(expected);
    });

    it.each([
        ["", 6, "invalid_decimal"],
        ["1e3", 6, "invalid_decimal"],
        ["1.0000001", 6, "too_many_decimals"],
        ["0", 6, "not_positive"],
        [(MAX_UINT248 + 1n).toString(), 0, "out_of_range"],
    ])("rejects token amount %s", (value, decimals, code) => {
        try {
            parseTokenAmount(value, decimals, "deposit", MAX_UINT248);
            throw new Error("expected validation to fail");
        } catch (error) {
            expect(error).toBeInstanceOf(InputValidationError);
            expect((error as InputValidationError).code).toBe(code);
        }
    });

    it("accepts the exact public integer boundaries", () => {
        expect(validatedFeeBps(0)).toBe(0);
        expect(validatedFeeBps(9_999)).toBe(9_999);
        expect(validatedSalt(0n)).toBe(0n);
        expect(validatedSalt(MAX_UINT64)).toBe(MAX_UINT64);
        expect(validatedUint(MAX_UINT248, 248, "amount")).toBe(MAX_UINT248);
    });

    it.each([
        () => validatedFeeBps(10_000),
        () => validatedFeeBps(0.5),
        () => validatedSalt(-1n),
        () => validatedSalt(MAX_UINT64 + 1n),
        () => validatedUint(MAX_UINT256 + 1n, 256, "amount"),
        () => validatedAddress("0x0", "token"),
        () => validatedAddress("0x0000000000000000000000000000000000000000", "token"),
    ])("rejects a value outside its protocol domain", (validate) => {
        expect(validate).toThrow(InputValidationError);
    });
});
