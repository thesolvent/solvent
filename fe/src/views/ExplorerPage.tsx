import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { Pagination } from "@/components/Pagination";
import { RebateList } from "@/components/RebateList";
import { useLoadedPagination } from "@/components/useLoadedPagination";
import {
  activityRow,
  explorerStats,
  ownTrades,
  tradeRow,
} from "@/lib/explorer";
import { loadedPageLabel } from "@/lib/pagination";
import type { ActivityFilter, TradeFilter } from "@/ports/explorer";
import type { RebateSubmissionStatus } from "@/ports/rebates";
import { useAssets } from "@/services/assets";
import {
  useActivity,
  useCrossChainOrders,
  useExplorerStats,
  useTrades,
} from "@/services/explorer";
import { usePools } from "@/services/pools";
import { useWalletAction } from "@/services/wallet";
import {
  useActiveRebatePages,
  useExecuteRebate,
  useRebates,
} from "@/services/rebates";
import { Term } from "@/components/Tooltip";
import { QueryFreshness } from "./QueryFreshness";
import { EVENT_TERMS, STATUS_TERMS } from "@/lib/glossary";

import styles from "./explorer.module.css";

const TABS = ["Trades", "Activity", "Rebates", "Your Trades"] as const;

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

/** Trades are served ten to a page; the placeholder shows the page that is coming. */
const PAGE_SIZE = 10;
/** Activity rows are twice the height, so fewer fill the same list. */
const ACTIVITY_PLACEHOLDER_ROWS = 6;

/** Each filter's options, the first being its default. Tab and filters live in the query string so
 *  a view can be shared, restored by Back and survive a reload; a default is left out of the URL,
 *  so a bare /explorer still opens on unfiltered Trades. */
const FILTERS = {
  status: [
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
  pair: ["All pairs"],
  type: ["All types", "pull", "push", "dock", "register"],
  entity: ["All entities", "Maker", "Resolver"],
  rebates: ["Active", "Confirmed"],
} as const;

type FilterKey = keyof typeof FILTERS;
type Options = readonly [string, ...string[]];

/** Matched case-insensitively, so `?type=pull` and `?entity=maker` read the way anyone would type
 *  them. Anything unrecognised falls back to the default rather than filtering to nothing. */
function chosen(options: Options, raw: string | null): string {
  if (!raw) return options[0];
  return (
    options.find((option) => option.toLowerCase() === raw.toLowerCase()) ??
    options[0]
  );
}

function FilterDrop({
  value,
  options,
  open,
  onToggle,
  onSelect,
}: {
  value: string;
  options: readonly string[];
  open: boolean;
  onToggle: () => void;
  onSelect: (value: string) => void;
}) {
  return (
    <div className={styles.dropWrap}>
      <button
        type="button"
        className={open ? styles.dropButtonOpen : styles.dropButton}
        aria-expanded={open}
        onClick={onToggle}
      >
        <span>{value}</span>
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
              className={value === option ? styles.dropItemOn : styles.dropItem}
              onClick={() => onSelect(option)}
            >
              <span>{option}</span>
              <span className={styles.dropMark}>
                {value === option ? "✓" : ""}
              </span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** One line of text, standing in for a value that has not arrived. */
function Bar({ className, width }: { className: string; width: string }) {
  return (
    <span className={`${className} ${styles.skeleton}`} style={{ width }}>
      &nbsp;
    </span>
  );
}

/**
 * Rows in the shape of the ones being fetched.
 *
 * A line of prose where a dense list will land moves every row the moment data arrives, so the
 * placeholder is the real row markup with its values blanked: same grid, same line boxes, same
 * height. Hidden from assistive tech — the list's own `role="status"` does the announcing.
 */
function PlaceholderRows({
  rows,
  variant,
}: {
  rows: number;
  variant: "trade" | "activity";
}) {
  return (
    <>
      {Array.from({ length: rows }, (_, index) =>
        variant === "trade" ? (
          <div
            key={index}
            className={styles.placeholderRow}
            data-placeholder="row"
            aria-hidden="true"
          >
            <span className={styles.tradePair}>
              <Bar className={styles.tradePairName} width="58%" />
              <Bar className={styles.tradeBlk} width="40%" />
            </span>
            <span className={styles.tradeFlow}>
              <Bar className={styles.tradeIn} width="42%" />
              <Bar className={styles.tradeOut} width="42%" />
            </span>
            <span className={styles.tradeCell}>
              <Bar className={styles.tradeCellValue} width="40%" />
              <Bar className={styles.tradeCellLabel} width="72%" />
            </span>
            <span className={styles.tradeCell}>
              <Bar className={styles.tradeCellValue} width="62%" />
              <Bar className={styles.tradeCellLabel} width="58%" />
            </span>
            <span className={styles.tradeStatusCell}>
              <Bar className={styles.statusPill} width="58px" />
              <Bar className={styles.tradeTx} width="84%" />
            </span>
            <span className={styles.chevron} />
          </div>
        ) : (
          <div
            key={index}
            className={styles.placeholderActivityRow}
            data-placeholder="row"
            aria-hidden="true"
          >
            <div className={styles.activityTop}>
              <Bar className={styles.mono} width="112px" />
              <Bar className={styles.kindTag} width="46px" />
              <Bar className={styles.who} width="88px" />
              <span className={styles.spacer} />
              <Bar className={styles.when} width="132px" />
            </div>
            <div className={styles.activityBottom}>
              <Bar className={styles.flow} width="28%" />
              <span className={styles.spacer} />
              <Bar className={styles.activityText} width="26%" />
            </div>
          </div>
        ),
      )}
    </>
  );
}

/** One trade row. Two lists render it: every trade, and the reader's own. */
function TradeRow({ trade }: { trade: ReturnType<typeof tradeRow> }) {
  return (
    <Link
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
          {STATUS_TERMS[trade.status] ? (
            <Term term={STATUS_TERMS[trade.status]}>{trade.status}</Term>
          ) : (
            trade.status
          )}
        </span>
        <span className={styles.tradeTx}>{trade.transactionLabel}</span>
      </span>
      <span className={styles.chevron}>›</span>
    </Link>
  );
}

function TradeList({ filter }: { filter: TradeFilter }) {
  const [cursors, setCursors] = useState<(string | undefined)[]>([undefined]);
  const query = useTrades(filter, cursors.at(-1));
  const rows = query.data?.items.map(tradeRow) ?? [];
  const page = cursors.length - 1;
  const hasNextPage = Boolean(query.data?.nextCursor);
  const first = page * PAGE_SIZE + 1;
  const last = page * PAGE_SIZE + rows.length;
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
          <>
            <p className={styles.srOnly} role="status">
              Loading trades…
            </p>
            <PlaceholderRows rows={PAGE_SIZE} variant="trade" />
          </>
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
          <TradeRow key={trade.id} trade={trade} />
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

/**
 * Everything one person has traded, both modes at once.
 *
 * Same-chain trades are served by each deployment; cross-chain orders live only in the proxy's
 * saga store. They are separate reads because they are separate stores, and merged here so the
 * reader sees one history rather than being asked which machinery settled their swap.
 */
function MyTrades() {
  // The wallet hooks live here rather than on the page: every other tab renders without a wallet,
  // and a hook cannot be called conditionally.
  const wallet = useWalletAction();
  const address = wallet.address;
  const sameChain = useTrades(address ? { taker: address } : undefined);
  const crossChain = useCrossChainOrders(address);
  const pending = sameChain.isPending || crossChain.isPending;
  const failed = sameChain.isError && crossChain.isError;
  const rows = ownTrades(sameChain.data?.items, crossChain.data).map(tradeRow);

  if (!address) {
    return (
      <p className={styles.emptyNote}>
        Connect a wallet to see the trades you have made.{" "}
        <button type="button" onClick={() => wallet.prepare()}>
          Connect
        </button>
      </p>
    );
  }

  return (
    <div
      data-scroll="1"
      className={styles.list}
      aria-busy={sameChain.isFetching || crossChain.isFetching}
    >
      {pending && (
        <>
          <p className={styles.srOnly} role="status">
            Loading your trades…
          </p>
          <PlaceholderRows rows={PAGE_SIZE} variant="trade" />
        </>
      )}
      {failed && (
        <p className={styles.emptyNote} role="alert">
          Couldn’t load your trades.{" "}
          <button
            type="button"
            onClick={() => {
              void sameChain.refetch();
              void crossChain.refetch();
            }}
          >
            Try again
          </button>
        </p>
      )}
      {!pending && !failed && rows.length === 0 && (
        <p className={styles.emptyNote}>
          You haven’t traded on this deployment yet.
        </p>
      )}
      {rows.map((trade) => (
        <TradeRow key={trade.id} trade={trade} />
      ))}
    </div>
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
          <>
            <p className={styles.srOnly} role="status">
              Loading activity…
            </p>
            <PlaceholderRows
              rows={ACTIVITY_PLACEHOLDER_ROWS}
              variant="activity"
            />
          </>
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
                {EVENT_TERMS[row.kind] ? (
                  <Term term={EVENT_TERMS[row.kind]}>{row.kind}</Term>
                ) : (
                  row.kind
                )}
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
  status: string;
}) {
  const assets = useAssets().data ?? [];
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
    />
  );
}

export function ExplorerPage() {
  const [params, setParams] = useSearchParams();
  const [openDrop, setOpenDrop] = useState<FilterKey | null>(null);
  const poolQuery = usePools();
  const pools = poolQuery.data ?? [];
  const stats = useExplorerStats();

  const tab = chosen(TABS, params.get("tab"));
  const isTrades = tab === "Trades";
  const isActivity = tab === "Activity";
  const isMine = tab === "Your Trades";
  const status = chosen(FILTERS.status, params.get("status"));
  const type = chosen(FILTERS.type, params.get("type"));
  const entity = chosen(FILTERS.entity, params.get("entity"));
  const rebateStatus = chosen(FILTERS.rebates, params.get("rebates"));

  const pairNames = [
    ...new Set(pools.map((pool) => pool.pair.replace(/\s/g, ""))),
  ];
  const pairOptions: Options = ["All pairs", ...pairNames];
  // An unknown pair keeps its URL spelling rather than collapsing to the default, so a link to a
  // pair this deployment does not serve says so instead of quietly showing everything.
  const pairParam = params.get("pair");
  const selectedPair = pairParam
    ? (pairOptions.find(
        (option) => option.toLowerCase() === pairParam.toLowerCase(),
      ) ?? pairParam)
    : pairOptions[0];
  const pair = pools.find(
    (pool) => pool.pair.replace(/\s/g, "") === selectedPair,
  )?.ref;

  /** Each change is a new history entry: Back steps through views instead of leaving the Explorer. */
  function select(key: "tab" | FilterKey, options: Options, value: string) {
    const next = new URLSearchParams(params);
    if (value === options[0]) next.delete(key);
    else next.set(key, value.toLowerCase());
    setOpenDrop(null);
    setParams(next);
  }

  function drop(key: FilterKey, value: string, options: Options) {
    return (
      <FilterDrop
        value={value}
        options={options}
        open={openDrop === key}
        onToggle={() => setOpenDrop(openDrop === key ? null : key)}
        onSelect={(next) => select(key, options, next)}
      />
    );
  }

  const filter: TradeFilter = {
    ...(status === "All status" ? {} : { status }),
    ...(pair ? { base: pair.base, quote: pair.quote } : {}),
  };
  const activityFilter: ActivityFilter = {
    ...(type === "All types" ? {} : { kind: type }),
    ...(entity === "All entities" ? {} : { entity }),
  };
  return (
    <div className={styles.root}>
      <div className={styles.headWide}>
        <div className={styles.headTitle}>
          <div className={styles.eyebrow}>Explorer</div>
          <h1 className={styles.titleLg}>
            {isMine
              ? "Your trades"
              : isTrades
                ? "Trades"
                : isActivity
                  ? "Protocol activity"
                  : "Rebates"}
          </h1>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>
          {isMine
            ? "Everything the connected wallet has traded — same-chain and cross-chain, newest first."
            : isTrades
              ? "Every intent through Solvent: pair, in → out, makers sourced, status, price impact and tx."
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
            <div className={styles.statLabel}>
              <Term term={stat.term}>{stat.label}</Term>
            </div>
            <div className={styles.statRow}>
              <span className={styles.statValue}>{stat.value}</span>
              {/* The head of the chain is the one figure whose age matters, so it carries the
                  poll's real state rather than a permanent "live". */}
              {stat.term === "blockHeight" ? (
                <QueryFreshness query={stats} />
              ) : (
                <span className={styles.statSub} style={{ color: stat.accent }}>
                  {stat.sub}
                </span>
              )}
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
          {TABS.map((name) => (
            <button
              key={name}
              type="button"
              className={name === tab ? styles.tabOn : styles.tab}
              onClick={() => select("tab", TABS, name)}
            >
              {name}
            </button>
          ))}
        </div>
        <div className={styles.drops}>
          {isMine ? null : isTrades ? (
            <>
              {drop("status", status, FILTERS.status)}
              {drop("pair", selectedPair, pairOptions)}
            </>
          ) : isActivity ? (
            <>
              {drop("type", type, FILTERS.type)}
              {drop("entity", entity, FILTERS.entity)}
            </>
          ) : (
            drop("rebates", rebateStatus, FILTERS.rebates)
          )}
        </div>
      </div>
      {isMine ? (
        <MyTrades />
      ) : isTrades ? (
        selectedPair === "All pairs" || pair ? (
          <TradeList key={JSON.stringify(filter)} filter={filter} />
        ) : poolQuery.isPending ? (
          <p className={styles.emptyNote} role="status">
            Loading trades…
          </p>
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
          key={rebateStatus}
          currentBlock={stats.data?.blockHeight}
          status={rebateStatus}
        />
      )}
    </div>
  );
}
