import { ConnectButton } from "@rainbow-me/rainbowkit";
import { useState } from "react";

import { NAV } from "@/data";
import { useAppActions } from "@/state";

import styles from "./Header.module.css";

export function Header() {
  const { page, navTo } = useAppActions();
  // Inert in the design; kept local so the field still accepts input.
  const [query, setQuery] = useState("");

  return (
    <header className={styles.header}>
      <div className={styles.left}>
        <button
          type="button"
          className={styles.logo}
          onClick={() => navTo("Home")}
        >
          Solvent
        </button>
      </div>

      <nav className={styles.nav}>
        {NAV.map((label) => (
          <button
            key={label}
            type="button"
            className={styles.navItem}
            onClick={() => navTo(label)}
          >
            <span
              className={
                label === page ? styles.navLabelActive : styles.navLabel
              }
            >
              {label}
            </span>
          </button>
        ))}
      </nav>

      <div className={styles.right}>
        <label className={styles.search}>
          <span className={styles.searchGlyph} />
          <input
            className={styles.searchInput}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search"
          />
        </label>
        <ConnectButton.Custom>
          {({
            account,
            chain,
            openConnectModal,
            openAccountModal,
            mounted,
          }) => {
            const connected = mounted && account && chain;
            return (
              <button
                type="button"
                className={styles.account}
                aria-label={connected ? "Wallet account" : "Connect wallet"}
                disabled={!mounted}
                onClick={connected ? openAccountModal : openConnectModal}
              >
                {connected ? (
                  <span className={styles.accountAddress}>
                    {account.displayName}
                  </span>
                ) : (
                  <span className={styles.accountConnect}>Connect</span>
                )}
              </button>
            );
          }}
        </ConnectButton.Custom>
      </div>
    </header>
  );
}
