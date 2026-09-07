import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import {
  AppCtx,
  DEFAULT_CONFIG,
  INITIAL_STATE,
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
  const [state, setState] = useState<AppState>(INITIAL_STATE);
  const wipeTimers = useRef<number[]>([]);

  useEffect(
    () => () => {
      wipeTimers.current.forEach(clearTimeout);
    },
    [],
  );

  const set = useCallback((patch: Partial<AppState>) => {
    setState((prev) => ({ ...prev, ...patch }));
  }, []);

  const page = state.page ?? config.landingPage;

  // The lime veil sweeps up over 520ms; the page swaps behind it at 210ms.
  const navTo = useCallback((next: Page) => {
    wipeTimers.current.forEach(clearTimeout);
    wipeTimers.current = [];
    setState((prev) => ({ ...prev, wipe: true }));
    wipeTimers.current.push(
      window.setTimeout(() => {
        setState((prev) => ({
          ...prev,
          page: next,
          trail: [],
          detail: null,
          create: false,
          xpTrade: null,
          xpStrat: null,
        }));
        wipeTimers.current.push(
          window.setTimeout(
            () => setState((prev) => ({ ...prev, wipe: false })),
            300,
          ),
        );
      }, 210),
    );
  }, []);

  const go = useCallback((next: Page) => () => navTo(next), [navTo]);

  const push = useCallback(
    (view: Partial<AppState>, label: string) => {
      setState((prev) => {
        const snap: Snap = {
          page: prev.page ?? config.landingPage,
          detail: prev.detail,
          create: prev.create,
          xpTrade: prev.xpTrade,
          xpStrat: prev.xpStrat,
          maker: prev.maker,
        };
        return {
          ...prev,
          ...view,
          trail: [...prev.trail, { snap, label }],
        };
      });
    },
    [config.landingPage],
  );

  const pop = useCallback(() => {
    setState((prev) => {
      const trail = prev.trail.slice();
      const last = trail.pop();
      if (!last) {
        return {
          ...prev,
          detail: null,
          create: false,
          xpTrade: null,
          xpStrat: null,
          maker: null,
          trail: [],
        };
      }
      return { ...prev, ...last.snap, trail };
    });
  }, []);

  const crumbs = useCallback(
    (current: string) =>
      state.trail
        .map((c, i) => ({
          label: c.label,
          sep: "›",
          fg: "var(--text-muted)",
          go: () =>
            setState((prev) => ({
              ...prev,
              ...c.snap,
              trail: prev.trail.slice(0, i),
            })),
        }))
        .concat([{ label: current, sep: "", fg: "var(--ink)", go: () => {} }]),
    [state.trail],
  );

  const value = useMemo<AppApi>(
    () => ({ state, config, page, set, navTo, go, push, pop, crumbs }),
    [state, config, page, set, navTo, go, push, pop, crumbs],
  );

  return <AppCtx.Provider value={value}>{children}</AppCtx.Provider>;
}
