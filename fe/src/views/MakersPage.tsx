import { Pagination } from "@/components/Pagination";
import { RebateList } from "@/components/RebateList";
import { useLoadedPagination } from "@/components/useLoadedPagination";
import { Term } from "@/components/Tooltip";
import { slug, usePools } from "@/services/pools";
import { useAssets } from "@/services/assets";
import { PERIODS, SPANS, makerView } from "@/lib/makers";
import { loadedPageLabel } from "@/lib/pagination";
import {
  useMakers,
  useMakerDashboard,
  useMakerPositions,
  useMakerInventory,
  useMakerSettlements,
} from "@/services/makers";
import { useManagePosition } from "@/services/positions";
import { useRebates } from "@/services/rebates";
import { useWalletAction } from "@/services/wallet";
import { useApp } from "@/state";
import { useNavigate, useParams } from "react-router-dom";

import { MakerAssets } from "./MakerAssets";
import { MakerPositions } from "./MakerPositions";
import styles from "./MakersPage.module.css";

function ChartTooltip({
  label,
  value,
  detail,
}: {
  label: string;
  value: string;
  detail: string;
}) {
  return (
    <div className={styles.chartTooltip} role="tooltip">
      <div className={styles.chartTooltipLabel}>{label}</div>
      <div className={styles.chartTooltipRow}>
        <span className={styles.chartTooltipValue}>{value}</span>
        <span className={styles.chartTooltipDetail}>{detail}</span>
      </div>
    </div>
  );
}

function sameAddress(left: string | undefined, right: string | undefined) {
  return Boolean(left && right && left.toLowerCase() === right.toLowerCase());
}

export function MakersPage() {
  const navigate = useNavigate();
  const { state, set } = useApp();
  const { maker } = useParams();
  const wallet = useWalletAction();
  const walletAddress = wallet.address;
  const roster = useMakers();
  const makers = [...(roster.data ?? [])].sort(
    (left, right) =>
      Number(sameAddress(right.address, walletAddress)) -
      Number(sameAddress(left.address, walletAddress)),
  );
  const ownMaker = makers.find(({ address: candidate }) =>
    sameAddress(candidate, walletAddress),
  );
  const address = maker ?? ownMaker?.address ?? makers[0]?.address;
  const canManage = sameAddress(address, walletAddress);
  const positionActions = useManagePosition();
  const period = PERIODS[state.mkSpan] ?? "7d";
  const dashboard = useMakerDashboard(address, period);
  const positions = useMakerPositions(address, period);
  const pools = usePools();
  const creationPair = positions.data?.[0]?.pair ?? pools[0]?.pair;
  const inventory = useMakerInventory(address, period);
  const settlements = useMakerSettlements(address, period);
  const assets = useAssets();
  const rebates = useRebates(
    address ? { maker: address, status: "executed" } : undefined,
  );
  const settlementPages =
    settlements.data?.pages.map(({ items }) => items) ?? [];
  const settlementPagination = useLoadedPagination(
    settlementPages,
    Boolean(settlements.hasNextPage),
    async () => !(await settlements.fetchNextPage()).isError,
  );
  const rebateRows = rebates.data?.pages.flatMap((page) => page.items) ?? [];
  const activeQuery =
    state.mkTab === "Positions"
      ? positions
      : state.mkTab === "Assets"
        ? inventory
        : state.mkTab === "Settlements"
          ? settlements
          : rebates;
  const failed = roster.isError || dashboard.isError || activeQuery.isError;
  const notice = failed
    ? "Updates unavailable · retrying"
    : dashboard.isPending
      ? "Loading maker…"
      : undefined;
  const mk = makerView(state, {
    address,
    dashboard: dashboard.data,
    positions: positions.data ?? [],
    inventory: inventory.data ?? [],
    settlements: settlementPagination.items,
    rebateCount: rebateRows.length,
    notice,
  });

  return (
    <div className={styles.root}>
      <div className={styles.head}>
        <div className={styles.headTitle}>
          <div className={styles.headLabel}>Maker</div>
          <div className={styles.addr}>
            {canManage ? "Your maker" : mk.addr}
          </div>
        </div>
        <div className={styles.segmented}>
          {makers.map(({ address: a }) => (
            <button
              key={a}
              type="button"
              className={
                a.toLowerCase() === address?.toLowerCase()
                  ? styles.rosterButtonOn
                  : styles.rosterButton
              }
              onClick={() => {
                set({ mkAsset: null, mkTip: null });
                navigate(`/makers/${a}`);
              }}
            >
              {sameAddress(a, walletAddress) ? "Your maker" : a.slice(-4)}
            </button>
          ))}
        </div>
        <span className={styles.spacer} />
        <div className={styles.spanGroup}>
          {SPANS.map((t) => (
            <button
              key={t}
              type="button"
              className={
                t === mk.span ? styles.spanButtonOn : styles.spanButton
              }
              onClick={() => set({ mkSpan: t })}
            >
              {t}
            </button>
          ))}
        </div>
        <button
          type="button"
          className={styles.newPos}
          disabled={!creationPair}
          onClick={() =>
            creationPair && navigate(`/pools/${slug(creationPair)}/new`)
          }
        >
          <span>Create position</span>
          <span className={styles.newPosPlus}>+</span>
        </button>
      </div>

      <div className={styles.kpis}>
        {mk.kpis.map((k) => (
          <div
            key={k.label}
            className={styles.kpi}
            style={{
              backgroundColor: k.bg,
              backgroundImage: `linear-gradient(${k.sep}, ${k.sep})`,
            }}
          >
            <div className={styles.kpiLabel}>
              {k.term ? <Term term={k.term}>{k.label}</Term> : k.label}
            </div>
            <div className={styles.kpiRow}>
              <span className={styles.kpiValue}>{k.value}</span>
              <span className={styles.kpiDelta} style={{ color: k.deltaFg }}>
                {k.delta}
              </span>
            </div>
          </div>
        ))}
      </div>

      <div className={styles.body}>
        <section className={styles.main}>
          <div className={styles.tabBar}>
            <div className={styles.tabGroup}>
              {mk.tabs.map((t) => (
                <button
                  key={t.key}
                  type="button"
                  className={t.key === mk.tab ? styles.tabOn : styles.tab}
                  onClick={() =>
                    set({
                      mkTab: t.key,
                      mkAsset:
                        t.key === "Assets" && state.mkTab !== "Assets"
                          ? 0
                          : state.mkAsset,
                    })
                  }
                >
                  {t.label}
                </button>
              ))}
            </div>
            <span className={styles.tabNote}>{mk.tabNote}</span>
          </div>

          {mk.tab === "Positions" && (
            <MakerPositions
              key={address}
              positions={mk.positions}
              canManage={canManage}
              walletAction={canManage ? wallet.switchTo : undefined}
              onPrepareWallet={
                canManage && wallet.switchTo ? wallet.prepare : undefined
              }
              actionStatus={positionActions.status}
              onClearAction={positionActions.clear}
              onOpenPosition={(hash) =>
                navigate(`/explorer/strategies/${encodeURIComponent(hash)}`)
              }
              onClone={(position) =>
                navigate(
                  `/pools/${slug(position.pair)}/new?clone=${encodeURIComponent(position.hash)}`,
                )
              }
              onPush={positionActions.push}
              onDock={positionActions.dock}
            />
          )}

          {mk.tab === "Assets" && (
            <MakerAssets
              assets={mk.assets}
              onToggle={(index) =>
                set({ mkAsset: mk.assets[index]?.open ? -1 : index })
              }
              onOpenPosition={(hash) =>
                navigate(`/explorer/strategies/${encodeURIComponent(hash)}`)
              }
            />
          )}

          {mk.tab === "Settlements" && (
            <>
              <div data-scroll="1" className={styles.list}>
                {mk.settlements.map((t) => (
                  <button
                    type="button"
                    key={t.trade}
                    className={styles.settleRow}
                    onClick={() => {
                      set({ xpStrat: null });
                      navigate(
                        `/explorer/trades/${encodeURIComponent(t.trade)}`,
                      );
                    }}
                  >
                    <span className={styles.settlePair}>
                      <span className={styles.settlePairName}>{t.pair}</span>
                      <span className={styles.settleBlk}>{t.blk}</span>
                    </span>
                    <span className={styles.settleFlow}>
                      <span className={styles.settleIn}>{t.inn}</span>
                      <span className={styles.settleArrow}>→</span>
                      <span className={styles.settleOut}>{t.out}</span>
                    </span>
                    <span className={styles.settleCell}>
                      <span className={styles.settleCellValue}>{t.fee}</span>
                      <span className={styles.settleCellLabel}>fee</span>
                    </span>
                    <span className={styles.settleCell}>
                      <span className={styles.settleCellValue}>{t.share}</span>
                      <span className={styles.settleCellLabel}>of fill</span>
                    </span>
                    <span className={styles.settleStatusCell}>
                      <span
                        className={styles.settleStatus}
                        style={{
                          background: t.stBg,
                          color: t.stFg,
                        }}
                      >
                        {t.status}
                      </span>
                      <span className={styles.settleTx}>{t.tx}</span>
                    </span>
                    <span className={styles.settleChevron}>›</span>
                  </button>
                ))}
              </div>
              <Pagination
                label={loadedPageLabel(
                  settlementPages,
                  settlementPagination.page,
                  Boolean(settlements.hasNextPage),
                  "settlements",
                )}
                page={settlementPagination.page}
                pageCount={settlementPagination.pageCount}
                disabled={settlements.isFetching || settlements.isError}
                onPage={(page) => void settlementPagination.select(page)}
              />
            </>
          )}

          {mk.tab === "Rebates" && (
            <RebateList
              key={`${address}:${period}`}
              assets={assets}
              pages={rebates.data?.pages.map(({ items }) => items) ?? []}
              pending={rebates.isPending}
              fetching={rebates.isFetching}
              error={rebates.isError}
              hasMore={Boolean(rebates.hasNextPage)}
              onRetry={() => void rebates.refetch()}
              onLoadMore={async () => !(await rebates.fetchNextPage()).isError}
            />
          )}

          <div className={styles.insight}>
            <span className={styles.insightTag}>Insight</span>
            <span className={styles.insightText}>{mk.insight}</span>
            <span className={styles.insightChevron}>›</span>
          </div>
        </section>

        <div data-scroll="1" className={styles.rail}>
          <section className={styles.card}>
            <div className={styles.cardHead}>
              <span className={styles.swatch} />
              <span className={styles.cardTitle}>Fill share</span>
              <span className={styles.spacer} />
              <span className={styles.cardMore}>···</span>
            </div>
            <div className={styles.donutRow}>
              <div className={styles.donutWrap}>
                <svg viewBox="0 0 120 120" className={styles.donutSvg}>
                  <circle
                    cx="60"
                    cy="60"
                    r="46"
                    fill="none"
                    stroke="var(--surface-alt)"
                    strokeWidth="11"
                    strokeLinecap="round"
                    strokeDasharray={mk.trackDash}
                  />
                  {mk.arcs.map((arc, i) => (
                    <circle
                      key={i}
                      cx="60"
                      cy="60"
                      r="46"
                      fill="none"
                      strokeLinecap="round"
                      strokeWidth="11"
                      className={styles.donutArc}
                      stroke={arc.color}
                      strokeDasharray={arc.dash}
                      strokeDashoffset={arc.offset}
                      opacity={arc.op}
                      onMouseEnter={() => set({ mkTip: i })}
                      onMouseLeave={() => set({ mkTip: null })}
                    />
                  ))}
                </svg>
                <div className={styles.donutCenter}>
                  <div className={styles.donutCap}>{mk.donutCap}</div>
                  <div className={styles.donutVal}>{mk.donutVal}</div>
                </div>
              </div>
              <div className={styles.shareList}>
                {mk.shares.map((s, i) => (
                  <div
                    key={s.label}
                    className={styles.shareRow}
                    style={{ opacity: s.op }}
                    onMouseEnter={() => set({ mkTip: i })}
                    onMouseLeave={() => set({ mkTip: null })}
                  >
                    <span
                      className={styles.shareDot}
                      style={{ background: s.dot }}
                    />
                    <span className={styles.shareLabel}>{s.label}</span>
                    <span className={styles.shareValue}>{s.value}</span>
                  </div>
                ))}
              </div>
            </div>
            {mk.tip && (
              <ChartTooltip
                label={mk.tipLabel}
                value={mk.tipPct}
                detail={mk.tipAmt}
              />
            )}
          </section>

          <section className={styles.card}>
            <div className={styles.cardHead}>
              <span className={styles.swatch} />
              <span className={styles.cardTitle}>Fills</span>
              <span className={styles.spacer} />
              <span className={styles.cardMore}>···</span>
            </div>
            <div className={styles.metricRow}>
              <span className={styles.metricValue}>{mk.fills}</span>
              <span className={styles.metricDelta}>{mk.fillsDelta}</span>
              <span className={styles.metricNote}>vs prev period</span>
            </div>
            <div className={styles.barsWrap}>
              <div className={styles.avgLine} style={{ top: mk.avgTop }} />
              <div className={styles.avgChip} style={{ top: mk.avgTop }}>
                {mk.avgVal}
              </div>
              <div className={styles.bars}>
                {mk.bars.map((b, i) => (
                  <span
                    key={i}
                    className={styles.barCol}
                    onMouseEnter={() => set({ mkBar: i })}
                    onMouseLeave={() => set({ mkBar: null })}
                  >
                    <span
                      className={styles.bar}
                      style={{
                        height: b.h,
                        background: b.bg,
                      }}
                    />
                    <span className={styles.barDay} style={{ color: b.dayFg }}>
                      {b.day}
                    </span>
                  </span>
                ))}
              </div>
            </div>
            {mk.fillsTip && <ChartTooltip {...mk.fillsTip} />}
          </section>

          <section className={styles.cardLast}>
            <div className={styles.cardHead}>
              <span className={styles.swatch} />
              <span className={styles.cardTitle}>Fill latency</span>
              <span className={styles.spacer} />
              <span className={styles.cardMore}>···</span>
            </div>
            <div className={styles.latMetric}>
              <span className={styles.metricValue}>{mk.latency}</span>
              <span className={styles.latUnit}>p50</span>
              <span className={styles.metricNote}>{mk.latDelta}</span>
            </div>
            <div className={styles.latWrap}>
              <div className={styles.latAxis}>
                {mk.latAxis.map((label) => (
                  <span key={label}>{label}</span>
                ))}
              </div>
              <div className={styles.latPlot}>
                <svg
                  viewBox="0 0 420 120"
                  preserveAspectRatio="none"
                  className={styles.latSvg}
                >
                  {[4, 60, 116].map((y) => (
                    <line
                      key={y}
                      x1="0"
                      y1={y}
                      x2="420"
                      y2={y}
                      stroke="var(--surface-alt)"
                      strokeWidth="1"
                      vectorEffect="non-scaling-stroke"
                    />
                  ))}
                  {mk.latLines.map((line, i) => (
                    <polyline
                      key={i}
                      points={line}
                      fill="none"
                      stroke="var(--green)"
                      strokeWidth="2"
                      strokeLinejoin="round"
                      strokeLinecap="round"
                      vectorEffect="non-scaling-stroke"
                    />
                  ))}
                </svg>
                {mk.latPts.map((pt, i) => (
                  <span
                    key={i}
                    className={styles.latHit}
                    style={{ left: pt.left }}
                    onMouseEnter={() => set({ mkLat: i })}
                    onMouseLeave={() => set({ mkLat: null })}
                  >
                    {pt.top !== null && (
                      <span className={styles.latDot} style={{ top: pt.top }} />
                    )}
                  </span>
                ))}
              </div>
              <div className={styles.latDays}>
                {mk.latDays.map((d, i) => (
                  <span key={i}>{d}</span>
                ))}
              </div>
            </div>
            {mk.latencyTip && <ChartTooltip {...mk.latencyTip} />}
          </section>
        </div>
      </div>
    </div>
  );
}
