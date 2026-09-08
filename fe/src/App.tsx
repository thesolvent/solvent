import { BrowserRouter, Navigate, Route } from "react-router-dom";

import { FaucetBanner } from "@/components/FaucetBanner";
import { Header } from "@/components/Header";
import { TransitionRoutes } from "@/components/TransitionRoutes";
import { AppProvider } from "@/AppProvider";
import { useApp, useAppActions } from "@/state";
import { CreatePoolPage } from "@/views/CreatePoolPage";
import { ExplorerPage } from "@/views/ExplorerPage";
import { HomePage } from "@/views/HomePage";
import { MakersPage } from "@/views/MakersPage";
import { PoolDetailPage } from "@/views/PoolDetailPage";
import { PoolsPage } from "@/views/PoolsPage";
import { StrategyPage } from "@/views/StrategyPage";
import { SwapPage } from "@/views/SwapPage";
import { TradeDetailPage } from "@/views/TradeDetailPage";

import styles from "./App.module.css";

/** Explorer's sub-views are still stack-driven, so its route resolves which one to show. */
function ExplorerRoute() {
  const { state } = useApp();

  if (state.xpStrat !== null) return <StrategyPage />;
  return <ExplorerPage />;
}

function Shell() {
  const { config } = useAppActions();

  return (
    <div className={styles.app}>
      {config.showFaucet && <FaucetBanner />}
      <Header />
      <TransitionRoutes>
        <Route path="/" element={<HomePage />} />
        <Route path="/swap" element={<SwapPage />} />
        <Route path="/docs" element={<SwapPage />} />
        <Route path="/pools" element={<PoolsPage />} />
        <Route path="/pools/:pair" element={<PoolDetailPage />} />
        <Route path="/pools/:pair/new" element={<CreatePoolPage />} />
        <Route path="/makers" element={<MakersPage />} />
        <Route path="/explorer" element={<ExplorerRoute />} />
        <Route path="/explorer/trades/:tradeId" element={<TradeDetailPage />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </TransitionRoutes>
    </div>
  );
}

export function App() {
  return (
    <BrowserRouter>
      <AppProvider>
        <Shell />
      </AppProvider>
    </BrowserRouter>
  );
}
