import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { GLOSSARY, type GlossaryKey } from "@/lib/glossary";

import styles from "./Tooltip.module.css";

/** Gap between the trigger and the bubble. */
const OFFSET = 8;
const MAX_WIDTH = 280;

type Position = { left: number; top: number; below: boolean };

/**
 * A term with its definition on hover or focus.
 *
 * The bubble is portalled to the body and positioned from a measured rect rather than laid out
 * next to the trigger: several of the panels it appears in are `overflow: hidden`, which would
 * clip an absolutely positioned child.
 *
 * Opens on focus as well as hover and is reachable by keyboard, so the definition is not
 * mouse-only. Native `title` is left for revealing text that was truncated — a different job.
 */
export function Term({
  term,
  children,
  className,
}: {
  term: GlossaryKey;
  children: React.ReactNode;
  className?: string;
}) {
  const id = useId();
  const ref = useRef<HTMLSpanElement>(null);
  const [at, setAt] = useState<Position | null>(null);

  const show = () => {
    const box = ref.current?.getBoundingClientRect();
    if (!box) return;
    // Flip under the trigger when there is not room above it.
    const below = box.top < 96;
    setAt({
      left: Math.min(
        Math.max(box.left + box.width / 2, MAX_WIDTH / 2 + 8),
        window.innerWidth - MAX_WIDTH / 2 - 8,
      ),
      top: below ? box.bottom + OFFSET : box.top - OFFSET,
      below,
    });
  };
  const hide = () => setAt(null);

  useEffect(() => {
    if (!at) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") hide();
    };
    window.addEventListener("keydown", onKey);
    // A tooltip anchored to a rect goes stale the moment anything moves.
    window.addEventListener("scroll", hide, true);
    window.addEventListener("resize", hide);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", hide, true);
      window.removeEventListener("resize", hide);
    };
  }, [at]);

  return (
    <>
      <span
        aria-describedby={at ? id : undefined}
        className={className ? `${styles.term} ${className}` : styles.term}
        onBlur={hide}
        onFocus={show}
        onMouseEnter={show}
        onMouseLeave={hide}
        ref={ref}
        tabIndex={0}
      >
        {children}
      </span>
      {at &&
        createPortal(
          <span
            className={at.below ? styles.bubbleBelow : styles.bubble}
            id={id}
            role="tooltip"
            style={{ left: at.left, top: at.top }}
          >
            {GLOSSARY[term]}
          </span>,
          document.body,
        )}
    </>
  );
}
