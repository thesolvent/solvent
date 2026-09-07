import { FaucetBanner } from "@/components/FaucetBanner";
import { Header } from "@/components/Header";
import { AppProvider } from "@/AppProvider";
import { useApp } from "@/state";
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

function CurrentView() {
  const { page, state } = useApp();

  if (page === "Home") return <HomePage />;
  if (page === "Swap" || page === "Docs") return <SwapPage />;

  if (page === "Pools") {
    if (state.detail === null) return <PoolsPage />;
    return state.create ? <CreatePoolPage /> : <PoolDetailPage />;
  }

  if (page === "Makers") return <MakersPage />;

  if (page === "Explorer") {
    if (state.xpStrat !== null) return <StrategyPage />;
    if (state.xpTrade !== null) return <TradeDetailPage />;
    return <ExplorerPage />;
  }

  return null;
}

function Shell() {
  const { config, state } = useApp();

  return (
    <div className={styles.app}>
      {config.showFaucet && <FaucetBanner />}
      <Header />
      {state.wipe && <div className={styles.wipe} />}
      <CurrentView />
    </div>
  );
}

export function App() {
  return (
    <AppProvider>
      <Shell />
    </AppProvider>
  );
}
