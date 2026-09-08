import type { CSSProperties } from "react";
import type { TradeRecord } from "@/data/explorer";
import { tradeLifecycle } from "@/lib/trade-lifecycle";
import styles from "./explorer.module.css";

export function TradeLifecycle({ trade }: { trade: TradeRecord }) {
  const lifecycle = tradeLifecycle(trade);
  return (
    <div className={styles.lifecycle}>
      <span
        role="status"
        aria-live="polite"
        aria-atomic="true"
        className={styles.srOnly}
      >
        Trade {trade.status}. {lifecycle.recordedCount} of{" "}
        {lifecycle.steps.length} stages recorded.
      </span>
      <div className={styles.lifecycleHead}>
        <div className={styles.lifecycleTitleRow}>
          <span className={styles.sectionTitle}>Lifecycle</span>
          <span className={styles.lifecycleCount}>
            <span className={styles.lifecycleCountStrong}>
              {lifecycle.recordedCount}
            </span>{" "}
            of {lifecycle.steps.length} stages complete
          </span>
        </div>
        <span className={styles.lifecycleTotal}>
          {lifecycle.elapsedSeconds === null
            ? "pending"
            : `${lifecycle.elapsedSeconds}s`}{" "}
          total
        </span>
      </div>

      <div className={styles.phases}>
        {lifecycle.phases.map((phase, index) => (
          <div
            key={phase.name}
            className={styles.phase}
            data-complete={phase.complete}
          >
            <div className={styles.phaseTag}>Phase {index + 1}</div>
            <div className={styles.phaseName}>{phase.name}</div>
            <div className={styles.phaseRule} />
          </div>
        ))}
      </div>

      <div className={styles.timeline} role="list" aria-label="Trade lifecycle">
        <div className={styles.timelineMeta} aria-hidden="true">
          {lifecycle.steps.map((step) => (
            <div key={step.label} className={styles.timelineMetaCell}>
              {step.elapsedSeconds === null
                ? step.state
                : `+${step.elapsedSeconds}s`}
            </div>
          ))}
        </div>
        {lifecycle.steps.map((step, index) => (
          <div
            key={step.label}
            className={styles.step}
            role="listitem"
            aria-label={`${step.label}: ${step.state}`}
            aria-current={step.state === "awaiting" ? "step" : undefined}
            data-state={step.state}
            data-phase={index < 3 ? "quote" : "settle"}
            style={{ "--stage-index": index } as CSSProperties}
          >
            <div className={styles.stepBar}>
              {step.state === "recorded" && (
                <span className={styles.stepFill} />
              )}
            </div>
            <div className={styles.stepLead} />
            <div className={styles.stepText}>
              <div className={styles.stepLabel}>{step.label}</div>
              <div className={styles.stepState}>
                {step.state === "recorded" ? "" : step.state}
              </div>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
