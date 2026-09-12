/**
 * The taker side: build and sign the order a swap is submitted as.
 *
 * A swap is a UniswapX V2 Dutch order the taker signs and the resolver cosigns, so the encoding and
 * the Permit2 witness are upstream's (`@uniswap/uniswapx-sdk`) rather than ours — this only fills
 * in what the quote and the deployment say.
 */
import { BigNumber } from "@ethersproject/bignumber";
import { _TypedDataEncoder } from "@ethersproject/hash";
import { V2DutchOrderBuilder } from "@uniswap/uniswapx-sdk";

import {
    getAddress,
    toHex,
    type Address,
    type Hex,
    type TypedData,
    type TypedDataDomain,
} from "viem";
import {
    assertFutureDeadline,
    assertDistinctAddresses,
    InputValidationError,
    validatedAddress,
    validatedUint,
} from "../validation";

export { assertFutureDeadline } from "../validation";

/** Upstream is ethers-typed; convert amounts at this boundary. */
const big = (amount: bigint) => BigNumber.from(amount.toString());

/** The addresses a taker's order must name, as `GET /v1/config` publishes them. */
export interface OrderVenue {
    chainId: number;
    reactor: string;
    permit2: string;
    cosigner: string;
}

export interface OrderTerms {
    swapper: string;
    tokenIn: string;
    tokenOut: string;
    amountIn: bigint;
    /** The least output the taker will accept, after slippage. */
    minAmountOut: bigint;
    /** Unix seconds after which the order may no longer be filled. */
    deadline: number;
    /** Permit2 unordered nonce; randomly generated when omitted. */
    nonce?: bigint;
}

/** A wallet-compatible EIP-712 payload; integer values are decimal strings. */
export interface SwapPermit {
    domain: TypedDataDomain;
    types: TypedData;
    primaryType: string;
    message: Record<string, unknown>;
}

export interface OrderApproval {
    owner: Address;
    chainId: number;
    token: Address;
    spender: Address;
    amount: bigint;
}

export interface UnsignedOrder {
    encodedOrder: Hex;
    permit: SwapPermit;
    approval: OrderApproval;
    deadline: number;
}

export * from "./erc7683";

type ValidatedOrderVenue = OrderVenue & {
    reactor: Address;
    permit2: Address;
    cosigner: Address;
};

type ValidatedOrderTerms = OrderTerms & {
    swapper: Address;
    tokenIn: Address;
    tokenOut: Address;
};

interface ValidatedSwapOrder {
    venue: ValidatedOrderVenue;
    terms: ValidatedOrderTerms;
}

/**
 * The order for one swap, unsigned.
 *
 * Input and output are quoted flat (start equals end): the resolver's cosignature carries the decay,
 * so the taker signs the bound it accepts rather than a curve of its own.
 */
export function buildSwapOrder(
    venue: OrderVenue,
    terms: OrderTerms,
): UnsignedOrder {
    const { venue: checkedVenue, terms: checkedTerms } = validateSwapOrder(
        venue,
        terms,
    );
    const order = new V2DutchOrderBuilder(
        checkedVenue.chainId,
        checkedVenue.reactor,
        checkedVenue.permit2,
    )
        .cosigner(checkedVenue.cosigner)
        .swapper(checkedTerms.swapper)
        .nonce(big(checkedTerms.nonce ?? randomNonce()))
        .deadline(checkedTerms.deadline)
        .input({
            token: checkedTerms.tokenIn,
            startAmount: big(checkedTerms.amountIn),
            endAmount: big(checkedTerms.amountIn),
        })
        .output({
            token: checkedTerms.tokenOut,
            startAmount: big(checkedTerms.minAmountOut),
            endAmount: big(checkedTerms.minAmountOut),
            recipient: checkedTerms.swapper,
        })
        .buildPartial();

    const permit = order.permitData();
    // Wallet RPC requires JSON integers; ethers BigNumbers otherwise become { type, hex } objects.
    const payload: SwapPermit = _TypedDataEncoder.getPayload(
        permit.domain,
        permit.types,
        permit.values,
    );
    return {
        encodedOrder: order.serialize() as Hex,
        permit: {
            ...payload,
            domain: { ...payload.domain, chainId: checkedVenue.chainId },
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

function validateSwapOrder(
    venue: OrderVenue,
    terms: OrderTerms,
): ValidatedSwapOrder {
    if (!Number.isSafeInteger(venue.chainId) || venue.chainId <= 0) {
        throw new InputValidationError(
            "chain ID",
            "invalid_integer",
            "Chain ID must be a positive safe integer",
        );
    }
    const reactor = validatedAddress(venue.reactor, "reactor");
    const permit2 = validatedAddress(venue.permit2, "Permit2");
    const cosigner = validatedAddress(venue.cosigner, "cosigner");
    const swapper = validatedAddress(terms.swapper, "swapper");
    const tokenIn = validatedAddress(terms.tokenIn, "input token");
    const tokenOut = validatedAddress(terms.tokenOut, "output token");
    assertDistinctAddresses(tokenIn, tokenOut, "swap tokens");
    validatedUint(terms.amountIn, 256, "input amount", { positive: true });
    validatedUint(terms.minAmountOut, 256, "minimum output", {
        positive: true,
    });
    if (terms.nonce !== undefined) {
        validatedUint(terms.nonce, 256, "nonce");
    }
    assertFutureDeadline(terms.deadline);
    return {
        venue: { ...venue, reactor, permit2, cosigner },
        terms: { ...terms, swapper, tokenIn, tokenOut },
    };
}

function randomNonce(): bigint {
    return BigInt(toHex(crypto.getRandomValues(new Uint8Array(32))));
}
