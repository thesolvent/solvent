import { useCallback, useEffect, useRef, useState } from "react";

import { useApp } from "@/state";

import styles from "./HomePage.module.css";

const LEGS = [
  { i: 0, name: "Maker A" },
  { i: 1, name: "Maker B" },
  { i: 2, name: "Maker C" },
];

/** Where the dot sits, how far each maker leg has filled, and the running ledger. */
type Frame = {
  frac: number;
  fill: number;
  weth: number;
  usdc: number;
  earned: number;
};

const SCENE_TARGETS: Frame[] = [
  { frac: 0.3, fill: 0, weth: 100, usdc: 290000, earned: 0 },
  { frac: 0.3, fill: 0, weth: 100, usdc: 290000, earned: 0 },
  { frac: 0.3, fill: 1, weth: 100, usdc: 290000, earned: 0 },
  { frac: 0.58, fill: 1, weth: 99, usdc: 292930, earned: 30 },
  { frac: 0.3, fill: 1, weth: 100, usdc: 290059, earned: 89 },
  { frac: 0.3, fill: 1, weth: 100, usdc: 290059, earned: 89 },
];

/** Each maker leg fills to a different depth of the split. */
const LEG_DEPTHS = [0.6, 0.82, 0.34];

const EPS: Frame = {
  frac: 0.0002,
  fill: 0.0005,
  weth: 0.005,
  usdc: 0.5,
  earned: 0.05,
};

const SEED_WORD = "liquidity layer.";

const LANG_WORDS = [
  "capa de liquidez.",
  "流動性レイヤー。",
  "couche de liquidité.",
  "Liquiditätsschicht.",
  "유동성 레이어.",
  "camada de liquidez.",
  "livello di liquidità.",
  "流动性层。",
  "слой ликвидности.",
  "طبقة السيولة.",
  "liquidity layer.",
];

function prefersReducedMotion() {
  return (
    typeof window !== "undefined" &&
    window.matchMedia?.("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * Reveals `[data-reveal]` nodes as they enter the scroll root, staggering siblings
 * by 90ms. Above-the-fold nodes paint immediately; a timed sweep rescues anything
 * the observer misses.
 */
function useReveal(rootRef: React.RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;

    const nodes = Array.from(
      root.querySelectorAll<HTMLElement>("[data-reveal]"),
    );
    if (!nodes.length) return;

    const show = (el: Element) => el.classList.add(styles.revealShown);

    nodes.forEach((el) => {
      const sibs = el.parentElement
        ? Array.from(el.parentElement.children).filter((c) =>
            c.hasAttribute("data-reveal"),
          )
        : [el];
      const i = sibs.indexOf(el);
      if (i > 0) el.style.transitionDelay = `${i * 90}ms`;
    });

    const inView = (el: Element) => {
      const r = el.getBoundingClientRect();
      const rr = root.getBoundingClientRect();
      const h = root.clientHeight || 0;
      return r.top < rr.top + h * 1.02 && r.bottom > rr.top;
    };

    if (typeof IntersectionObserver !== "function") {
      nodes.forEach(show);
      return;
    }

    const io = new IntersectionObserver(
      (entries) => {
        entries.forEach((e) => {
          if (e.isIntersecting) {
            show(e.target);
            io.unobserve(e.target);
          }
        });
      },
      { root, rootMargin: "0px 0px -6% 0px", threshold: 0.01 },
    );

    nodes.forEach((el) => {
      if (inView(el)) {
        requestAnimationFrame(() => show(el));
        return;
      }
      io.observe(el);
    });

    let sweepT = 0;
    const sweep = () => {
      root.querySelectorAll<HTMLElement>("[data-reveal]").forEach((el) => {
        if (!el.classList.contains(styles.revealShown) && inView(el)) show(el);
      });
    };
    const onScroll = () => {
      clearTimeout(sweepT);
      sweepT = window.setTimeout(sweep, 60);
    };
    root.addEventListener("scroll", onScroll, { passive: true });

    const rvT = window.setTimeout(sweep, 700);
    // Last resort so no section is ever stranded (capture pipelines, no IO callback).
    const lastT = window.setTimeout(
      () => root.querySelectorAll("[data-reveal]").forEach(show),
      2400,
    );

    return () => {
      io.disconnect();
      root.removeEventListener("scroll", onScroll);
      clearTimeout(sweepT);
      clearTimeout(rvT);
      clearTimeout(lastT);
    };
  }, [rootRef]);
}

/**
 * Cycles the headline's knockout through translations, accelerating toward the
 * English settle. The box is locked to its initial size in em so the surrounding
 * headline geometry never shifts as scripts change width.
 */
function useLangCycle(elRef: React.RefObject<HTMLSpanElement | null>) {
  useEffect(() => {
    const el = elRef.current;
    if (!el || prefersReducedMotion()) return;

    const scales = new WeakMap<HTMLElement, number>();
    const timers: number[] = [];
    let current: HTMLElement | null = null;
    let cancelled = false;

    const makeInner = (txt: string) => {
      const inner = document.createElement("span");
      inner.style.display = "inline-block";
      inner.style.whiteSpace = "nowrap";
      inner.style.lineHeight = "1";
      inner.style.willChange = "transform";
      inner.style.position = "absolute";
      inner.style.left = "10px";
      inner.textContent = txt;
      el.appendChild(inner);
      const avail = el.clientWidth;
      const need = inner.scrollWidth;
      if (avail > 0 && need > avail) {
        inner.style.transformOrigin = "left center";
        scales.set(inner, avail / need);
      } else {
        scales.set(inner, 1);
      }
      return inner;
    };

    const at = (inner: HTMLElement, y: string) => {
      inner.style.transform = `translateY(${y}) scale(${(scales.get(inner) ?? 1).toFixed(4)})`;
    };

    const paint = (txt: string, dur: number) => {
      const ease = `transform ${dur}ms cubic-bezier(0.22,0.85,0.24,1)`;
      const prev = current;
      const next = makeInner(txt);
      next.style.transition = "none";
      at(next, "115%");
      void next.offsetWidth;
      next.style.transition = ease;
      at(next, "0%");
      if (prev) {
        prev.style.transition = ease;
        at(prev, "-115%");
        timers.push(window.setTimeout(() => prev.remove(), dur + 60));
      }
      current = next;
      return dur;
    };

    // Measure only once the display face is live — the fallback metrics would
    // lock the knockout box to the wrong size for the rest of the cycle.
    const start = () => {
      if (cancelled) return;

      // Reset to pristine geometry first: this effect mutates the node, and a
      // re-run (StrictMode, remount) would otherwise measure its own output.
      el.removeAttribute("style");
      el.textContent = SEED_WORD;

      const basePx = parseFloat(getComputedStyle(el).fontSize) || 16;
      const r0 = el.getBoundingClientRect();
      el.style.width = `${(r0.width / basePx).toFixed(4)}em`;
      // Slack so descenders never clip.
      el.style.height = `${(r0.height / basePx + 0.1).toFixed(4)}em`;
      el.style.display = "inline-flex";
      el.style.alignItems = "center";
      el.style.justifyContent = "flex-start";
      el.style.position = "relative";
      el.style.overflow = "hidden";

      el.textContent = "";
      current = makeInner(SEED_WORD);
      at(current, "0%");

      const n = LANG_WORDS.length;
      let i = 0;
      const step = () => {
        const last = i === n - 1;
        const p = i / (n - 1);
        const dur = last ? 760 : 300 + Math.round(300 * p ** 2);
        const span = paint(LANG_WORDS[i], dur);
        if (last) {
          timers.push(
            window.setTimeout(() => {
              if (current) current.style.willChange = "auto";
            }, span + 240),
          );
          return;
        }
        i += 1;
        timers.push(
          window.setTimeout(
            step,
            span * 0.7 + 110 + Math.round(460 * p ** 2.4),
          ),
        );
      };
      timers.push(window.setTimeout(step, 560));
    };

    if (document.fonts?.ready) {
      document.fonts.ready.then(start);
    } else {
      start();
    }

    return () => {
      cancelled = true;
      timers.forEach(clearTimeout);
      el.removeAttribute("style");
      el.textContent = SEED_WORD;
    };
  }, [elRef]);
}

type AquaRefs = {
  wrap: React.RefObject<HTMLElement | null>;
  curve: React.RefObject<SVGPathElement | null>;
  dot: React.RefObject<SVGCircleElement | null>;
  fills: React.MutableRefObject<(SVGPathElement | null)[]>;
  legs: React.MutableRefObject<(HTMLSpanElement | null)[]>;
  legPcts: React.MutableRefObject<(HTMLSpanElement | null)[]>;
  weth: React.RefObject<HTMLSpanElement | null>;
  usdc: React.RefObject<HTMLSpanElement | null>;
  earned: React.RefObject<HTMLSpanElement | null>;
  copies: React.MutableRefObject<(HTMLElement | null)[]>;
};

/**
 * Drives the sticky Aqua stage from the scroll position of the copy column.
 * Scene selection is React state; the numeric tween runs on a rAF loop writing
 * straight to the DOM, since it repaints every frame.
 */
function useAquaScene(
  rootRef: React.RefObject<HTMLDivElement | null>,
  refs: AquaRefs,
) {
  const [scene, setScene] = useState(prefersReducedMotion() ? 5 : 0);
  const cur = useRef<Frame>({ ...SCENE_TARGETS[0] });
  const target = useRef<Frame>(SCENE_TARGETS[prefersReducedMotion() ? 5 : 0]);
  const lens = useRef<number[] | null>(null);
  const curveLen = useRef(0);

  useEffect(() => {
    target.current =
      SCENE_TARGETS[Math.max(0, Math.min(SCENE_TARGETS.length - 1, scene))];
  }, [scene]);

  // Publish the scroll root's height so sticky stage and copy panes share it.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const sync = () =>
      root.style.setProperty("--av-vh", `${root.clientHeight}px`);
    sync();
    window.addEventListener("resize", sync);
    const ro =
      typeof ResizeObserver === "function" ? new ResizeObserver(sync) : null;
    ro?.observe(root);
    return () => {
      window.removeEventListener("resize", sync);
      ro?.disconnect();
    };
  }, [rootRef]);

  useEffect(() => {
    const root = rootRef.current;
    const wrap = refs.wrap.current;
    if (!root || !wrap || prefersReducedMotion()) return;

    const onScroll = () => {
      const n = SCENE_TARGETS.length;
      const raw = (root.scrollTop - wrap.offsetTop) / (root.clientHeight || 1);
      const i = Math.max(0, Math.min(n - 1, Math.round(raw + 0.34)));
      setScene((prev) => (prev === i ? prev : i));
    };

    root.addEventListener("scroll", onScroll, { passive: true });
    onScroll();

    const io =
      typeof IntersectionObserver === "function"
        ? new IntersectionObserver(() => onScroll(), {
            root,
            rootMargin: "-30% 0px -30% 0px",
            threshold: [0, 0.5, 1],
          })
        : null;
    refs.copies.current.forEach((el) => el && io?.observe(el));

    return () => {
      root.removeEventListener("scroll", onScroll);
      io?.disconnect();
    };
  }, [rootRef, refs.wrap, refs.copies]);

  // The tween loop. ~340ms ease toward the active scene's target frame.
  useEffect(() => {
    const reduced = prefersReducedMotion();
    let raf = 0;

    const tick = () => {
      raf = requestAnimationFrame(tick);
      const t = target.current;
      const c = cur.current;
      const k = reduced ? 1 : 0.16;

      (Object.keys(EPS) as (keyof Frame)[]).forEach((key) => {
        const d = t[key] - c[key];
        c[key] = Math.abs(d) > EPS[key] ? c[key] + d * k : t[key];
      });

      const curve = refs.curve.current;
      const dot = refs.dot.current;
      const fills = refs.fills.current;

      if (curve) {
        if (!lens.current || lens.current.length !== fills.length) {
          curveLen.current = curve.getTotalLength();
          lens.current = fills.map((p) => {
            if (!p) return 0;
            const l = p.getTotalLength();
            p.setAttribute("stroke-dasharray", String(l));
            return l;
          });
        }
        const pt = curve.getPointAtLength(curveLen.current * c.frac);
        dot?.setAttribute("cx", String(pt.x));
        dot?.setAttribute("cy", String(pt.y));

        fills.forEach((el, i) => {
          const kk = c.fill * LEG_DEPTHS[i];
          el?.setAttribute(
            "stroke-dashoffset",
            String((lens.current?.[i] ?? 0) * (1 - kk)),
          );
          const leg = refs.legs.current[i];
          const pct = refs.legPcts.current[i];
          if (leg) leg.style.width = `${Math.round(kk * 100)}%`;
          if (pct) pct.textContent = `${Math.round(kk * 100)}%`;
        });
      }

      const fmt = (n: number) => Math.round(n).toLocaleString("en-US");
      if (refs.weth.current) {
        refs.weth.current.textContent =
          c.weth > 99.95 ? "100" : c.weth.toFixed(1);
      }
      if (refs.usdc.current) refs.usdc.current.textContent = fmt(c.usdc);
      if (refs.earned.current) refs.earned.current.textContent = fmt(c.earned);
    };

    tick();
    return () => cancelAnimationFrame(raf);
  }, [
    refs.curve,
    refs.dot,
    refs.fills,
    refs.legs,
    refs.legPcts,
    refs.weth,
    refs.usdc,
    refs.earned,
  ]);

  return scene;
}

export function HomePage() {
  const { navTo } = useApp();

  const rootRef = useRef<HTMLDivElement>(null);
  const langRef = useRef<HTMLSpanElement>(null);
  const wrapRef = useRef<HTMLElement>(null);
  const curveRef = useRef<SVGPathElement>(null);
  const dotRef = useRef<SVGCircleElement>(null);
  const wethRef = useRef<HTMLSpanElement>(null);
  const usdcRef = useRef<HTMLSpanElement>(null);
  const earnedRef = useRef<HTMLSpanElement>(null);
  const fillRefs = useRef<(SVGPathElement | null)[]>([]);
  const legRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const legPctRefs = useRef<(HTMLSpanElement | null)[]>([]);
  const copyRefs = useRef<(HTMLElement | null)[]>([]);
  const sceneRefs = useRef<(HTMLDivElement | null)[]>([]);

  useReveal(rootRef);
  useLangCycle(langRef);
  const scene = useAquaScene(rootRef, {
    wrap: wrapRef,
    curve: curveRef,
    dot: dotRef,
    fills: fillRefs,
    legs: legRefs,
    legPcts: legPctRefs,
    weth: wethRef,
    usdc: usdcRef,
    earned: earnedRef,
    copies: copyRefs,
  });

  // Shrink an overflowing scene to fit the stage rather than clipping it.
  useEffect(() => {
    const el = sceneRefs.current[scene];
    const inner = el?.firstElementChild as HTMLElement | null;
    if (!el || !inner) return;
    inner.style.transformOrigin = "left center";
    inner.style.transform = "none";
    const avail = el.clientHeight;
    const h = inner.scrollHeight;
    if (avail > 20 && h > avail) {
      inner.style.transform = `scale(${Math.max(0.66, avail / h).toFixed(3)})`;
    }
  }, [scene]);

  const setScene = useCallback(
    (i: number) => (el: HTMLDivElement | null) => {
      sceneRefs.current[i] = el;
    },
    [],
  );

  const sceneClass = (i: number) =>
    `${styles.scene} ${i === scene ? styles.sceneOn : ""}`;

  return (
    <div
      ref={rootRef}
      data-home-root="1"
      data-scroll="1"
      className={styles.root}
    >
      <section className={styles.hero}>
        <h1 data-reveal="1" className={`${styles.heroTitle} ${styles.reveal}`}>
          Every intent.
          <br />
          One{" "}
          <span ref={langRef} className={styles.lang}>
            liquidity layer.
          </span>
        </h1>

        <div className={styles.heroGrid}>
          <div
            data-reveal="1"
            className={`${styles.heroLeft} ${styles.reveal}`}
          >
            <div className={styles.eyebrowStack}>
              <div>Built on 1inch Aqua</div>
              <div className={styles.eyebrowDim}>
                Zero-inventory intent execution
              </div>
              <div className={styles.eyebrowDim}>
                Shared, self-custodial liquidity
              </div>
            </div>
            <button
              type="button"
              className={styles.provide}
              onClick={() => navTo("Pools")}
            >
              <span className={styles.provideLine}>Provide liquidity.</span>
              <span className={styles.provideLine}>
                <span className={`${styles.mark} ${styles.markThick}`}>
                  Earn from fills and rebates.
                </span>
              </span>
            </button>
          </div>

          <div
            data-reveal="1"
            className={`${styles.heroRight} ${styles.reveal}`}
          >
            <p className={styles.heroLede}>
              Ready to put your{" "}
              <span className={styles.mark}>liquidity to work</span>? Solvent
              fills{" "}
              <span className={styles.mark}>UniswapX, ERC-7683 and more</span>{" "}
              from shared Aqua depth. Resolvers execute{" "}
              <span className={styles.knockout}>without ever holding</span>{" "}
              inventory.
            </p>
            <div className={styles.heroLinks}>
              <button
                type="button"
                className={styles.heroLink}
                onClick={() => navTo("Swap")}
              >
                Swap ↗
              </button>
              <button
                type="button"
                className={styles.heroLink}
                onClick={() => navTo("Makers")}
              >
                Strategies ↗
              </button>
              <button
                type="button"
                className={styles.heroLink}
                onClick={() => navTo("Explorer")}
              >
                Explorer ↗
              </button>
            </div>
          </div>
        </div>
      </section>

      <section ref={wrapRef} data-av-wrap="1" className={styles.av}>
        <div className={styles.avGrid}>
          <div className={styles.stage}>
            <div className={styles.ledger}>
              <div className={styles.ledgerCell}>
                <div className={styles.ledgerLabel}>Maker&apos;s pool</div>
                <div className={styles.ledgerValue}>
                  <span ref={wethRef}>100</span> WETH ·{" "}
                  <span ref={usdcRef}>290,000</span> USDC
                </div>
              </div>
              <div className={styles.ledgerCell}>
                <div className={styles.ledgerLabel}>Maker earned</div>
                <div className={styles.ledgerValueLime}>
                  <span ref={earnedRef}>0</span> USDC
                </div>
              </div>
            </div>

            <div className={styles.scenes}>
              <div ref={setScene(0)} className={sceneClass(0)}>
                <div className={styles.sceneInner}>
                  <div className={styles.row3}>
                    <div className={styles.card}>
                      <div className={styles.cardLabel}>
                        Maker&apos;s wallet
                      </div>
                      <div className={styles.cardValue}>100 WETH</div>
                      <div
                        className={styles.cardValue}
                        style={{ marginTop: 4 }}
                      >
                        290,000 USDC
                      </div>
                      <div className={styles.cardFoot}>Never leaves</div>
                    </div>
                    <div className={styles.connector}>
                      <div className={styles.connectorLabel}>Allowed</div>
                      <div className={styles.dashRule} />
                      <div className={styles.connectorSub}>Not deposited</div>
                    </div>
                    <div className={styles.card}>
                      <div className={styles.cardLabel}>Solvent</div>
                      <div className={styles.cardValue}>May use them</div>
                      <div className={styles.cardFoot}>
                        Only to fill an order
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <div ref={setScene(1)} className={sceneClass(1)}>
                <div className={styles.sceneInner}>
                  <div className={styles.row3Center}>
                    <div className={styles.cardLime}>
                      <div className={styles.cardLabelLime}>Someone signs</div>
                      <div className={styles.cardValue}>I want 1 WETH</div>
                      <div className={styles.cardNote}>Signed, not sent</div>
                    </div>
                    <div className={styles.connectorNarrow}>
                      <div className={styles.solidRule} />
                      <div className={styles.ruleSub}>Picked up</div>
                    </div>
                    <div className={styles.card}>
                      <div className={styles.cardLabel}>Solvent</div>
                      <div className={styles.cardValue}>Sees the order</div>
                      <div className={styles.cardNote}>Maker stays offline</div>
                    </div>
                  </div>
                </div>
              </div>

              <div ref={setScene(2)} className={sceneClass(2)}>
                <div className={styles.sceneInner}>
                  <div className={styles.legs}>
                    <div className={styles.legList}>
                      {LEGS.map((g) => (
                        <div key={g.i} className={styles.leg}>
                          <span className={styles.legName}>{g.name}</span>
                          <span className={styles.legTrack}>
                            <span
                              ref={(el) => {
                                legRefs.current[g.i] = el;
                              }}
                              className={styles.legFill}
                            />
                          </span>
                          <span
                            ref={(el) => {
                              legPctRefs.current[g.i] = el;
                            }}
                            className={styles.legPct}
                          >
                            0%
                          </span>
                        </div>
                      ))}
                    </div>
                    <div className={styles.heldRow}>
                      <span className={styles.heldLabel}>Solvent held</span>
                      <span className={styles.heldChip}>0</span>
                      <span className={styles.heldLabel}>One transaction</span>
                    </div>
                  </div>
                </div>
              </div>

              <div ref={setScene(3)} className={sceneClass(3)}>
                <div className={styles.sceneInner}>
                  <div className={styles.grid3}>
                    <div className={styles.card}>
                      <div className={styles.cardLabel}>WETH</div>
                      <div className={styles.cardValue}>99</div>
                      <div className={styles.cardNote}>One sold</div>
                    </div>
                    <div className={styles.card}>
                      <div className={styles.cardLabel}>USDC</div>
                      <div className={styles.cardValue}>292,930</div>
                      <div className={styles.cardNote}>Paid in</div>
                    </div>
                    <div className={styles.cardLime}>
                      <div className={styles.cardLabelLime}>Their price</div>
                      <div className={styles.cardValue}>Nudged up</div>
                      <div className={styles.cardNote}>Above market</div>
                    </div>
                  </div>
                </div>
              </div>

              <div ref={setScene(4)} className={sceneClass(4)}>
                <div className={styles.sceneInner}>
                  <div className={styles.row2}>
                    <div className={styles.cardLime}>
                      <div className={styles.cardLabelLime}>Traders</div>
                      <div className={styles.cardValue}>Buy it back</div>
                      <div className={styles.cardNote}>
                        Paying the maker&apos;s fee
                      </div>
                    </div>
                    <div className={styles.card}>
                      <div className={styles.cardLabel}>Maker ends with</div>
                      <div className={styles.cardValue}>Same tokens</div>
                      <div
                        className={styles.cardValueLime}
                        style={{ marginTop: 4 }}
                      >
                        Paid twice
                      </div>
                    </div>
                  </div>
                </div>
              </div>

              <div
                ref={setScene(5)}
                className={sceneClass(5)}
                style={{ overflow: "hidden" }}
              >
                <div className={styles.sceneInner}>
                  <div className={styles.grid3Tight}>
                    <div className={styles.card}>
                      <div
                        className={styles.cardLabel}
                        style={{ marginBottom: 5 }}
                      >
                        Maker&apos;s pool
                      </div>
                      <div className={styles.cardValue}>100 WETH</div>
                      <div
                        className={styles.cardValue}
                        style={{ marginTop: 3 }}
                      >
                        290,059 USDC
                      </div>
                    </div>
                    <div className={styles.cardLime}>
                      <div
                        className={styles.cardLabelLime}
                        style={{ marginBottom: 5 }}
                      >
                        Maker earned
                      </div>
                      <div className={styles.bigNum}>89</div>
                      <div className={styles.bigNumUnit}>USDC</div>
                    </div>
                    <div className={styles.card}>
                      <div
                        className={styles.cardLabel}
                        style={{ marginBottom: 5 }}
                      >
                        Solvent held
                      </div>
                      <div className={styles.bigNumWhite}>0</div>
                      <div className={styles.bigNumUnit}>
                        Before, during, after
                      </div>
                    </div>
                  </div>
                  <div className={styles.sceneFoot}>
                    Dot resting at the start of the curve — home again
                  </div>
                </div>
              </div>
            </div>

            <div className={styles.chartWrap}>
              <svg
                viewBox="-8 34 1016 372"
                preserveAspectRatio="none"
                className={styles.chart}
              >
                <g
                  stroke="#26281f"
                  strokeWidth="1"
                  vectorEffect="non-scaling-stroke"
                >
                  <line x1="0" y1="110" x2="1000" y2="110" />
                  <line x1="0" y1="240" x2="1000" y2="240" />
                  <line x1="0" y1="370" x2="1000" y2="370" />
                </g>
                <g opacity={scene >= 2 ? 1 : 0}>
                  <path
                    d="M 20 390 C 300 382 560 340 980 200"
                    fill="none"
                    stroke="var(--on-lime-deep)"
                    strokeWidth="2"
                    vectorEffect="non-scaling-stroke"
                  />
                  <path
                    d="M 20 170 C 340 166 660 162 980 158"
                    fill="none"
                    stroke="var(--on-lime-deep)"
                    strokeWidth="2"
                    vectorEffect="non-scaling-stroke"
                  />
                  <path
                    d="M 20 320 C 400 316 450 180 500 60"
                    fill="none"
                    stroke="var(--on-lime-deep)"
                    strokeWidth="2"
                    vectorEffect="non-scaling-stroke"
                  />
                  {[
                    "M 20 390 C 300 382 560 340 980 200",
                    "M 20 170 C 340 166 660 162 980 158",
                    "M 20 320 C 400 316 450 180 500 60",
                  ].map((d, i) => (
                    <path
                      key={d}
                      ref={(el) => {
                        fillRefs.current[i] = el;
                      }}
                      d={d}
                      fill="none"
                      stroke="var(--lime-hero)"
                      strokeWidth="2.6"
                      strokeLinecap="round"
                      vectorEffect="non-scaling-stroke"
                    />
                  ))}
                </g>
                <path
                  ref={curveRef}
                  d="M 20 396 C 320 388 520 316 980 44"
                  fill="none"
                  stroke="var(--paper)"
                  strokeWidth="1.6"
                  vectorEffect="non-scaling-stroke"
                />
                <circle
                  ref={dotRef}
                  cx="0"
                  cy="0"
                  r="8"
                  fill="var(--lime-hero)"
                  stroke="var(--ink)"
                  strokeWidth="2"
                  vectorEffect="non-scaling-stroke"
                />
              </svg>
              <div className={styles.chartCaption}>
                Maker&apos;s price curve
              </div>
            </div>
          </div>

          <div className={styles.copyCol}>
            <article
              ref={(el) => {
                copyRefs.current[0] = el;
              }}
              className={styles.copy}
            >
              <div
                className={`${styles.eyebrow} ${scene === 0 ? styles.eyebrowActive : ""}`}
              >
                Scene 01
              </div>
              <h3 className={styles.copyTitle}>
                Makers keep
                <br />
                <span className={styles.copyMark}>their tokens.</span>
              </h3>
              <p className={styles.copyBody}>
                They never deposit. Their coins stay in their own wallet — they
                just allow Solvent to use them when an order needs filling.
              </p>
            </article>

            <article
              ref={(el) => {
                copyRefs.current[1] = el;
              }}
              className={styles.copy}
            >
              <div
                className={`${styles.eyebrow} ${scene === 1 ? styles.eyebrowActive : ""}`}
              >
                Scene 02
              </div>
              <h3 className={styles.copyTitle}>
                An order
                <br />
                <span className={styles.copyMark}>shows up.</span>
              </h3>
              <p className={styles.copyBody}>
                Someone signs a swap: “I want 1 WETH.” Solvent picks it up. The
                maker never has to be online.
              </p>
            </article>

            <article
              ref={(el) => {
                copyRefs.current[2] = el;
              }}
              className={styles.copy}
            >
              <div
                className={`${styles.eyebrow} ${scene === 2 ? styles.eyebrowActive : ""}`}
              >
                Scene 03
              </div>
              <h3 className={styles.copyTitle}>
                Best price,
                <br />
                <span className={styles.copyMark}>zero inventory.</span>
              </h3>
              <p className={styles.copyBody}>
                Solvent splits the order across many makers for the best price
                and fills it in one transaction — holding nothing itself.
              </p>
            </article>

            <article
              ref={(el) => {
                copyRefs.current[3] = el;
              }}
              className={styles.copy}
            >
              <div
                className={`${styles.eyebrow} ${scene === 3 ? styles.eyebrowActive : ""}`}
              >
                Scene 04
              </div>
              <h3 className={styles.copyTitle}>
                The <span className={styles.copyMark}>price moves.</span>
              </h3>
              <p className={styles.copyBody}>
                The swap nudges the maker&apos;s price up: a little more USDC, a
                little less WETH.
              </p>
            </article>

            <article
              ref={(el) => {
                copyRefs.current[4] = el;
              }}
              data-av-ground="lime"
              className={styles.copyLime}
            >
              <div
                className={`${styles.eyebrowLime} ${scene === 4 ? styles.eyebrowLimeActive : ""}`}
              >
                Scene 05
              </div>
              <h3 className={styles.copyTitleInk}>
                Arbitrage,
                <br />
                the <span className={styles.copyMarkInvert}>rebate.</span>
              </h3>
              <p className={styles.copyBodyInk}>
                Traders arbitrage the price back to normal, paying the
                maker&apos;s fee on the way. The maker ends where they started,
                paid twice. That&apos;s the rebate.
              </p>
            </article>

            <div
              ref={(el) => {
                copyRefs.current[5] = el;
              }}
              className={styles.payoff}
            >
              <div className={styles.payoffTop}>
                <div
                  className={`${styles.eyebrow} ${scene === 5 ? styles.eyebrowActive : ""}`}
                >
                  The payoff
                </div>
                <h3 className={styles.payoffTitle}>
                  Same balance.
                  <br />
                  <span className={styles.copyMark}>Twice the flow.</span>
                </h3>
                <p className={styles.payoffBody}>
                  The maker never deposited, never gave up custody, never chose
                  an order — and got paid twice.
                </p>
              </div>
              <div className={styles.payoffLinks}>
                <button
                  type="button"
                  className={styles.payoffLink}
                  onClick={() => navTo("Pools")}
                >
                  Provide liquidity ↗
                </button>
                <button
                  type="button"
                  className={styles.payoffLink}
                  onClick={() => navTo("Swap")}
                >
                  Swap ↗
                </button>
                <button
                  type="button"
                  className={styles.payoffLink}
                  onClick={() => navTo("Explorer")}
                >
                  Explorer ↗
                </button>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
