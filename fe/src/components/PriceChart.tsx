import type { PriceChartModel, PriceSpan } from "@/lib/price-chart";
import { PRICE_SPANS } from "@/lib/price-chart";

import styles from "./PriceChart.module.css";

export function PriceChart({
  data: d,
  title,
  span,
  onSpanChange,
  notice,
  className,
}: {
  data: PriceChartModel;
  title: string;
  span: PriceSpan;
  onSpanChange: (span: PriceSpan) => void;
  /** Shown instead of the plot while the series is loading or unavailable. */
  notice?: string;
  className?: string;
}) {
  const empty = notice ?? (d.linePath ? undefined : d.summary);

  return (
    <section className={className ?? styles.price} aria-label={title}>
      <div className={styles.head}>
        <span className={styles.title}>{title}</span>
        {d.change && !empty && (
          <span className={d.changeUp ? styles.changeUp : styles.changeDown}>
            {d.change}
          </span>
        )}
        <span className={styles.spacer} />
        <div className={styles.spans}>
          {PRICE_SPANS.map((option) => (
            <button
              key={option.id}
              type="button"
              aria-pressed={option.id === span}
              className={option.id === span ? styles.spanOn : styles.span}
              onClick={() => onSpanChange(option.id)}
            >
              {option.label}
            </button>
          ))}
        </div>
      </div>

      <div className={styles.plot}>
        <div className={styles.yAxis}>
          {(empty ? [] : d.yTicks).map((tick) => (
            <span
              key={tick.label + tick.top}
              className={styles.yTick}
              style={{ top: tick.top }}
            >
              {tick.label}
            </span>
          ))}
        </div>

        <div className={styles.canvas}>
          {empty ? (
            <div className={styles.notice} role="status">
              {empty}
            </div>
          ) : (
            <>
              {d.last && (
                <span className={styles.lastRule} style={{ top: d.last.top }}>
                  <span className={styles.lastValue}>{d.last.label}</span>
                </span>
              )}
              <svg
                aria-label={d.summary}
                className={styles.svg}
                preserveAspectRatio="none"
                role="img"
                viewBox="0 0 1000 320"
              >
                <defs>
                  <linearGradient id="priceFill" x1="0" x2="0" y1="0" y2="1">
                    <stop
                      offset="0%"
                      stopColor="var(--lime)"
                      stopOpacity="0.55"
                    />
                    <stop
                      offset="100%"
                      stopColor="var(--lime)"
                      stopOpacity="0"
                    />
                  </linearGradient>
                </defs>
                <path className={styles.area} d={d.areaPath} />
                <path
                  className={styles.line}
                  d={d.linePath}
                  vectorEffect="non-scaling-stroke"
                />
              </svg>
            </>
          )}
        </div>
      </div>

      <div className={styles.xAxis}>
        {(empty ? [] : d.xTicks).map((tick) => (
          <span
            key={tick.label + tick.left}
            className={styles.xTick}
            style={{ left: tick.left }}
          >
            {tick.label}
          </span>
        ))}
      </div>
    </section>
  );
}
