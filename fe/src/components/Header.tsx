import { ConnectButton } from "@rainbow-me/rainbowkit";
import { useEffect, useRef, useState } from "react";

import { NAV } from "@/data";
import { useAppActions, type ProductMode } from "@/state";
import { useAppSlice } from "@/store";

import styles from "./Header.module.css";

export function Header() {
  const { page, navTo, set } = useAppActions();
  const productMode = useAppSlice((state) => state.productMode);
  // Inert in the design; kept local so the field still accepts input.
  const [query, setQuery] = useState("");
  const [productsOpen, setProductsOpen] = useState(false);
  const productMenu = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!productsOpen) return;
    const close = (event: MouseEvent) => {
      if (!productMenu.current?.contains(event.target as Node)) {
        setProductsOpen(false);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setProductsOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [productsOpen]);

  const selectProduct = (mode: ProductMode) => {
    set({ productMode: mode, pNet: "All networks" });
    setProductsOpen(false);
  };

  return (
    <header className={styles.header}>
      <div ref={productMenu} className={styles.left}>
        <button
          type="button"
          className={styles.productTrigger}
          aria-haspopup="menu"
          aria-expanded={productsOpen}
          onClick={() => setProductsOpen((open) => !open)}
        >
          <span>{productMode}</span>
          <span
            className={
              productsOpen ? styles.productCaretOpen : styles.productCaret
            }
            aria-hidden="true"
          />
        </button>
        {productsOpen && (
          <div className={styles.productMenu} role="menu" aria-label="Product">
            <div className={styles.productEyebrow}>Product</div>
            <ProductOption
              mode="Solvent"
              active={productMode === "Solvent"}
              description="Intent swaps and Aqua pools. One signature, the resolver finds the fill."
              onSelect={selectProduct}
            />
            <ProductOption
              mode="SolventX"
              active={productMode === "SolventX"}
              description="Cross-chain swaps with coordinated execution across networks."
              onSelect={selectProduct}
            />
            <div className={styles.productFoot}>
              Same account, same balances
            </div>
          </div>
        )}
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

function ProductOption({
  mode,
  active,
  description,
  onSelect,
}: {
  mode: ProductMode;
  active: boolean;
  description: string;
  onSelect: (mode: ProductMode) => void;
}) {
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={active}
      className={active ? styles.productOptionActive : styles.productOption}
      onClick={() => onSelect(mode)}
    >
      <span className={styles.productMark}>
        {mode === "Solvent" ? "S" : "X"}
      </span>
      <span className={styles.productCopy}>
        <span className={styles.productName}>{mode}</span>
        <span className={styles.productDescription}>{description}</span>
      </span>
      <span className={styles.productStatus}>{active ? "Active" : ""}</span>
    </button>
  );
}
