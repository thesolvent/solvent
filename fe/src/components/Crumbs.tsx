import { Fragment } from "react";
import { useNavigate } from "react-router-dom";

import { useApp } from "@/state";

import styles from "./Crumbs.module.css";

/** A step a routed page names itself, rather than one recorded on the view stack. */
export type CrumbLink = { label: string; to: string };

export function Crumbs({
  current,
  trail,
}: {
  current: string;
  trail?: CrumbLink[];
}) {
  const { crumbs } = useApp();
  const navigate = useNavigate();

  // A page that lives at its own address knows its ancestry; only stack-driven views need the trail.
  const items = trail
    ? [
        ...trail.map((crumb) => ({
          label: crumb.label,
          sep: "›",
          fg: "var(--text-muted)",
          go: () => navigate(crumb.to),
        })),
        { label: current, sep: "", fg: "var(--ink)", go: () => {} },
      ]
    : crumbs(current);

  return (
    <nav aria-label="Breadcrumb" className={styles.crumbs}>
      {items.map((c, i) => (
        <Fragment key={`${c.label}-${i}`}>
          {i === items.length - 1 ? (
            <span
              aria-current="page"
              className={styles.current}
              style={{ color: c.fg }}
            >
              {c.label}
            </span>
          ) : (
            <button
              type="button"
              className={styles.crumb}
              style={{ color: c.fg }}
              onClick={c.go}
            >
              {c.label}
            </button>
          )}
          <span className={styles.sep}>{c.sep}</span>
        </Fragment>
      ))}
    </nav>
  );
}
