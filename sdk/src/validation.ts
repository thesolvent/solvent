import {
    getAddress,
    isAddress,
    isAddressEqual,
    parseUnits,
    zeroAddress,
    type Address,
} from "viem";

export const MAX_UINT64 = (1n << 64n) - 1n;
export const MAX_UINT248 = (1n << 248n) - 1n;
export const MAX_UINT256 = (1n << 256n) - 1n;

const PLAIN_DECIMAL = /^(?:\d+(?:\.\d*)?|\.\d+)$/;

export type ValidationCode =
    | "invalid_address"
    | "zero_address"
    | "duplicate_address"
    | "mismatched_address"
    | "invalid_integer"
    | "out_of_range"
    | "invalid_decimal"
    | "too_many_decimals"
    | "not_positive"
    | "expired";

/** A stable validation failure suitable for form fields as well as SDK callers. */
export class InputValidationError extends RangeError {
    constructor(
        readonly field: string,
        readonly code: ValidationCode,
        message: string,
    ) {
        super(message);
        this.name = "InputValidationError";
    }
}

export function validatedAddress(value: string, field: string): Address {
    if (!isAddress(value, { strict: false })) {
        throw new InputValidationError(
            field,
            "invalid_address",
            `${field} must be a valid address`,
        );
    }
    const address = getAddress(value);
    if (isAddressEqual(address, zeroAddress)) {
        throw new InputValidationError(
            field,
            "zero_address",
            `${field} must not be the zero address`,
        );
    }
    return address;
}

export function assertDistinctAddresses(
    left: Address,
    right: Address,
    field: string,
): void {
    if (isAddressEqual(left, right)) {
        throw new InputValidationError(
            field,
            "duplicate_address",
            `${field} must contain distinct token addresses`,
        );
    }
}

export function validatedUint(
    value: bigint,
    bits: 64 | 248 | 256,
    field: string,
    options: { positive?: boolean } = {},
): bigint {
    if (typeof value !== "bigint") {
        throw new InputValidationError(
            field,
            "invalid_integer",
            `${field} must be an integer`,
        );
    }
    if (options.positive && value <= 0n) {
        throw new InputValidationError(
            field,
            "not_positive",
            `${field} must be greater than zero`,
        );
    }
    const maximum = uintMaximum(bits);
    if (value < 0n || value > maximum) {
        throw new InputValidationError(
            field,
            "out_of_range",
            `${field} must fit in uint${bits}`,
        );
    }
    return value;
}

export function validatedTokenDecimals(
    decimals: number,
    field = "token decimals",
): number {
    if (!Number.isInteger(decimals) || decimals < 0 || decimals > 255) {
        throw new InputValidationError(
            field,
            "out_of_range",
            `${field} must be an integer between 0 and 255`,
        );
    }
    return decimals;
}

/** Validate a plain positive decimal while preserving the caller's chosen formatting. */
export function validatedPositiveDecimal(value: string, field: string): string {
    validateDecimalSyntax(value, field);
    if (!/[1-9]/.test(value)) {
        throw new InputValidationError(
            field,
            "not_positive",
            `${field} must be greater than zero`,
        );
    }
    return value;
}

export function decimalValuesEqual(left: string, right: string): boolean {
    return normalizeDecimal(left) === normalizeDecimal(right);
}

/** Parse a human token amount without accepting exponent notation or excess precision. */
export function parseTokenAmount(
    value: string,
    decimals: number,
    field = "amount",
    maximum = MAX_UINT256,
): bigint {
    validatedTokenDecimals(decimals);
    const fractionDigits = validateDecimalSyntax(value, field);
    if (fractionDigits > decimals) {
        throw new InputValidationError(
            field,
            "too_many_decimals",
            `${field} supports at most ${decimals} decimal places`,
        );
    }
    let amount: bigint;
    try {
        amount = parseUnits(value, decimals);
    } catch {
        throw new InputValidationError(
            field,
            "invalid_decimal",
            `${field} must be a decimal number`,
        );
    }
    if (amount <= 0n) {
        throw new InputValidationError(
            field,
            "not_positive",
            `${field} must be greater than zero`,
        );
    }
    if (amount > maximum) {
        throw new InputValidationError(
            field,
            "out_of_range",
            `${field} exceeds the supported on-chain amount`,
        );
    }
    return amount;
}

export function validatedFeeBps(value: number): number {
    if (!Number.isInteger(value) || value < 0 || value > 9_999) {
        throw new InputValidationError(
            "fee",
            "out_of_range",
            "Fee must resolve to an integer from 0 to 9,999 basis points",
        );
    }
    return value;
}

export function validatedSalt(value: bigint): bigint {
    return validatedUint(value, 64, "salt");
}

function uintMaximum(bits: 64 | 248 | 256): bigint {
    switch (bits) {
        case 64:
            return MAX_UINT64;
        case 248:
            return MAX_UINT248;
        case 256:
            return MAX_UINT256;
    }
}

function validateDecimalSyntax(value: string, field: string): number {
    if (!PLAIN_DECIMAL.test(value)) {
        throw new InputValidationError(
            field,
            "invalid_decimal",
            `${field} must be a decimal number`,
        );
    }
    return value.split(".")[1]?.length ?? 0;
}

function normalizeDecimal(value: string): string {
    const [integer = "0", fraction = ""] = value.split(".");
    const whole = integer.replace(/^0+(?=\d)/, "");
    const decimal = fraction.replace(/0+$/, "");
    return decimal ? `${whole || "0"}.${decimal}` : whole || "0";
}
