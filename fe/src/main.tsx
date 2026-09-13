import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { PrivyProvider } from "@privy-io/react-auth";
import { WagmiProvider } from "@privy-io/wagmi";

import "@fontsource/anton/400.css";
import "@fontsource-variable/manrope";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@fontsource/ibm-plex-mono/600.css";

import "./styles/tokens.css";
import "./styles/base.css";

import { httpServices } from "./adapters/http";
import { chain, destinationChain, wagmiConfig } from "./adapters/wallet/config";
import { WalletConfigurationError } from "./components/WalletConfigurationError";
import { ServicesProvider } from "./services/ServicesProvider";
import { App } from "./App";

const root = document.getElementById("root");
if (!root) throw new Error("#root missing from index.html");

const privyAppId = import.meta.env.VITE_PRIVY_APP_ID;

const queryClient = new QueryClient();

createRoot(root).render(
  privyAppId ? (
    <PrivyProvider
      appId={privyAppId}
      config={{
        supportedChains: [chain, destinationChain],
        defaultChain: chain,
        embeddedWallets: {
          ethereum: { createOnLogin: "users-without-wallets" },
          showWalletUIs: false,
        },
      }}
    >
      <QueryClientProvider client={queryClient}>
        <WagmiProvider config={wagmiConfig}>
          <StrictMode>
            <ServicesProvider services={httpServices} queryClient={queryClient}>
              <App />
            </ServicesProvider>
          </StrictMode>
        </WagmiProvider>
      </QueryClientProvider>
    </PrivyProvider>
  ) : (
    <WalletConfigurationError />
  ),
);
