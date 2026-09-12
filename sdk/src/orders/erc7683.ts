import {
    encodeAbiParameters,
    getAddress,
    toHex,
    type Address,
    type Hex,
} from "viem";

import {
    assertDistinctAddresses,
    assertFutureDeadline,
    InputValidationError,
    validatedAddress,
    validatedUint,
} from "../validation";
import type { OrderApproval, SwapPermit } from "./index";

export interface Erc7683OrderVenue {
    chainId: number;
    settler: string;
    permit2: string;
}

export interface Erc7683OrderTerms {
    swapper: string;
    tokenIn: string;
    tokenOut: string;
    amountIn: bigint;
    /** The least output the taker will accept, after slippage. */
    minAmountOut: bigint;
    /** The exact executor fee returned by the ERC-7683 quote. */
    executorFee: bigint;
    /** Unix seconds after which the order may no longer be filled. */
    deadline: number;
    /** Permit2 unordered nonce; randomly generated when omitted. */
    nonce?: bigint;
}

export interface UnsignedErc7683Order {
    encodedOrder: Hex;
    permit: SwapPermit;
    approval: OrderApproval;
    deadline: number;
}

const orderComponents = [
    { name: "settler", type: "address" },
    { name: "user", type: "address" },
    { name: "chainId", type: "uint256" },
    { name: "inputToken", type: "address" },
    { name: "inputAmount", type: "uint256" },
    { name: "outputToken", type: "address" },
    { name: "outputAmount", type: "uint256" },
    { name: "recipient", type: "address" },
    { name: "executorFee", type: "uint256" },
    { name: "nonce", type: "uint256" },
    { name: "deadline", type: "uint256" },
] as const;

const permitTypes = {
    TokenPermissions: [
        { name: "token", type: "address" },
        { name: "amount", type: "uint256" },
    ],
    Solvent7683Order: orderComponents,
    PermitWitnessTransferFrom: [
        { name: "permitted", type: "TokenPermissions" },
        { name: "spender", type: "address" },
        { name: "nonce", type: "uint256" },
        { name: "deadline", type: "uint256" },
        { name: "witness", type: "Solvent7683Order" },
    ],
} as const;

/** Build the Permit2 witness consumed by SolventSameChainSettler. */
export function buildErc7683Order(
    venue: Erc7683OrderVenue,
    terms: Erc7683OrderTerms,
): UnsignedErc7683Order {
    const checkedVenue = validateVenue(venue);
    const checkedTerms = validateTerms(terms);
    const nonce = checkedTerms.nonce ?? randomNonce();
    const witness = {
        settler: checkedVenue.settler,
        user: checkedTerms.swapper,
        chainId: BigInt(checkedVenue.chainId),
        inputToken: checkedTerms.tokenIn,
        inputAmount: checkedTerms.amountIn,
        outputToken: checkedTerms.tokenOut,
        outputAmount: checkedTerms.minAmountOut,
        recipient: checkedTerms.swapper,
        executorFee: checkedTerms.executorFee,
        nonce,
        deadline: BigInt(checkedTerms.deadline),
    };
    return {
        encodedOrder: encodeAbiParameters(
            [{ type: "tuple", components: orderComponents }],
            [witness],
        ),
        permit: {
            domain: {
                name: "Permit2",
                chainId: checkedVenue.chainId,
                verifyingContract: checkedVenue.permit2,
            },
            types: permitTypes,
            primaryType: "PermitWitnessTransferFrom",
            message: {
                permitted: {
                    token: checkedTerms.tokenIn,
                    amount: checkedTerms.amountIn,
                },
                spender: checkedVenue.settler,
                nonce,
                deadline: BigInt(checkedTerms.deadline),
                witness,
            },
        },
        approval: {
            chainId: checkedVenue.chainId,
            owner: getAddress(checkedTerms.swapper),
            token: getAddress(checkedTerms.tokenIn),
            spender: getAddress(checkedVenue.permit2),
            amount: checkedTerms.amountIn,
        },
        deadline: checkedTerms.deadline,
    };
}

function validateVenue(venue: Erc7683OrderVenue): {
    chainId: number;
    settler: Address;
    permit2: Address;
} {
    if (!Number.isSafeInteger(venue.chainId) || venue.chainId <= 0) {
        throw new InputValidationError(
            "chain ID",
            "invalid_integer",
            "Chain ID must be a positive safe integer",
        );
    }
    return {
        chainId: venue.chainId,
        settler: validatedAddress(venue.settler, "ERC-7683 settler"),
        permit2: validatedAddress(venue.permit2, "Permit2"),
    };
}

function validateTerms(terms: Erc7683OrderTerms): Erc7683OrderTerms & {
    swapper: Address;
    tokenIn: Address;
    tokenOut: Address;
} {
    const swapper = validatedAddress(terms.swapper, "swapper");
    const tokenIn = validatedAddress(terms.tokenIn, "input token");
    const tokenOut = validatedAddress(terms.tokenOut, "output token");
    assertDistinctAddresses(tokenIn, tokenOut, "swap tokens");
    validatedUint(terms.amountIn, 256, "input amount", { positive: true });
    validatedUint(terms.minAmountOut, 256, "minimum output", {
        positive: true,
    });
    validatedUint(terms.executorFee, 256, "executor fee", { positive: true });
    if (terms.executorFee >= terms.amountIn) {
        throw new InputValidationError(
            "executor fee",
            "out_of_range",
            "Executor fee must be smaller than the input amount",
        );
    }
    if (terms.nonce !== undefined) validatedUint(terms.nonce, 256, "nonce");
    assertFutureDeadline(terms.deadline);
    return { ...terms, swapper, tokenIn, tokenOut };
}

function randomNonce(): bigint {
    return BigInt(toHex(crypto.getRandomValues(new Uint8Array(32))));
}
