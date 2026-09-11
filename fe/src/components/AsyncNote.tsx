import styles from "./AsyncNote.module.css";

/** The part of a query a note needs. Any `useQuery` result satisfies it. */
export interface AsyncState {
  isPending: boolean;
  isError: boolean;
  refetch: () => unknown;
}

/**
 * The one note a surface renders instead of its rows: unread, empty, or failed.
 *
 * Without it a view can only distinguish "no rows" from "no answer" by hand, and every view that
 * skipped the distinction told the user its data was empty while the read was still in flight or
 * already broken.
 *
 * `isPending` is the loading flag throughout: under `skipToken` a read whose key has not resolved
 * is still waiting on something, and blank is a worse answer than "Loading…".
 */
export function AsyncNote({
  className,
  empty,
  query,
  subject,
}: {
  className?: string;
  /** What an answered read with nothing in it says; omit where the surface says it elsewhere. */
  empty?: string;
  query: AsyncState;
  subject: string;
}) {
  if (query.isError) {
    return (
      <p className={[className, styles.error].filter(Boolean).join(" ")}>
        <span role="alert">Couldn’t load {subject}.</span>{" "}
        <button
          type="button"
          className={styles.retry}
          onClick={() => void query.refetch()}
        >
          Try again
        </button>
      </p>
    );
  }
  if (query.isPending) {
    return (
      <p className={className} role="status">
        Loading {subject}…
      </p>
    );
  }
  return empty === undefined ? null : <p className={className}>{empty}</p>;
}
