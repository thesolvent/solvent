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
    const order = new V2DutchOrderBuilder(
        venue.chainId,
        venue.reactor,
        venue.permit2,
    )
        .cosigner(venue.cosigner)
        .swapper(terms.swapper)
        .nonce(
            big(
                terms.nonce ??
                    BigInt(toHex(crypto.getRandomValues(new Uint8Array(32)))),
            ),
        )
        .deadline(terms.deadline)
        .input({
            token: terms.tokenIn,
            startAmount: big(terms.amountIn),
            endAmount: big(terms.amountIn),
        })
        .output({
            token: terms.tokenOut,
            startAmount: big(terms.minAmountOut),
            endAmount: big(terms.minAmountOut),
            recipient: terms.swapper,
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
            domain: { ...payload.domain, chainId: venue.chainId },
        },
        approval: {
            chainId: venue.chainId,
            owner: getAddress(terms.swapper),
            token: getAddress(terms.tokenIn),
            spender: getAddress(venue.permit2),
            amount: terms.amountIn,
        },
        deadline: terms.deadline,
    };
}
