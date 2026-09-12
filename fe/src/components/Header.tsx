import { useExportWallet, usePrivy, useWallets } from "@privy-io/react-auth";
import { useEffect, useRef, useState } from "react";
import { useAccount, useDisconnect } from "wagmi";

import solventMarkActive from "@/assets/solvent-mark-active.svg";
import solventMarkInactive from "@/assets/solvent-mark-inactive.svg";
import solventXMarkActive from "@/assets/solventx-mark-active.svg";
import solventXMarkInactive from "@/assets/solventx-mark-inactive.svg";
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
  const [accountOpen, setAccountOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const [accountProblem, setAccountProblem] = useState<string>();
  const productMenu = useRef<HTMLDivElement>(null);
  const accountMenu = useRef<HTMLDivElement>(null);
  const { ready, authenticated, connectOrCreateWallet, logout } = usePrivy();
  const { exportWallet } = useExportWallet();
  const { wallets } = useWallets();
  const { address } = useAccount();
  const { disconnect } = useDisconnect();
  // wagmi's active wallet is the signer the application will use.
  const connected = address !== undefined;
  const embeddedWallet = wallets.find(
    (wallet) =>
      wallet.walletClientType === "privy" &&
      wallet.address.toLowerCase() === address?.toLowerCase(),
  );

  useEffect(() => {
    if (!productsOpen && !accountOpen) return;
    const close = (event: MouseEvent) => {
      const target = event.target as Node;
      if (!productMenu.current?.contains(target)) setProductsOpen(false);
      if (!accountMenu.current?.contains(target)) setAccountOpen(false);
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setProductsOpen(false);
      setAccountOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [productsOpen, accountOpen]);

  const selectProduct = (mode: ProductMode) => {
    set({ productMode: mode, pNet: "All networks" });
    setProductsOpen(false);
  };

  const copyAddress = async () => {
    if (!address) return;
    try {
      await navigator.clipboard.writeText(address);
      setCopied(true);
    } catch {
      setCopied(false);
    }
  };

  const disconnectWallet = () => {
    setAccountOpen(false);
    setAccountProblem(undefined);
    // MetaMask retains site permission, but this ends Solvent's active signer session.
    disconnect();
    void logout().catch(() => undefined);
  };

  const exportEmbeddedWallet = () => {
    if (!authenticated || !embeddedWallet) return;
    setAccountOpen(false);
    setAccountProblem(undefined);
    void exportWallet({ address: embeddedWallet.address }).catch(() => {
      setAccountProblem("Wallet export was not opened. Try again.");
      setAccountOpen(true);
    });
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
            <ProductOption
              mode="Solvent"
              active={productMode === "Solvent"}
              description="Same-chain intent swaps, powered by Aqua."
              onSelect={selectProduct}
            />
            <ProductOption
              mode="SolventX"
              active={productMode === "SolventX"}
              description="Cross-chain intent swaps, built on Aqua + Compact + CCTP + CCIP"
              onSelect={selectProduct}
            />
          </div>
        )}
      </div>

      <nav className={styles.nav} aria-label="Primary">
        {NAV.map((label) => (
          <button
            key={label}
            type="button"
            className={styles.navItem}
            aria-current={label === page ? "page" : undefined}
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
            aria-label="Search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search"
          />
        </label>
        <div ref={accountMenu} className={styles.accountMenuWrap}>
          <button
            type="button"
            className={styles.account}
            aria-label={connected ? "Wallet account" : "Connect wallet"}
            aria-haspopup={connected ? "menu" : undefined}
            aria-expanded={connected ? accountOpen : undefined}
            disabled={!ready}
            onClick={
              connected
                ? () => {
                    setAccountOpen((open) => !open);
                    setCopied(false);
                    setAccountProblem(undefined);
                  }
                : connectOrCreateWallet
            }
          >
            {connected ? (
              <span className={styles.accountAddress}>
                {shortAddress(address)}
              </span>
            ) : (
              <span className={styles.accountConnect}>Connect</span>
            )}
          </button>
          {connected && accountOpen && (
            <div
              className={styles.accountMenu}
              role="menu"
              aria-label="Wallet account"
            >
              <button
                type="button"
                className={styles.accountMenuAddress}
                role="menuitem"
                onClick={() => void copyAddress()}
              >
                <span>{shortAddress(address)}</span>
                <span>{copied ? "Copied" : "Copy address"}</span>
              </button>
              {authenticated && embeddedWallet && (
                <button
                  type="button"
                  className={styles.accountMenuExport}
                  role="menuitem"
                  onClick={exportEmbeddedWallet}
                >
                  Export wallet
                </button>
              )}
              <button
                type="button"
                className={styles.accountMenuDisconnect}
                role="menuitem"
                onClick={disconnectWallet}
              >
                Disconnect
              </button>
              {accountProblem && (
                <p className={styles.accountMenuProblem} role="alert">
                  {accountProblem}
                </p>
              )}
            </div>
          )}
        </div>
      </div>
    </header>
  );
}

function shortAddress(address: `0x${string}` | undefined) {
  if (!address) return "";
  return `${address.slice(0, 6)}...${address.slice(-4)}`;
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
  const mark =
    mode === "Solvent"
      ? active
        ? solventMarkActive
        : solventMarkInactive
      : active
        ? solventXMarkActive
        : solventXMarkInactive;
  return (
    <button
      type="button"
      role="menuitemradio"
      aria-checked={active}
      className={active ? styles.productOptionActive : styles.productOption}
      onClick={() => onSelect(mode)}
    >
      <span className={styles.productMark} aria-hidden="true">
        <img src={mark} alt="" />
      </span>
      <span className={styles.productCopy}>
        <span className={styles.productName}>{mode}</span>
        <span className={styles.productDescription}>{description}</span>
      </span>
    </button>
  );
}
