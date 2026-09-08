import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { Routes, useLocation, useNavigationType } from "react-router-dom";
import { useAppStore } from "@/store";
import type { RouteState } from "@/routes";

import styles from "./TransitionRoutes.module.css";

export function TransitionRoutes({ children }: { children: ReactNode }) {
  const location = useLocation();
  const navigationType = useNavigationType();
  const previous = useRef(location);
  const [displayed, setDisplayed] = useState(location);
  const [animationKey, setAnimationKey] = useState<string | null>(null);
  const set = useAppStore((store) => store.set);

  useLayoutEffect(() => {
    const reveal = () => {
      if (
        navigationType !== "POP" &&
        (location.state as RouteState | null)?.resetSubviews
      ) {
        set({ trail: [], detail: null, create: false, xpStrat: null });
      }
      setDisplayed(location);
    };
    const changed = previous.current !== location;
    previous.current = location;
    if (
      !changed ||
      window.matchMedia?.("(prefers-reduced-motion: reduce)").matches
    ) {
      reveal();
      setAnimationKey(null);
      return;
    }

    setAnimationKey(location.key);
    // Keep the outgoing page mounted until the green sweep covers it.
    const revealTimer = window.setTimeout(reveal, 210);
    const finish = window.setTimeout(() => {
      setAnimationKey(null);
    }, 520);
    return () => {
      window.clearTimeout(revealTimer);
      window.clearTimeout(finish);
    };
  }, [location, navigationType, set]);

  return (
    <>
      {animationKey !== null && (
        <div
          key={animationKey}
          className={styles.wipe}
          aria-hidden="true"
          data-testid="route-transition"
        />
      )}
      <Routes location={displayed}>{children}</Routes>
    </>
  );
}
