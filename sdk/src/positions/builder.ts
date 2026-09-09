import {
    ABI,
    Address as SdkAddress,
    AquaProtocolContract,
    HexString,
} from "@1inch/aqua-sdk";
import { encodeFunctionData, erc20Abi, type Address, type Hex } from "viem";

/** An unsigned transaction request the maker's wallet signs and sends. */
export interface TxRequest {
    to: Address;
    data: Hex;
    value: bigint;
}

/** The four position actions, bound to one Aqua deployment (`aqua`) and app/router (`app`). */
export interface Positions {
    /** ERC-20 approve letting the Aqua contract pull `amount` of `token`. */
    approve(p: { token: Address; amount: bigint }): TxRequest;
    /** Ship a strategy: deposit `amounts` backing `strategy` (a `Strategy.build().order`). */
    ship(p: {
        strategy: Hex;
        amounts: readonly { token: Address; amount: bigint }[];
    }): TxRequest;
    /** Dock (close) a strategy: withdraw all `tokens`. */
    dock(p: { strategyHash: Hex; tokens: Address[] }): TxRequest;
    /** Push (top up) more of `token` into an active strategy. */
    push(p: {
        maker: Address;
        strategyHash: Hex;
        token: Address;
        amount: bigint;
    }): TxRequest;
}

/** Bind the position builders to one Aqua deployment. Addresses come from `GET /config`. */
export function positions(config: { aqua: Address; app: Address }): Positions {
    const app = new SdkAddress(config.app);
    const contract = new AquaProtocolContract(new SdkAddress(config.aqua));

    return {
        approve({ token, amount }) {
            const data = encodeFunctionData({
                abi: erc20Abi,
                functionName: "approve",
                args: [config.aqua, amount],
            });
            return { to: token, data, value: 0n };
        },

        ship({ strategy, amounts }) {
            return toTxRequest(
                contract.ship({
                    app,
                    strategy: new HexString(strategy),
                    amountsAndTokens: amounts.map((a) => ({
                        amount: a.amount,
                        token: new SdkAddress(a.token),
                    })),
                }),
            );
        },

        dock({ strategyHash, tokens }) {
            return toTxRequest(
                contract.dock({
                    app,
                    strategyHash: new HexString(strategyHash),
                    tokens: tokens.map((t) => new SdkAddress(t)),
                }),
            );
        },

        // Aqua's `push` has no SDK builder, so encode it directly against the shipped ABI.
        push({ maker, strategyHash, token, amount }) {
            const data = encodeFunctionData({
                abi: ABI.AQUA_ABI,
                functionName: "push",
                args: [maker, config.app, strategyHash, token, amount],
            });
            return { to: config.aqua, data, value: 0n };
        },
    };
}

function toTxRequest(ci: { to: Hex; data: Hex; value: bigint }): TxRequest {
    return { to: ci.to as Address, data: ci.data, value: ci.value };
}
