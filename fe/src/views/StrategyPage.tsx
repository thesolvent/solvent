import { useId, useState } from "react";
import { DepthChart } from "@/components/DepthChart";
import { depthChart } from "@/lib/depth-chart";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import {
  usePosition,
  usePositionHistory,
  usePositionDepth,
} from "@/services/makers";
import { useTrades } from "@/services/explorer";
import { slug } from "@/services/pools";
import { Crumbs } from "@/components/Crumbs";
import { strategyDetail, rangeDescription } from "@/lib/strategy";
import type { RouteState } from "@/routes";

import { PairIcons } from "@/components/TokenIcon";
import { useTokenIcon } from "@/services/assets";

import styles from "./explorer.module.css";

export function StrategyPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const { strategyHash } = useParams();
  const waitForIndex = Boolean(
    (location.state as RouteState | null)?.waitForStrategyIndex,
  );
  const position = usePosition(strategyHash, { waitForIndex });
  const history = usePositionHistory(position.data ? strategyHash : undefined);
  const depth = usePositionDepth(position.data, {
    waitForLiquidity: waitForIndex,
  });
  const [hoverFrac, setHoverFrac] = useState<number | null>(null);
  const rangeHint = useId();
  const settlements = useTrades(
    strategyHash
      ? { status: "confirmed", strategy_hash: strategyHash }
      : undefined,
  );
  const sd = strategyDetail(
    position.data,
    history.data,
    settlements.data?.items ?? [],
  );
  const iconOf = useTokenIcon();
  const [baseSymbol = "", quoteSymbol = ""] = (position.data?.pair ?? "").split(
    /\s*\/\s*/,
  );
  const chart = {
    ...depthChart({
      depth: position.isError || depth.isError ? undefined : depth.data,
      baseSymbol,
      quoteSymbol,
      hoverFrac,
    }),
    axisTitle: `Cumulative ${baseSymbol || "base token"} in · marginal price (${quoteSymbol || "quote"} per ${baseSymbol || "base"})`,
  };

  const notice = position.isError
    ? "Couldn’t load this strategy"
    : depth.isError
      ? "Couldn’t load strategy depth"
      : position.isPending || depth.isPending
        ? "Loading strategy depth…"
        : !chart.aggPath
          ? waitForIndex
            ? "Synchronizing strategy liquidity…"
            : "No executable liquidity for this strategy"
          : undefined;

  return (
    <div className={styles.rootFramed}>
      <div className={styles.frame}>
        <div className={styles.head}>
          <button
            type="button"
            className={styles.back}
            onClick={() => {
              if (window.history.state?.idx > 0) {
                navigate(-1);
              } else {
                navigate("/explorer", { replace: true });
              }
            }}
          >
            ←
          </button>
          <div className={styles.headTitle}>
            <Crumbs current="Strategy" />
            <div className={styles.titleSm}>
              <PairIcons
                base={{ symbol: baseSymbol, logoUri: iconOf(baseSymbol) }}
                quote={{ symbol: quoteSymbol, logoUri: iconOf(quoteSymbol) }}
                size={18}
              />
              {sd.title}
            </div>
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
            className={styles.primaryAction}
            disabled={!position.data}
            onClick={() =>
              position.data &&
              navigate(
                `/pools/${slug(position.data.pair)}/new?clone=${encodeURIComponent(position.data.hash)}`,
              )
            }
          >
            Clone
          </button>
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
              className={
                k.label === "Range" ? styles.rangeStat : styles.statTight
              }
              tabIndex={k.label === "Range" ? 0 : undefined}
              aria-describedby={k.label === "Range" ? rangeHint : undefined}
              style={{
                backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
              }}
            >
              {k.label === "Range" && (
                <span
                  id={rangeHint}
                  role="tooltip"
                  className={styles.rangeTooltip}
                >
                  {rangeDescription(position.data)}
                </span>
              )}
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
          <DepthChart
            data={chart}
            title="Strategy depth"
            liquidityLabel="Quoted depth"
            legend="This position"
            className={styles.curvePane}
            onHoverChange={setHoverFrac}
            notice={notice}
          />

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
