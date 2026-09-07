import type { Address } from "@/domain";

/** The connected wallet as the app consumes it. Connection/identity now; the signing + send
 *  surface (for the swap and maker write paths) is added to this port when those phases need it. */
export interface Wallet {
  address?: Address;
  chainId?: number;
  isConnected: boolean;
}
