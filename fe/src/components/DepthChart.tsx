import { useId, type MouseEvent, type ReactNode } from "react";
import type { DepthChartModel } from "@/lib/depth-chart";
import styles from "./DepthChart.module.css";

const METRIC_HELP = {
  bestPrice: "Current best executable price before price impact.",
  near: "Estimated price after a trade moves the market by 0.5%.",
  far: "Estimated price after a trade moves the market by 1.0%.",
  liquidity: "Total base asset currently available across makers.",
} as const;

function ImpactMetric({
  children,
  className,
  help,
  id,
}: {
  children: ReactNode;
  className: string;
  help?: string;
  id: string;
}) {
  if (!help) return <div className={className}>{children}</div>;

  return (
    <div
      className={`${className} ${styles.metricHint}`}
      tabIndex={0}
      aria-describedby={id}
    >
      <span id={id} role="tooltip" className={styles.metricTooltip}>
        {help}
      </span>
      {children}
    </div>
  );
}

export function DepthChart({
  data: d,
  title,
  legend,
  onHoverChange,
  className,
  notice,
  liquidityLabel = "Total liquidity",
  showMetricHelp = false,
  titleHelp,
}: {
  data: DepthChartModel;
  title: string;
  legend: ReactNode;
  onHoverChange: (fraction: number | null) => void;
  className?: string;
  notice?: string;
  liquidityLabel?: string;
  showMetricHelp?: boolean;
  titleHelp?: string;
}) {
  const titleHelpId = useId();
  const impactHelpId = useId();
  const onHover = (event: MouseEvent<HTMLDivElement>) => {
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width <= 0) return;
    onHoverChange(
      Math.min(1, Math.max(0, (event.clientX - bounds.left) / bounds.width)),
    );
  };
  return (
    <section className={className ?? styles.depth}>
      <div className={styles.depthHead}>
        <span className={styles.kpiSwatch} />
        {titleHelp ? (
          <span
            className={`${styles.depthTitle} ${styles.depthTitleHint}`}
            tabIndex={0}
            aria-describedby={titleHelpId}
          >
            <span
              id={titleHelpId}
              role="tooltip"
              className={styles.depthTitleTooltip}
            >
              {titleHelp}
            </span>
            {title}
          </span>
        ) : (
          <span className={styles.depthTitle}>{title}</span>
        )}
        <span className={styles.depthSub}>{d.priceTitle}</span>
      </div>

      <div className={styles.depthMeta}>
        <div className={styles.depthLegend}>
          <span className={styles.depthLegendMark} />
          <span className={styles.depthLegendText}>{legend}</span>
        </div>
        {d.impacts.length > 0 && (
          <div className={styles.impactControls}>
            {showMetricHelp && (
              <span
                className={`${styles.impactKey} ${styles.impactKeyHint}`}
                tabIndex={0}
                aria-describedby={impactHelpId}
              >
                <span className={styles.impactKeyMark} />
                Target price impact
                <span
                  id={impactHelpId}
                  role="tooltip"
                  className={styles.impactKeyTooltip}
                >
                  The percentage is the price movement used to mark each trade
                  size.
                </span>
              </span>
            )}
            <div className={styles.segmented}>
              {d.impacts.map((stop) => (
                <button
                  key={stop.frac}
                  type="button"
                  className={styles.segment}
                  onFocus={() => onHoverChange(stop.frac)}
                  onBlur={() => onHoverChange(null)}
                  onClick={() => onHoverChange(stop.frac)}
                  onMouseEnter={() => onHoverChange(stop.frac)}
                  onMouseLeave={() => onHoverChange(null)}
                >
                  {stop.label}
                </button>
              ))}
            </div>
          </div>
        )}
      </div>

      <div className={styles.plot}>
        <div className={styles.yAxis}>
          {(!notice ? d.yTicks : []).map((t) => (
            <span
              key={t.label + t.top}
              className={styles.yTick}
              style={{ top: t.top }}
            >
              {t.label}
            </span>
          ))}
        </div>
        <div
          className={styles.canvas}
          onMouseMove={onHover}
          onMouseLeave={() => onHoverChange(null)}
        >
          {notice && (
            <div className={styles.notice} role="status">
              {notice}
            </div>
          )}
          <svg
            aria-label={`${title}: ${d.priceTitle}; ${d.axisTitle}`}
            role="img"
            viewBox="0 0 1000 400"
            preserveAspectRatio="none"
            className={styles.svg}
          >
            {[30, 112, 195, 277].map((y) => (
              <line
                key={y}
                x1="0"
                y1={y}
                x2="1000"
                y2={y}
                stroke="var(--surface-alt)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
            ))}
            <line
              x1="0"
              y1="360"
              x2="1000"
              y2="360"
              stroke="var(--line)"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
            <path
              // Remounting on a new curve restarts the draw, so switching pool redraws.
              key={d.aggPath}
              className={styles.curve}
              d={d.aggPath}
              // Normalises the dash units, so the draw needs no measured length.
              pathLength={1}
              fill="none"
              stroke="var(--green)"
              strokeWidth="2.4"
              strokeLinejoin="round"
              strokeLinecap="round"
              vectorEffect="non-scaling-stroke"
            />
          </svg>

          {d.hover && (
            <div className={styles.hoverLayer}>
              <span
                className={styles.hoverLine}
                style={{ left: d.hover.left }}
              />
              <span
                className={styles.hoverDot}
                style={{
                  left: d.hover.left,
                  top: d.hover.dotTop,
                }}
              />
              <div
                className={styles.tooltip}
                role="tooltip"
                style={{
                  left: d.hover.left,
                  transform: d.hover.shift,
                }}
              >
                <div className={styles.tipTop}>
                  <span className={styles.tipLabel}>Trade size</span>
                  <span className={styles.tipValue}>{d.hover.size}</span>
                </div>
                <div className={styles.tipMid}>
                  <span className={styles.tipLabel}>Effective price</span>
                  <span className={styles.tipValueLime}>{d.hover.price}</span>
                </div>
                <div className={styles.tipOut}>
                  <span className={styles.tipLabel}>Output</span>
                  <span className={styles.tipValue}>{d.hover.output}</span>
                </div>
                <div className={styles.tipFoot}>
                  <span className={styles.tipLabel}>Makers used</span>
                  <span className={styles.tipDots}>
                    {d.hover.dots.map((dot, i) => (
                      <span
                        key={i}
                        className={styles.tipDot}
                        style={{
                          background: dot.bg,
                        }}
                      />
                    ))}
                    <span className={styles.tipCount}>{d.hover.makers}</span>
                  </span>
                </div>
              </div>
            </div>
          )}

          <div className={styles.xAxis}>
            {(!notice ? d.xTicks : []).map((t) => (
              <span
                key={t.left}
                className={styles.xTick}
                style={{ left: t.left }}
              >
                {t.label}
              </span>
            ))}
          </div>
        </div>
      </div>

      <div className={styles.axisTitle}>{d.axisTitle}</div>

      <div className={styles.impacts}>
        <ImpactMetric
          className={styles.impactFirst}
          id="depth-best-price-tooltip"
          help={showMetricHelp ? METRIC_HELP.bestPrice : undefined}
        >
          <div className={styles.impactLabel}>Best price</div>
          <div className={styles.impactValue}>{notice ? "—" : d.bestPrice}</div>
        </ImpactMetric>
        <ImpactMetric
          className={styles.impact}
          id="depth-half-impact-tooltip"
          help={showMetricHelp ? METRIC_HELP.near : undefined}
        >
          <div className={styles.impactLabel}>{d.near.label}</div>
          <div className={styles.impactValue}>{d.near.price}</div>
          <div className={styles.impactSize}>{d.near.size}</div>
        </ImpactMetric>
        <ImpactMetric
          className={styles.impact}
          id="depth-one-impact-tooltip"
          help={showMetricHelp ? METRIC_HELP.far : undefined}
        >
          <div className={styles.impactLabel}>{d.far.label}</div>
          <div className={styles.impactValue}>{d.far.price}</div>
          <div className={styles.impactSize}>{d.far.size}</div>
        </ImpactMetric>
        <ImpactMetric
          className={styles.impactLast}
          id="depth-total-liquidity-tooltip"
          help={showMetricHelp ? METRIC_HELP.liquidity : undefined}
        >
          <div className={styles.impactLabel}>{liquidityLabel}</div>
          <div className={styles.impactValueGreen}>{d.totalLiq}</div>
        </ImpactMetric>
      </div>
    </section>
  );
}
