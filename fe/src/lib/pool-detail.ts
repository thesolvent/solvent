import type { DepthCurve, Pool, PoolRoster, RosterMaker } from "@/data";
import { depthChart, type DepthChartModel } from "./depth-chart";

export type DetailMaker = {
  strategyHash: string;
  addr: string;
  curve: string;
  up: string;
  act: string;
  /** Teal when the maker quotes under its virtual size, lime when at full size. */
  gap: string;
  stateBg: string;
  stateFg: string;
};

export interface PoolDetail extends DepthChartModel {
  pair: string;
  fee: string;
  apr: string;
  tvl: string;
  /** Absent while the move is unknown, so the badge can be left off. */
  tvlChange?: string;
  vol: string;
  fills: string;
  spread: string;
  makers: DetailMaker[];
  makerTotal: number;
}

export interface PoolDetailInput {
  pool: Pool | undefined;
  roster: PoolRoster | undefined;
  depth: DepthCurve | undefined;
  /** Where along the curve the pointer is asking to look — over the plot, or over a tier. */
  hoverFrac: number | null;
  /** "Virtual" shows what each maker committed; "Actual" what it could deliver now. */
  makerSort: string;
}

/**
 * The roster, showing whichever balance the toggle asks for and ranked by it.
 *
 * A maker quoting less than it committed is short of its own position, which the row says by
 * carrying the warmer state colours rather than the lime ones.
 */
function rosterBy(
  roster: PoolRoster | undefined,
  deliverable: boolean,
): DetailMaker[] {
  const shown = (maker: RosterMaker) =>
    deliverable ? maker.actualUsd : maker.virtualUsd;

  return [...(roster?.makers ?? [])]
    .sort((a, b) => (shown(b) ?? -Infinity) - (shown(a) ?? -Infinity))
    .map((maker) => {
      const value = shown(maker);
      const short =
        maker.actualUsd != null &&
        maker.virtualUsd != null &&
        maker.actualUsd < maker.virtualUsd;
      return {
        strategyHash: maker.strategyHash,
        addr: maker.address,
        curve: maker.curve,
        // Quote uptime has no server source.
        up: "—",
        act: value == null ? "—" : USD.format(value),
        gap: short ? "var(--ok-ink)" : "var(--green)",
        stateBg: short ? "var(--ok-bg)" : "var(--lime-wash-soft)",
        stateFg: short ? "var(--ok-ink-deep)" : "var(--green-darkest)",
      };
    });
}

const USD = new Intl.NumberFormat("en-US", {
  style: "currency",
  currency: "USD",
  notation: "compact",
  maximumFractionDigits: 1,
});

export function poolDetail(input: PoolDetailInput): PoolDetail {
  const { pool, roster, depth, hoverFrac, makerSort } = input;
  const pair = pool?.pair ?? "";
  const [baseSymbol = "", quoteSymbol = ""] = pair.split(" / ");
  return {
    ...depthChart({
      depth,
      baseSymbol,
      quoteSymbol,
      hoverFrac,
    }),
    pair: pair || "—",
    fee: pool?.fee ?? "—",
    apr: pool?.apr ?? "—",
    tvl: pool?.tvl ?? "—",
    tvlChange: pool?.tvlChange,
    vol: pool?.vol ?? "—",
    fills: pool?.fills ?? "—",
    spread: (pool?.range ?? "").replace(" spread", "") || "—",
    makers: rosterBy(roster, makerSort === "Actual"),
    makerTotal: new Set(
      roster?.makers.map((maker) => maker.address.toLowerCase()),
    ).size,
  };
}
