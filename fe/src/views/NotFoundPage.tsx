import { Link, useLocation } from "react-router-dom";

import styles from "./NotFoundPage.module.css";

export function NotFoundPage() {
  const { pathname } = useLocation();

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <div className={styles.headTitle}>
          <div className={styles.eyebrow}>404</div>
          <h1 className={styles.title}>No such page</h1>
        </div>
        <span className={styles.limeSquare} />
        <p className={styles.pageDesc}>
          Nothing is served at this address. The link is wrong, or what it
          pointed at is gone.
        </p>
      </div>
      <div className={styles.body}>
        <p className={styles.path}>{pathname}</p>
        <Link className={styles.home} to="/">
          Back to home
        </Link>
      </div>
    </div>
  );
}
