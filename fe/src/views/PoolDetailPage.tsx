import { Link, useNavigate, useParams } from "react-router-dom";

import { DepthChart } from "@/components/DepthChart";
import { Crumbs } from "@/components/Crumbs";
import { RetryNotice } from "@/components/RetryNotice";
import { poolDetail } from "@/lib/pool-detail";
import { tokenText } from "@/lib/explorer";
import { useTrades } from "@/services/explorer";
import { usePool, usePoolDepth, usePoolRoster } from "@/services/pools";
import { useApp } from "@/state";
import { QueryFreshness } from "./QueryFreshness";

import styles from "./PoolDetailPage.module.css";

const MAKER_SORTS = ["Virtual", "Actual"];

export function PoolDetailPage() {
  const { state, set } = useApp();
  const navigate = useNavigate();
  const { pair } = useParams();
  const pool = usePool(pair);
  const settlements = useTrades(
    pool?.ref
      ? { status: "confirmed", base: pool.ref.base, quote: pool.ref.quote }
      : undefined,
  );
  const d = poolDetail({
    pool,
    roster: usePoolRoster(pool),
    depth: usePoolDepth(pool),
    hoverFrac: state.hoverFrac,
    makerSort: state.makerSort,
  });

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <button
          type="button"
          className={styles.back}
          onClick={() => navigate("/pools")}
        >
          ←
        </button>
        <div className={styles.headTitle}>
          <Crumbs current={d.pair} trail={[{ label: "Pools", to: "/pools" }]} />
          <div className={styles.pair}>{d.pair}</div>
        </div>
        <span className={styles.limeSquare} />
        <span className={styles.spacer} />
        <div className={styles.headActions}>
          <span className={styles.tag}>fee {d.fee}</span>
          <button
            type="button"
            className={styles.create}
            onClick={() => navigate(`/pools/${pair}/new`)}
          >
            <span>Create position</span>
            <span className={styles.createArrow}>→</span>
          </button>
        </div>
      </div>

      <div className={styles.grid}>
        <div className={styles.kpis}>
          <section className={styles.kpiWide}>
            <div className={styles.kpiTag}>
              <span className={styles.kpiSwatch} />
              <span className={styles.kpiLabel}>Depth</span>
            </div>
            <div>
              <div className={styles.kpiRow}>
                <span className={styles.kpiValue}>{d.tvl}</span>
                {d.tvlChange && (
                  <span className={styles.kpiDelta}>{d.tvlChange}</span>
                )}
              </div>
              <div className={styles.kpiSub}>Total value locked</div>
            </div>
            <div className={styles.kpiFoot}>
              Zero-inventory fills routed through{" "}
              <span className={styles.kpiFootMark}>Aqua</span>
            </div>
          </section>

          <section className={styles.kpiDark}>
            <div className={styles.kpiTag}>
              <span className={styles.kpiSwatch} />
              <span className={styles.kpiLabelDark}>Net APR</span>
            </div>
            <div>
              <div className={styles.kpiValueLime}>{d.apr}</div>
              <div className={styles.kpiSubDark}>
                After {d.fee} fee
                <br />
                {d.spread} spread
              </div>
            </div>
          </section>

          <section className={styles.kpiSmall}>
            <span className={styles.kpiLabel}>24h volume</span>
            <span className={styles.kpiSmallValue}>{d.vol}</span>
          </section>
          <section className={styles.kpiSmallRight}>
            <span className={styles.kpiLabel}>24h fills</span>
            <span className={styles.kpiSmallValue}>{d.fills}</span>
          </section>
        </div>

        <DepthChart
          data={d}
          title="Aggregated depth"
          legend={`${d.makerTotal} ${d.makerTotal === 1 ? "maker" : "makers"}`}
          onHoverChange={(hoverFrac) => set({ hoverFrac })}
        />

        <section className={styles.side}>
          <div className={styles.sideHead}>
            <div className={styles.sideTitleGroup}>
              <span className={styles.kpiSwatch} />
              <span className={styles.sideTitle}>Makers</span>
              <span className={styles.sideCount}>{d.makerTotal}</span>
            </div>
            <div className={styles.segmented}>
              {MAKER_SORTS.map((t) => (
                <button
                  key={t}
                  type="button"
                  className={
                    t === state.makerSort
                      ? styles.segmentSmallOn
                      : styles.segmentSmall
                  }
                  onClick={() => set({ makerSort: t })}
                >
                  {t}
                </button>
              ))}
            </div>
          </div>

          <div data-scroll="1" className={styles.makerList}>
            {d.makers.map((m) => (
              <button
                key={m.addr}
                type="button"
                className={styles.makerRow}
                onClick={() =>
                  navigate(`/explorer/strategies/${m.strategyHash}`)
                }
              >
                <span
                  className={styles.makerDot}
                  style={{ background: m.gap }}
                />
                <span className={styles.makerAddr}>{m.addr}</span>
                <span className={styles.makerAct}>{m.act}</span>
                <span
                  className={styles.makerState}
                  style={{
                    background: m.stateBg,
                    color: m.stateFg,
                  }}
                >
                  {m.up}
                </span>
              </button>
            ))}
          </div>

          <div className={styles.settleHead}>
            <div className={styles.settleTag}>
              <span className={styles.settleTitle}>Settlements</span>
            </div>
            <QueryFreshness query={settlements} />
          </div>

          <div
            data-scroll="1"
            className={styles.settleList}
            aria-busy={settlements.isFetching}
          >
            {settlements.isLoading && (
              <p className={styles.settleLive} role="status">
                Loading settlements…
              </p>
            )}
            {settlements.isError && (
              <RetryNotice
                message="Couldn’t refresh settlements."
                onRetry={() => void settlements.refetch()}
              />
            )}
            {settlements.isSuccess && settlements.data.items.length === 0 && (
              <p className={styles.settleLive}>
                No confirmed trades for this pair yet.
              </p>
            )}
            {settlements.data?.items.map((trade) => {
              const settledAt =
                trade.settledAt == null
                  ? null
                  : new Date(trade.settledAt * 1000);
              return (
                <Link
                  key={trade.id}
                  className={styles.settleRow}
                  to={`/explorer/trades/${encodeURIComponent(trade.id)}`}
                  aria-label={`Open trade ${trade.id}`}
                >
                  <span className={styles.settleFrom}>
                    {tokenText(trade.input)}
                  </span>
                  <span className={styles.settleArrow}>→</span>
                  <span className={styles.settleTo}>
                    {tokenText(trade.output)}
                  </span>
                  <time
                    className={styles.settleAgo}
                    dateTime={settledAt?.toISOString()}
                    title={settledAt?.toLocaleString()}
                  >
                    {settledAt?.toLocaleString(undefined, {
                      month: "short",
                      day: "numeric",
                      hour: "2-digit",
                      minute: "2-digit",
                    }) ?? "Time unavailable"}
                  </time>
                </Link>
              );
            })}
          </div>
        </section>
      </div>
    </div>
  );
}
