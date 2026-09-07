import { DROP_OPTIONS, explorerView, type DropKey } from "@/lib/explorer";
import { useApp } from "@/state";

import styles from "./explorer.module.css";

const TABS = ["Trades", "Activity"];

export function ExplorerPage() {
  const { state, set, push } = useApp();
  const xp = explorerView(state);

  const Drop = ({ dkey }: { dkey: DropKey }) => {
    const open = state.xpOpen === dkey;
    const current = state[dkey];
    return (
      <div className={styles.dropWrap}>
        <button
          type="button"
          className={open ? styles.dropButtonOpen : styles.dropButton}
          onClick={() => set({ xpOpen: open ? null : dkey })}
        >
          <span>{current}</span>
          <span className={open ? styles.dropCaretOpen : styles.dropCaret}>
            ▾
          </span>
        </button>
        {open && (
          <div className={styles.dropMenu}>
            {DROP_OPTIONS[dkey].map((o) => (
              <button
                key={o}
                type="button"
                className={current === o ? styles.dropItemOn : styles.dropItem}
                onClick={() => set({ [dkey]: o, xpOpen: null })}
              >
                <span>{o}</span>
                <span className={styles.dropMark}>
                  {current === o ? "✓" : ""}
                </span>
              </button>
            ))}
          </div>
        )}
      </div>
    );
  };

  return (
    <div className={styles.root}>
      <div className={styles.headWide}>
        <div className={styles.headTitle}>
          <div className={styles.eyebrow}>Explorer</div>
          <div className={styles.titleLg}>{xp.pageTitle}</div>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>{xp.pageDesc}</p>
      </div>

      <div className={styles.stats5}>
        {xp.stats.map((k) => (
          <div
            key={k.label}
            className={styles.stat}
            style={{
              backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
            }}
          >
            <div className={styles.statLabel}>{k.label}</div>
            <div className={styles.statRow}>
              <span className={styles.statValue}>{k.value}</span>
              <span className={styles.statSub} style={{ color: k.accent }}>
                {k.sub}
              </span>
            </div>
          </div>
        ))}
      </div>

      <div className={styles.filterBar}>
        <div className={styles.tabGroup}>
          {TABS.map((t) => (
            <button
              key={t}
              type="button"
              className={t === state.xpTab ? styles.tabOn : styles.tab}
              onClick={() => set({ xpTab: t, xpOpen: null })}
            >
              {t}
            </button>
          ))}
        </div>
        <div className={styles.drops}>
          {xp.isTrades ? (
            <>
              <Drop dkey="xpStatus" />
              <Drop dkey="xpPair" />
            </>
          ) : (
            <>
              <Drop dkey="xpType" />
              <Drop dkey="xpEnt" />
            </>
          )}
        </div>
      </div>

      {xp.isTrades ? (
        <>
          <div data-scroll="1" className={styles.list}>
            {xp.trades.map((t) => (
              <div
                key={t.index}
                className={styles.tradeRow}
                onClick={() => push({ xpTrade: t.index }, "Explorer")}
              >
                <span className={styles.tradePair}>
                  <span className={styles.tradePairName}>{t.pair}</span>
                  <span className={styles.tradeBlk}>{t.blk}</span>
                </span>
                <span className={styles.tradeFlow}>
                  <span className={styles.tradeIn}>{t.inn}</span>
                  <span className={styles.tradeArrow}>→</span>
                  <span className={styles.tradeOut}>{t.out}</span>
                </span>
                <span className={styles.tradeCell}>
                  <span className={styles.tradeCellValue}>{t.makers}</span>
                  <span className={styles.tradeCellLabel}>makers</span>
                </span>
                <span className={styles.tradeCell}>
                  <span className={styles.tradeCellValue}>{t.impact}</span>
                  <span className={styles.tradeCellLabel}>impact</span>
                </span>
                <span className={styles.tradeStatusCell}>
                  <span
                    className={styles.statusPill}
                    style={{
                      background: t.stBg,
                      color: t.stFg,
                    }}
                  >
                    {t.status}
                  </span>
                  <span className={styles.tradeTx}>{t.tx}</span>
                </span>
                <span className={styles.chevron}>›</span>
              </div>
            ))}
          </div>
          <div className={styles.footBar}>
            <button type="button" className={styles.footButton}>
              ← Prev
            </button>
            <span className={styles.footNote}>{xp.tradeNote}</span>
            <button type="button" className={styles.footButton}>
              Next →
            </button>
          </div>
        </>
      ) : (
        <>
          <div data-scroll="1" className={styles.list}>
            {xp.rows.map(({ row: r, hasTrade, kindBg, kindFg, target }, i) => (
              <button
                key={`${r.tx}-${i}`}
                type="button"
                className={styles.activityRow}
                onClick={() =>
                  target.kind === "trade"
                    ? push(
                        {
                          page: "Explorer",
                          xpTrade: target.index,
                        },
                        "Explorer",
                      )
                    : push(
                        {
                          page: "Explorer",
                          xpStrat: target.sel,
                        },
                        "Explorer",
                      )
                }
              >
                <div className={styles.activityTop}>
                  <span className={styles.mono}>{r.tx}</span>
                  <span
                    className={styles.kindTag}
                    style={{
                      background: kindBg,
                      color: kindFg,
                    }}
                  >
                    {r.kind}
                  </span>
                  <span className={styles.who}>{r.who}</span>
                  <span className={styles.spacer} />
                  <span className={styles.when}>
                    blk {r.blk} · {r.ago} ago
                  </span>
                </div>
                <div className={styles.activityBottom}>
                  <span className={styles.flow}>{r.flow}</span>
                  <span className={styles.spacer} />
                  {hasTrade && (
                    <span className={styles.tradeLinkWrap}>
                      <span className={styles.tradeLink}>
                        trade {r.trade} ↗
                      </span>
                    </span>
                  )}
                  <span className={styles.activityText}>{r.text}</span>
                </div>
              </button>
            ))}
          </div>
          <div className={styles.footBar}>
            <span className={styles.liveTag}>
              <span className={styles.livePulse} />
              <span>live · {xp.rowNote}</span>
            </span>
            <button type="button" className={styles.footButton}>
              Load more
            </button>
          </div>
        </>
      )}
    </div>
  );
}
