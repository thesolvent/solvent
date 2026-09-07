import {
  useCallback,
  useEffect,
  useRef,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";

import { Crumbs } from "@/components/Crumbs";
import { BAND_K0, CORE6 } from "@/data";
import { clampBand, createPosition } from "@/lib/create-position";
import { useApp } from "@/state";

import styles from "./CreatePoolPage.module.css";

const STRATEGIES = ["Concentrated", "Pegged", "Full range"];
const SPANS = ["7d", "3m", "All"];
const PRESETS = ["±0.01%", "±0.04%", "±0.1%", "Market", "Full range", "Custom"];
const TOKEN_TAGS = ["USD", "ETH", "BTC", "DeFi"];
const RAIL_LABELS = ["Pair", "Active price", "Amount", "Summary"];
const PANE_SUBS = [
  "Supported devnet pairs",
  "Drag the band or pick a preset",
  "Capped to your wallet balance",
  "Immutable once shipped",
];

type BandEdge = "max" | "min" | "body";

export function CreatePoolPage() {
  const { state, set, pop } = useApp();
  const c = createPosition(state);
  const plotRef = useRef<HTMLDivElement>(null);

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
      const startY = e.clientY;
      const start = { max: state.bandMax, min: state.bandMin };

      const move = (ev: PointerEvent) => {
        const dPct = (((ev.clientY - startY) / r.height) * 100) / K;
        let next =
          edge === "max"
            ? { bandMax: start.max - dPct }
            : edge === "min"
              ? { bandMin: start.min - dPct }
              : {
                  bandMax: start.max - dPct,
                  bandMin: start.min - dPct,
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
      const hit = CORE6.findIndex(
        (x) => (x.a === one && x.b === two) || (x.a === two && x.b === one),
      );
      if (hit > -1) {
        return set({
          ...next,
          corePair: hit,
          flipped: CORE6[hit].a !== one,
        });
      }
    }
    set(next);
  };

  const pickPreset = (x: string) => {
    if (x === "Full range")
      return set({ strategy: "Full range", createPreset: x });
    const w = (
      {
        "±0.01%": 0.01,
        "±0.04%": 0.04,
        "±0.1%": 0.1,
        Market: c.pr.band,
        Custom: Math.abs(state.bandMax),
      } as Record<string, number>
    )[x];
    set({
      createPreset: x,
      strategy: c.pegged ? "Pegged" : "Concentrated",
      chartZoom: Math.max(0.5, Math.min(400, c.fitSpan / (w * 2.4))),
      ...clampBand(state, { bandMax: w, bandMin: -w }),
    });
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
        onClick={() => set({ step: Math.min(4, state.step + 1) })}
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
              onClick={() =>
                set({
                  strategy: x,
                  createPreset: x === "Full range" ? "Full range" : "Custom",
                  ...(x === "Pegged"
                    ? clampBand(state, {
                        bandMax: c.pr.band,
                        bandMin: -c.pr.band,
                      })
                    : {}),
                })
              }
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
                if (!st.locked) set({ step: st.n });
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
                    <button
                      type="button"
                      className={styles.flip}
                      onClick={() => set({ flipped: !state.flipped })}
                    >
                      {c.flipLabel}
                    </button>
                  </div>

                  <div className={styles.pairGrid}>
                    {CORE6.map((q, qi) => {
                      const on = qi === state.corePair;
                      return (
                        <button
                          key={`${q.a}/${q.b}`}
                          type="button"
                          className={on ? styles.pairChipOn : styles.pairChip}
                          onClick={() =>
                            set({
                              step: 2,
                              stepDirty: {
                                2: true,
                                3: true,
                                4: true,
                              },
                              corePair: qi,
                              flipped: false,
                              createPreset: "Market",
                              strategy:
                                q.type === "Stable" ? "Pegged" : "Concentrated",
                              createFee: `Auto ${q.fee}`,
                              amtA: (q.walA * 0.5).toFixed(2),
                              amtB: (q.walB * 0.5).toFixed(2),
                              ...clampBand(state, {
                                bandMax: q.band,
                                bandMin: -q.band,
                              }),
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
                      {TOKEN_TAGS.map((t) => (
                        <button
                          key={t}
                          type="button"
                          className={
                            state.tokenTag === t
                              ? styles.tokenTagOn
                              : styles.tokenTag
                          }
                          onClick={() =>
                            set({
                              tokenTag: state.tokenTag === t ? null : t,
                            })
                          }
                        >
                          {t}
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
                                  c.fitSpan /
                                    (Math.max(
                                      Math.abs(state.bandMax),
                                      Math.abs(state.bandMin),
                                    ) *
                                      2.4),
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
                            key={t.label}
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
                      {c.feeOptions.map((x) => (
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
                      ))}
                    </div>
                  </div>

                  <div className={styles.curveRow}>
                    <span className={styles.curveLabel}>{c.curveLabel}</span>
                    <button
                      type="button"
                      className={styles.useFull}
                      onClick={() => {
                        const capA = Math.min(
                          c.wallet.a,
                          c.bPerA ? c.wallet.b / c.bPerA : c.wallet.a,
                        );
                        set({
                          amtA: capA.toFixed(2),
                          amtB: (capA * c.bPerA).toFixed(2),
                        });
                      }}
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
                          onChange={(e) =>
                            set({
                              amtA: e.target.value,
                              amtB: (
                                (parseFloat(e.target.value) || 0) * c.bPerA
                              ).toFixed(2),
                            })
                          }
                        />
                        <span className={styles.quickGroup}>
                          <button
                            type="button"
                            className={styles.quick}
                            onClick={() =>
                              set({
                                amtA: (c.wallet.a / 2).toFixed(2),
                                amtB: ((c.wallet.a / 2) * c.bPerA).toFixed(2),
                              })
                            }
                          >
                            50%
                          </button>
                          <button
                            type="button"
                            className={styles.quickNext}
                            onClick={() =>
                              set({
                                amtA: c.wallet.a.toFixed(2),
                                amtB: (c.wallet.a * c.bPerA).toFixed(2),
                              })
                            }
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
                          onChange={(e) =>
                            set({
                              amtB: e.target.value,
                              amtA: (c.bPerA
                                ? (parseFloat(e.target.value) || 0) / c.bPerA
                                : 0
                              ).toFixed(2),
                            })
                          }
                        />
                        <span className={styles.quickGroup}>
                          <button
                            type="button"
                            className={styles.quick}
                            onClick={() =>
                              set({
                                amtB: (c.wallet.b / 2).toFixed(2),
                                amtA: (c.bPerA
                                  ? c.wallet.b / 2 / c.bPerA
                                  : 0
                                ).toFixed(2),
                              })
                            }
                          >
                            50%
                          </button>
                          <button
                            type="button"
                            className={styles.quickNext}
                            onClick={() =>
                              set({
                                amtB: c.wallet.b.toFixed(2),
                                amtA: (c.bPerA
                                  ? c.wallet.b / c.bPerA
                                  : 0
                                ).toFixed(2),
                              })
                            }
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
                    {c.footNote}
                  </p>
                  <button
                    type="button"
                    className={styles.cta}
                    disabled={c.ctaDisabled}
                    style={{
                      background: c.ctaBg,
                      color: c.ctaFg,
                      cursor: c.ctaCursor,
                    }}
                  >
                    {c.cta}
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
