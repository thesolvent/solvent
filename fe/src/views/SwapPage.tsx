import { useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";

import { DASH, type Asset } from "@/data";
import { fit, money } from "@/lib/format";
import {
  ANY_NETWORK,
  ANY_TAG,
  assetKey,
  choices,
  networkOptions,
  selectedAsset,
  settleLegs,
  swapAction,
  tagOptions,
} from "@/lib/swap";
import { useAssets } from "@/services/assets";
import { useQuote } from "@/services/quote";
import { useSubmitSwap } from "@/services/swap";
import { useConfig } from "@/services/system";
import { type SwapProtocol, useApp } from "@/state";
import { AssetIdentity } from "@/components/AssetIdentity";
import { useAssetBalances, useWalletAction } from "@/services/wallet";

import styles from "./SwapPage.module.css";

/** Matches `.amountInput::placeholder`, so the caret is the height of the words it replaces. */
const PLACEHOLDER_SIZE = "clamp(30px, 8cqi, 42px)";

/**
 * Whether a keystroke leaves something that is still on its way to being a number.
 *
 * `inputMode` only hints at which keyboard to raise; it refuses nothing, so a typed letter
 * reaches the amount and every consumer downstream has to survive it. A partial entry — "", "0.",
 * "." — is accepted because it is a decimal mid-typing, not a wrong one.
 */
function isAmountDraft(value: string): boolean {
  return /^\d*\.?\d*$/.test(value);
}

type ProtocolOption = {
  value: SwapProtocol;
  label: string;
  description: string;
};

const PROTOCOL_OPTIONS = [
  {
    value: "uniswapx",
    label: "UniswapX",
    description: "Dutch-auction intent settlement",
  },
  {
    value: "erc7683",
    label: "ERC-7683",
    description: "Standardized same-chain order settlement",
  },
] satisfies readonly ProtocolOption[];

export function SwapPage() {
  const { state, set, config } = useApp();
  const [protocolMenuOpen, setProtocolMenuOpen] = useState(false);
  const crossChain = state.productMode === "SolventX";
  const runtimeConfig = useConfig();
  const erc7683Available = Boolean(runtimeConfig.data?.erc7683_settler);
  const protocol =
    crossChain || !erc7683Available ? "uniswapx" : state.swapProtocol;
  const protocolLabel =
    PROTOCOL_OPTIONS.find((option) => option.value === protocol)?.label ??
    protocol;
  const { pathname } = useLocation();
  const navigate = useNavigate();

  const assets = useAssets(crossChain);
  const from = selectedAsset(assets, state.fromToken);
  const to = selectedAsset(assets, state.toToken);

  useEffect(() => {
    const settled = settleLegs(
      assets,
      state.fromToken,
      state.toToken,
      crossChain,
    );
    if (settled) set(settled);
  }, [assets, crossChain, state.fromToken, state.toToken, set]);

  const typed = state.amount;
  const hasAmount = typed.trim() !== "";
  const numericAmount = Number(typed);
  const amt = Number.isFinite(numericAmount) ? numericAmount : 0;
  const fromUsdNum = amt * (from?.price ?? 0);

  // The output is the server's price for this size, not the mid — it carries fee and impact.
  const { quote, pricing, problem } = useQuote(from, to, typed, protocol);
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

  const wallet = useWalletAction();
  const balances = useAssetBalances(assets);
  const submission = useSubmitSwap(
    {
      from,
      to,
      amount: typed,
      quote,
      slippagePct: config.slippage,
      protocol,
    },
    ({ tradeId }) => {
      // BrowserRouter updates history before React renders a requested departure.
      if (window.location.pathname !== pathname) return;
      navigate(`/explorer/trades/${encodeURIComponent(tradeId)}`);
    },
  );

  // One button, whichever of the three things is missing.
  const act = () => {
    if (wallet.prepare()) submission.send();
  };

  const action = to
    ? swapAction({
        connected: wallet.connected,
        switchTo: wallet.switchTo,
        submitting: submission.submitting,
        submitted: submission.result !== undefined,
        amount: amt,
        pricing,
        quote,
        problem,
        submissionProblem: submission.problem,
        submissionStatus: submission.status,
        inputToken: from?.symbol,
      })
    : { label: "Select receive asset", ready: false };

  const matches = useMemo(() => {
    const q = state.pQuery.trim().toLowerCase();
    return choices(
      assets,
      state.picker ?? "from",
      state.fromToken,
      crossChain,
    ).filter((t) => {
      const okQ =
        !q ||
        t.symbol.toLowerCase().includes(q) ||
        t.name.toLowerCase().includes(q);
      const okTag = state.pTag === ANY_TAG || t.tags.indexOf(state.pTag) > -1;
      const okNet = state.pNet === ANY_NETWORK || t.net === state.pNet;
      return okQ && okTag && okNet;
    });
  }, [
    assets,
    state.picker,
    state.fromToken,
    state.pQuery,
    state.pTag,
    state.pNet,
    crossChain,
  ]);

  useEffect(() => {
    if (!state.picker) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") set({ picker: null });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [state.picker, set]);

  useEffect(() => {
    if (!protocolMenuOpen) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setProtocolMenuOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [protocolMenuOpen]);

  useEffect(() => {
    if (crossChain || !erc7683Available) setProtocolMenuOpen(false);
  }, [crossChain, erc7683Available]);

  // Picking the asset already on the other leg swaps the two rather than
  // leaving both legs on the same token.
  const choose = (asset: Asset) => {
    const selected = assetKey(asset);
    const other = selectedAsset(
      assets,
      state.picker === "from" ? state.toToken : state.fromToken,
    );
    const sameAsset = other && assetKey(other) === selected;
    if (state.picker === "from") {
      set({
        fromToken: selected,
        toToken: sameAsset ? state.fromToken : state.toToken,
        picker: null,
      });
    } else {
      set({
        toToken: selected,
        fromToken: sameAsset ? state.toToken : state.fromToken,
        picker: null,
      });
    }
  };

  return (
    <div data-scroll="1" className={styles.root}>
      <section className={styles.card}>
        <div className={styles.cardHead}>
          <div className={styles.headActions}>
            {!crossChain && erc7683Available && (
              <div className={styles.protocolMenu}>
                <button
                  type="button"
                  className={
                    protocolMenuOpen
                      ? styles.protocolTriggerOpen
                      : styles.protocolTrigger
                  }
                  aria-label="Select swap protocol"
                  aria-expanded={protocolMenuOpen}
                  aria-haspopup="menu"
                  onClick={() => setProtocolMenuOpen((open) => !open)}
                >
                  <span>{protocolLabel}</span>
                  <span
                    className={
                      protocolMenuOpen
                        ? styles.protocolTriggerMoreOpen
                        : styles.protocolTriggerMore
                    }
                    aria-hidden="true"
                  >
                    {protocolMenuOpen ? "▴" : "···"}
                  </span>
                </button>
                {protocolMenuOpen && (
                  <div className={styles.protocolOptions} role="menu">
                    {PROTOCOL_OPTIONS.map(({ value, label, description }) => (
                      <button
                        key={value}
                        type="button"
                        role="menuitemradio"
                        aria-checked={protocol === value}
                        className={
                          protocol === value
                            ? styles.protocolOptionActive
                            : styles.protocolOption
                        }
                        onClick={() => {
                          set({
                            swapProtocol: value,
                          });
                          setProtocolMenuOpen(false);
                        }}
                      >
                        <span className={styles.protocolCopy}>
                          <span className={styles.protocolName}>{label}</span>
                          <span className={styles.protocolDescription}>
                            {description}
                          </span>
                        </span>
                        <span
                          className={styles.protocolMark}
                          aria-hidden="true"
                        >
                          {protocol === value ? "✓" : ""}
                        </span>
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
            {(crossChain || !erc7683Available) && (
              <button type="button" className={styles.moreButton} disabled>
                ···
              </button>
            )}
          </div>
        </div>

        <div className={styles.leg}>
          <button
            type="button"
            className={styles.assetButton}
            onClick={() => set({ picker: "from", pQuery: "" })}
          >
            <span className={styles.assetSymbol}>
              {from?.symbol || "Select"}
            </span>
            <AssetIdentity asset={from} />
            <span className={styles.assetCaret}>▾</span>
          </button>
          <div className={styles.amountCol}>
            <div className={styles.amountLabel}>Swap from</div>
            <div className={styles.amountBox}>
              <input
                className={styles.amountInput}
                // An empty field still carries the largest size, and the caret is drawn at the
                // font size rather than the placeholder's, so it stands as tall as the box.
                style={{
                  fontSize: state.amount ? fit(state.amount) : PLACEHOLDER_SIZE,
                }}
                value={state.amount}
                onChange={(e) => {
                  if (isAmountDraft(e.target.value))
                    set({ amount: e.target.value });
                }}
                placeholder="Enter amount"
                aria-label="Swap amount"
                inputMode="decimal"
                maxLength={258}
              />
            </div>
            <div className={styles.amountUsd} aria-hidden={!hasAmount}>
              {hasAmount ? `~$ ${money(fromUsdNum)}` : "\u00a0"}
            </div>
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
            <span className={styles.assetSymbol}>{to?.symbol || "Select"}</span>
            <AssetIdentity asset={to} />
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
            <div className={styles.amountUsd} aria-hidden={!quote}>
              {quote ? `~$ ${money(quote.amountOutUsd)}` : "\u00a0"}
            </div>
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
          className={`${styles.cta} ${action.retry ? styles.ctaRetry : ""}`}
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
                  const selected = selectedAsset(
                    assets,
                    state.picker === "from" ? state.fromToken : state.toToken,
                  );
                  const active =
                    selected !== undefined &&
                    assetKey(selected) === assetKey(t);
                  const down = t.change.charAt(0) === "-";
                  const balance = balances.get(assetKey(t)) ?? DASH;
                  return (
                    <button
                      key={assetKey(t)}
                      type="button"
                      className={
                        active ? styles.tokenRowActive : styles.tokenRow
                      }
                      onClick={() => choose(t)}
                    >
                      <span className={styles.tokenChip}>
                        {t.symbol.slice(0, 2)}
                      </span>
                      <span className={styles.tokenMain}>
                        <span className={styles.tokenName}>{t.name}</span>
                        <span className={styles.tokenMeta}>
                          <span className={styles.tokenBalance}>
                            Balance {balance} {t.symbol}
                          </span>
                          <span aria-hidden="true"> · </span>
                          {t.net}
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

            {crossChain && (
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
            )}
          </div>
        )}
      </section>
    </div>
  );
}
