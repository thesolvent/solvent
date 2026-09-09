import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { useNavigate, useParams, useSearchParams } from "react-router-dom";
import { formatUnits } from "viem";

import { Crumbs } from "@/components/Crumbs";
import { BAND_K0 } from "@/data";
import {
  MIN_PEGGED_BOUND_PERCENT,
  clampBand,
  createPosition,
} from "@/lib/create-position";
import type { CreatePair, PositionCurve } from "@/ports/positions";
import type { Position } from "@/data/makers";
import {
  useCreatePairs,
  useCreatePosition,
  usePairPriceHistory,
} from "@/services/positions";
import { usePosition } from "@/services/makers";
import { slug } from "@/services/pools";
import { useApp, type AppState } from "@/state";

import styles from "./CreatePoolPage.module.css";

const STRATEGIES: readonly PositionCurve[] = [
  "Concentrated",
  "Pegged",
  "Full range",
];
const SPANS = ["7d", "3m", "All"] as const;
const PRESET_WIDTHS = {
  "±0.01%": 0.01,
  "±0.04%": 0.04,
  "±0.1%": 0.1,
} as const;
const PRESETS = [
  "±0.01%",
  "±0.04%",
  "±0.1%",
  "Market",
  "Full range",
  "Custom",
] as const;
const TOKEN_TAGS = [
  { label: "All", value: null },
  { label: "USD", value: "USD" },
  { label: "ETH", value: "ETH" },
  { label: "BTC", value: "BTC" },
  { label: "DeFi", value: "DeFi" },
] as const;
const RAIL_LABELS = ["Pair", "Active price", "Amount", "Summary"];
const PANE_SUBS = [
  "Top pools by TVL",
  "Drag the band or pick a preset",
  "Capped to your wallet balance",
  "Immutable once shipped",
];
const EMPTY_PAIRS: readonly CreatePair[] = [];

type BandEdge = "max" | "min" | "body";

function pairDefaults(pair: CreatePair, corePair: number) {
  const minimum = pair.type === "Stable" ? MIN_PEGGED_BOUND_PERCENT : 0.004;
  const band = Math.min(10, Math.max(minimum, pair.defaultBandPct));
  return {
    corePair,
    flipped: false,
    createPreset: "Market",
    strategy: pair.type === "Stable" ? "Pegged" : "Concentrated",
    pegSym: pair.type === "Stable",
    createFee: `Auto ${(pair.defaultFeeBps / 100).toFixed(2)}%`,
    amtA: formatUnits(pair.base.balanceRaw / 2n, pair.base.decimals),
    amtB: formatUnits(pair.quote.balanceRaw / 2n, pair.quote.decimals),
    bandMax: band,
    bandMin: -band,
  };
}

function sameAddress(left: string, right: string) {
  return left.toLowerCase() === right.toLowerCase();
}

function cloneDefaults(
  position: Position,
  pairs: readonly CreatePair[],
): Partial<AppState> | undefined {
  const corePair = pairs.findIndex(
    ({ base, quote }) =>
      (sameAddress(base.address, position.base.address) &&
        sameAddress(quote.address, position.quote.address)) ||
      (sameAddress(base.address, position.quote.address) &&
        sameAddress(quote.address, position.base.address)),
  );
  const pair = pairs[corePair];
  if (!pair) return undefined;

  const flipped = sameAddress(pair.base.address, position.quote.address);
  const market = flipped ? 1 / pair.mid : pair.mid;
  const defaults = {
    ...pairDefaults(pair, corePair),
    flipped,
    step: 3,
    stepDirty: { 2: true, 3: true, 4: true },
    amtA: "",
    amtB: "",
    customFeePct: "0.10",
    slotA: null,
    slotB: null,
    chartHover: null,
    hoverFrac: null,
  };

  if (position.curve === "XYC") {
    return {
      ...defaults,
      strategy: "Full range",
      createPreset: "Full range",
      pegSym: false,
      chartZoom: 1,
    };
  }

  if (position.curve === "Pegged") {
    const width = Math.min(
      50,
      Math.max(
        MIN_PEGGED_BOUND_PERCENT,
        position.belowPct ?? 0,
        position.abovePct ?? 0,
      ),
    );
    return {
      ...defaults,
      strategy: "Pegged",
      createPreset: "Custom",
      pegSym: true,
      bandMin: -width,
      bandMax: width,
      chartZoom: 1,
    };
  }

  const lower = position.lowerPrice;
  const upper = position.upperPrice;
  if (
    position.curve !== "Concentrated" ||
    lower == null ||
    upper == null ||
    !Number.isFinite(lower) ||
    !Number.isFinite(upper) ||
    !(lower > 0 && lower < upper && market > 0)
  ) {
    return undefined;
  }

  return {
    ...defaults,
    strategy: "Concentrated",
    createPreset: "Custom",
    pegSym: false,
    bandMin: Math.max(-50, (lower / market - 1) * 100),
    bandMax: Math.min(50, (upper / market - 1) * 100),
    chartZoom: 1,
  };
}

export function CreatePoolPage() {
  const { state, set, pop } = useApp();
  const navigate = useNavigate();
  const { pair: routePair } = useParams();
  const [searchParams] = useSearchParams();
  const cloneHash = searchParams.get("clone") ?? undefined;
  const cloneQuery = usePosition(cloneHash);
  const pairQuery = useCreatePairs();
  const pairs = pairQuery.data ?? EMPTY_PAIRS;
  const selectedPair = pairs[state.corePair] ?? pairs[0];
  const historyQuery = usePairPriceHistory(selectedPair, state.createSpan);
  const c = createPosition(
    state,
    pairs,
    pairQuery.isError ? "Couldn’t load supported pairs." : undefined,
    historyQuery.data,
  );
  const creation = useCreatePosition(
    c.pair
      ? {
          pair: c.pair,
          flipped: state.flipped,
          curve: state.strategy as PositionCurve,
          feeBps: c.feeBps,
          spotPrice: c.spotPrice,
          priceMin: c.priceMin,
          priceMax: c.priceMax,
          halfWidthPct: c.halfWidthPct,
          peggedSymmetric: c.peggedSymmetric,
          amountBase: state.amtA,
          amountQuote: state.amtB,
        }
      : undefined,
    ({ strategyHash }) =>
      navigate(`/explorer/strategies/${encodeURIComponent(strategyHash)}`, {
        state: { waitForStrategyIndex: true },
      }),
  );
  const plotRef = useRef<HTMLDivElement>(null);
  const initializedRoute = useRef<string | null>(null);
  const initializedClone = useRef<string | null>(null);

  useLayoutEffect(() => {
    if (
      !routePair ||
      pairs.length === 0 ||
      initializedRoute.current === routePair
    )
      return;
    const index = pairs.findIndex(
      ({ base, quote }) => slug(`${base.symbol}/${quote.symbol}`) === routePair,
    );
    if (index >= 0) {
      initializedRoute.current = routePair;
      set(pairDefaults(pairs[index], index));
    }
  }, [pairs, routePair, set]);

  useLayoutEffect(() => {
    if (
      !cloneHash ||
      !cloneQuery.data ||
      pairs.length === 0 ||
      initializedClone.current === cloneHash
    ) {
      return;
    }
    const defaults = cloneDefaults(cloneQuery.data, pairs);
    if (!defaults) return;
    initializedClone.current = cloneHash;
    set(defaults);
  }, [cloneHash, cloneQuery.data, pairs, set]);

  // Wheel-zoom must be non-passive to preventDefault, which React's onWheel can't do.
  useEffect(() => {
    const el = plotRef.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      set({
        chartZoom: Math.min(
          400,
          Math.max(0.5, state.chartZoom * (e.deltaY < 0 ? 1.15 : 1 / 1.15)),
        ),
      });
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [state.chartZoom, set]);

  const bandDrag = useCallback(
    (edge: BandEdge) => (e: ReactPointerEvent<HTMLDivElement>) => {
      e.preventDefault();
      const box = (e.currentTarget as HTMLElement).closest("[data-band-plot]");
      if (!box) return;
      const r = box.getBoundingClientRect();
      const K = Number(box.getAttribute("data-k")) || BAND_K0;
      const scaleMax =
        Number(box.getAttribute("data-scale-max")) ||
        Math.log1p(state.bandMax / 100) * 100;
      const scaleMin =
        Number(box.getAttribute("data-scale-min")) ||
        Math.log1p(state.bandMin / 100) * 100;
      const startY = e.clientY;

      const move = (ev: PointerEvent) => {
        const dScale = (((ev.clientY - startY) / r.height) * 100) / K;
        const percentAtScale = (scale: number) => Math.expm1(scale / 100) * 100;
        let next =
          edge === "max"
            ? { bandMax: percentAtScale(scaleMax - dScale) }
            : edge === "min"
              ? { bandMin: percentAtScale(scaleMin - dScale) }
              : {
                  bandMax: percentAtScale(scaleMax - dScale),
                  bandMin: percentAtScale(scaleMin - dScale),
                };
        if (state.pegSym && state.strategy === "Pegged" && edge !== "body") {
          const w = Math.abs(edge === "max" ? next.bandMax! : next.bandMin!);
          next = { bandMax: w, bandMin: -w };
        }
        set({
          createPreset: "Custom",
          dragging: edge,
          ...clampBand(state, next),
        });
      };
      const up = () => {
        set({ dragging: null });
        window.removeEventListener("pointermove", move);
        window.removeEventListener("pointerup", up);
      };
      window.addEventListener("pointermove", move);
      window.addEventListener("pointerup", up);
    },
    [state, set],
  );

  const nudge =
    (edge: "max" | "min") => (e: ReactKeyboardEvent<HTMLDivElement>) => {
      if (e.key !== "ArrowUp" && e.key !== "ArrowDown") return;
      e.preventDefault();
      const step = (e.shiftKey ? 0.05 : 0.01) * (e.key === "ArrowUp" ? 1 : -1);
      const next =
        edge === "max"
          ? { bandMax: state.bandMax + step }
          : { bandMin: state.bandMin + step };
      set({ createPreset: "Custom", ...clampBand(state, next) });
    };

  const onChartMove = (e: ReactMouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    set({
      volHover: null,
      chartHover: {
        x: ((e.clientX - r.left) / r.width) * 100,
        y: ((e.clientY - r.top) / r.height) * 100,
      },
    });
  };

  const pickWalletToken = (sym: string) => {
    if (state.slotA === sym) return set({ slotA: null, q1: "", pickerSlot: 1 });
    if (state.slotB === sym) return set({ slotB: null, q2: "", pickerSlot: 2 });

    const toB = state.pickerSlot === 2 || (!!state.slotA && !state.slotB);
    const next = toB
      ? { slotB: sym, q2: "", pickerSlot: 1 }
      : { slotA: sym, q1: "", pickerSlot: 2 };
    const one = toB ? state.slotA : sym;
    const two = toB ? sym : state.slotB;

    // Landing on a supported pair jumps the wizard's pair selection to it.
    if (one && two && one !== two) {
      const hit = c.pairs.findIndex(
        (x) => (x.a === one && x.b === two) || (x.a === two && x.b === one),
      );
      if (hit > -1) {
        return set({
          ...next,
          corePair: hit,
          flipped: c.pairs[hit].a !== one,
        });
      }
    }
    set(next);
  };

  const pickPreset = (preset: (typeof PRESETS)[number]) => {
    if (preset === "Full range") {
      set({ strategy: "Full range", createPreset: preset, chartZoom: 1 });
      return;
    }

    const strategy =
      state.strategy === "Full range"
        ? c.pr.type === "Stable"
          ? "Pegged"
          : "Concentrated"
        : state.strategy;
    const boundedFitSpan = Math.max(c.pr.band * 2.6, c.pr.vol * 1.4);
    if (preset === "Custom") {
      const halfWidth = Math.max(
        Math.abs(state.bandMax),
        Math.abs(state.bandMin),
      );
      set({
        createPreset: preset,
        strategy,
        chartZoom: Math.max(
          0.5,
          Math.min(400, boundedFitSpan / (halfWidth * 2.4)),
        ),
      });
      return;
    }

    const halfWidth = preset === "Market" ? c.pr.band : PRESET_WIDTHS[preset];
    const pegSym = strategy === "Pegged" ? true : state.pegSym;
    set({
      createPreset: preset,
      strategy,
      pegSym,
      chartZoom: Math.max(
        0.5,
        Math.min(400, boundedFitSpan / (halfWidth * 2.4)),
      ),
      ...clampBand(
        { ...state, strategy, pegSym },
        { bandMax: halfWidth, bandMin: -halfWidth },
      ),
    });
  };

  const flipOrientation = () =>
    set({
      flipped: !state.flipped,
      amtA: state.amtB,
      amtB: state.amtA,
    });

  const selectStrategy = (strategy: PositionCurve) => {
    const patch = {
      strategy,
      createPreset: strategy === "Full range" ? "Full range" : "Custom",
      ...(strategy === "Pegged"
        ? {
            pegSym: true,
            ...clampBand(
              { ...state, strategy, pegSym: true },
              {
                bandMax: c.pr.band,
                bandMin: -c.pr.band,
              },
            ),
          }
        : {}),
    };
    if (state.step < 3) {
      set(patch);
      return;
    }

    const next = createPosition(
      { ...state, ...patch },
      pairs,
      pairQuery.isError ? "Couldn’t load supported pairs." : undefined,
      historyQuery.data,
    );
    set({ ...patch, ...next.amountsFromA(state.amtA) });
  };

  const stepFoot = (
    <div className={styles.paneFoot}>
      <button
        type="button"
        className={styles.goBack}
        style={{ visibility: c.backVis as "visible" | "hidden" }}
        onClick={() => set({ step: Math.max(1, state.step - 1) })}
      >
        Go back
      </button>
      <p className={styles.footNote} style={{ color: c.footFg }}>
        {c.footNote}
      </p>
      <button
        type="button"
        className={styles.next}
        onClick={() => {
          const step = Math.min(4, state.step + 1);
          set({
            step,
            ...(step >= 3 ? c.amountsFromA(state.amtA) : {}),
          });
        }}
      >
        {c.nextLabel}
      </button>
    </div>
  );

  const paneHead = (i: number) => (
    <div className={styles.paneHead}>
      <span className={styles.paneTitle}>{c.paneTitle}</span>
      <span className={styles.paneSub}>{PANE_SUBS[i]}</span>
    </div>
  );

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <button type="button" className={styles.back} onClick={pop}>
          ←
        </button>
        <div className={styles.headTitle}>
          <Crumbs current="Create position" />
          <div className={styles.title}>Create a position</div>
        </div>
        <span className={styles.spacer} />
        <span
          className={styles.rangeTag}
          style={{ background: c.rangeTagBg, color: c.rangeTagFg }}
        >
          {c.rangeTag}
        </span>
        <div className={styles.segmented}>
          {STRATEGIES.map((x) => (
            <button
              key={x}
              type="button"
              className={
                x === state.strategy ? styles.strategyOn : styles.strategy
              }
              onClick={() => selectStrategy(x)}
            >
              {x}
            </button>
          ))}
        </div>
      </div>

      <div className={styles.panels}>
        {c.steps.map((st, i) => (
          <div
            key={st.n}
            className={styles.panel}
            style={{ flex: st.panelFlex, background: st.panelBg }}
          >
            <button
              type="button"
              className={styles.rail}
              style={{ cursor: st.cursor }}
              aria-expanded={st.open}
              onClick={() => {
                if (!st.locked)
                  set({
                    step: st.n,
                    ...(st.n >= 3 ? c.amountsFromA(state.amtA) : {}),
                  });
              }}
            >
              <span className={styles.railNum} style={{ color: st.numFg }}>
                {String(st.n).padStart(2, "0")}
              </span>
              <span
                className={styles.railTick}
                style={{ background: st.tickBg }}
              />
              <span className={styles.railBarWrap}>
                <span className={styles.railBar} style={{ color: st.barFg }}>
                  {RAIL_LABELS[i]}
                </span>
              </span>
            </button>

            {st.open && st.n === 1 && (
              <div className={styles.pane}>
                {paneHead(0)}
                <div data-scroll="1" className={styles.paneBody}>
                  <div className={styles.pairHead}>
                    <span className={styles.microLabel}>
                      Pair · {c.pairType}
                    </span>
                  </div>

                  <div className={styles.pairGrid}>
                    {c.recommendations.map((q) => {
                      const on = q.catalogIndex === state.corePair;
                      return (
                        <button
                          key={`${q.a}/${q.b}`}
                          type="button"
                          className={on ? styles.pairChipOn : styles.pairChip}
                          onClick={() =>
                            set({
                              ...pairDefaults(q.source!, q.catalogIndex),
                              step: 2,
                              stepDirty: {
                                2: true,
                                3: true,
                                4: true,
                              },
                            })
                          }
                        >
                          <span
                            className={styles.pairDot}
                            style={{
                              background: on
                                ? "var(--ink)"
                                : "var(--line-soft)",
                            }}
                          />
                          <span className={styles.pairLabel}>
                            {q.a} / {q.b}
                          </span>
                        </button>
                      );
                    })}
                  </div>

                  <div className={styles.divider}>
                    <span className={styles.microLabel}>
                      Or build a custom pair
                    </span>
                    <span className={styles.dividerLine} />
                  </div>

                  <div className={styles.custom}>
                    <div className={styles.slots}>
                      <div className={styles.slotA}>
                        {state.slotA ? (
                          <div className={styles.slotFilled}>
                            <span
                              className={styles.slotChip}
                              style={{
                                background: c.tintA,
                              }}
                            >
                              {state.slotA}
                            </span>
                            <span className={styles.slotSym}>
                              {state.slotA}
                            </span>
                            <button
                              type="button"
                              className={styles.slotClear}
                              onClick={() =>
                                set({
                                  slotA: null,
                                  q1: "",
                                  pickerSlot: 1,
                                })
                              }
                            >
                              ✕
                            </button>
                          </div>
                        ) : (
                          <input
                            className={styles.slotInput}
                            value={state.q1}
                            onChange={(e) =>
                              set({
                                q1: e.target.value,
                                slotA: null,
                                pickerSlot: 1,
                              })
                            }
                            onFocus={() =>
                              set({
                                pickerSlot: 1,
                              })
                            }
                            placeholder="Search token 1"
                          />
                        )}
                      </div>
                      <div className={styles.slotB}>
                        {state.slotB ? (
                          <div className={styles.slotFilledRight}>
                            <span
                              className={styles.slotChip}
                              style={{
                                background: c.tintB,
                              }}
                            >
                              {state.slotB}
                            </span>
                            <span className={styles.slotSym}>
                              {state.slotB}
                            </span>
                            <button
                              type="button"
                              className={styles.slotClear}
                              onClick={() =>
                                set({
                                  slotB: null,
                                  q2: "",
                                  pickerSlot: 2,
                                })
                              }
                            >
                              ✕
                            </button>
                          </div>
                        ) : (
                          <input
                            className={styles.slotInputRight}
                            value={state.q2}
                            onChange={(e) =>
                              set({
                                q2: e.target.value,
                                slotB: null,
                                pickerSlot: 2,
                              })
                            }
                            onFocus={() =>
                              set({
                                pickerSlot: 2,
                              })
                            }
                            placeholder="Search token 2"
                          />
                        )}
                      </div>
                      <button
                        type="button"
                        className={styles.swapSlots}
                        onClick={() =>
                          set({
                            slotA: state.slotB,
                            slotB: state.slotA,
                            q1: "",
                            q2: "",
                          })
                        }
                      >
                        ⇄
                      </button>
                    </div>

                    <div className={styles.tokenTags}>
                      {TOKEN_TAGS.map(({ label, value }) => (
                        <button
                          key={label}
                          type="button"
                          className={
                            state.tokenTag === value
                              ? styles.tokenTagOn
                              : styles.tokenTag
                          }
                          onClick={() => set({ tokenTag: value })}
                        >
                          {label}
                        </button>
                      ))}
                    </div>

                    <div className={styles.walletHead}>
                      <span className={styles.walletTitle}>Your tokens</span>
                      <span className={styles.walletHint}>{c.slotHint}</span>
                    </div>

                    {c.pairWarn && (
                      <div className={styles.warn}>{c.pairWarn}</div>
                    )}

                    <div data-scroll="1" className={styles.walletList}>
                      {c.walletRows.map((row) => (
                        <button
                          key={row.token.sym}
                          type="button"
                          className={styles.walletRow}
                          style={{
                            background: row.rowBg,
                            opacity: row.dim,
                          }}
                          onClick={() => pickWalletToken(row.token.sym)}
                        >
                          <span
                            className={styles.walletChip}
                            style={{
                              background: row.token.tint,
                            }}
                          >
                            {row.token.sym}
                          </span>
                          <span className={styles.walletMain}>
                            <span className={styles.walletName}>
                              {row.token.name}
                            </span>
                            <span className={styles.walletMeta}>
                              {row.amt} · {row.token.addr}
                            </span>
                          </span>
                          <span className={styles.walletFigures}>
                            <span className={styles.walletUsd}>{row.usd}</span>
                            <span
                              className={styles.walletDelta}
                              style={{
                                color: row.deltaFg,
                              }}
                            >
                              {row.delta}
                            </span>
                          </span>
                          <span
                            className={styles.walletMark}
                            style={{
                              background: row.markBg,
                              color: row.markFg,
                            }}
                          >
                            {row.mark}
                          </span>
                        </button>
                      ))}
                      {c.emptyList && (
                        <div className={styles.walletEmpty}>
                          No tokens match that search.
                        </div>
                      )}
                    </div>
                  </div>
                </div>
                {stepFoot}
              </div>
            )}

            {st.open && st.n === 2 && (
              <div className={styles.pane}>
                {paneHead(1)}
                <div data-scroll="1" className={styles.paneBody}>
                  <div className={styles.overviewRow}>
                    <div className={styles.overview}>
                      <div className={styles.overviewLabel}>
                        Position overview
                      </div>
                      <div className={styles.overviewFigures}>
                        <span className={styles.overviewPrice}>
                          {c.opening}
                        </span>
                        <span className={styles.overviewQuote}>
                          {c.quote} opening
                        </span>
                        <span className={styles.overviewMarket}>
                          market {c.market}
                        </span>
                      </div>
                    </div>
                    <div className={styles.chartTools}>
                      {c.isPegged && (
                        <button
                          type="button"
                          className={styles.pegToggle}
                          style={{
                            background: c.pegSymBg,
                          }}
                          onClick={() => {
                            const nextSym = !c.symmetric;
                            const w = Math.max(
                              Math.abs(state.bandMax),
                              Math.abs(state.bandMin),
                            );
                            set({
                              pegSym: nextSym,
                              ...(nextSym
                                ? clampBand(
                                    {
                                      ...state,
                                      pegSym: true,
                                    },
                                    {
                                      bandMax: w,
                                      bandMin: -w,
                                    },
                                  )
                                : {}),
                            });
                          }}
                        >
                          {c.pegSymLabel}
                        </button>
                      )}
                      <button
                        type="button"
                        className={styles.orientation}
                        onClick={flipOrientation}
                      >
                        {c.flipLabel}
                      </button>
                      <div className={styles.zoomGroup}>
                        <button
                          type="button"
                          className={styles.zoomStep}
                          onClick={() =>
                            set({
                              chartZoom: Math.max(0.5, state.chartZoom / 1.5),
                            })
                          }
                        >
                          −
                        </button>
                        <span className={styles.zoomLabel}>{c.zoomLabel}</span>
                        <button
                          type="button"
                          className={styles.zoomStep}
                          onClick={() =>
                            set({
                              chartZoom: Math.min(400, state.chartZoom * 1.5),
                            })
                          }
                        >
                          +
                        </button>
                        <button
                          type="button"
                          className={styles.zoomWord}
                          onClick={() =>
                            set({
                              chartZoom: Math.max(
                                0.5,
                                Math.min(
                                  400,
                                  c.fitSpan / (c.bandScaleExtent * 2.4),
                                ),
                              ),
                            })
                          }
                        >
                          fit
                        </button>
                        <button
                          type="button"
                          className={styles.zoomWord}
                          onClick={() => set({ chartZoom: 1 })}
                        >
                          reset
                        </button>
                      </div>
                      <div className={styles.segmented}>
                        {SPANS.map((t) => (
                          <button
                            key={t}
                            type="button"
                            className={
                              t === state.createSpan
                                ? styles.spanOn
                                : styles.span
                            }
                            onClick={() =>
                              set({
                                createSpan: t,
                              })
                            }
                          >
                            {t}
                          </button>
                        ))}
                      </div>
                    </div>
                  </div>

                  <div className={styles.plotGrid}>
                    <div
                      ref={plotRef}
                      data-band-plot="1"
                      data-k={c.scaleK}
                      data-scale-max={c.bandScaleMax}
                      data-scale-min={c.bandScaleMin}
                      className={styles.plot}
                      onMouseMove={onChartMove}
                      onMouseLeave={() => set({ chartHover: null })}
                    >
                      <div
                        className={styles.band}
                        style={{
                          top: c.bandTop,
                          height: c.bandHeight,
                          background: c.bandFill,
                          cursor: c.bodyCursor,
                        }}
                        onPointerDown={bandDrag("body")}
                      />
                      <span className={styles.midLine} />
                      <svg
                        viewBox="0 0 1000 400"
                        preserveAspectRatio="none"
                        className={styles.plotSvg}
                      >
                        <polyline
                          points={c.series}
                          fill="none"
                          stroke="var(--green)"
                          strokeWidth="1.8"
                          strokeLinejoin="round"
                          strokeLinecap="round"
                          vectorEffect="non-scaling-stroke"
                        />
                      </svg>

                      {c.notFull && (
                        <>
                          <div
                            className={styles.edge}
                            style={{
                              top: c.bandTop,
                            }}
                            onPointerDown={bandDrag("max")}
                            onKeyDown={nudge("max")}
                            tabIndex={0}
                            role="slider"
                            aria-label={c.ariaMax}
                            aria-valuenow={state.bandMax}
                          >
                            <span
                              className={styles.edgeLine}
                              style={{
                                borderTopColor: c.edgeColor,
                                borderTopWidth: c.edgeW,
                              }}
                            />
                            <span
                              className={styles.edgePillMax}
                              style={{
                                background: c.edgeColor,
                              }}
                            >
                              {c.maxPill}
                            </span>
                          </div>
                          <div
                            className={styles.edge}
                            style={{
                              top: c.bandBottom,
                            }}
                            onPointerDown={bandDrag("min")}
                            onKeyDown={nudge("min")}
                            tabIndex={0}
                            role="slider"
                            aria-label={c.ariaMin}
                            aria-valuenow={state.bandMin}
                          >
                            <span
                              className={styles.edgeLine}
                              style={{
                                borderTopColor: c.edgeColor,
                                borderTopWidth: c.edgeW,
                              }}
                            />
                            <span
                              className={styles.edgePillMin}
                              style={{
                                background: c.edgeColor,
                              }}
                            >
                              {c.minPill}
                            </span>
                          </div>
                        </>
                      )}

                      {c.volTip && (
                        <div className={styles.tipLayer}>
                          <span
                            className={styles.tipDot}
                            style={{
                              left: c.volTip.left,
                              top: c.volTip.top,
                            }}
                          />
                          <div
                            className={styles.tip}
                            style={{
                              left: c.volTip.left,
                              top: c.volTip.top,
                              transform: c.volTip.shift,
                            }}
                          >
                            <div className={styles.tipMain}>
                              {c.volTip.px}{" "}
                              <span className={styles.tipAccent}>
                                {c.volTip.vol}
                              </span>
                            </div>
                            <div className={styles.tipSub}>{c.volTip.when}</div>
                          </div>
                        </div>
                      )}

                      {c.cross && (
                        <div className={styles.tipLayer}>
                          <span
                            className={styles.tipDot}
                            style={{
                              left: c.cross.left,
                              top: c.cross.top,
                            }}
                          />
                          <div
                            className={styles.tipCross}
                            style={{
                              left: c.cross.left,
                              top: c.cross.top,
                              transform: c.cross.shift,
                            }}
                          >
                            <div className={styles.tipMain}>
                              {c.cross.price}{" "}
                              <span className={styles.tipAccent}>
                                {c.cross.pct}
                              </span>
                            </div>
                            <div className={styles.tipSub}>{c.cross.date}</div>
                          </div>
                        </div>
                      )}
                    </div>

                    <div className={styles.axis}>
                      <span className={styles.axisTop}>{c.axisHi}</span>
                      {c.notFull && (
                        <>
                          <span
                            className={styles.axisEdge}
                            style={{
                              top: c.bandTop,
                              color: c.edgeColor,
                            }}
                          >
                            {c.axisMax}
                          </span>
                          <span
                            className={styles.axisEdge}
                            style={{
                              top: c.bandBottom,
                              color: c.edgeColor,
                            }}
                          >
                            {c.axisMin}
                          </span>
                        </>
                      )}
                      <span className={styles.axisMid}>{c.axisMid}</span>
                      <span className={styles.axisBottom}>{c.axisLo}</span>
                    </div>

                    <div
                      className={styles.volRow}
                      onMouseLeave={() => set({ volHover: null })}
                    >
                      <div className={styles.volBars}>
                        {c.vols.map((v, i) => (
                          <span
                            key={i}
                            className={v.on ? styles.volBarOn : styles.volBar}
                            style={{ height: v.h }}
                            onMouseEnter={() =>
                              set({
                                volHover: i,
                                chartHover: null,
                              })
                            }
                          />
                        ))}
                      </div>
                      <div className={styles.timeTicks}>
                        {c.timeTicks.map((t) => (
                          <span
                            key={t.key}
                            className={styles.timeTick}
                            style={{
                              left: t.left,
                              transform: t.shift,
                            }}
                          >
                            {t.label}
                          </span>
                        ))}
                      </div>
                    </div>

                    <div className={styles.volCorner} />
                  </div>

                  <div className={styles.presets}>
                    {PRESETS.map((x) => (
                      <button
                        key={x}
                        type="button"
                        className={
                          x === state.createPreset
                            ? styles.presetOn
                            : styles.preset
                        }
                        onClick={() => pickPreset(x)}
                      >
                        {x}
                      </button>
                    ))}
                  </div>
                </div>
                {stepFoot}
              </div>
            )}

            {st.open && st.n === 3 && (
              <div className={styles.pane}>
                {paneHead(2)}
                <div data-scroll="1" className={styles.paneBody}>
                  <div className={styles.feeRow}>
                    <span className={styles.microLabel}>Swap fee</span>
                    <div className={styles.feeGrid}>
                      {c.feeOptions.map((x) =>
                        x === "Custom" && x === c.activeFee ? (
                          <label key={x} className={styles.feeCustom}>
                            <span>Custom</span>
                            <span className={styles.feeCustomValue}>
                              <input
                                autoFocus
                                aria-label="Custom fee percentage"
                                className={styles.feeInput}
                                inputMode="decimal"
                                value={state.customFeePct}
                                onChange={(event) =>
                                  set({ customFeePct: event.target.value })
                                }
                              />
                              <span>%</span>
                            </span>
                          </label>
                        ) : (
                          <button
                            key={x}
                            type="button"
                            className={
                              x === c.activeFee ? styles.feeOn : styles.fee
                            }
                            onClick={() => set({ createFee: x })}
                          >
                            {x}
                          </button>
                        ),
                      )}
                    </div>
                  </div>

                  <div className={styles.curveRow}>
                    <span className={styles.curveLabel}>{c.curveLabel}</span>
                    <button
                      type="button"
                      className={styles.useFull}
                      onClick={() => set(c.maxAmounts)}
                    >
                      Use full balances
                    </button>
                  </div>

                  <div className={styles.pairNote}>
                    {c.pairNote} · funds are never locked, quoting is capped to
                    your wallet balance.
                  </div>

                  <div className={styles.amounts}>
                    <div
                      className={styles.amountCard}
                      style={{
                        border: `1px solid ${c.bdA}`,
                      }}
                    >
                      <div className={styles.amountHead}>
                        <span className={styles.amountSym}>{c.A}</span>
                        <span className={styles.amountBal}>
                          bal {c.walletA}
                        </span>
                      </div>
                      <div className={styles.amountRow}>
                        <input
                          className={styles.amountInput}
                          value={state.amtA}
                          inputMode="decimal"
                          onChange={(e) => set(c.amountsFromA(e.target.value))}
                        />
                        <span className={styles.quickGroup}>
                          <button
                            type="button"
                            className={styles.quick}
                            onClick={() => set(c.halfFromA)}
                          >
                            50%
                          </button>
                          <button
                            type="button"
                            className={styles.quickNext}
                            onClick={() => set(c.maxFromA)}
                          >
                            Max
                          </button>
                        </span>
                      </div>
                      <div className={styles.amountState}>
                        <span
                          className={styles.amountStateTag}
                          style={{
                            background: c.tagA,
                            color: c.fgA,
                          }}
                        >
                          {c.covA} · {c.stateA}
                        </span>
                      </div>
                    </div>

                    <div
                      className={styles.amountCard}
                      style={{
                        border: `1px solid ${c.bdB}`,
                      }}
                    >
                      <div className={styles.amountHead}>
                        <span className={styles.amountSym}>{c.B}</span>
                        <span className={styles.amountBal}>
                          bal {c.walletB}
                        </span>
                      </div>
                      <div className={styles.amountRow}>
                        <input
                          className={styles.amountInput}
                          value={state.amtB}
                          inputMode="decimal"
                          onChange={(e) => set(c.amountsFromB(e.target.value))}
                        />
                        <span className={styles.quickGroup}>
                          <button
                            type="button"
                            className={styles.quick}
                            onClick={() => set(c.halfFromB)}
                          >
                            50%
                          </button>
                          <button
                            type="button"
                            className={styles.quickNext}
                            onClick={() => set(c.maxFromB)}
                          >
                            Max
                          </button>
                        </span>
                      </div>
                      <div className={styles.amountState}>
                        <span
                          className={styles.amountStateTag}
                          style={{
                            background: c.tagB,
                            color: c.fgB,
                          }}
                        >
                          {c.covB} · {c.stateB}
                        </span>
                      </div>
                    </div>
                  </div>
                </div>
                {stepFoot}
              </div>
            )}

            {st.open && st.n === 4 && (
              <div className={styles.pane}>
                {paneHead(3)}
                <div data-scroll="1" className={styles.paneBody}>
                  <div className={styles.recap}>
                    {c.recap.map((r) => (
                      <div key={r.label} className={styles.recapRow}>
                        <span className={styles.recapLabel}>{r.label}</span>
                        <span className={styles.recapValue}>{r.value}</span>
                      </div>
                    ))}
                  </div>
                  <p className={styles.immutable}>
                    <strong className={styles.immutableLead}>Immutable:</strong>{" "}
                    a shipped position can&apos;t be edited — to change it, dock
                    and ship a new one. You can always push more inventory.
                  </p>
                </div>
                <div className={styles.paneFoot}>
                  <button
                    type="button"
                    className={styles.goBack}
                    style={{
                      visibility: c.backVis as "visible" | "hidden",
                    }}
                    onClick={() =>
                      set({
                        step: Math.max(1, state.step - 1),
                      })
                    }
                  >
                    Go back
                  </button>
                  <p className={styles.footNote} style={{ color: c.footFg }}>
                    {creation.problem ?? c.footNote}
                  </p>
                  <button
                    type="button"
                    className={styles.cta}
                    disabled={c.ctaDisabled || creation.submitting}
                    style={{
                      background: c.ctaBg,
                      color: c.ctaFg,
                      cursor: creation.submitting ? "wait" : c.ctaCursor,
                    }}
                    onClick={creation.send}
                  >
                    {creation.submitting
                      ? "Creating position…"
                      : creation.problem
                        ? "Could not create — try again"
                        : c.cta}
                  </button>
                </div>
              </div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
