import type { PointerEvent as ReactPointerEvent } from "react";

import { POOLS } from "@/data";
import { poolType } from "@/lib/format";
import { useApp, type PoolQuery } from "@/state";

import styles from "./PoolsPage.module.css";

const PAGE_SIZE = 4;

const QUERY_CELLS: {
  key: keyof PoolQuery;
  label: string;
  options: string[];
}[] = [
  {
    key: "ptype",
    label: "Pool type",
    options: ["All pools", "Stable", "Volatile", "Incentivised"],
  },
  {
    key: "sell",
    label: "Sell asset",
    options: ["Any", "ETH", "SOL", "USDC", "WBTC", "1INCH"],
  },
  {
    key: "buy",
    label: "Buy asset",
    options: ["Any", "USDC", "USDT", "ETH", "WBTC"],
  },
  {
    key: "fee",
    label: "Fee tier",
    options: ["Any", "0.01%", "0.05%", "0.30%", "1.00%"],
  },
  {
    key: "venue",
    label: "Venue",
    options: ["Any", "Aqua core", "Aqua extended", "Aqua incentive"],
  },
  {
    key: "apr",
    label: "Min APR",
    options: ["Any", "5%", "8%", "12%", "20%"],
  },
];

const FILTER_GROUPS = [
  {
    title: "Curve type",
    options: [
      { label: "Single-sided", hint: "" },
      { label: "Paired inventory", hint: "" },
      { label: "Concentrated", hint: "" },
    ],
  },
  {
    title: "Maker uptime",
    options: [
      { label: "Any", hint: "" },
      { label: "Above 95%", hint: "24h" },
      { label: "Above 99%", hint: "7d" },
    ],
  },
];

const SORTS = ["Best", "Highest APR", "Most TVL", "Newest"];

export function PoolsPage() {
  const { state, set, push } = useApp();

  const ptype = state.poolQuery.ptype || "All pools";
  const inType = (p: (typeof POOLS)[number]) =>
    ptype === "All pools" || poolType(p) === ptype;

  const matching = POOLS.filter(inType);
  const page = matching.slice(
    state.poolPage * PAGE_SIZE,
    state.poolPage * PAGE_SIZE + PAGE_SIZE,
  );
  const pageCount = Math.ceil(POOLS.length / PAGE_SIZE);

  // Matches the mock, which always surfaces the first pool: its recommender
  // filters on a state key nothing ever sets, so the filter never matches and it
  // falls through to POOLS[0]. Swap in the commented line to rank by APR instead.
  // const recommendation = [...matching].sort((a, b) => parseFloat(b.apr) - parseFloat(a.apr))[0] ?? POOLS[0];
  const recommendation = POOLS[0];

  const pageLabel =
    matching.length === 0
      ? "No pools match this type"
      : `Showing ${Math.min(state.poolPage * PAGE_SIZE + 1, matching.length)}–${Math.min(
          state.poolPage * PAGE_SIZE + PAGE_SIZE,
          matching.length,
        )} of ${matching.length} pools`;

  const onTvlDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const apply = (cx: number) =>
      set({
        tvlMin: Math.round(
          Math.min(100, Math.max(0, ((cx - r.left) / r.width) * 100)),
        ),
      });
    apply(e.clientX);
    const move = (ev: PointerEvent) => apply(ev.clientX);
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  };

  return (
    <div data-scroll="1" className={styles.root}>
      <div className={styles.queryBar}>
        {QUERY_CELLS.map((c) => {
          const open = state.openCell === c.key;
          return (
            <div key={c.key} className={styles.queryCell}>
              <button
                type="button"
                className={open ? styles.queryButtonOpen : styles.queryButton}
                onClick={() => set({ openCell: open ? null : c.key })}
              >
                <span className={styles.queryText}>
                  <span className={styles.queryLabel}>{c.label}</span>
                  <span className={styles.queryValue}>
                    {state.poolQuery[c.key]}
                  </span>
                </span>
                <span className={open ? styles.caretOpen : styles.caret}>
                  ▾
                </span>
              </button>
              {open && (
                <div className={styles.menu}>
                  {c.options.map((o) => {
                    const on = state.poolQuery[c.key] === o;
                    return (
                      <button
                        key={o}
                        type="button"
                        className={on ? styles.menuItemActive : styles.menuItem}
                        onClick={() =>
                          set({
                            poolQuery: {
                              ...state.poolQuery,
                              [c.key]: o,
                            },
                            openCell: null,
                          })
                        }
                      >
                        <span>{o}</span>
                        <span className={styles.menuMark}>{on ? "✓" : ""}</span>
                      </button>
                    );
                  })}
                </div>
              )}
            </div>
          );
        })}
        <button type="button" className={styles.searchAction}>
          <span className={styles.searchActionGlyph} />
        </button>
      </div>

      <div className={styles.recommend}>
        <span className={styles.recommendTag}>
          <span className={styles.pulse} />
          <span className={styles.recommendTagText}>Recommended</span>
        </span>
        <span
          key={ptype + recommendation.pair}
          className={styles.recommendBody}
        >
          <span className={styles.recommendPair}>{recommendation.pair}</span>
          <span className={styles.recommendMeta}>
            {recommendation.apr} net APR · {recommendation.tvl} depth ·{" "}
            {recommendation.fee}
          </span>
        </span>
        <span className={styles.recommendOpen}>Open pool ↗</span>
      </div>

      <div className={styles.body}>
        <aside>
          <div className={styles.filtersTitle}>Filters</div>
          {FILTER_GROUPS.map((g, gi) => (
            <div key={g.title} className={styles.filterGroup}>
              <div className={styles.filterHead}>
                <span className={styles.filterTitle}>{g.title}</span>
                <span className={styles.filterChevron}>▼</span>
              </div>
              <div className={styles.filterOptions}>
                {g.options.map((o, i) => (
                  <button
                    key={o.label}
                    type="button"
                    className={styles.filterOption}
                    onClick={() =>
                      set({
                        filterPick: {
                          ...state.filterPick,
                          [gi]: i,
                        },
                      })
                    }
                  >
                    <span
                      className={
                        state.filterPick[gi] === i
                          ? styles.radioOn
                          : styles.radio
                      }
                    />
                    <span className={styles.filterLabel}>{o.label}</span>
                    <span className={styles.filterHint}>{o.hint}</span>
                  </button>
                ))}
              </div>
            </div>
          ))}

          <div className={styles.tvl} onPointerDown={onTvlDrag}>
            <div
              className={styles.tvlFill}
              style={{ width: `${state.tvlMin}%` }}
            />
            <span
              className={styles.tvlHandle}
              style={{ left: `${state.tvlMin}%` }}
            >
              <span className={styles.tvlArrowLeft}>◀</span>
              <span className={styles.tvlArrowRight}>▶</span>
            </span>
            <div className={styles.tvlOverlay}>
              <span className={styles.tvlLabel}>Min TVL</span>
              <span className={styles.tvlValue}>
                ${(state.tvlMin / 10).toFixed(1)}M+
              </span>
            </div>
          </div>
        </aside>

        <div>
          <div className={styles.sorts}>
            {SORTS.map((t) => (
              <button
                key={t}
                type="button"
                className={
                  t === state.poolSort ? styles.sortActive : styles.sort
                }
                onClick={() => set({ poolSort: t })}
              >
                {t}
              </button>
            ))}
          </div>

          <div className={styles.poolList}>
            {page.map((p) => {
              const i = POOLS.indexOf(p);
              const picked = state.poolPicked === i;
              return (
                <button
                  key={p.pair}
                  type="button"
                  className={picked ? styles.poolPicked : styles.pool}
                  onClick={() => {
                    set({ poolPicked: i });
                    push({ detail: i }, "Pools");
                  }}
                >
                  <div className={styles.poolMain}>
                    <div className={styles.poolRule}>
                      <span className={styles.poolRuleTag}>Pair</span>
                      <span className={styles.poolRuleLine} />
                      <span className={styles.poolRuleValue}>{p.range}</span>
                      <span className={styles.poolRuleLine} />
                      <span className={styles.poolRuleTag}>Depth</span>
                    </div>
                    <div className={styles.poolFigures}>
                      <div style={{ minWidth: 0 }}>
                        <div className={styles.poolBig}>{p.pair}</div>
                        <div className={styles.poolSub}>{p.venue}</div>
                      </div>
                      <div style={{ textAlign: "right" }}>
                        <div className={styles.poolBig}>{p.tvl}</div>
                        <div className={styles.poolSub}>Depth</div>
                      </div>
                    </div>
                    <div className={styles.poolFoot}>
                      <span>{p.vol} 24h vol</span>
                      <span>{p.fills} fills</span>
                    </div>
                  </div>
                  <div className={styles.poolSide}>
                    <div className={styles.poolFee}>
                      <span className={styles.poolFeeChip} />
                      <div style={{ minWidth: 0 }}>
                        <div className={styles.poolMicro}>Fee tier</div>
                        <div className={styles.poolFeeValue}>{p.fee}</div>
                      </div>
                    </div>
                    <div className={styles.poolApr}>
                      <div className={styles.poolMicro}>Net APR</div>
                      <div className={styles.poolAprValue}>{p.apr}</div>
                    </div>
                  </div>
                </button>
              );
            })}
          </div>

          <div className={styles.pager}>
            <span className={styles.pagerLabel}>{pageLabel}</span>
            <div className={styles.pagerControls}>
              <button
                type="button"
                className={styles.pagerStep}
                style={{
                  opacity: state.poolPage === 0 ? 0.35 : 1,
                }}
                onClick={() =>
                  set({
                    poolPage: Math.max(0, state.poolPage - 1),
                  })
                }
              >
                ← Prev
              </button>
              {Array.from({ length: pageCount }, (_, i) => (
                <button
                  key={i}
                  type="button"
                  className={
                    i === state.poolPage
                      ? styles.pagerNumActive
                      : styles.pagerNum
                  }
                  onClick={() => set({ poolPage: i })}
                >
                  {i + 1}
                </button>
              ))}
              <button
                type="button"
                className={styles.pagerStep}
                style={{
                  opacity: state.poolPage >= pageCount - 1 ? 0.35 : 1,
                }}
                onClick={() =>
                  set({
                    poolPage: Math.min(pageCount - 1, state.poolPage + 1),
                  })
                }
              >
                Next →
              </button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
