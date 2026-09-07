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
        <div className={styles.currency}>
          <span className={styles.currencyGlyph} />
          <span>USD</span>
        </div>
        <div className={styles.account}>
          <span className={styles.accountGlyph} />
        </div>
      </div>
    </header>
  );
}
