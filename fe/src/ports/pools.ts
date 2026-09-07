import type { Pool, PoolRoster, DepthCurve, PairRef } from "@/data";

/** Pool reads, in the shape the views render. Implemented by `adapters/http`. */
export interface PoolsPort {
  list(): Promise<Pool[]>;
  /** The pool row plus the makers quoting it. */
  detail(pair: PairRef): Promise<PoolRoster>;
  /** Executable size against price impact, deepest first. */
  depth(pair: PairRef): Promise<DepthCurve>;
}
