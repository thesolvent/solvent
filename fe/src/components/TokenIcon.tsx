import { useState } from "react";

import { pairSymbols } from "@/lib/pair";
import { useTokenIcon } from "@/services/assets";

import styles from "./TokenIcon.module.css";

/** Initials stand in until an icon loads, and permanently for a token the list gave none. */
function initials(symbol: string): string {
  return symbol.slice(0, 2).toUpperCase();
}

type Props = {
  symbol: string;
  logoUri?: string | null;
  /** Matches the box the caller already reserved; the fallback initials scale with it. */
  size?: number;
  className?: string;
};

/**
 * One token's icon, over its initials.
 *
 * The initials are always rendered and the image is layered on top, so a request that is slow,
 * blocked, or never answered degrades to initials on its own. Keying the fallback on `onError`
 * instead leaves an empty circle whenever a request hangs rather than fails — which is what a
 * blocked CDN actually does.
 *
 * Both are decorative: every caller renders the symbol beside them, so announcing the icon too
 * would read the token twice.
 */
export function TokenIcon({ symbol, logoUri, size, className }: Props) {
  const [broken, setBroken] = useState(false);
  const box = size == null ? undefined : { width: size, height: size };
  // A caller's class owns the box when it has one; both are single-class selectors, so leaving
  // our own size in would decide the tie on stylesheet order rather than intent.
  const sizing = className ?? (size == null ? styles.defaultSize : "");

  return (
    <span
      aria-hidden="true"
      className={`${styles.icon} ${sizing}`.trim()}
      style={box}
    >
      {initials(symbol)}
      {logoUri && !broken ? (
        <img
          alt=""
          className={styles.image}
          loading="lazy"
          onError={() => setBroken(true)}
          src={logoUri}
        />
      ) : null}
    </span>
  );
}

type PairProps = {
  base: { symbol: string; logoUri?: string | null };
  quote: { symbol: string; logoUri?: string | null };
  size?: number;
};

/** The two sides of a pair, overlapped, base in front. */
export function PairIcons({ base, quote, size }: PairProps) {
  return (
    <span className={styles.pair}>
      <TokenIcon logoUri={base.logoUri} size={size} symbol={base.symbol} />
      <TokenIcon
        className={styles.trailing}
        logoUri={quote.logoUri}
        size={size}
        symbol={quote.symbol}
      />
    </span>
  );
}

/** Both sides of a pair named only by its label, e.g. `"DAI/USDC"`. */
export function PairMark({ pair, size }: { pair: string; size?: number }) {
  const iconOf = useTokenIcon();
  const { base, quote } = pairSymbols(pair);
  return (
    <PairIcons
      base={{ symbol: base, logoUri: iconOf(base) }}
      quote={{ symbol: quote, logoUri: iconOf(quote) }}
      size={size}
    />
  );
}
