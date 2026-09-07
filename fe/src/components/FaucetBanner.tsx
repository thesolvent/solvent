import styles from "./FaucetBanner.module.css";

export function FaucetBanner() {
  return (
    <div className={styles.banner}>
      <span className={styles.tag}>Devnet</span>
      <span className={styles.copy}>
        Balances are test-only. Top up before resolving intents.
      </span>
      <button type="button" className={styles.action}>
        Get test tokens
      </button>
    </div>
  );
}
