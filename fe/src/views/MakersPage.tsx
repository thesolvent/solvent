import { DEFAULT_MAKER, ROSTER, SPANS, makerView } from "@/lib/makers";
import { useApp } from "@/state";
import { useNavigate } from "react-router-dom";

import styles from "./MakersPage.module.css";

const POS_ACTIONS = ["Push", "Dock"];
const LAT_DAYS = ["S", "M", "T", "W", "T", "F", "S"];

export function MakersPage() {
  const navigate = useNavigate();
  const { state, set, push } = useApp();
  const mk = makerView(state);

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <div className={styles.headTitle}>
          <div className={styles.headLabel}>Maker</div>
          <div className={styles.addr}>{mk.addr}</div>
        </div>
        <div className={styles.segmented}>
          {ROSTER.map((a) => (
            <button
              key={a}
              type="button"
              className={
                a === (state.mkSel ?? DEFAULT_MAKER)
                  ? styles.rosterButtonOn
                  : styles.rosterButton
              }
              onClick={() => set({ mkSel: a })}
            >
              {a.split("…")[1]}
            </button>
          ))}
        </div>
        <span className={styles.spacer} />
        <div className={styles.spanGroup}>
          {SPANS.map((t) => (
            <button
              key={t}
              type="button"
              className={
                t === mk.span ? styles.spanButtonOn : styles.spanButton
              }
              onClick={() => set({ mkSpan: t })}
            >
              {t}
            </button>
          ))}
        </div>
        <button
          type="button"
          className={styles.newPos}
          onClick={() =>
            push(
              {
                page: "Pools",
                detail: state.detail ?? 0,
                create: true,
              },
              "Makers",
            )
          }
        >
          <span>Create position</span>
          <span className={styles.newPosPlus}>+</span>
        </button>
      </div>

      <div className={styles.kpis}>
        {mk.kpis.map((k) => (
          <div
            key={k.label}
            className={styles.kpi}
            style={{
              backgroundColor: k.bg,
              backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
            }}
          >
            <div className={styles.kpiLabel}>{k.label}</div>
            <div className={styles.kpiRow}>
              <span className={styles.kpiValue}>{k.value}</span>
              <span className={styles.kpiDelta} style={{ color: k.deltaFg }}>
                {k.delta}
              </span>
            </div>
          </div>
        ))}
      </div>

      <div className={styles.body}>
        <section className={styles.main}>
          <div className={styles.tabBar}>
            <div className={styles.tabGroup}>
              {mk.tabs.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  className={t.key === mk.tab ? styles.tabOn : styles.tab}
                  onClick={() => set({ mkTab: t.key })}
                >
                  {t.label}
                </button>
              ))}
            </div>
            <span className={styles.tabNote}>{mk.tabNote}</span>
          </div>

          {mk.tab === "Positions" && (
            <div data-scroll="1" className={styles.list}>
              {mk.positions.map((p, i) => (
                <div key={p.pair} className={styles.posGroup}>
                  <div
                    className={p.open ? styles.posRowOpen : styles.posRow}
                    onClick={() => set({ mkOpen: p.open ? -1 : i })}
                  >
                    <span className={p.open ? styles.caretOpen : styles.caret}>
                      ▸
                    </span>
                    <span className={styles.posPair}>{p.pair}</span>
                    <span className={styles.posMeta}>{p.meta}</span>
                    <span className={styles.posCov}>{p.cov}</span>
                    <span
                      className={styles.posWidth}
                      style={{
                        background: p.widthBg,
                        color: p.widthFg,
                      }}
                    >
                      {p.width}
                    </span>
                    <span className={styles.posActions}>
                      {POS_ACTIONS.map((label) => (
                        <button
                          key={label}
                          type="button"
                          className={styles.posAction}
                          onClick={(e) => e.stopPropagation()}
                        >
                          {label}
                        </button>
                      ))}
                    </span>
                  </div>

                  {p.open && (
                    <div className={styles.posDetail}>
                      <div className={styles.posStats}>
                        {p.stats.map((st) => (
                          <div
                            key={st.label}
                            className={styles.posStat}
                            style={{
                              backgroundImage: `linear-gradient(${st.sep}, ${st.sep})`,
                            }}
                          >
                            <div className={styles.posStatLabel}>
                              {st.label}
                            </div>
                            <div className={styles.posStatValue}>
                              {st.value}
                            </div>
                          </div>
                        ))}
                      </div>
                      <div className={styles.coverage}>
                        <span className={styles.coverageLabel}>
                          Coverage {p.covNum}
                        </span>
                        <span className={styles.coverageBar}>
                          <span
                            className={styles.coverageA}
                            style={{
                              width: p.splitA,
                            }}
                          >
                            {p.labelA}
                          </span>
                          <span className={styles.coverageB}>{p.labelB}</span>
                        </span>
                        <button type="button" className={styles.clone}>
                          Clone
                        </button>
                      </div>
                    </div>
                  )}
                </div>
              ))}
            </div>
          )}

          {mk.tab === "Assets" && (
            <>
              <div className={styles.assetHead}>
                <span />
                <span>Token</span>
                <span className={styles.right}>Wallet</span>
                <span className={styles.right}>Shared liq.</span>
                <span className={styles.right}>Fees · APY</span>
                <span className={styles.right}>Ratio</span>
              </div>
              <div data-scroll="1" className={styles.list}>
                {mk.assets.map((t, i) => (
                  <div key={t.sym} className={styles.assetGroup}>
                    <div
                      className={t.open ? styles.assetRowOpen : styles.assetRow}
                      onClick={() =>
                        set({
                          mkAsset: t.open ? -1 : i,
                        })
                      }
                    >
                      <span className={styles.assetGlyph}>
                        <span
                          className={
                            t.open ? styles.assetCaretOpen : styles.assetCaret
                          }
                        >
                          ▸
                        </span>
                        <span
                          className={styles.assetChip}
                          style={{
                            background: t.tint,
                          }}
                        >
                          {t.sym}
                        </span>
                      </span>
                      <span className={styles.stack}>
                        <span className={styles.cellStrong}>{t.sym}</span>
                        <span className={styles.cellSub}>{t.across}</span>
                      </span>
                      <span className={styles.stackRight}>
                        <span className={styles.cellStrong}>{t.wallet}</span>
                        <span className={styles.cellSub}>{t.walletAmt}</span>
                      </span>
                      <span className={styles.stackRight}>
                        <span className={styles.cellStrong}>{t.shared}</span>
                        <span className={styles.cellSub}>{t.sharedAmt}</span>
                      </span>
                      <span className={styles.stackRight}>
                        <span className={styles.cellNum}>{t.fees}</span>
                        <span className={styles.cellSub}>{t.apy}</span>
                      </span>
                      <span className={styles.cellRatio}>{t.ratio}</span>
                    </div>

                    {t.open && (
                      <div className={styles.legPanel}>
                        <div className={styles.legHead}>
                          <span>Position</span>
                          <span className={styles.right}>Current</span>
                          <span className={styles.right}>Opening</span>
                          <span className={styles.right}>Fees · APY</span>
                          <span className={styles.right}>Cov.</span>
                        </div>
                        {t.legs.map((l) => (
                          <div key={l.pair} className={styles.legRow}>
                            <span className={styles.stack}>
                              <span className={styles.legPair}>{l.pair}</span>
                              <span className={styles.cellSub}>{l.meta}</span>
                            </span>
                            <span className={styles.stackRight}>
                              <span className={styles.legCell}>{l.cur}</span>
                              <span className={styles.cellSub}>{l.curUsd}</span>
                            </span>
                            <span className={styles.legCell}>{l.op}</span>
                            <span className={styles.stackRight}>
                              <span className={styles.legCell}>{l.fees}</span>
                              <span className={styles.cellSub}>{l.apy}</span>
                            </span>
                            <span className={styles.legCov}>{l.cov}</span>
                          </div>
                        ))}
                      </div>
                    )}
                  </div>
                ))}
              </div>
            </>
          )}

          {mk.tab === "Settlements" && (
            <div data-scroll="1" className={styles.list}>
              {mk.settlements.map((t, i) => (
                <div
                  key={`${t.pair}-${i}`}
                  className={styles.settleRow}
                  onClick={() => {
                    set({ xpStrat: null });
                    navigate("/explorer");
                  }}
                >
                  <span className={styles.settlePair}>
                    <span className={styles.settlePairName}>{t.pair}</span>
                    <span className={styles.settleBlk}>{t.blk}</span>
                  </span>
                  <span className={styles.settleFlow}>
                    <span className={styles.settleIn}>{t.inn}</span>
                    <span className={styles.settleArrow}>→</span>
                    <span className={styles.settleOut}>{t.out}</span>
                  </span>
                  <span className={styles.settleCell}>
                    <span className={styles.settleCellValue}>{t.fee}</span>
                    <span className={styles.settleCellLabel}>fee</span>
                  </span>
                  <span className={styles.settleCell}>
                    <span className={styles.settleCellValue}>{t.share}</span>
                    <span className={styles.settleCellLabel}>of fill</span>
                  </span>
                  <span className={styles.settleStatusCell}>
                    <span
                      className={styles.settleStatus}
                      style={{
                        background: t.stBg,
                        color: t.stFg,
                      }}
                    >
                      {t.status}
                    </span>
                    <span className={styles.settleTx}>{t.tx}</span>
                  </span>
                  <span className={styles.settleChevron}>›</span>
                </div>
              ))}
            </div>
          )}

          <div className={styles.insight}>
            <span className={styles.insightTag}>Insight</span>
            <span className={styles.insightText}>{mk.insight}</span>
            <span className={styles.insightChevron}>›</span>
          </div>
        </section>

        <div data-scroll="1" className={styles.rail}>
          <section className={styles.card}>
            <div className={styles.cardHead}>
              <span className={styles.swatch} />
              <span className={styles.cardTitle}>Fill share</span>
              <span className={styles.spacer} />
              <span className={styles.cardMore}>···</span>
            </div>
            <div className={styles.donutRow}>
              <div className={styles.donutWrap}>
                <svg viewBox="0 0 120 120" className={styles.donutSvg}>
                  <circle
                    cx="60"
                    cy="60"
                    r="46"
                    fill="none"
                    stroke="var(--surface-alt)"
                    strokeWidth="11"
                    strokeLinecap="round"
                    strokeDasharray={mk.trackDash}
                  />
                  {mk.arcs.map((arc, i) => (
                    <circle
                      key={i}
                      cx="60"
                      cy="60"
                      r="46"
                      fill="none"
                      strokeLinecap="round"
                      strokeWidth="11"
                      className={styles.donutArc}
                      stroke={arc.color}
                      strokeDasharray={arc.dash}
                      strokeDashoffset={arc.offset}
                      opacity={arc.op}
                      onMouseEnter={() => set({ mkTip: i })}
                      onMouseLeave={() => set({ mkTip: null })}
                    />
                  ))}
                </svg>
                <div className={styles.donutCenter}>
                  <div className={styles.donutCap}>{mk.donutCap}</div>
                  <div className={styles.donutVal}>{mk.donutVal}</div>
                </div>
              </div>
              <div className={styles.shareList}>
                {mk.shares.map((s, i) => (
                  <div
                    key={s.label}
                    className={styles.shareRow}
                    style={{ opacity: s.op }}
                    onMouseEnter={() => set({ mkTip: i })}
                    onMouseLeave={() => set({ mkTip: null })}
                  >
                    <span
                      className={styles.shareDot}
                      style={{ background: s.dot }}
                    />
                    <span className={styles.shareLabel}>{s.label}</span>
                    <span className={styles.shareValue}>{s.value}</span>
                  </div>
                ))}
              </div>
            </div>
            {mk.tip && (
              <div className={styles.donutTip}>
                <div className={styles.donutTipLabel}>{mk.tipLabel}</div>
                <div className={styles.donutTipRow}>
                  <span className={styles.donutTipPct}>{mk.tipPct}</span>
                  <span className={styles.donutTipAmt}>{mk.tipAmt}</span>
                </div>
              </div>
            )}
          </section>

          <section className={styles.card}>
            <div className={styles.cardHead}>
              <span className={styles.swatch} />
              <span className={styles.cardTitle}>Fills</span>
              <span className={styles.spacer} />
              <span className={styles.cardMore}>···</span>
            </div>
            <div className={styles.metricRow}>
              <span className={styles.metricValue}>{mk.fills}</span>
              <span className={styles.metricDelta}>{mk.fillsDelta}</span>
              <span className={styles.metricNote}>vs prev period</span>
            </div>
            <div className={styles.barsWrap}>
              <div className={styles.avgLine} style={{ top: mk.avgTop }} />
              <div className={styles.avgChip} style={{ top: mk.avgTop }}>
                {mk.avgVal}
              </div>
              <div className={styles.bars}>
                {mk.bars.map((b, i) => (
                  <span
                    key={i}
                    className={styles.barCol}
                    onMouseEnter={() => set({ mkBar: i })}
                    onMouseLeave={() => set({ mkBar: null })}
                  >
                    <span
                      className={styles.bar}
                      style={{
                        height: b.h,
                        background: b.bg,
                      }}
                    >
                      {b.tip && (
                        <span className={styles.barTip}>{b.value}</span>
                      )}
                    </span>
                    <span className={styles.barDay} style={{ color: b.dayFg }}>
                      {b.day}
                    </span>
                  </span>
                ))}
              </div>
            </div>
          </section>

          <section className={styles.cardLast}>
            <div className={styles.cardHead}>
              <span className={styles.swatch} />
              <span className={styles.cardTitle}>Fill latency</span>
              <span className={styles.spacer} />
              <span className={styles.cardMore}>···</span>
            </div>
            <div className={styles.latMetric}>
              <span className={styles.metricValue}>{mk.latency}</span>
              <span className={styles.latUnit}>p50</span>
              <span className={styles.metricNote}>{mk.latDelta}</span>
            </div>
            <div className={styles.latWrap}>
              <div className={styles.latAxis}>
                <span>1.2k</span>
                <span>600</span>
                <span>0</span>
              </div>
              <div className={styles.latPlot}>
                <svg
                  viewBox="0 0 420 120"
                  preserveAspectRatio="none"
                  className={styles.latSvg}
                >
                  {[4, 60, 116].map((y) => (
                    <line
                      key={y}
                      x1="0"
                      y1={y}
                      x2="420"
                      y2={y}
                      stroke="var(--surface-alt)"
                      strokeWidth="1"
                      vectorEffect="non-scaling-stroke"
                    />
                  ))}
                  <polyline
                    points={mk.latLine}
                    fill="none"
                    stroke="var(--green)"
                    strokeWidth="2"
                    strokeLinejoin="round"
                    strokeLinecap="round"
                    vectorEffect="non-scaling-stroke"
                  />
                </svg>
                {mk.latPts.map((pt, i) => (
                  <span
                    key={i}
                    className={styles.latHit}
                    style={{ left: pt.left }}
                    onMouseEnter={() => set({ mkLat: i })}
                    onMouseLeave={() => set({ mkLat: null })}
                  >
                    {pt.tip && (
                      <>
                        <span
                          className={styles.latDot}
                          style={{ top: pt.top }}
                        />
                        <span
                          className={styles.latTip}
                          style={{
                            top: pt.top,
                            transform: `translate(${pt.shift}, -160%)`,
                          }}
                        >
                          {pt.value}
                        </span>
                      </>
                    )}
                  </span>
                ))}
              </div>
              <div className={styles.latDays}>
                {LAT_DAYS.map((d, i) => (
                  <span key={i}>{d}</span>
                ))}
              </div>
            </div>
          </section>
        </div>
      </div>
    </div>
  );
}
