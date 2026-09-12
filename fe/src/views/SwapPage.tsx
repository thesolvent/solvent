import { useEffect, useMemo, useState } from "react";
import { useLocation, useNavigate } from "react-router-dom";

import { DASH, type Asset } from "@/data";
import { fit, usd } from "@/lib/format";
import {
  ANY_NETWORK,
  ANY_TAG,
  assetKey,
  choices,
  impactLevel,
  isAboveBalance,
  isAmountDraft,
  minimumReceived,
  chainLogo,
  chainOptions,
  selectedAsset,
  settleLegs,
  swapAction,
  tagOptions,
  type ImpactLevel,
} from "@/lib/swap";
import { useAssets } from "@/services/assets";
import { AsyncNote } from "@/components/AsyncNote";
import { useQuote } from "@/services/quote";
import { useTokenBalance } from "@/services/balance";
import { useSubmitSwap } from "@/services/swap";
import { useConfig } from "@/services/system";
import { type SwapProtocol, useApp } from "@/state";
import { useAssetBalances, useWalletAction } from "@/services/wallet";
import { AssetIdentity, TokenMark } from "@/components/AssetIdentity";
import { Term } from "@/components/Tooltip";
import type { GlossaryKey } from "@/lib/glossary";

import styles from "./SwapPage.module.css";

/** Matches `.amountInput::placeholder`, so the caret is the height of the words it replaces. */
const PLACEHOLDER_SIZE = "clamp(30px, 8cqi, 42px)";

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

  const assetsQuery = useAssets(crossChain);
  // A fresh [] on every render would restart the leg-settling effect below on every render.
  const assets = useMemo(() => assetsQuery.data ?? [], [assetsQuery.data]);
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
  // An unpriced token has no dollar value to show; zero would read as worthless.
  const fromUsdNum = from?.price == null ? null : amt * from.price;

  // The output is the server's price for this size, not the mid — it carries fee and impact.
  const { quote, pricing, problem, stale } = useQuote(
    from,
    to,
    typed,
    protocol,
  );
  // Nothing in means nothing out; anything else without a price is unknown, not zero.
  const outStr = quote?.amountOut ?? (hasAmount && amt > 0 && to ? DASH : "");
  const dotAt = outStr.indexOf(".");

  const impact = impactLevel(quote?.priceImpact);
  const routeStats: {
    label: string;
    term: GlossaryKey;
    value: string;
    impact?: ImpactLevel;
  }[] = [
    {
      label: "Fills",
      term: "fills",
      value: quote
        ? `${quote.makersSourced} ${quote.makersSourced === 1 ? "maker" : "makers"}`
        : DASH,
    },
    {
      label: "Price impact",
      term: "priceImpact",
      value: quote?.priceImpact ?? DASH,
      impact,
    },
    {
      // The slippage percentage is the setting; this is what it costs at worst.
      label: "Min received",
      term: "minimumReceived",
      value:
        quote && to
          ? minimumReceived(quote, to.decimals, config.slippage)
          : DASH,
    },
    {
      label: "Max slippage",
      term: "maxSlippage",
      value: `${config.slippage}%`,
    },
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

  const balance = useTokenBalance(from);
  const short =
    from && isAboveBalance(typed, from.decimals, balance)
      ? from.symbol
      : undefined;

  const [impactAcknowledged, setImpactAcknowledged] = useState(false);
  useEffect(() => {
    setImpactAcknowledged(false);
  }, [state.fromToken, state.toToken, typed]);

  // One button, whichever step the trade is missing. `prepare` connects or switches chain and
  // reports whether the trade can go ahead now.
  const act = () => {
    if (!wallet.prepare()) return;
    if (impact === "severe" && !impactAcknowledged)
      return setImpactAcknowledged(true);
    submission.send();
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
        short,
        impact,
        impactAcknowledged,
        submissionStatus: submission.status,
        inputToken: from?.symbol,
      })
    : { label: "Select receive asset", ready: false };

  const offered = useMemo(
    () => choices(assets, state.picker ?? "from", state.fromToken, crossChain),
    [assets, state.picker, state.fromToken, crossChain],
  );
  const chains = useMemo(() => chainOptions(offered), [offered]);
  // A chain the current leg cannot reach must not stay selected from the previous one.
  const activeChain = chains.includes(state.pNet) ? state.pNet : chains[0];

  const matches = useMemo(() => {
    const q = state.pQuery.trim().toLowerCase();
    return offered.filter((t) => {
      const okQ =
        !q ||
        t.symbol.toLowerCase().includes(q) ||
        t.name.toLowerCase().includes(q);
      const okTag = state.pTag === ANY_TAG || t.tags.indexOf(state.pTag) > -1;
      const okNet = activeChain === ANY_NETWORK || t.net === activeChain;
      return okQ && okTag && okNet;
    });
  }, [offered, activeChain, state.pQuery, state.pTag]);

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
      <h1 className="srOnly">Swap</h1>
      <section className={styles.card} aria-label="Swap">
        <div className={styles.cardHead}>
          <div className={styles.headActions}>
            {!crossChain && erc7683Available ? (
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
            ) : (
              /* Nothing to choose here — cross-chain has one protocol, and a deployment without
                 the settler cannot offer the other — but the control keeps its place. */
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
            {/* The row keeps its height whether or not there is a figure, so typing the first
                digit does not shift everything below it. */}
            <div className={styles.amountUsd} aria-hidden={!hasAmount}>
              {hasAmount ? `~${usd(fromUsdNum, { compact: false })}` : "\u00a0"}
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
              {quote
                ? `~${usd(quote.amountOutUsd, { compact: false })}`
                : "\u00a0"}
            </div>
          </div>
        </div>

        {config.showResolverRoute && (
          <div className={styles.route}>
            {routeStats.map((s) => (
              <div key={s.label} className={styles.routeCell}>
                <div className={styles.routeLabel}>
                  <Term term={s.term}>{s.label}</Term>
                </div>
                <div className={styles.routeValue} data-impact={s.impact}>
                  {s.value}
                </div>
              </div>
            ))}
          </div>
        )}

        {stale && (
          <p className={styles.routeNote} role="status">
            Refresh failed — showing the last price
          </p>
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
                  aria-label="Search assets"
                  value={state.pQuery}
                  onChange={(e) => set({ pQuery: e.target.value })}
                  placeholder="Search name or paste address"
                  autoFocus
                />
              </label>

              {/* Shown even when the leg can reach only one chain: which chain this leg is on is
                  the fact the reader needs, and it is not otherwise on screen. */}
              {chains.length > 0 && (
                <div className={styles.tagRow}>
                  {chains.map((n) => (
                    <button
                      key={n}
                      type="button"
                      className={
                        n === activeChain
                          ? styles.tagChipActive
                          : styles.tagChip
                      }
                      aria-pressed={n === activeChain}
                      disabled={chains.length === 1}
                      onClick={() => set({ pNet: n })}
                    >
                      {chainLogo(assets, n) && (
                        <img
                          alt=""
                          className={styles.chainPillMark}
                          loading="lazy"
                          src={chainLogo(assets, n)}
                        />
                      )}
                      {n}
                    </button>
                  ))}
                </div>
              )}
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
                      <TokenMark
                        className={styles.tokenChip}
                        logoUri={t.logoUri}
                        symbol={t.symbol}
                      />
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
                          {usd(t.price, { compact: false })}
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
                <AsyncNote
                  className={styles.empty}
                  empty={
                    matches.length === 0
                      ? "No assets match that filter."
                      : undefined
                  }
                  query={assetsQuery}
                  subject="assets"
                />
              </div>
            </div>
          </div>
        )}
      </section>
    </div>
  );
}
