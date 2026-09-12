import styles from "./RetryNotice.module.css";

/** A consistent recovery affordance while a page continues to show cached data. */
export function RetryNotice({
  message,
  onRetry,
}: {
  message: string;
  onRetry: () => void;
}) {
  return (
    <div className={styles.root} role="alert">
      <span>{message}</span>
      <button type="button" className={styles.retry} onClick={onRetry}>
        Try again
      </button>
    </div>
  );
}
