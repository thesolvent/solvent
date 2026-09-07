import type { Page } from "./state";

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

export function pageFromPath(pathname: string): Page | null {
  const path = pathname.length > 1 ? pathname.replace(/\/+$/, "") : pathname;
  return BY_PATH.get(path) ?? null;
}
