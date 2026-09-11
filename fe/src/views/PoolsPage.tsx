import type { PointerEvent as ReactPointerEvent } from "react";
import { useNavigate } from "react-router-dom";

import type { Pool } from "@/data";
import { AsyncNote } from "@/components/AsyncNote";
import { Pagination } from "@/components/Pagination";

import {
  aprOptions,
  bestByApr,
  feeTierOptions,
  filterPools,
  poolTypeOptions,
  sortPools,
  POOL_SORTS,
  poolRanks,
  curveMark,
} from "@/lib/pools";
import { useAssetSymbols } from "@/services/assets";
import { slug, usePools } from "@/services/pools";
import { Icon } from "@/components/Icon";
import { Term } from "@/components/Tooltip";
import { useApp, type PoolQuery } from "@/state";

import styles from "./PoolsPage.module.css";

const PAGE_SIZE = 4;

type QueryCell = { key: keyof PoolQuery; label: string; options: string[] };

/** Every choice is drawn from what the server actually serves, so no option filters to nothing. */
function queryCells(symbols: string[], pools: Pool[]): QueryCell[] {
  const assets = ["Any", ...symbols];
  return [
    {
      key: "ptype",
      label: "Pool type",
      options: ["All pools", ...poolTypeOptions(pools)],
    },
    { key: "sell", label: "Sell asset", options: assets },
    { key: "buy", label: "Buy asset", options: assets },
    {
      key: "fee",
      label: "Fee tier",
      options: ["Any", ...feeTierOptions(pools)],
    },
    { key: "apr", label: "Min APR", options: ["Any", ...aprOptions(pools)] },
  ];
}

/** `filterPick` defaults to each group's first option, so "Any" must lead or the page loads filtered. */
const FILTER_GROUPS = [
  {
    title: "Curve type",
    options: [
      { label: "Any", hint: "" },
      { label: "Constant product", hint: "" },
      { label: "Concentrated", hint: "" },
      { label: "Pegged", hint: "" },
    ],
  },
];

const CURVE_GROUP = 0;

export function PoolsPage() {
  const { state, set } = useApp();
  const navigate = useNavigate();
  const poolsQuery = usePools();
  const pools = poolsQuery.data ?? [];
  const cells = queryCells(useAssetSymbols(), pools);
  const ptype = state.poolQuery.ptype;

  const curve =
    FILTER_GROUPS[CURVE_GROUP].options[state.filterPick[CURVE_GROUP] ?? 0]
      ?.label;
  const matching = sortPools(
    filterPools(pools, {
      query: state.poolQuery,
      tvlSliderPct: state.tvlMin,
      curve,
    }),
    state.poolSort,
  );
  const ranks = poolRanks(matching, state.poolSort);
  const page = matching.slice(
    state.poolPage * PAGE_SIZE,
    state.poolPage * PAGE_SIZE + PAGE_SIZE,
  );
  const pageCount = Math.ceil(matching.length / PAGE_SIZE);

  // The best yield among the pools the query admits; recommending an excluded pool would misdirect.
  const recommendation = bestByApr(matching);

  // An unread or failed list has no count to report, and no filter verdict to report either.
  const pageLabel =
    poolsQuery.isPending || poolsQuery.isError
      ? ""
      : matching.length === 0
        ? "No pools match this type"
        : `Showing ${Math.min(state.poolPage * PAGE_SIZE + 1, matching.length)}–${Math.min(
            state.poolPage * PAGE_SIZE + PAGE_SIZE,
            matching.length,
          )} of ${matching.length} pools`;

  const onTvlDrag = (e: ReactPointerEvent<HTMLDivElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const apply = (cx: number) =>
      set({
        poolPage: 0,
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
        {cells.map((c) => {
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
                            poolPage: 0,
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
        {recommendation ? (
          <>
            <span className={styles.recommendTag}>
              <span className={styles.pulse} />
              <span className={styles.recommendTagText}>Recommended</span>
            </span>
            <span
              key={ptype + recommendation.pair}
              className={styles.recommendBody}
            >
              <span className={styles.recommendPair}>
                {recommendation.pair}
              </span>
              <span className={styles.recommendMeta}>
                {recommendation.apr} net APR · {recommendation.tvl} depth ·{" "}
                {recommendation.fee}
              </span>
            </span>
            <span className={styles.recommendOpen}>Open pool ↗</span>
          </>
        ) : null}
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
                        poolPage: 0,
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
            {POOL_SORTS.map((t) => (
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
            <AsyncNote
              className={styles.listNote}
              query={poolsQuery}
              subject="pools"
            />
            {page.map((p) => (
              <button
                key={p.pair}
                type="button"
                className={styles.pool}
                onClick={() => navigate(`/pools/${slug(p.pair)}`)}
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
                      <div className={styles.poolBig}>
                        {p.pair}
                        {ranks[p.pair] && (
                          <span
                            className={styles.poolRank}
                            title={`#${ranks[p.pair]} by ${state.poolSort.toLowerCase()}`}
                          >
                            #{ranks[p.pair]}
                          </span>
                        )}
                      </div>
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
                    <span
                      aria-label={curveMark(p.curves).label}
                      className={styles.poolFeeChip}
                      role="img"
                      title={curveMark(p.curves).label}
                    >
                      <Icon
                        className={styles.poolFeeGlyph}
                        name={curveMark(p.curves).name}
                      />
                    </span>
                    <div style={{ minWidth: 0 }}>
                      <div className={styles.poolMicro}>
                        <Term term="feeTier">Fee tier</Term>
                      </div>
                      <div className={styles.poolFeeValue}>{p.fee}</div>
                    </div>
                  </div>
                  <div className={styles.poolApr}>
                    <div className={styles.poolMicro}>
                      <Term term="netApr">Net APR</Term>
                    </div>
                    <div className={styles.poolAprValue}>{p.apr}</div>
                  </div>
                </div>
              </button>
            ))}
          </div>

          <Pagination
            label={pageLabel}
            page={state.poolPage}
            pageCount={pageCount}
            onPage={(poolPage) => set({ poolPage })}
          />
        </div>
      </div>
    </div>
  );
}
