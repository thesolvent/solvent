import { useState } from "react";
import { Link } from "react-router-dom";
import { useConnectModal } from "@rainbow-me/rainbowkit";
import { useAccount, useSwitchChain } from "wagmi";
import { Pagination } from "@/components/Pagination";
import { RebateList } from "@/components/RebateList";
import { useLoadedPagination } from "@/components/useLoadedPagination";
import { chain } from "@/adapters/wallet/config";
import type { ObservedOrder } from "@/data/explorer";
import {
  activityRow,
  explorerStats,
  orderFlow,
  orderRow,
  tradeRow,
} from "@/lib/explorer";
import { loadedPageLabel } from "@/lib/pagination";
import type { ActivityFilter, TradeFilter } from "@/ports/explorer";
import { useAssets } from "@/services/assets";
import {
  useActivity,
  useExplorerStats,
  useObservedOrders,
  useTrades,
} from "@/services/explorer";
import { usePools } from "@/services/pools";
import {
  useActiveRebatePages,
  useExecuteRebate,
  useRebates,
} from "@/services/rebates";
import type { RebateExplorerStatus } from "@/state";
import { useApp } from "@/state";
import styles from "./explorer.module.css";

const TABS = ["Trades", "Activity", "Order feed", "Rebates"];

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
  xpRebateStatus: ["Active", "Confirmed"],
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
  const page = cursors.length - 1;
  const hasNextPage = Boolean(query.data?.nextCursor);
  const first = page * 10 + 1;
  const last = page * 10 + rows.length;
  const pageLabel = rows.length
    ? `Showing ${first}–${last}${hasNextPage ? "" : ` of ${last}`} trades`
    : "No trades on this page";

  function selectPage(nextPage: number) {
    if (nextPage < page) {
      setCursors((previous) => previous.slice(0, nextPage + 1));
      return;
    }
    if (nextPage === page + 1 && query.data?.nextCursor) {
      setCursors((previous) => [...previous, query.data.nextCursor]);
    }
  }

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
      <Pagination
        label={pageLabel}
        page={page}
        pageCount={cursors.length + Number(hasNextPage)}
        disabled={query.isFetching || query.isError}
        onPage={selectPage}
      />
    </>
  );
}

function ActivityList({ filter }: { filter: ActivityFilter }) {
  const query = useActivity(filter);
  const pages = query.data?.pages.map(({ items }) => items) ?? [];
  const pagination = useLoadedPagination(
    pages,
    Boolean(query.hasNextPage),
    async () => !(await query.fetchNextPage()).isError,
  );
  const rows = pagination.items.map(activityRow);
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
              ? "No matching events on this page. Continue to an older page."
              : "No activity matches these filters."}
          </p>
        )}
        {rows.map((row) => (
          <Link
            key={row.id}
            className={styles.activityRow}
            to={`/explorer/strategies/${row.strategyHash}`}
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
          </Link>
        ))}
      </div>
      <Pagination
        label={loadedPageLabel(
          pages,
          pagination.page,
          Boolean(query.hasNextPage),
          "events",
        )}
        page={pagination.page}
        pageCount={pagination.pageCount}
        disabled={query.isFetching || query.isError}
        onPage={(page) => void pagination.select(page)}
      />
    </>
  );
}

function ExplorerRebates({
  currentBlock,
  status,
}: {
  currentBlock?: number;
  status: RebateExplorerStatus;
}) {
  const assets = useAssets();
  const active = status === "Active";
  const query = useRebates({ status: active ? "ready" : "executed" });
  const continuity = useActiveRebatePages(
    active ? query.data : undefined,
    query.dataUpdatedAt,
  );
  const execution = useExecuteRebate();
  const { isConnected, chainId } = useAccount();
  const { openConnectModal } = useConnectModal();
  const { switchChain } = useSwitchChain();
  const wrongChain = isConnected && chainId !== chain.id;

  function execute(id: string) {
    if (!isConnected) return openConnectModal?.();
    if (wrongChain) return switchChain({ chainId: chain.id });
    execution.execute(id);
  }

  return (
    <RebateList
      assets={assets}
      pages={
        active
          ? continuity.pages
          : (query.data?.pages.map(({ items }) => items) ?? [])
      }
      pending={query.isPending}
      fetching={query.isFetching}
      error={query.isError}
      hasMore={Boolean(query.hasNextPage)}
      onRetry={() => void query.refetch()}
      onLoadMore={async () => !(await query.fetchNextPage()).isError}
      action={
        active
          ? {
              availableIds: continuity.availableIds,
              label: !isConnected
                ? "Connect"
                : wrongChain
                  ? "Switch network"
                  : "Earn",
              onExecute: execute,
              pendingId: execution.pendingId,
              completedId: execution.completedId,
              failedId: execution.failedId,
              problem: execution.problem,
            }
          : undefined
      }
      currentBlock={currentBlock}
    />
  );
}

export function ExplorerPage() {
  const { state, set } = useApp();
  const pools = usePools();
  const stats = useExplorerStats();
  const observed = useObservedOrders();
  const assets = useAssets();
  // The order log stores addresses; the catalog is what turns them into something readable.
  const tokenOf = (address: string) => {
    const asset = assets.find(
      (a) => a.address.toLowerCase() === address.toLowerCase(),
    );
    return {
      symbol: asset?.symbol ?? `${address.slice(0, 6)}…`,
      decimals: asset?.decimals ?? 18,
    };
  };
  const isTrades = state.xpTab === "Trades";
  const isActivity = state.xpTab === "Activity";
  const isOrderFeed = state.xpTab === "Order feed";
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
            {isTrades
              ? "Trades"
              : isOrderFeed
                ? "Order feed"
                : isActivity
                  ? "Protocol activity"
                  : "Rebates"}
          </div>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>
          {isTrades
            ? "Every intent through Solvent: pair, in → out, makers sourced, status, price impact and tx."
            : isOrderFeed
              ? "Everything the resolver was shown, whatever became of it: the pair, our quote, the venue, and the real outcome."
              : isActivity
                ? "Aqua-level events: makers registering strategies, pushing and pulling balance, and docking positions."
                : "Protected strategies share profitable price restoration between their maker and executor."}
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
                {stats.isError && stat.label === "Block height"
                  ? "updates delayed"
                  : stat.sub}
              </span>
            </div>
          </div>
        ))}
      </div>
      {stats.isError && (
        <p role="alert" className={styles.srOnly}>
          Couldn’t refresh stats. Retrying automatically.
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
          {isOrderFeed ? null : isTrades ? (
            <>
              <FilterDrop dkey="xpStatus" options={DROP_OPTIONS.xpStatus} />
              <FilterDrop dkey="xpPair" options={["All pairs", ...pairs]} />
            </>
          ) : isActivity ? (
            <>
              <FilterDrop dkey="xpType" options={DROP_OPTIONS.xpType} />
              <FilterDrop dkey="xpEnt" options={DROP_OPTIONS.xpEnt} />
            </>
          ) : (
            <FilterDrop
              dkey="xpRebateStatus"
              options={DROP_OPTIONS.xpRebateStatus}
            />
          )}
        </div>
      </div>
      {isOrderFeed ? (
        <OrderFeedList orders={observed.data} tokenOf={tokenOf} />
      ) : isTrades ? (
        state.xpPair === "All pairs" || pair ? (
          <TradeList key={JSON.stringify(filter)} filter={filter} />
        ) : (
          <p className={styles.emptyNote}>Selected pair is unavailable.</p>
        )
      ) : isActivity ? (
        <ActivityList
          key={JSON.stringify(activityFilter)}
          filter={activityFilter}
        />
      ) : (
        <ExplorerRebates
          key={state.xpRebateStatus}
          currentBlock={stats.data?.blockHeight}
          status={state.xpRebateStatus}
        />
      )}
    </div>
  );
}

/** The order feed as a peer of the trade and activity lists: everything the resolver was shown,
 *  what it would have cost us, and why most of it went nowhere. */
function OrderFeedList({
  orders,
  tokenOf,
}: {
  orders: ObservedOrder[] | undefined;
  tokenOf: (address: string) => { symbol: string; decimals: number };
}) {
  const flow = orderFlow(orders);
  const rows = (orders ?? []).map((order) => orderRow(order, tokenOf));
  return (
    <div data-scroll="1" className={styles.list} aria-label="Order feed">
      {rows.length === 0 ? (
        <p className={styles.emptyNote}>
          No orders seen yet. The feed records every order it is shown,
          including the ones this resolver cannot settle.
        </p>
      ) : (
        <>
          <div className={styles.orderSummary}>
            {flow.seen} seen · {flow.admitted} admitted ({flow.admittedPct}) ·{" "}
            {flow.dropped} refused
          </div>
          <div className={styles.orderHeadRow} aria-hidden="true">
            <span>Pair</span>
            <span>Taker pays</span>
            <span>Order wants</span>
            <span>Our sourcing cost</span>
            <span>Source</span>
            <span>State</span>
          </div>
          {rows.map((row) => (
            <div key={row.id} className={styles.orderRow}>
              <span className={styles.tradePair}>
                <span className={styles.tradePairName}>{row.pair}</span>
                <span className={styles.tradeBlk}>{row.hashLabel}</span>
              </span>
              <span className={styles.orderNum}>{row.input}</span>
              <span className={styles.orderNum}>{row.asked}</span>
              <span className={styles.orderNum}>{row.best}</span>
              <span className={styles.orderSource}>{row.source}</span>
              <span className={styles.tradeStatusCell}>
                <span className={styles.statusPill} style={row.stateStyle}>
                  {row.state}
                </span>
                <span className={styles.tradeTx}>{row.detail}</span>
              </span>
            </div>
          ))}
        </>
      )}
    </div>
  );
}
