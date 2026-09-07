import { useEffect, useMemo } from "react";

import { NETWORKS, TAGS, TOKENS } from "@/data";
import { clean, fit, money, price } from "@/lib/format";
import { useApp } from "@/state";

import styles from "./SwapPage.module.css";

const SWAP_TABS = ["Swap", "Send", "Buy"];

export function SwapPage() {
  const { state, set, config } = useApp();

  const amt = parseFloat(String(state.amount).replace(/,/g, "")) || 0;
  const fromUsdNum = amt * price(state.fromToken);
  const out = fromUsdNum / price(state.toToken);
  const decimals = out >= 1000 ? 2 : out >= 1 ? 4 : 6;
  const outStr = out.toLocaleString("en-US", {
    minimumFractionDigits: decimals,
    maximumFractionDigits: decimals,
  });
  const dotAt = outStr.indexOf(".");

  const routeStats = [
    { label: "Resolver", value: "Zero-inventory" },
    { label: "Fills", value: "3 makers" },
    { label: "Price impact", value: "0.04%" },
    { label: "Max slippage", value: `${config.slippage}%` },
  ];

  const matches = useMemo(() => {
    const q = state.pQuery.trim().toLowerCase();
    return TOKENS.filter((t) => {
      const okQ =
        !q ||
        t.symbol.toLowerCase().includes(q) ||
        t.name.toLowerCase().includes(q);
      const okTag = state.pTag === "All" || t.tags.indexOf(state.pTag) > -1;
      const okNet = state.pNet === "All networks" || t.net === state.pNet;
      return okQ && okTag && okNet;
    });
  }, [state.pQuery, state.pTag, state.pNet]);

  useEffect(() => {
    if (!state.picker) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") set({ picker: null });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [state.picker, set]);

  // Picking the asset already on the other leg swaps the two rather than
  // leaving both legs on the same token.
  const choose = (sym: string) => {
    if (state.picker === "from") {
      set({
        fromToken: sym,
        toToken: state.toToken === sym ? state.fromToken : state.toToken,
        picker: null,
        swapped: false,
      });
    } else {
      set({
        toToken: sym,
        fromToken: state.fromToken === sym ? state.toToken : state.fromToken,
        picker: null,
        swapped: false,
      });
    }
  };

  return (
    <div data-scroll="1" className={styles.root}>
      <section className={styles.card}>
        <div className={styles.cardHead}>
          <div className={styles.tabs}>
            {SWAP_TABS.map((t) => (
              <button
                key={t}
                type="button"
                className={t === state.swapTab ? styles.tabActive : styles.tab}
                onClick={() => set({ swapTab: t })}
              >
                {t}
              </button>
            ))}
          </div>
          <div className={styles.headActions}>
            <button type="button" className={styles.iconButton}>
              <span className={styles.iconGlyph} />
            </button>
            <button type="button" className={styles.moreButton}>
              ···
            </button>
          </div>
        </div>

        <div className={styles.leg}>
          <button
            type="button"
            className={styles.assetButton}
            onClick={() => set({ picker: "from", pQuery: "" })}
          >
            <span className={styles.assetChip}>
              {state.fromToken.slice(0, 2)}
            </span>
            <span className={styles.assetSymbol}>{state.fromToken}</span>
            <span className={styles.assetCaret}>▾</span>
          </button>
          <div className={styles.amountCol}>
            <div className={styles.amountLabel}>Swap from</div>
            <div className={styles.amountBox}>
              <input
                className={styles.amountInput}
                style={{ fontSize: fit(state.amount) }}
                value={state.amount}
                onChange={(e) =>
                  set({
                    amount: clean(e.target.value),
                    swapped: false,
                  })
                }
                inputMode="decimal"
                maxLength={16}
              />
            </div>
            <div className={styles.amountUsd}>~$ {money(fromUsdNum)}</div>
          </div>
        </div>

        <div className={styles.flipRail}>
          <button
            type="button"
            className={styles.flip}
            onClick={() =>
              set({
                fromToken: state.toToken,
                toToken: state.fromToken,
                swapped: false,
              })
            }
          >
            ⇅
          </button>
        </div>

        <div className={styles.legTo}>
          <button
            type="button"
            className={styles.assetButton}
            onClick={() => set({ picker: "to", pQuery: "" })}
          >
            <span className={styles.assetChip}>
              {state.toToken.slice(0, 2)}
            </span>
            <span className={styles.assetSymbol}>{state.toToken}</span>
            <span className={styles.assetCaret}>▾</span>
          </button>
          <div className={styles.amountCol}>
            <div className={styles.amountLabel}>Swap to</div>
            <div className={styles.amountBox}>
              <div
                className={styles.amountOut}
                style={{ fontSize: fit(outStr) }}
              >
                <span>{dotAt > -1 ? outStr.slice(0, dotAt) : outStr}</span>
                <span className={styles.amountFraction}>
                  {dotAt > -1 ? outStr.slice(dotAt) : ""}
                </span>
              </div>
            </div>
            <div className={styles.amountUsd}>~$ {money(fromUsdNum)}</div>
          </div>
        </div>

        {config.showResolverRoute && (
          <div className={styles.route}>
            {routeStats.map((s) => (
              <div key={s.label} className={styles.routeCell}>
                <div className={styles.routeLabel}>{s.label}</div>
                <div className={styles.routeValue}>{s.value}</div>
              </div>
            ))}
          </div>
        )}

        <button
          type="button"
          className={styles.cta}
          onClick={() => set({ swapped: true })}
        >
          {state.swapped ? "Intent submitted to Aqua" : "Swap"}
        </button>

        {state.picker && (
          <div className={styles.veil}>
            <div className={styles.sheet}>
              <div className={styles.sheetHead}>
                <span className={styles.sheetTitle}>
                  {state.picker === "to" ? "Receive asset" : "Pay asset"}
                </span>
                <button
                  type="button"
                  className={styles.close}
                  onClick={() => set({ picker: null })}
                >
                  ×
                </button>
              </div>

              <label className={styles.searchField}>
                <span className={styles.searchGlyph} />
                <input
                  className={styles.searchInput}
                  value={state.pQuery}
                  onChange={(e) => set({ pQuery: e.target.value })}
                  placeholder="Search name or paste address"
                  autoFocus
                />
              </label>

              <div className={styles.tagRow}>
                {TAGS.map((t) => (
                  <button
                    key={t}
                    type="button"
                    className={
                      t === state.pTag ? styles.tagChipActive : styles.tagChip
                    }
                    onClick={() => set({ pTag: t })}
                  >
                    {t}
                  </button>
                ))}
              </div>

              <div data-scroll="1" className={styles.tokenList}>
                {matches.map((t) => {
                  const active =
                    (state.picker === "from"
                      ? state.fromToken
                      : state.toToken) === t.symbol;
                  const down = t.change.charAt(0) === "-";
                  return (
                    <button
                      key={t.symbol}
                      type="button"
                      className={
                        active ? styles.tokenRowActive : styles.tokenRow
                      }
                      onClick={() => choose(t.symbol)}
                    >
                      <span className={styles.tokenChip}>
                        {t.symbol.slice(0, 2)}
                      </span>
                      <span className={styles.tokenMain}>
                        <span className={styles.tokenName}>{t.name}</span>
                        <span className={styles.tokenMeta}>
                          {t.symbol} · {t.net}
                        </span>
                      </span>
                      <span className={styles.tokenPrices}>
                        <span className={styles.tokenUsd}>
                          $
                          {t.price.toLocaleString("en-US", {
                            minimumFractionDigits: 2,
                            maximumFractionDigits: 2,
                          })}
                        </span>
                        <span
                          className={
                            down ? styles.tokenChangeDown : styles.tokenChange
                          }
                        >
                          {t.change}
                        </span>
                      </span>
                      <span className={styles.tokenMark}>
                        {active ? "✓" : ""}
                      </span>
                    </button>
                  );
                })}
                {matches.length === 0 && (
                  <div className={styles.empty}>
                    No assets match that filter.
                  </div>
                )}
              </div>
            </div>

            <div className={styles.netCol}>
              <div className={styles.netHead}>Network</div>
              {NETWORKS.map((n) => (
                <button
                  key={n}
                  type="button"
                  className={
                    n === state.pNet ? styles.netRowActive : styles.netRow
                  }
                  onClick={() => set({ pNet: n })}
                >
                  <span className={styles.netDot} />
                  <span className={styles.netLabel}>{n}</span>
                  <span className={styles.netMark}>
                    {n === state.pNet ? "✓" : ""}
                  </span>
                </button>
              ))}
            </div>
          </div>
        )}
      </section>

      <p className={styles.footnote}>
        *Resolver rewards are non-monetary points and hold no direct value
      </p>
    </div>
  );
}
