import { useId, useState } from "react";
import { AssetIdentity } from "@/components/AssetIdentity";
import { DepthChart } from "@/components/DepthChart";
import { depthChart } from "@/lib/depth-chart";
import { useLocation, useNavigate, useParams } from "react-router-dom";
import {
  usePosition,
  usePositionHistory,
  usePositionDepth,
} from "@/services/makers";
import { useTrades } from "@/services/explorer";
import { useAssets } from "@/services/assets";
import { slug } from "@/services/pools";
import { Crumbs } from "@/components/Crumbs";
import { strategyDetail, rangeDescription } from "@/lib/strategy";
import type { RouteState } from "@/routes";

import styles from "./explorer.module.css";

const STAT_HELP: Record<string, string> = {
  "Virtual balance": "Inventory assigned to this strategy at current prices.",
  "Actual / pullable": "Token inventory available to serve fills or withdraw.",
  Fee: "Fee charged on each input amount filled by this strategy.",
  "Fills (7d)": "Completed fills in the past seven days.",
  "Volume (7d)": "Value routed through this strategy in the past seven days.",
  "Quote uptime":
    "Share of the past seven days this strategy was available to quote.",
  "Last fill": "Time since this strategy last completed a fill.",
};

export function StrategyPage() {
  const navigate = useNavigate();
  const location = useLocation();
  const { strategyHash } = useParams();
  const assets = useAssets();
  const chainParam = new URLSearchParams(location.search).get("chain");
  const source =
    new URLSearchParams(location.search).get("source") === "direct"
      ? "direct"
      : undefined;
  const parsedChainId = chainParam == null ? undefined : Number(chainParam);
  const chainId =
    parsedChainId != null &&
    Number.isSafeInteger(parsedChainId) &&
    parsedChainId > 0
      ? parsedChainId
      : undefined;
  const waitForIndex = Boolean(
    (location.state as RouteState | null)?.waitForStrategyIndex,
  );
  const position = usePosition(strategyHash, {
    waitForIndex,
    chainId,
    source,
  });
  const history = usePositionHistory(position.data ? strategyHash : undefined, {
    chainId,
    source,
  });
  const depth = usePositionDepth(position.data, {
    waitForLiquidity: waitForIndex,
    chainId,
    source,
  });
  const [hoverFrac, setHoverFrac] = useState<number | null>(null);
  const rangeHint = useId();
  const settlements = useTrades(
    strategyHash
      ? { status: "confirmed", strategy_hash: strategyHash, chainId }
      : undefined,
  );
  const sd = strategyDetail(
    position.data,
    history.data,
    settlements.data?.items ?? [],
  );
  const [baseSymbol = "", quoteSymbol = ""] = (position.data?.pair ?? "").split(
    /\s*\/\s*/,
  );
  const base = assets.find(
    (asset) =>
      asset.address.toLowerCase() === position.data?.base.address.toLowerCase(),
  );
  const quote = assets.find(
    (asset) =>
      asset.address.toLowerCase() ===
      position.data?.quote.address.toLowerCase(),
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
            <div className={styles.strategyTitle}>
              <span className={styles.strategyIdentity}>
                <AssetIdentity asset={base} />
                <AssetIdentity asset={quote} />
              </span>
              <span className={styles.titleSm}>{sd.title}</span>
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
                k.label === "Range"
                  ? styles.rangeStat
                  : `${styles.statTight} ${styles.statHelp}`
              }
              tabIndex={0}
              aria-describedby={
                k.label === "Range"
                  ? rangeHint
                  : `strategy-${k.label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-tooltip`
              }
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
              {k.label !== "Range" && (
                <span
                  id={`strategy-${k.label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-tooltip`}
                  role="tooltip"
                  className={styles.statTooltip}
                >
                  {STAT_HELP[k.label]}
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
              className={`${styles.statTighter} ${styles.statHelp}`}
              tabIndex={0}
              aria-describedby={`strategy-${k.label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-tooltip`}
              style={{
                backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
              }}
            >
              <span
                id={`strategy-${k.label.toLowerCase().replace(/[^a-z0-9]+/g, "-")}-tooltip`}
                role="tooltip"
                className={styles.statTooltip}
              >
                {STAT_HELP[k.label]}
              </span>
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
            showMetricHelp
            titleHelp="Liquidity this strategy can quote across its active price range."
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
