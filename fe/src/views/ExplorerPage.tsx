import { useState } from "react";
import { Link, useNavigate } from "react-router-dom";
import { Pagination } from "@/components/Pagination";
import { RebateList } from "@/components/RebateList";
import { RetryNotice } from "@/components/RetryNotice";
import { useLoadedPagination } from "@/components/useLoadedPagination";
import {
  activityRow,
  explorerUrl,
  explorerStats,
  tradeRow,
  uniswapXFeedRow,
} from "@/lib/explorer";
import { loadedPageLabel } from "@/lib/pagination";
import type { ActivityFilter, TradeFilter } from "@/ports/explorer";
import type { RebateSubmissionStatus } from "@/ports/rebates";
import { useAssets } from "@/services/assets";
import {
  useActivity,
  useExplorerStats,
  useTrades,
  useUniswapXFeed,
} from "@/services/explorer";
import { useConfig } from "@/services/system";
import { usePools } from "@/services/pools";
import {
  useActiveRebatePages,
  useExecuteRebate,
  useRebates,
} from "@/services/rebates";
import type { RebateExplorerStatus } from "@/state";
import { useApp } from "@/state";
import { useWalletAction } from "@/services/wallet";
import styles from "./explorer.module.css";

const TABS = ["Trades", "Activity", "Rebates", "UniswapX Feed"];

function rebateSubmissionLabel(status: RebateSubmissionStatus | undefined) {
  switch (status?.kind) {
    case "approving":
      return "Approve input token…";
    case "signing":
      return "Sign rebate…";
    case "submitting":
      return "Submitting rebate…";
    case "confirming":
      return "Confirming rebate…";
    case "preparing":
    default:
      return "Preparing rebate…";
  }
}

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
          <RetryNotice
            message={
              query.data ? "Couldn’t refresh trades." : "Couldn’t load trades."
            }
            onRetry={() => void query.refetch()}
          />
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
  const config = useConfig();
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
          <RetryNotice
            message={
              query.data
                ? "Couldn’t refresh activity."
                : "Couldn’t load activity."
            }
            onRetry={() => void query.refetch()}
          />
        )}
        {!query.isPending && !query.isError && rows.length === 0 && (
          <p className={styles.emptyNote}>
            {query.hasNextPage
              ? "No matching events on this page. Continue to an older page."
              : "No activity matches these filters."}
          </p>
        )}
        {rows.map((row) => {
          const txUrl = explorerUrl(
            config.data?.block_explorer_url,
            "tx",
            row.txHash,
          );
          const content = (
            <>
              <span className={styles.activityKindCell}>
                <span
                  className={styles.tradeCellValue}
                  title={row.strategyHash}
                >
                  {row.position}
                </span>
                <span className={styles.tradeCellLabel}>position</span>
              </span>
              <span className={styles.activityFlow}>{row.flow}</span>
              <span className={styles.tradeCell}>
                <span className={styles.tradeCellValue} title={row.maker}>
                  {row.who}
                </span>
                <span className={styles.tradeCellLabel}>maker</span>
              </span>
              <span className={styles.tradeCell}>
                <span className={styles.tradeBlk}>{row.tx}</span>
                <span className={styles.tradeCellLabel}>transaction</span>
              </span>
              <span className={styles.tradeStatusCell}>
                <span
                  className={styles.kindTag}
                  style={{ background: row.kindBg, color: row.kindFg }}
                >
                  {row.kind}
                </span>
                <span className={styles.tradeTx}>
                  {row.block} · {row.age}
                </span>
              </span>
            </>
          );
          return txUrl ? (
            <a
              key={row.id}
              className={`${styles.activityRow} ${styles.activityRowLink}`}
              href={txUrl}
              target="_blank"
              rel="noopener noreferrer"
              aria-label={`View ${row.kind} transaction on chain explorer`}
            >
              {content}
            </a>
          ) : (
            <div key={row.id} className={styles.activityRow}>
              {content}
            </div>
          );
        })}
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

function UniswapXFeedList() {
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]);
  const query = useUniswapXFeed(cursors.at(-1));
  const rows = query.data?.items.map(uniswapXFeedRow) ?? [];
  const page = cursors.length - 1;
  const hasNextPage = Boolean(query.data?.nextCursor);
  const first = page * 10 + 1;
  const last = page * 10 + rows.length;
  const pageLabel = rows.length
    ? `Showing ${first}–${last}${hasNextPage ? "" : ` of ${last}`} feed orders`
    : "No feed orders on this page";

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
            Loading UniswapX feed…
          </p>
        )}
        {query.isError && (
          <RetryNotice
            message={
              query.data
                ? "Couldn’t refresh the UniswapX feed."
                : "Couldn’t load the UniswapX feed."
            }
            onRetry={() => void query.refetch()}
          />
        )}
        {!query.isPending && !query.isError && rows.length === 0 && (
          <p className={styles.emptyNote}>No simulated orders available yet.</p>
        )}
        {rows.map((row) => (
          <div key={row.id} className={styles.feedRow}>
            <span className={styles.tradePair}>
              <span className={styles.tradePairName}>{row.pair}</span>
              <span className={styles.tradeBlk}>{row.source}</span>
            </span>
            <span className={styles.tradeFlow}>
              <span className={styles.tradeIn}>{row.input}</span>
              <span className={styles.tradeArrow}>→</span>
              <span className={styles.tradeOut}>{row.requiredOutput}</span>
            </span>
            <span className={styles.tradeCell}>
              <span className={styles.tradeCellValue}>{row.market}</span>
              <span className={styles.tradeCellLabel}>market rate</span>
            </span>
            <span className={styles.tradeCell}>
              <span className={styles.tradeCellValue}>
                {row.simulatedOutput}
              </span>
              <span className={styles.tradeCellLabel}>simulated output</span>
            </span>
            <span className={styles.tradeStatusCell}>
              <span className={styles.simulatedPill}>simulated</span>
              <span className={styles.tradeTx}>{row.batch}</span>
            </span>
          </div>
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

function ExplorerRebates({
  currentBlock,
  status,
}: {
  currentBlock?: number;
  status: RebateExplorerStatus;
}) {
  const navigate = useNavigate();
  const assets = useAssets();
  const active = status === "Active";
  const query = useRebates({ status: active ? "ready" : "executed" });
  const continuity = useActiveRebatePages(
    active ? query.data : undefined,
    query.dataUpdatedAt,
  );
  const execution = useExecuteRebate();
  const wallet = useWalletAction();

  function execute(id: string) {
    if (wallet.prepare()) execution.execute(id);
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
              label: !wallet.connected
                ? "Connect"
                : wallet.switchTo
                  ? `Switch to ${wallet.switchTo}`
                  : "Earn",
              onExecute: execute,
              pendingId: execution.pendingId,
              pendingLabel: rebateSubmissionLabel(execution.status),
              completedId: execution.completedId,
              failedId: execution.failedId,
              problem: execution.problem,
            }
          : undefined
      }
      currentBlock={currentBlock}
      highlightRowsOnHover
      onOpenTrade={(tradeId) =>
        navigate(`/explorer/trades/${encodeURIComponent(tradeId)}`)
      }
    />
  );
}

export function ExplorerPage() {
  const { state, set } = useApp();
  const pools = usePools();
  const stats = useExplorerStats();
  const isTrades = state.xpTab === "Trades";
  const isActivity = state.xpTab === "Activity";
  const isUniswapXFeed = state.xpTab === "UniswapX Feed";
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
              : isActivity
                ? "Protocol activity"
                : isUniswapXFeed
                  ? "UniswapX feed"
                  : "Rebates"}
          </div>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>
          {isTrades
            ? "Every intent through Solvent: pair, in → out, makers sourced, status, price impact and tx."
            : isActivity
              ? "Aqua-level events: makers registering strategies, pushing and pulling balance, and docking positions."
              : isUniswapXFeed
                ? "Public UniswapX Dutch orders evaluated against eight virtual makers using cached market pricing."
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
          {isTrades ? (
            <>
              <FilterDrop dkey="xpStatus" options={DROP_OPTIONS.xpStatus} />
              <FilterDrop dkey="xpPair" options={["All pairs", ...pairs]} />
            </>
          ) : isActivity ? (
            <>
              <FilterDrop dkey="xpType" options={DROP_OPTIONS.xpType} />
              <FilterDrop dkey="xpEnt" options={DROP_OPTIONS.xpEnt} />
            </>
          ) : isUniswapXFeed ? null : (
            <FilterDrop
              dkey="xpRebateStatus"
              options={DROP_OPTIONS.xpRebateStatus}
            />
          )}
        </div>
      </div>
      {isTrades ? (
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
      ) : isUniswapXFeed ? (
        <UniswapXFeedList />
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
