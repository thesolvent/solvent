import type { Address } from "viem";

export interface FaucetResult {
  tokenCount: number;
  gasFunded: boolean;
}

export interface FaucetPort {
  fund(address: Address): Promise<FaucetResult>;
}
