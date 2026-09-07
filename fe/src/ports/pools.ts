import type { Pool } from "@/data";

/** Pool reads, in the shape the views render. Implemented by `adapters/http`. */
export interface PoolsPort {
  list(): Promise<Pool[]>;
}
