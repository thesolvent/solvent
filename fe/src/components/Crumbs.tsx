import { Fragment } from "react";

import { useApp } from "@/state";

import styles from "./Crumbs.module.css";

export function Crumbs({ current }: { current: string }) {
  const { crumbs } = useApp();

  return (
    <div className={styles.crumbs}>
      {crumbs(current).map((c, i) => (
        <Fragment key={`${c.label}-${i}`}>
          <button
            type="button"
            className={styles.crumb}
            style={{ color: c.fg }}
            onClick={c.go}
          >
            {c.label}
          </button>
          <span className={styles.sep}>{c.sep}</span>
        </Fragment>
      ))}
    </div>
  );
}
