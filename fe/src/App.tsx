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
      <a className={styles.skipLink} href="#main">
        Skip to main content
      </a>
      {config.showFaucet && <FaucetBanner />}
      <Header />
      {/* `display: contents` keeps the landmark out of the shell's flex layout. */}
      <main id="main" className={styles.main} tabIndex={-1}>
        <TransitionRoutes>{routes}</TransitionRoutes>
      </main>
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
