import { BrowserRouter } from "react-router-dom";

import { FaucetBanner } from "@/components/FaucetBanner";
import { Header } from "@/components/Header";
import { TransitionRoutes } from "@/components/TransitionRoutes";
import { routes } from "@/route-table";
import { AppProvider } from "@/AppProvider";
import { useAppActions } from "@/state";

import styles from "./App.module.css";

function Shell() {
  const { config } = useAppActions();

  return (
    <div className={styles.app}>
      {config.showFaucet && <FaucetBanner />}
      <Header />
      <TransitionRoutes>{routes}</TransitionRoutes>
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
