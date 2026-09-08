import type { UseQueryResult } from "@tanstack/react-query";
import styles from "./explorer.module.css";

export function QueryFreshness({
  query,
}: {
  query: Pick<
    UseQueryResult,
    "dataUpdatedAt" | "isFetching" | "isError" | "fetchStatus"
  >;
}) {
  const updated = query.dataUpdatedAt ? new Date(query.dataUpdatedAt) : null;
  const state =
    query.fetchStatus === "paused"
      ? "paused"
      : query.isFetching
        ? "refreshing"
        : query.isError
          ? "delayed"
          : "current";
  const label = {
    paused: "Updates paused",
    refreshing: "Refreshing…",
    delayed: "Refresh delayed",
    current: updated ? "Updated" : "Waiting for data",
  }[state];
  return (
    <span
      className={styles.freshness}
      data-state={state}
      title={updated ? `Last updated ${updated.toLocaleString()}` : undefined}
    >
      <span className={styles.refreshDot} aria-hidden="true" />
      <span>
        {label}
        {updated && ` · ${updated.toLocaleTimeString()}`}
      </span>
    </span>
  );
}
