/**
 * `@solvent/sdk` — a client-side, non-custodial SDK for Solvent.
 *
 * Layout (built out across M5):
 *   construction/  pure strategy building     — wraps `@1inch/swap-vm-sdk`
 *   positions/     pure position calldata     — wraps `@1inch/aqua-sdk` + `viem`
 *   client/        the one I/O adapter        — a typed API client over a `Transport` port
 *
 * The two pure modules never touch the network and never hold keys; `positions`
 * returns unsigned `{ to, data, value }` calldata for the maker's own wallet to send.
 *
 * This entry also re-exposes the `viem` address and unit helpers the SDK is built
 * on, so consumers reach for them via `@solvent/sdk` rather than depending on
 * `viem`'s exact version directly.
 */
export { parseUnits, formatUnits, isAddress } from "viem";
export type { Address, Hex } from "viem";

export * from "./construction";
export * from "./positions";
