import { Crumbs } from "@/components/Crumbs";
import { tradeDetail } from "@/lib/explorer";
import { useApp } from "@/state";

import styles from "./explorer.module.css";

export function TradeDetailPage() {
  const { state, set, push, pop } = useApp();
  const td = tradeDetail(state);

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <button type="button" className={styles.back} onClick={pop}>
          ←
        </button>
        <div className={styles.headTitle}>
          <Crumbs current={td.id} />
          <div className={styles.title}>{td.id}</div>
        </div>
        <span
          className={styles.statusPill}
          style={{ background: td.stBg, color: td.stFg }}
        >
          {td.status}
        </span>
        <span className={styles.headMeta}>{td.meta}</span>
        <span className={styles.spacer} />
        <span>
          <span className={styles.txLink}>{td.tx} ↗</span>
        </span>
      </div>

      <div className={styles.stats4}>
        {td.summary.map((k) => (
          <div
            key={k.label}
            className={styles.stat}
            style={{
              backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
            }}
          >
            <div className={styles.statLabel}>{k.label}</div>
            <div className={styles.statValueSm}>{k.value}</div>
          </div>
        ))}
      </div>

      <div className={styles.split}>
        <section className={styles.mainCol}>
          <div className={styles.lifecycle}>
            <div className={styles.lifecycleHead}>
              <div className={styles.lifecycleTitleRow}>
                <span className={styles.sectionTitle}>Lifecycle</span>
                <span className={styles.lifecycleCount}>
                  <span className={styles.lifecycleCountStrong}>
                    {td.stageDone}
                  </span>{" "}
                  of 6 stages complete
                </span>
              </div>
              <span className={styles.lifecycleTotal}>{td.headMeta} total</span>
            </div>

            <div className={styles.phases}>
              {td.phases.map((ph) => (
                <div key={ph.tag} style={{ minWidth: 0 }}>
                  <div className={styles.phaseTag}>{ph.tag}</div>
                  <div className={styles.phaseName}>{ph.name}</div>
                  <div
                    className={styles.phaseRule}
                    style={{ background: ph.rule }}
                  />
                </div>
              ))}
            </div>

            <div className={styles.timeline}>
              <div className={styles.timelineMeta}>
                {td.steps.map((st) => (
                  <div key={st.label} className={styles.timelineMetaCell}>
                    {st.meta}
                  </div>
                ))}
              </div>

              {td.steps.map((st, i) => (
                <div
                  key={st.label}
                  className={styles.step}
                  style={{ left: st.barX }}
                  onMouseEnter={() => set({ tdStage: i })}
                  onMouseLeave={() => set({ tdStage: null })}
                >
                  <div
                    className={styles.stepBar}
                    style={{
                      boxShadow: st.barShadow,
                      borderStyle: st.barStyle,
                      borderColor: st.barBd,
                      transform: st.scale,
                    }}
                  >
                    <span
                      className={styles.stepFill}
                      style={{
                        animationDelay: st.delay,
                        background: st.barBg,
                      }}
                    />
                  </div>
                  <div
                    className={styles.stepLead}
                    style={{ height: st.leadH }}
                  />
                  <div className={styles.stepText}>
                    <div className={styles.stepLabel} style={{ color: st.fg }}>
                      {st.label}
                    </div>
                    <div className={styles.stepState}>{st.state}</div>
                  </div>
                </div>
              ))}
            </div>
          </div>

          <div className={styles.sourcedHead}>
            <span className={styles.sourcedTitle}>Sourced from</span>
            <span className={styles.sourcedHint}>
              click a leg to open the maker
            </span>
          </div>
          <div data-scroll="1" className={styles.legList}>
            {td.legs.map((l) => (
              <button
                key={l.hash}
                type="button"
                className={styles.legRow}
                onClick={() => push({ xpStrat: l.sel }, td.id)}
              >
                <span className={styles.legMaker}>
                  <span className={styles.legChip}>{l.tag}</span>
                  <span className={styles.legStack}>
                    <span className={styles.legName}>{l.maker}</span>
                    <span className={styles.legHash}>{l.hash}</span>
                  </span>
                </span>
                <span className={styles.legAmountCol}>
                  <span className={styles.legAmountRow}>
                    <span className={styles.legAmount}>{l.amt}</span>
                    <span className={styles.legCurve}>{l.curve}</span>
                  </span>
                  <span className={styles.legTrack}>
                    <span
                      className={styles.legFill}
                      style={{ width: l.barW }}
                    />
                  </span>
                </span>
                <span className={styles.legShare}>{l.share}</span>
                <span className={styles.legChevron}>›</span>
              </button>
            ))}
            {td.empty && (
              <div className={styles.emptyNote}>
                No makers were sourced — the quote was declined before
                reservation.
              </div>
            )}
          </div>
        </section>

        <section data-scroll="1" className={styles.sideCol}>
          <div className={styles.profit}>
            <div className={styles.profitHead}>
              <span className={styles.profitSwatch} />
              <span className={styles.profitLabel}>Expected profit</span>
            </div>
            <div className={styles.profitValue}>{td.profit}</div>
            <div className={styles.profitTag}>{td.profitTag}</div>
          </div>

          <div className={styles.facts}>
            <div className={styles.factsTitle}>Order details</div>
            {td.facts.map((d) => (
              <div key={d.label} className={styles.factRow}>
                <span className={styles.factLabel}>{d.label}</span>
                <span className={styles.factValue}>{d.value}</span>
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
