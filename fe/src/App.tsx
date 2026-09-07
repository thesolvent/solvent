import { BrowserRouter, Navigate, Route, Routes } from "react-router-dom";

import { FaucetBanner } from "@/components/FaucetBanner";
import { Header } from "@/components/Header";
import { AppProvider } from "@/AppProvider";
import { useApp, useAppActions } from "@/state";
import { useAppSlice } from "@/store";
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

/** Sub-views of a page are still stack-driven, so each route resolves its own. */
function PoolsRoute() {
  const { state } = useApp();

  if (state.detail === null) return <PoolsPage />;
  return state.create ? <CreatePoolPage /> : <PoolDetailPage />;
}

function ExplorerRoute() {
  const { state } = useApp();

  if (state.xpStrat !== null) return <StrategyPage />;
  if (state.xpTrade !== null) return <TradeDetailPage />;
  return <ExplorerPage />;
}

function Shell() {
  const { config } = useAppActions();
  const wipe = useAppSlice((state) => state.wipe);

  return (
    <div className={styles.app}>
      {config.showFaucet && <FaucetBanner />}
      <Header />
      {wipe && <div className={styles.wipe} />}
      <Routes>
        <Route path="/" element={<HomePage />} />
        <Route path="/swap" element={<SwapPage />} />
        <Route path="/docs" element={<SwapPage />} />
        <Route path="/pools" element={<PoolsRoute />} />
        <Route path="/makers" element={<MakersPage />} />
        <Route path="/explorer" element={<ExplorerRoute />} />
        <Route path="*" element={<Navigate to="/" replace />} />
      </Routes>
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
