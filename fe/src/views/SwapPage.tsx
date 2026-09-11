import { useConnectModal } from "@rainbow-me/rainbowkit";
import { useEffect, useMemo } from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { useAccount, useSwitchChain } from "wagmi";

import { DASH } from "@/data";
import { fit, money } from "@/lib/format";
import {
  ANY_NETWORK,
  ANY_TAG,
  choices,
  networkOptions,
  settleLegs,
  swapAction,
  tagOptions,
} from "@/lib/swap";
import { useAssets } from "@/services/assets";
import { useQuote } from "@/services/quote";
import { useSubmitSwap } from "@/services/swap";
import { chain } from "@/adapters/wallet/config";
import { useApp } from "@/state";

import styles from "./SwapPage.module.css";

const SWAP_TABS = ["Swap"];

export function SwapPage() {
  const { state, set, config } = useApp();
  const { pathname } = useLocation();
  const navigate = useNavigate();

  const assets = useAssets();
  const bySymbol = (symbol: string) => assets.find((a) => a.symbol === symbol);
  const from = bySymbol(state.fromToken);
  const to = bySymbol(state.toToken);

  useEffect(() => {
    const settled = settleLegs(assets, state.fromToken, state.toToken);
    if (settled) set(settled);
  }, [assets, state.fromToken, state.toToken, set]);

  const typed = state.amount;
  const hasAmount = typed.trim() !== "";
  const numericAmount = Number(typed);
  const amt = Number.isFinite(numericAmount) ? numericAmount : 0;
  const fromUsdNum = amt * (from?.price ?? 0);

  // The output is the server's price for this size, not the mid — it carries fee and impact.
  const { quote, pricing, problem } = useQuote(from, to, typed);
  // Nothing in means nothing out; anything else without a price is unknown, not zero.
  const outStr = quote?.amountOut ?? (hasAmount && amt > 0 && to ? DASH : "");
  const dotAt = outStr.indexOf(".");

  const routeStats = [
    {
      label: "Fills",
      value: quote
        ? `${quote.makersSourced} ${quote.makersSourced === 1 ? "maker" : "makers"}`
        : DASH,
    },
    { label: "Price impact", value: quote?.priceImpact ?? DASH },
    { label: "Max slippage", value: `${config.slippage}%` },
  ];

  const { isConnected, chainId } = useAccount();
  const { openConnectModal } = useConnectModal();
  const { switchChain } = useSwitchChain();
  const submission = useSubmitSwap(
    {
      from,
      to,
      amount: typed,
      quote,
      slippagePct: config.slippage,
    },
    ({ tradeId }) => {
      // BrowserRouter updates history before React renders a requested departure.
      if (window.location.pathname !== pathname) return;
      navigate(`/explorer/trades/${encodeURIComponent(tradeId)}`);
    },
  );

  const switchTo = isConnected && chainId !== chain.id ? chain.name : undefined;

  // One button, whichever of the three things is missing.
  const act = () => {
    if (!isConnected) return openConnectModal?.();
    if (switchTo) return switchChain({ chainId: chain.id });
    submission.send();
  };

  const action = to
    ? swapAction({
        connected: isConnected,
        switchTo,
        submitting: submission.submitting,
        submitted: submission.result !== undefined,
        amount: amt,
        pricing,
        quote,
        problem,
        submissionProblem: submission.problem,
      })
    : { label: "Select receive asset", ready: false };

  const matches = useMemo(() => {
    const q = state.pQuery.trim().toLowerCase();
    return choices(assets, state.picker ?? "from", state.fromToken).filter(
      (t) => {
        const okQ =
          !q ||
          t.symbol.toLowerCase().includes(q) ||
          t.name.toLowerCase().includes(q);
        const okTag = state.pTag === ANY_TAG || t.tags.indexOf(state.pTag) > -1;
        const okNet = state.pNet === ANY_NETWORK || t.net === state.pNet;
        return okQ && okTag && okNet;
      },
    );
  }, [
    assets,
    state.picker,
    state.fromToken,
    state.pQuery,
    state.pTag,
    state.pNet,
  ]);

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
      });
    } else {
      set({
        toToken: sym,
        fromToken: state.fromToken === sym ? state.toToken : state.fromToken,
        picker: null,
      });
    }
  };

  return (
    <div data-scroll="1" className={styles.root}>
      <section className={styles.card}>
        <div className={styles.cardHead}>
          <div className={styles.tabs}>
            {SWAP_TABS.map((t) => (
              <button key={t} type="button" className={styles.tabActive}>
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
            <span className={styles.assetSymbol}>
              {state.fromToken || "Select"}
            </span>
            <span className={styles.assetCaret}>▾</span>
          </button>
          <div className={styles.amountCol}>
            <div className={styles.amountLabel}>Swap from</div>
            <div className={styles.amountBox}>
              <input
                className={styles.amountInput}
                style={{ fontSize: fit(state.amount) }}
                value={state.amount}
                onChange={(e) => set({ amount: e.target.value })}
                placeholder="Enter amount"
                aria-label="Swap amount"
                inputMode="decimal"
                maxLength={258}
              />
            </div>
            {hasAmount && (
              <div className={styles.amountUsd}>~$ {money(fromUsdNum)}</div>
            )}
          </div>
        </div>

        <div className={styles.flipRail}>
          <button
            type="button"
            className={styles.flip}
            disabled={!from || !to}
            onClick={() =>
              set({
                fromToken: state.toToken,
                toToken: state.fromToken,
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
            <span className={styles.assetSymbol}>
              {state.toToken || "Select"}
            </span>
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
            {quote && (
              <div className={styles.amountUsd}>
                ~$ {money(quote.amountOutUsd)}
              </div>
            )}
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
          disabled={!action.ready}
          onClick={act}
        >
          {action.label}
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
                {tagOptions(assets).map((t) => (
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
              {networkOptions(assets).map((n) => (
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
    </div>
  );
}
