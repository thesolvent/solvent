import "@rainbow-me/rainbowkit/styles.css";

import { RainbowKitProvider } from "@rainbow-me/rainbowkit";
import { QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { WagmiProvider } from "wagmi";

import { ServicesContext } from "@/application/services";
import { WalletContext } from "@/application/wallet";
import { faucet } from "@/infrastructure/faucet";
import { queryClient } from "@/infrastructure/query-client";
import { solventClient } from "@/infrastructure/solvent-client";
import { wagmiConfig } from "@/infrastructure/wagmi";
import { useWagmiWallet } from "@/infrastructure/wallet";

const services = { client: solventClient, faucet };

// Bridges wagmi's account state (only available inside WagmiProvider) into the app's Wallet port.
function WalletBridge({ children }: { children: ReactNode }) {
  const wallet = useWagmiWallet();
  return (
    <WalletContext.Provider value={wallet}>{children}</WalletContext.Provider>
  );
}

export function AppProviders({ children }: { children: ReactNode }) {
  return (
    <WagmiProvider config={wagmiConfig}>
      <QueryClientProvider client={queryClient}>
        <RainbowKitProvider>
          <ServicesContext.Provider value={services}>
            <WalletBridge>{children}</WalletBridge>
          </ServicesContext.Provider>
        </RainbowKitProvider>
      </QueryClientProvider>
    </WagmiProvider>
  );
}
