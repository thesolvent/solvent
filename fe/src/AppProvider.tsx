import { useCallback, useMemo, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router-dom";

import { PATHS, pageFromPath, type RouteState } from "./routes";
import { useAppStore } from "./store";
import {
  AppActionsCtx,
  AppCtx,
  DEFAULT_CONFIG,
  type AppActions,
  type AppApi,
  type AppConfig,
  type AppState,
  type Page,
  type Snap,
} from "./state";

export function AppProvider({
  children,
  config = DEFAULT_CONFIG,
}: {
  children: ReactNode;
  config?: AppConfig;
}) {
  const state = useAppStore();
  const set = useAppStore((store) => store.set);
  const navigate = useNavigate();
  const { pathname } = useLocation();

  const page = pageFromPath(pathname) ?? config.landingPage;

  const navTo = useCallback(
    (next: Page) => {
      navigate(PATHS[next], {
        state: { resetSubviews: true } satisfies RouteState,
      });
    },
    [navigate],
  );

  const go = useCallback((next: Page) => () => navTo(next), [navTo]);

  /** Sub-views live on a stack rather than the URL: they are still keyed by position in static
   *  data, and would deep-link to the wrong row once that data comes from the server. */
  const push = useCallback(
    (view: Partial<AppState>, label: string) => {
      const previous = useAppStore.getState();
      const snap: Snap = {
        page,
        detail: previous.detail,
        create: previous.create,
        xpStrat: previous.xpStrat,
        maker: previous.maker,
      };
      set({ ...view, trail: [...previous.trail, { snap, label }] });
    },
    [page, set],
  );

  const restore = useCallback(
    (snap: Snap, trail: AppState["trail"]) => {
      const { page: from, ...view } = snap;
      set({ ...view, trail });
      if (from !== page) navigate(PATHS[from]);
    },
    [navigate, page, set],
  );

  const pop = useCallback(() => {
    const trail = useAppStore.getState().trail.slice();
    const last = trail.pop();
    if (!last) {
      set({
        detail: null,
        create: false,
        xpStrat: null,
        maker: null,
        trail: [],
      });
      return;
    }
    restore(last.snap, trail);
  }, [restore, set]);

  const crumbs = useCallback(
    (current: string) =>
      state.trail
        .map((crumb, i) => ({
          label: crumb.label,
          sep: "›",
          fg: "var(--text-muted)",
          go: () => restore(crumb.snap, state.trail.slice(0, i)),
        }))
        .concat([{ label: current, sep: "", fg: "var(--ink)", go: () => {} }]),
    [restore, state.trail],
  );

  const actions = useMemo<AppActions>(
    () => ({ config, page, set, navTo, go, push, pop, crumbs }),
    [config, page, set, navTo, go, push, pop, crumbs],
  );

  const value = useMemo<AppApi>(
    () => ({ ...actions, state }),
    [actions, state],
  );

  return (
    <AppActionsCtx.Provider value={actions}>
      <AppCtx.Provider value={value}>{children}</AppCtx.Provider>
    </AppActionsCtx.Provider>
  );
}
