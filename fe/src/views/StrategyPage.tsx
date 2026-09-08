import { useNavigate, useParams } from "react-router-dom";
import { usePosition, usePositionHistory } from "@/services/makers";
import { useTrades } from "@/services/explorer";
import { slug } from "@/services/pools";
import { Crumbs } from "@/components/Crumbs";
import { strategyDetail } from "@/lib/strategy";

import styles from "./explorer.module.css";

export function StrategyPage() {
  const navigate = useNavigate();
  const { strategyHash } = useParams();
  const position = usePosition(strategyHash);
  const history = usePositionHistory(strategyHash);
  const settlements = useTrades(
    strategyHash
      ? { status: "confirmed", strategy_hash: strategyHash }
      : undefined,
  );
  const notice = position.isError
    ? "Couldn’t load this strategy"
    : history.isError
      ? "Price history unavailable"
      : position.isPending
        ? "Loading strategy…"
        : "";
  const sd = strategyDetail(
    position.data,
    history.data,
    settlements.data?.items ?? [],
    notice,
  );

  return (
    <div className={styles.rootFramed}>
      <div className={styles.frame}>
        <div className={styles.head}>
          <button
            type="button"
            className={styles.back}
            onClick={() => navigate(-1)}
          >
            ←
          </button>
          <div className={styles.headTitle}>
            <Crumbs current="Strategy" />
            <div className={styles.titleSm}>{sd.title}</div>
          </div>
          <span
            className={styles.statePill}
            style={{ background: sd.stBg, color: sd.stFg }}
          >
            {sd.state}
          </span>
          <span className={styles.headMeta}>{sd.since}</span>
          <span className={styles.spacer} />
          <button
            type="button"
            className={styles.crossLink}
            disabled={!position.data}
            onClick={() =>
              position.data && navigate(`/pools/${slug(position.data.pair)}`)
            }
          >
            part of {sd.pool} pool ↗
          </button>
          <button
            type="button"
            className={styles.crossLinkMono}
            disabled={!position.data}
            onClick={() =>
              position.data && navigate(`/makers/${position.data.maker}`)
            }
          >
            by {sd.maker} ↗
          </button>
        </div>

        <div className={styles.stats4}>
          {sd.shape.map((k) => (
            <div
              key={k.label}
              className={styles.statTight}
              style={{
                backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
              }}
            >
              <div className={styles.statLabel}>{k.label}</div>
              <div className={styles.statValueXs}>{k.value}</div>
              <div className={styles.statTagRow}>
                <span
                  className={styles.statTag}
                  style={{
                    background: k.tagBg,
                    color: k.tagFg,
                  }}
                >
                  {k.tag}
                </span>
              </div>
            </div>
          ))}
        </div>

        <div className={styles.stats4Soft}>
          {sd.active.map((k) => (
            <div
              key={k.label}
              className={styles.statTighter}
              style={{
                backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
              }}
            >
              <div className={styles.statLabel}>{k.label}</div>
              <div className={styles.statRow}>
                <span className={styles.statValueXs}>{k.value}</span>
                <span className={styles.statSubPlain}>{k.sub}</span>
              </div>
            </div>
          ))}
        </div>

        <div className={styles.split}>
          <section className={styles.curvePane}>
            <div className={styles.curveHead}>
              <span className={styles.curveTag}>Curve &amp; range</span>
              <span className={styles.curveNote}>{sd.bandNote}</span>
              <span className={styles.spacer} />
              <span className={styles.curveMid}>mid {sd.mid}</span>
            </div>
            <svg
              viewBox="0 0 640 300"
              preserveAspectRatio="none"
              className={styles.curveSvg}
            >
              {sd.showBand && (
                <rect
                  x="0"
                  y={sd.bandY}
                  width="640"
                  height={sd.bandH}
                  fill="#f4f9e4"
                />
              )}
              {sd.showBounds && (
                <line
                  x1="0"
                  y1={sd.bandY}
                  x2="640"
                  y2={sd.bandY}
                  stroke="var(--green)"
                  strokeWidth="1.4"
                  strokeDasharray="5 5"
                  vectorEffect="non-scaling-stroke"
                />
              )}
              {sd.showBounds && (
                <line
                  x1="0"
                  y1={sd.bandY2}
                  x2="640"
                  y2={sd.bandY2}
                  stroke="var(--green)"
                  strokeWidth="1.4"
                  strokeDasharray="5 5"
                  vectorEffect="non-scaling-stroke"
                />
              )}
              <line
                x1="0"
                y1="150"
                x2="640"
                y2="150"
                stroke="var(--line)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              <line
                x1="0"
                y1="60"
                x2="640"
                y2="60"
                stroke="var(--surface-alt)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              <line
                x1="0"
                y1="240"
                x2="640"
                y2="240"
                stroke="var(--surface-alt)"
                strokeWidth="1"
                vectorEffect="non-scaling-stroke"
              />
              {sd.lines.map((line, i) => (
                <polyline
                  key={i}
                  points={line}
                  fill="none"
                  stroke="var(--ink)"
                  strokeWidth="1.8"
                  strokeLinejoin="round"
                  strokeLinecap="round"
                  vectorEffect="non-scaling-stroke"
                />
              ))}
            </svg>
            <div className={styles.spark}>
              {sd.spark.map((b, i) => (
                <span
                  key={i}
                  className={styles.sparkBar}
                  style={{ height: b.h, background: b.bg }}
                />
              ))}
            </div>
            <div className={styles.curveAxis}>
              <span>{sd.axisFrom}</span>
              <span className={styles.curveAxisMid}>
                {sd.tickLo} · {sd.tickHi}
              </span>
              <span>fills per day</span>
              <span>{sd.axisTo}</span>
            </div>
          </section>

          <section data-scroll="1" className={styles.fillsPane}>
            <div className={styles.fillsHead}>
              <span className={styles.curveTag}>Recent settlements</span>
              <span className={styles.curveNote}>this strategy</span>
            </div>
            {sd.fills.map((x, i) => (
              <button
                key={`${x.hash}-${i}`}
                type="button"
                className={styles.fillRow}
                onClick={() => navigate(`/explorer/trades/${x.id}`)}
              >
                <div className={styles.fillTop}>
                  <span className={styles.mono}>{x.hash}</span>
                  <span className={styles.fillKind}>pull</span>
                  <span className={styles.spacer} />
                  <span className={styles.fillBlk}>blk {x.blk}</span>
                </div>
                <div className={styles.fillBottom}>
                  <span className={styles.fillFlow}>{x.flow}</span>
                  <span className={styles.spacer} />
                  <span className={styles.tradeLinkWrap}>
                    <span className={styles.tradeLink}>{x.trade} ↗</span>
                  </span>
                </div>
              </button>
            ))}
            {sd.noFills && (
              <div className={styles.emptyNote}>
                {settlements.isError
                  ? "Couldn’t load settlements."
                  : settlements.isPending
                    ? "Loading settlements…"
                    : "No settlements yet — this strategy has not been pulled."}
              </div>
            )}
          </section>
        </div>
      </div>
    </div>
  );
}
