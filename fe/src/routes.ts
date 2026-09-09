import type { Page } from "./state";

export interface RouteState {
  resetSubviews?: boolean;
  waitForStrategyIndex?: boolean;
}

/** The address of each top-level page. The URL is the source of truth for which one is showing,
 *  so a reload or a shared link lands where it should. */
export const PATHS: Record<Page, string> = {
  Home: "/",
  Swap: "/swap",
  Pools: "/pools",
  Makers: "/makers",
  Explorer: "/explorer",
  Docs: "/docs",
};

const BY_PATH = new Map(
  Object.entries(PATHS).map(([page, path]) => [path, page as Page]),
);

/** Matches on the leading segment, so a page's own sub-routes still resolve to it. */
export function pageFromPath(pathname: string): Page | null {
  const [, segment = ""] = pathname.split("/");
  return BY_PATH.get(`/${segment}`) ?? null;
}
