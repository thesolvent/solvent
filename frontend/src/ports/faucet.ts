import type { Address } from "@/domain";

/** The devnet faucet behind the "Get test tokens" action. Resolves on a receipt-verified mint,
 *  throws on failure or cooldown. Devnet-only; absent in production builds. */
export interface Faucet {
  requestTokens(address: Address): Promise<void>;
}
