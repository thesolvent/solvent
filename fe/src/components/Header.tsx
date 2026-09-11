import { ConnectButton } from "@rainbow-me/rainbowkit";
import { useState } from "react";

import { NAV } from "@/data";
import { useAppActions } from "@/state";

import { Icon } from "@/components/Icon";
import { TokenIcon } from "@/components/TokenIcon";
import { useConfig } from "@/services/system";

import styles from "./Header.module.css";

export function Header() {
  // A deployment whose server predates `chains` still answers `/config`, so the array itself has
  // to be guarded — not just the response.
  const chain = useConfig().data?.chains?.[0];
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
        {chain ? (
          <span className={styles.chain}>
            <TokenIcon
              className={styles.chainIcon}
              logoUri={chain.logo_uri}
              symbol={chain.name}
            />
            <span className={styles.chainName}>{chain.name}</span>
          </span>
        ) : null}
        <label className={styles.search}>
          <Icon className={styles.searchGlyph} name="search" />
          <input
            aria-label="Search"
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
