import { useState } from "react";
import { Link, useSearchParams } from "react-router-dom";
import { useConnectModal } from "@rainbow-me/rainbowkit";
import { useAccount, useSwitchChain } from "wagmi";
import { Pagination } from "@/components/Pagination";
import { RebateList } from "@/components/RebateList";
import { useLoadedPagination } from "@/components/useLoadedPagination";
import { chain } from "@/adapters/wallet/config";
import { activityRow, explorerStats, tradeRow } from "@/lib/explorer";
import { loadedPageLabel } from "@/lib/pagination";
import type { ActivityFilter, TradeFilter } from "@/ports/explorer";
import { useAssets } from "@/services/assets";
import { useActivity, useExplorerStats, useTrades } from "@/services/explorer";
import { usePools } from "@/services/pools";
import {
  useActiveRebatePages,
  useExecuteRebate,
  useRebates,
} from "@/services/rebates";
import { Term } from "@/components/Tooltip";
import { EVENT_TERMS, STATUS_TERMS } from "@/lib/glossary";

import styles from "./explorer.module.css";

const TABS = ["Trades", "Activity", "Rebates"] as const;

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
  const [params, setParams] = useSearchParams();
  const [openDrop, setOpenDrop] = useState<FilterKey | null>(null);
  const poolQuery = usePools();
  const pools = poolQuery.data ?? [];
  const stats = useExplorerStats();

  const tab = chosen(TABS, params.get("tab"));
  const isTrades = tab === "Trades";
  const isActivity = tab === "Activity";
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
            {isTrades ? "Trades" : isActivity ? "Protocol activity" : "Rebates"}
          </h1>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>
          {isTrades
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
          {isTrades ? (
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
      {isTrades ? (
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
