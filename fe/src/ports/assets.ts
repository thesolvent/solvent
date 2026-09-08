import type { Asset } from "@/data";

/** The assets this deployment serves, in the shape the picker renders. */
export interface AssetsPort {
  list(): Promise<Asset[]>;
}
