import { useEffect } from "react";
import { Link, useParams } from "react-router-dom";
import { tradeProblem, useTrade } from "@/services/explorer";
import { useConfig } from "@/services/system";
import { Crumbs } from "@/components/Crumbs";
import { AssetIdentity } from "@/components/AssetIdentity";
import { RetryNotice } from "@/components/RetryNotice";
import { explorerUrl, tradeDetail } from "@/lib/explorer";
import { useAppActions } from "@/state";
import { TradeLifecycle } from "./TradeLifecycle";

import styles from "./explorer.module.css";

export function TradeDetailPage() {
  const { set } = useAppActions();
  const { tradeId } = useParams();
  const query = useTrade(tradeId);
  const config = useConfig();
  // A direct trade URL must return to the list regardless of the previous strategy selection.
  useEffect(() => set({ xpStrat: null, trail: [] }), [set]);
  if (!query.data)
    return (
      <div className={styles.root}>
        <Link
          to="/explorer"
          className={styles.back}
          aria-label="Back to Explorer"
        >
          ←
        </Link>
        {query.isError ? (
          <RetryNotice
            message={tradeProblem(query.error)}
            onRetry={() => void query.refetch()}
          />
        ) : (
          <p className={styles.emptyNote} role="status">
            Loading trade…
          </p>
        )}
      </div>
    );
  const trade = query.data;
  const detail = tradeDetail(trade);
  const txUrl = explorerUrl(
    config.data?.block_explorer_url,
    "tx",
    trade.txHash,
  );

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <Link
          to="/explorer"
          className={styles.back}
          aria-label="Back to Explorer"
        >
          ←
        </Link>
        <div className={styles.headTitle}>
          <Crumbs
            current={detail.title}
            trail={[{ label: "Explorer", to: "/explorer" }]}
          />
          <div className={styles.title}>{detail.title}</div>
        </div>
        <span
          key={detail.status}
          className={styles.liveStatus}
          style={detail.statusStyle}
        >
          {detail.status}
        </span>
        <span className={styles.headMeta}>
          {query.isError ? "Refresh delayed" : detail.blockLabel}
        </span>
        <span className={styles.spacer} />
        <span>
          <a
            className={styles.txLink}
            href={txUrl}
            target="_blank"
            rel="noopener noreferrer"
            aria-label="View transaction"
            title={trade.txHash ?? undefined}
          >
            {detail.transactionLabel}
            {txUrl && " ↗"}
          </a>
        </span>
      </div>

      {query.isError && (
        <RetryNotice
          message="Couldn’t refresh this trade."
          onRetry={() => void query.refetch()}
        />
      )}
      <div className={styles.stats4}>
        {detail.summary.map((stat) => (
          <div
            key={stat.label}
            className={styles.stat}
            style={{
              backgroundImage: `linear-gradient(${stat.sep}, ${stat.sep})`,
            }}
          >
            <div className={styles.statLabel}>{stat.label}</div>
            {trade.flow === "cross-chain" && stat.label === "In → out" ? (
              <div className={styles.crossChainFlow} title={stat.value}>
                <span>{trade.input.display}</span>
                <AssetIdentity
                  asset={
                    trade.input.net
                      ? { ...trade.input, net: trade.input.net }
                      : undefined
                  }
                />
                <span aria-hidden="true">→</span>
                <span>
                  {trade.status === "confirmed" ? "" : "min. "}
                  {trade.output.display}
                </span>
                <AssetIdentity
                  asset={
                    trade.output.net
                      ? { ...trade.output, net: trade.output.net }
                      : undefined
                  }
                />
              </div>
            ) : (
              <div className={styles.statValueSm} title={stat.value}>
                {stat.value}
              </div>
            )}
          </div>
        ))}
      </div>

      <div className={styles.split}>
        <section className={styles.mainCol}>
          <TradeLifecycle trade={trade} />

          <div className={styles.sourcedHead}>
            <span className={styles.sourcedTitle}>
              {trade.flow === "cross-chain"
                ? "Destination liquidity"
                : "Sourced from"}
            </span>
            <span className={styles.sourcedHint}>
              {trade.flow === "cross-chain" && trade.output.net
                ? `${trade.output.net} execution · click a row to open its strategy`
                : "click a leg to open the maker"}
            </span>
          </div>
          <div data-scroll="1" className={styles.legList}>
            {detail.legs.map((leg) => (
              <Link
                key={`${leg.maker}:${leg.hash}`}
                className={styles.legRow}
                to={
                  leg.chainId == null
                    ? `/explorer/strategies/${leg.hash}`
                    : `/explorer/strategies/${leg.hash}?chain=${leg.chainId}`
                }
                title={leg.maker}
              >
                <span className={styles.legMaker}>
                  <span className={styles.legChip}>{leg.tag}</span>
                  <span className={styles.legStack}>
                    <span className={styles.legName}>{leg.name}</span>
                    <span className={styles.legHash} title={leg.hash}>
                      {leg.shortHash}
                    </span>
                  </span>
                </span>
                <span className={styles.legAmountCol}>
                  <span className={styles.legAmountRow}>
                    {trade.flow === "cross-chain" ? (
                      <span
                        className={styles.crossChainLegFlow}
                        title={leg.amt}
                      >
                        <span>{leg.input.display}</span>
                        <AssetIdentity
                          asset={
                            leg.input.net
                              ? { ...leg.input, net: leg.input.net }
                              : undefined
                          }
                        />
                        <span aria-hidden="true">→</span>
                        <span>{leg.output.display}</span>
                        <AssetIdentity
                          asset={
                            leg.output.net
                              ? { ...leg.output, net: leg.output.net }
                              : undefined
                          }
                        />
                      </span>
                    ) : (
                      <>
                        <span className={styles.legAmount} title={leg.amt}>
                          {leg.amt}
                        </span>
                        <span className={styles.legCurve}>{leg.curve}</span>
                      </>
                    )}
                  </span>
                  <span className={styles.legTrack}>
                    <span
                      className={styles.legFill}
                      style={{ width: leg.barW }}
                    />
                  </span>
                </span>
                <span className={styles.legShare}>{leg.share}</span>
                <span className={styles.legChevron}>›</span>
              </Link>
            ))}
            {detail.empty && (
              <div className={styles.emptyNote}>{detail.emptyText}</div>
            )}
          </div>
        </section>

        <section data-scroll="1" className={styles.sideCol}>
          <div className={styles.profit}>
            <div className={styles.profitHead}>
              <span className={styles.profitSwatch} />
              <span className={styles.profitLabel}>Expected profit</span>
            </div>
            <div className={styles.profitValue}>{detail.profit}</div>
            <div className={styles.profitTag}>{detail.profitTag}</div>
          </div>

          <div className={styles.facts}>
            <div className={styles.factsTitle}>Order details</div>
            {detail.facts.map((d) => (
              <div key={d.label} className={styles.factRow}>
                <span className={styles.factLabel}>{d.label}</span>
                <span
                  className={styles.factValue}
                  title={d.fullValue ?? d.value}
                >
                  {d.value}
                </span>
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}
