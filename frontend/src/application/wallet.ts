import { createContext, useContext } from "react";

import type { Wallet } from "@/ports/wallet";

/** Disconnected by default; `app/` provides the live wagmi-backed value. */
export const WalletContext = createContext<Wallet>({ isConnected: false });

export function useWallet(): Wallet {
  return useContext(WalletContext);
}
