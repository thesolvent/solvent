import { useState } from "react";
import { Link } from "react-router-dom";
import {
  activityRow,
  explorerStats,
  explorerUrl,
  tradeRow,
} from "@/lib/explorer";
import type { ActivityFilter, TradeFilter } from "@/ports/explorer";
import { useActivity, useExplorerStats, useTrades } from "@/services/explorer";
import { usePools } from "@/services/pools";
import { useConfig } from "@/services/system";
import { useApp } from "@/state";
import { QueryFreshness } from "./QueryFreshness";
import styles from "./explorer.module.css";

const TABS = ["Trades", "Activity"];

const DROP_OPTIONS = {
  xpType: ["All types", "pull", "push", "dock", "register"],
  xpEnt: ["All entities", "Maker", "Resolver"],
  xpStatus: [
    "All status",
    "created",
    "quoted",
    "reserved",
    "simulated",
    "submitted",
    "confirmed",
    "declined",
    "failed",
  ],
  xpPair: ["All pairs"],
};
type DropKey = keyof typeof DROP_OPTIONS;

function FilterDrop({ dkey, options }: { dkey: DropKey; options: string[] }) {
  const { state, set } = useApp();
  const open = state.xpOpen === dkey;
  const current = state[dkey];
  return (
    <div className={styles.dropWrap}>
      <button
        type="button"
        className={open ? styles.dropButtonOpen : styles.dropButton}
        aria-expanded={open}
        onClick={() => set({ xpOpen: open ? null : dkey })}
      >
        <span>{current}</span>
        <span
          aria-hidden="true"
          className={open ? styles.dropCaretOpen : styles.dropCaret}
        >
          ▾
        </span>
      </button>
      {open && (
        <div className={styles.dropMenu}>
          {options.map((option) => (
            <button
              key={option}
              type="button"
              className={
                current === option ? styles.dropItemOn : styles.dropItem
              }
              onClick={() => set({ [dkey]: option, xpOpen: null })}
            >
              <span>{option}</span>
              <span className={styles.dropMark}>
                {current === option ? "✓" : ""}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function TradeList({ filter }: { filter: TradeFilter }) {
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]);
  const query = useTrades(filter, cursors.at(-1));
  const rows = query.data?.items.map(tradeRow) ?? [];
  return (
    <>
      <div data-scroll="1" className={styles.list} aria-busy={query.isFetching}>
        {query.isPending && (
          <p className={styles.emptyNote} role="status">
            Loading trades…
          </p>
        )}
        {query.isError && (
          <p className={styles.emptyNote} role="alert">
            {query.data ? "Couldn’t refresh trades." : "Couldn’t load trades."}{" "}
            <button type="button" onClick={() => void query.refetch()}>
              Try again
            </button>
          </p>
        )}
        {!query.isPending && !query.isError && rows.length === 0 && (
          <p className={styles.emptyNote}>No trades match these filters.</p>
        )}
        {rows.map((trade) => (
          <Link
            key={trade.id}
            className={styles.tradeRow}
            to={`/explorer/trades/${encodeURIComponent(trade.id)}`}
            aria-label={`Open trade ${trade.id}`}
          >
            <span className={styles.tradePair}>
              <span className={styles.tradePairName}>{trade.pair}</span>
              <span className={styles.tradeBlk}>{trade.blockLabel}</span>
            </span>
            <span className={styles.tradeFlow}>
              <span className={styles.tradeIn}>{trade.input}</span>
              <span className={styles.tradeArrow}>→</span>
              <span className={styles.tradeOut}>{trade.output}</span>
            </span>
            <span className={styles.tradeCell}>
              <span className={styles.tradeCellValue}>{trade.makers}</span>
              <span className={styles.tradeCellLabel}>makers</span>
            </span>
            <span className={styles.tradeCell}>
              <span className={styles.tradeCellValue}>{trade.impact}</span>
              <span className={styles.tradeCellLabel}>impact</span>
            </span>
            <span className={styles.tradeStatusCell}>
              <span className={styles.statusPill} style={trade.statusStyle}>
                {trade.status}
              </span>
              <span className={styles.tradeTx}>{trade.transactionLabel}</span>
            </span>
            <span className={styles.chevron}>›</span>
          </Link>
        ))}
      </div>
      <div className={styles.footBar}>
        <button
          type="button"
          className={styles.footButton}
          disabled={cursors.length === 1 || query.isFetching}
          onClick={() => setCursors((previous) => previous.slice(0, -1))}
        >
          ← Prev
        </button>
        <span className={styles.footNote}>
          Page {cursors.length} · {rows.length} shown
          <QueryFreshness query={query} />
        </span>
        <button
          type="button"
          className={styles.footButton}
          disabled={
            !query.data?.nextCursor || query.isFetching || query.isError
          }
          onClick={() => {
            if (query.data?.nextCursor)
              setCursors((previous) => [...previous, query.data.nextCursor]);
          }}
        >
          Next →
        </button>
      </div>
    </>
  );
}

function ActivityList({
  filter,
  explorerBase,
}: {
  filter: ActivityFilter;
  explorerBase: string | undefined;
}) {
  const query = useActivity(filter);
  const rows =
    query.data?.pages.flatMap((page) => page.items).map(activityRow) ?? [];
  return (
    <>
      <div data-scroll="1" className={styles.list} aria-busy={query.isFetching}>
        {query.isPending && (
          <p className={styles.emptyNote} role="status">
            Loading activity…
          </p>
        )}
        {query.isError && (
          <p className={styles.emptyNote} role="alert">
            {query.data
              ? "Couldn’t refresh activity."
              : "Couldn’t load activity."}{" "}
            <button type="button" onClick={() => void query.refetch()}>
              Try again
            </button>
          </p>
        )}
        {!query.isPending && !query.isError && rows.length === 0 && (
          <p className={styles.emptyNote}>
            {query.hasNextPage
              ? "No matching events loaded. Load more to search older events."
              : "No activity matches these filters."}
          </p>
        )}
        {rows.map((row) => (
          <a
            key={row.id}
            className={styles.activityRow}
            href={explorerUrl(explorerBase, "tx", row.txHash)}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={`View ${row.kind} transaction ${row.tx}`}
          >
            <div className={styles.activityTop}>
              <span className={styles.mono}>{row.tx}</span>
              <span
                className={styles.kindTag}
                style={{ background: row.kindBg, color: row.kindFg }}
              >
                {row.kind}
              </span>
              <span className={styles.who} title={row.maker}>
                {row.who}
              </span>
              <span className={styles.spacer} />
              <span className={styles.when}>{row.when}</span>
            </div>
            <div className={styles.activityBottom}>
              <span className={styles.flow}>{row.flow}</span>
              <span className={styles.spacer} />
              <span className={styles.activityText} title={row.strategyHash}>
                {row.text}
              </span>
            </div>
          </a>
        ))}
      </div>
      <div className={styles.footBar}>
        <span className={styles.liveTag}>
          <span>{rows.length} events loaded</span>
          <QueryFreshness query={query} />
        </span>
        <button
          type="button"
          className={styles.footButton}
          disabled={!query.hasNextPage || query.isFetching}
          onClick={() => void query.fetchNextPage()}
        >
          Load more
        </button>
      </div>
    </>
  );
}

export function ExplorerPage() {
  const { state, set } = useApp();
  const pools = usePools();
  const config = useConfig();
  const stats = useExplorerStats();
  const isTrades = state.xpTab === "Trades";
  const pairs = [...new Set(pools.map((pool) => pool.pair.replace(/\s/g, "")))];
  const pair = pools.find(
    (pool) => pool.pair.replace(/\s/g, "") === state.xpPair,
  )?.ref;
  const filter: TradeFilter = {
    ...(state.xpStatus === "All status" ? {} : { status: state.xpStatus }),
    ...(pair ? { base: pair.base, quote: pair.quote } : {}),
  };
  const activityFilter: ActivityFilter = {
    ...(state.xpType === "All types" ? {} : { kind: state.xpType }),
    ...(state.xpEnt === "All entities" ? {} : { entity: state.xpEnt }),
  };
  return (
    <div className={styles.root}>
      <div className={styles.headWide}>
        <div className={styles.headTitle}>
          <div className={styles.eyebrow}>Explorer</div>
          <div className={styles.titleLg}>
            {isTrades ? "Trades" : "Protocol activity"}
          </div>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>
          {isTrades
            ? "Every intent through Solvent: pair, in → out, makers sourced, status, price impact and tx."
            : "Aqua-level events: makers registering strategies, pushing and pulling balance, and docking positions."}
        </p>
      </div>
      <div className={styles.stats5}>
        {explorerStats(stats.data).map((stat) => (
          <div
            key={stat.label}
            className={styles.stat}
            style={{
              backgroundImage: `linear-gradient(${stat.sep}, ${stat.sep})`,
            }}
          >
            <div className={styles.statLabel}>{stat.label}</div>
            <div className={styles.statRow}>
              <span className={styles.statValue}>{stat.value}</span>
              <span className={styles.statSub} style={{ color: stat.accent }}>
                {stat.sub}
              </span>
            </div>
            {stat.label === "Block height" && <QueryFreshness query={stats} />}
          </div>
        ))}
      </div>
      {stats.isError && (
        <p role="alert" className={styles.emptyNote}>
          Couldn’t refresh stats.{" "}
          <button type="button" onClick={() => void stats.refetch()}>
            Try again
          </button>
        </p>
      )}
      <div className={styles.filterBar}>
        <div className={styles.tabGroup}>
          {TABS.map((tab) => (
            <button
              key={tab}
              type="button"
              className={tab === state.xpTab ? styles.tabOn : styles.tab}
              onClick={() => set({ xpTab: tab, xpOpen: null })}
            >
              {tab}
            </button>
          ))}
        </div>
        <div className={styles.drops}>
          {isTrades ? (
            <>
              <FilterDrop dkey="xpStatus" options={DROP_OPTIONS.xpStatus} />
              <FilterDrop dkey="xpPair" options={["All pairs", ...pairs]} />
            </>
          ) : (
            <>
              <FilterDrop dkey="xpType" options={DROP_OPTIONS.xpType} />
              <FilterDrop dkey="xpEnt" options={DROP_OPTIONS.xpEnt} />
            </>
          )}
        </div>
      </div>
      {isTrades ? (
        state.xpPair === "All pairs" || pair ? (
          <TradeList key={JSON.stringify(filter)} filter={filter} />
        ) : (
          <p className={styles.emptyNote}>Selected pair is unavailable.</p>
        )
      ) : (
        <ActivityList
          filter={activityFilter}
          explorerBase={config.data?.block_explorer_url}
        />
      )}
    </div>
  );
}
