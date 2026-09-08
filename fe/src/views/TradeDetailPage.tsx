import { useEffect } from "react";
import { Link, useParams } from "react-router-dom";
import { useTrade } from "@/services/explorer";
import { useConfig } from "@/services/system";
import { Crumbs } from "@/components/Crumbs";
import { explorerUrl, tradeDetail, tradeProblem } from "@/lib/explorer";
import { useApp } from "@/state";
import { QueryFreshness } from "./QueryFreshness";

import styles from "./explorer.module.css";

export function TradeDetailPage() {
  const { state, set } = useApp();
  const { tradeId } = useParams();
  const query = useTrade(tradeId);
  const config = useConfig();
  // A direct trade URL must return to the list regardless of the previous strategy selection.
  useEffect(() => set({ xpStrat: null, trail: [] }), [set]);
  if (!query.data)
    return (
      <div className={styles.root}>
        <Link
          to="/explorer"
          className={styles.back}
          aria-label="Back to Explorer"
        >
          ←
        </Link>
        {query.isError ? (
          <p className={styles.emptyNote} role="alert">
            {tradeProblem(query.error)}{" "}
            <button type="button" onClick={() => void query.refetch()}>
              Try again
            </button>
          </p>
        ) : (
          <p className={styles.emptyNote} role="status">
            Loading trade…
          </p>
        )}
      </div>
    );
  const trade = query.data;
  const td = tradeDetail(trade, state.tdStage);
  const txUrl = explorerUrl(
    config.data?.block_explorer_url,
    "tx",
    trade.txHash,
  );

  return (
    <div className={styles.root}>
      <span
        role="status"
        aria-live="polite"
        aria-atomic="true"
        className={styles.srOnly}
      >
        Trade {td.status}. {td.stageDone} of 6 stages recorded.
      </span>
      <div className={styles.head}>
        <Link
          to="/explorer"
          className={styles.back}
          aria-label="Back to Explorer"
        >
          ←
        </Link>
        <div className={styles.headTitle}>
          <Crumbs
            current={td.id}
            trail={[{ label: "Explorer", to: "/explorer" }]}
          />
          <div className={styles.title}>{td.id}</div>
        </div>
        <span
          key={td.status}
          className={styles.liveStatus}
          style={{ background: td.stBg, color: td.stFg }}
        >
          {td.status}
        </span>
        <span className={styles.headMeta}>{td.meta}</span>
        <span className={styles.spacer} />
        <QueryFreshness query={query} />
        <span>
          <a
            className={styles.txLink}
            href={txUrl}
            target="_blank"
            rel="noopener noreferrer"
            aria-label="View transaction"
            title={trade.txHash ?? undefined}
          >
            {td.tx}
            {txUrl && " ↗"}
          </a>
        </span>
      </div>

      {query.isError && (
        <p role="alert" className={styles.emptyNote}>
          Couldn’t refresh this trade.{" "}
          <button type="button" onClick={() => void query.refetch()}>
            Try again
          </button>
        </p>
      )}
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
            <div className={styles.statValueSm} title={k.value}>
              {k.value}
            </div>
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

            <div
              className={styles.timeline}
              role="list"
              aria-label="Trade lifecycle"
            >
              <div className={styles.timelineMeta} aria-hidden="true">
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
                  role="listitem"
                  aria-label={`${st.label}: ${st.done ? "recorded" : st.state}`}
                  aria-current={st.current ? "step" : undefined}
                  style={{ left: st.barX }}
                  onMouseEnter={() => set({ tdStage: i })}
                  onMouseLeave={() => set({ tdStage: null })}
                >
                  <div
                    className={
                      st.current ? styles.stepBarCurrent : styles.stepBar
                    }
                    style={{
                      boxShadow: st.barShadow,
                      borderStyle: st.barStyle,
                      borderColor: st.barBd,
                      transform: st.scale,
                    }}
                  >
                    {st.done && (
                      <span
                        className={styles.stepFill}
                        style={{
                          animationDelay: st.delay,
                          background: st.barBg,
                        }}
                      />
                    )}
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
              open a maker in the block explorer
            </span>
          </div>
          <div data-scroll="1" className={styles.legList}>
            {td.legs.map((l) => (
              <a
                key={`${l.maker}:${l.hash}`}
                className={styles.legRow}
                href={explorerUrl(
                  config.data?.block_explorer_url,
                  "address",
                  l.maker,
                )}
                target="_blank"
                rel="noopener noreferrer"
                title={l.maker}
              >
                <span className={styles.legMaker}>
                  <span className={styles.legChip}>{l.tag}</span>
                  <span className={styles.legStack}>
                    <span className={styles.legName}>{l.name}</span>
                    <span className={styles.legHash} title={l.hash}>
                      {l.shortHash}
                    </span>
                  </span>
                </span>
                <span className={styles.legAmountCol}>
                  <span className={styles.legAmountRow}>
                    <span className={styles.legAmount} title={l.amt}>
                      {l.amt}
                    </span>
                  </span>
                  <span className={styles.legTrack}>
                    <span
                      className={styles.legFill}
                      style={{ width: l.barW }}
                    />
                  </span>
                </span>
                <span className={styles.legShare}>{l.share}</span>
                <span className={styles.legChevron}>↗</span>
              </a>
            ))}
            {td.empty && <div className={styles.emptyNote}>{td.emptyText}</div>}
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
                <span
                  className={styles.factValue}
                  title={d.fullValue ?? d.value}
                >
                  {d.value}
                </span>
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
