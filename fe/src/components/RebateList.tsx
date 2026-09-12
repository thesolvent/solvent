import type { Asset } from "@/data";
import type { RebateRecord } from "@/data/rebates";
import { loadedPageLabel } from "@/lib/pagination";
import { rebateRow } from "@/lib/rebates";
import { Pagination } from "./Pagination";
import { useLoadedPagination } from "./useLoadedPagination";
import styles from "./RebateList.module.css";

export interface RebateAction {
  availableIds?: ReadonlySet<string>;
  completedId?: string;
  failedId?: string;
  pendingId?: string;
  pendingLabel?: string;
  problem?: string;
  label: string;
  onExecute: (id: string) => void;
}

export function RebateList({
  assets,
  pages,
  pending,
  fetching,
  error,
  hasMore,
  onRetry,
  onLoadMore,
  action,
  currentBlock,
}: {
  assets: Asset[];
  pages: RebateRecord[][];
  pending: boolean;
  fetching: boolean;
  error: boolean;
  hasMore: boolean;
  onRetry: () => void;
  onLoadMore: () => Promise<boolean>;
  action?: RebateAction;
  currentBlock?: number;
}) {
  const pagination = useLoadedPagination(pages, hasMore, onLoadMore);
  const rebates = pagination.items;
  const rows = rebates.map((rebate) => rebateRow(rebate, assets));

  return (
    <div className={styles.root}>
      <div data-scroll="1" className={styles.list} aria-busy={fetching}>
        {pending && (
          <p className={styles.emptyNote} role="status">
            Loading rebates…
          </p>
        )}
        {error && (
          <p className={styles.emptyNote} role="alert">
            {rebates.length
              ? "Couldn’t refresh rebates."
              : "Couldn’t load rebates."}{" "}
            <button type="button" onClick={onRetry}>
              Try again
            </button>
          </p>
        )}
        {!pending && !error && rows.length === 0 && (
          <p className={styles.emptyNote}>No rebates available yet.</p>
        )}
        {rows.map((row) => {
          const executing = action?.pendingId === row.id;
          const completed = action?.completedId === row.id;
          const failed = action?.failedId === row.id;
          const available = action?.availableIds?.has(row.id) ?? true;
          const expired =
            row.status === "ready" &&
            (row.deadlineBlock == null ||
              (currentBlock != null && currentBlock >= row.deadlineBlock));
          return (
            <div key={row.id} className={styles.row}>
              <span className={styles.pair}>
                <span className={styles.pairName}>{row.pair}</span>
                <span className={styles.sub}>{row.strategy}</span>
              </span>
              <span className={styles.flow}>
                <span title={row.deposit}>{row.deposit}</span>
                <span className={styles.arrow}>→</span>
                <strong title={row.output}>{row.output}</strong>
              </span>
              <span className={styles.cell}>
                <strong title={row.makerRebate}>{row.makerRebate}</strong>
                <span className={styles.sub}>maker rebate</span>
              </span>
              <span className={styles.cell}>
                <strong title={row.executorProfit}>{row.executorProfit}</strong>
                <span className={styles.sub}>executor profit</span>
              </span>
              <span className={styles.cell}>
                <strong>{row.deviation}</strong>
                <span className={styles.sub} title={row.statusDetail}>
                  {row.statusDetail}
                </span>
              </span>
              <span className={styles.actionCell}>
                {row.status === "ready" && action ? (
                  <>
                    <button
                      type="button"
                      className={styles.earnButton}
                      disabled={executing || completed || !available || expired}
                      onClick={() => action.onExecute(row.id)}
                    >
                      {executing
                        ? (action.pendingLabel ?? "Preparing rebate…")
                        : completed
                          ? "Earned"
                          : failed
                            ? "Try again"
                            : action.label}
                    </button>
                    {failed && (
                      <span className={styles.error} title={action.problem}>
                        {action.problem ?? "Execution failed"}
                      </span>
                    )}
                  </>
                ) : (
                  <span
                    className={
                      row.status === "executed"
                        ? styles.executed
                        : expired
                          ? styles.expired
                          : styles.ready
                    }
                  >
                    {expired ? "expired" : row.status}
                  </span>
                )}
              </span>
            </div>
          );
        })}
      </div>
      <Pagination
        label={loadedPageLabel(pages, pagination.page, hasMore, "rebates")}
        page={pagination.page}
        pageCount={pagination.pageCount}
        disabled={fetching || error}
        onPage={(page) => void pagination.select(page)}
      />
    </div>
  );
}
