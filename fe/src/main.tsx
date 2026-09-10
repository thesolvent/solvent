import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import "@fontsource/anton/400.css";
import "@fontsource-variable/manrope";
import "@fontsource/ibm-plex-mono/400.css";
import "@fontsource/ibm-plex-mono/500.css";
import "@fontsource/ibm-plex-mono/600.css";

import "./styles/tokens.css";
import "./styles/base.css";

import "@rainbow-me/rainbowkit/styles.css";
import { RainbowKitProvider } from "@rainbow-me/rainbowkit";
import { WagmiProvider } from "wagmi";

import { httpServices } from "./adapters/http";
import { devnetConnector, wagmiConfig } from "./adapters/wallet/config";
import { DevnetWalletConnector } from "./adapters/wallet/DevnetWalletConnector";
import { ServicesProvider } from "./services/ServicesProvider";
import { App } from "./App";

const root = document.getElementById("root");
if (!root) throw new Error("#root missing from index.html");

createRoot(root).render(
  <StrictMode>
    <WagmiProvider config={wagmiConfig} reconnectOnMount={!devnetConnector}>
      <DevnetWalletConnector />
      <ServicesProvider services={httpServices}>
        <RainbowKitProvider>
          <App />
        </RainbowKitProvider>
      </ServicesProvider>
    </WagmiProvider>
  </StrictMode>,
);
