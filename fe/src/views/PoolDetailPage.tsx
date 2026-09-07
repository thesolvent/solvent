import type { MouseEvent as ReactMouseEvent } from "react";
import { useNavigate, useParams } from "react-router-dom";

import { Crumbs } from "@/components/Crumbs";
import { poolDetail } from "@/lib/pool-detail";
import { usePool, usePoolDepth, usePoolRoster } from "@/services/pools";
import { useApp } from "@/state";

import styles from "./PoolDetailPage.module.css";

const MAKER_SORTS = ["Virtual", "Actual"];

export function PoolDetailPage() {
  const { state, set } = useApp();
  const navigate = useNavigate();
  const { pair } = useParams();
  const pool = usePool(pair);
  const d = poolDetail({
    pool,
    roster: usePoolRoster(pool),
    depth: usePoolDepth(pool),
    hoverFrac: state.hoverFrac,
    makerSort: state.makerSort,
  });

  const onHover = (e: ReactMouseEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    set({
      hoverFrac: Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)),
    });
  };

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <button
          type="button"
          className={styles.back}
          onClick={() => navigate("/pools")}
        >
          ←
        </button>
        <div className={styles.headTitle}>
          <Crumbs current={d.pair} trail={[{ label: "Pools", to: "/pools" }]} />
          <div className={styles.pair}>{d.pair}</div>
        </div>
        <span className={styles.limeSquare} />
        <span className={styles.spacer} />
        <div className={styles.headActions}>
          <span className={styles.tag}>fee {d.fee}</span>
          <button
            type="button"
            className={styles.create}
            onClick={() => navigate(`/pools/${pair}/new`)}
          >
            <span>Create position</span>
            <span className={styles.createArrow}>→</span>
          </button>
        </div>
      </div>

      <div className={styles.grid}>
        <div className={styles.kpis}>
          <section className={styles.kpiWide}>
            <div className={styles.kpiTag}>
              <span className={styles.kpiSwatch} />
              <span className={styles.kpiLabel}>Depth</span>
            </div>
            <div>
              <div className={styles.kpiRow}>
                <span className={styles.kpiValue}>{d.tvl}</span>
                {d.tvlChange && (
                  <span className={styles.kpiDelta}>{d.tvlChange}</span>
                )}
              </div>
              <div className={styles.kpiSub}>Total value locked</div>
            </div>
            <div className={styles.kpiFoot}>
              Zero-inventory fills routed through{" "}
              <span className={styles.kpiFootMark}>Aqua</span>
            </div>
          </section>

          <section className={styles.kpiDark}>
            <div className={styles.kpiTag}>
              <span className={styles.kpiSwatch} />
              <span className={styles.kpiLabelDark}>Net APR</span>
            </div>
            <div>
              <div className={styles.kpiValueLime}>{d.apr}</div>
              <div className={styles.kpiSubDark}>
                After {d.fee} fee
                <br />
                {d.spread} spread
              </div>
            </div>
          </section>

          <section className={styles.kpiSmall}>
            <span className={styles.kpiLabel}>24h volume</span>
            <span className={styles.kpiSmallValue}>{d.vol}</span>
          </section>
          <section className={styles.kpiSmallRight}>
            <span className={styles.kpiLabel}>24h fills</span>
            <span className={styles.kpiSmallValue}>{d.fills}</span>
          </section>
        </div>

        <section className={styles.depth}>
          <div className={styles.depthHead}>
            <span className={styles.kpiSwatch} />
            <span className={styles.depthTitle}>Aggregated depth</span>
            <span className={styles.depthSub}>{d.priceTitle}</span>
          </div>

          <div className={styles.depthMeta}>
            <div className={styles.depthLegend}>
              <span className={styles.depthLegendMark} />
              <span className={styles.depthLegendText}>
                {d.makerTotal} {d.makerTotal === 1 ? "maker" : "makers"}
              </span>
            </div>
            {d.impacts.length > 0 && (
              <div className={styles.segmented}>
                {d.impacts.map((stop) => (
                  <button
                    key={stop.label}
                    type="button"
                    className={styles.segment}
                    onMouseEnter={() => set({ hoverFrac: stop.frac })}
                    onMouseLeave={() => set({ hoverFrac: null })}
                  >
                    {stop.label}
                  </button>
                ))}
              </div>
            )}
          </div>

          <div className={styles.plot}>
            <div className={styles.yAxis}>
              {d.yTicks.map((t) => (
                <span
                  key={t.label + t.top}
                  className={styles.yTick}
                  style={{ top: t.top }}
                >
                  {t.label}
                </span>
              ))}
            </div>
            <div
              className={styles.canvas}
              onMouseMove={onHover}
              onMouseLeave={() => set({ hoverFrac: null })}
            >
              <svg
                viewBox="0 0 1000 400"
                preserveAspectRatio="none"
                className={styles.svg}
              >
                {[30, 112, 195, 277].map((y) => (
                  <line
                    key={y}
                    x1="0"
                    y1={y}
                    x2="1000"
                    y2={y}
                    stroke="var(--surface-alt)"
                    strokeWidth="1"
                    vectorEffect="non-scaling-stroke"
                  />
                ))}
                <line
                  x1="0"
                  y1="360"
                  x2="1000"
                  y2="360"
                  stroke="var(--line)"
                  strokeWidth="1"
                  vectorEffect="non-scaling-stroke"
                />
                <path
                  // Remounting on a new curve restarts the draw, so switching pool redraws.
                  key={d.aggPath}
                  className={styles.curve}
                  d={d.aggPath}
                  // Normalises the dash units, so the draw needs no measured length.
                  pathLength={1}
                  fill="none"
                  stroke="var(--green)"
                  strokeWidth="2.4"
                  strokeLinejoin="round"
                  strokeLinecap="round"
                  vectorEffect="non-scaling-stroke"
                />
              </svg>

              {d.hover && (
                <div className={styles.hoverLayer}>
                  <span
                    className={styles.hoverLine}
                    style={{ left: d.hover.left }}
                  />
                  <span
                    className={styles.hoverDot}
                    style={{
                      left: d.hover.left,
                      top: d.hover.dotTop,
                    }}
                  />
                  <div
                    className={styles.tooltip}
                    style={{
                      left: d.hover.left,
                      transform: d.hover.shift,
                    }}
                  >
                    <div className={styles.tipTop}>
                      <span className={styles.tipLabel}>Trade size</span>
                      <span className={styles.tipValue}>{d.hover.size}</span>
                    </div>
                    <div className={styles.tipMid}>
                      <span className={styles.tipLabel}>Effective price</span>
                      <span className={styles.tipValueLime}>
                        {d.hover.price}
                      </span>
                    </div>
                    <div className={styles.tipOut}>
                      <span className={styles.tipLabel}>Output</span>
                      <span className={styles.tipValue}>{d.hover.output}</span>
                    </div>
                    <div className={styles.tipFoot}>
                      <span className={styles.tipLabel}>Makers used</span>
                      <span className={styles.tipDots}>
                        {d.hover.dots.map((dot, i) => (
                          <span
                            key={i}
                            className={styles.tipDot}
                            style={{
                              background: dot.bg,
                            }}
                          />
                        ))}
                        <span className={styles.tipCount}>
                          {d.hover.makers}
                        </span>
                      </span>
                    </div>
                  </div>
                </div>
              )}

              <div className={styles.xAxis}>
                {d.xTicks.map((t) => (
                  <span
                    key={t.left}
                    className={styles.xTick}
                    style={{ left: t.left }}
                  >
                    {t.label}
                  </span>
                ))}
              </div>
            </div>
          </div>

          <div className={styles.axisTitle}>{d.axisTitle}</div>

          <div className={styles.impacts}>
            <div className={styles.impactFirst}>
              <div className={styles.impactLabel}>Best price</div>
              <div className={styles.impactValue}>{d.bestPrice}</div>
            </div>
            <div className={styles.impact}>
              <div className={styles.impactLabel}>{d.near.label}</div>
              <div className={styles.impactValue}>{d.near.price}</div>
              <div className={styles.impactSize}>{d.near.size}</div>
            </div>
            <div className={styles.impact}>
              <div className={styles.impactLabel}>{d.far.label}</div>
              <div className={styles.impactValue}>{d.far.price}</div>
              <div className={styles.impactSize}>{d.far.size}</div>
            </div>
            <div className={styles.impactLast}>
              <div className={styles.impactLabel}>Total liquidity</div>
              <div className={styles.impactValueGreen}>{d.totalLiq}</div>
            </div>
          </div>
        </section>

        <section className={styles.side}>
          <div className={styles.sideHead}>
            <div className={styles.sideTitleGroup}>
              <span className={styles.kpiSwatch} />
              <span className={styles.sideTitle}>Makers</span>
              <span className={styles.sideCount}>{d.makerTotal}</span>
            </div>
            <div className={styles.segmented}>
              {MAKER_SORTS.map((t) => (
                <button
                  key={t}
                  type="button"
                  className={
                    t === state.makerSort
                      ? styles.segmentSmallOn
                      : styles.segmentSmall
                  }
                  onClick={() => set({ makerSort: t })}
                >
                  {t}
                </button>
              ))}
            </div>
          </div>

          <div data-scroll="1" className={styles.makerList}>
            {d.makers.map((m) => (
              <button
                key={m.addr}
                type="button"
                className={styles.makerRow}
                onClick={() => {
                  set({
                    xpTrade: null,
                    xpStrat: {
                      maker: m.addr,
                      curve: m.curve,
                      pair: d.pair,
                    },
                  });
                  navigate("/explorer");
                }}
              >
                <span
                  className={styles.makerDot}
                  style={{ background: m.gap }}
                />
                <span className={styles.makerAddr}>{m.addr}</span>
                <span className={styles.makerAct}>{m.act}</span>
                <span
                  className={styles.makerState}
                  style={{
                    background: m.stateBg,
                    color: m.stateFg,
                  }}
                >
                  {m.up}
                </span>
              </button>
            ))}
          </div>

          <div className={styles.settleHead}>
            <div className={styles.settleTag}>
              <span className={styles.settlePulse} />
              <span className={styles.settleTitle}>Settlements</span>
            </div>
            <span className={styles.settleLive}>live</span>
          </div>

          <div data-scroll="1" className={styles.settleList}>
            {d.settlements.map((x) => (
              <button
                key={x.from + x.ago}
                type="button"
                className={styles.settleRow}
                onClick={() => {
                  set({ xpTrade: 0, xpStrat: null });
                  navigate("/explorer");
                }}
              >
                <span className={styles.settleFrom}>{x.from}</span>
                <span className={styles.settleArrow}>→</span>
                <span className={styles.settleTo}>{x.to}</span>
                <span className={styles.settleAgo}>{x.ago}</span>
              </button>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
