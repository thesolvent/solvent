/**
 * The one place a number becomes text.
 *
 * Every figure the app prints is grouped and punctuated the same way whatever the reader's browser
 * is set to: a financial figure that changes meaning with the reader's locale is a wrong figure.
 */
export const LOCALE = "en-US";

/** What an absent value looks like. Never `0`, never `$0.00` — those are values, not absences. */
export const DASH = "—";

type Maybe = number | null | undefined;

function absent(value: Maybe): value is null | undefined {
  return value == null || !Number.isFinite(value);
}

/**
 * A wallet address, shortened. Deliberately shorter at both ends than {@link truncateHash}: an
 * address is 42 characters and a transaction hash is 66, and the reader has to be able to tell
 * which kind of identifier a cell holds without expanding it.
 */
export function truncateAddress(value: string | null | undefined): string {
  return truncate(value, 6, 4);
}

/** A transaction, order or strategy hash, shortened. Wider than an address on both sides. */
export function truncateHash(value: string | null | undefined): string {
  return truncate(value, 8, 6);
}

function truncate(
  value: string | null | undefined,
  head: number,
  tail: number,
): string {
  if (!value) return DASH;
  // Shortening something already shorter than its own shortening only adds an ellipsis to a lie.
  if (value.length <= head + tail + 1) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

const USD_COMPACT = new Intl.NumberFormat(LOCALE, {
  style: "currency",
  currency: "USD",
  notation: "compact",
  maximumFractionDigits: 1,
});
const USD_FULL = new Intl.NumberFormat(LOCALE, {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

/** Below this a dollar figure reads as a quantity and wants its cents; above it, as a magnitude. */
const COMPACT_FROM = 10_000;

/**
 * A dollar figure.
 *
 * Compact from $10K up (`$12.3K`), exact below it (`$1,234.56`), unless the caller says otherwise —
 * a figure the user is about to trade against is always exact, however large. A non-zero amount
 * too small to show is marked as such rather than rounded down to `$0.00`, which would read as
 * free.
 */
export function usd(value: Maybe, options?: { compact?: boolean }): string {
  if (absent(value)) return DASH;
  const magnitude = Math.abs(value);
  const compact = options?.compact ?? magnitude >= COMPACT_FROM;
  if (compact) return USD_COMPACT.format(value);
  if (magnitude > 0 && magnitude < 0.005) {
    return `${value < 0 ? "-" : ""}<$0.01`;
  }
  return USD_FULL.format(value);
}

/** A plain count of things — blocks, fills, events. */
export function count(value: Maybe): string {
  return absent(value) ? DASH : value.toLocaleString(LOCALE);
}

/**
 * A percentage.
 *
 * `sign` picks the surface's convention: `"arrow"` for a move over a period, `"plus"` where the
 * direction is the point but the glyph would crowd the cell, `"none"` for a magnitude that cannot
 * be negative. There is one arrow pair in the product, and this is it.
 */
export function percent(
  value: Maybe,
  options?: { digits?: number; sign?: "none" | "plus" | "arrow" },
): string {
  if (absent(value)) return DASH;
  const digits = options?.digits ?? 2;
  const sign = options?.sign ?? "none";
  const shown = sign === "none" ? value : Math.abs(value);
  const body = `${shown.toLocaleString(LOCALE, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  })}%`;
  if (sign === "arrow") return `${value < 0 ? "↘" : "↗"} ${body}`;
  if (sign === "plus") return `${value < 0 ? "-" : "+"}${body}`;
  return body;
}

/**
 * A token amount.
 *
 * Amounts arrive as exact decimal strings straight from base units, and `Number()` silently rounds
 * away everything past the fifteenth significant digit, so the integer part is grouped as text
 * instead. The fraction is truncated rather than rounded: a balance must never read higher than
 * what is held. Small amounts get more places, because that is where their information is; an
 * amount too small for even those places is marked as such, because a truncated `0` is
 * indistinguishable from an empty wallet.
 */
export function tokenAmount(value: string | number | null | undefined): string {
  const decimal = decimalString(value);
  if (decimal === null) return DASH;

  const negative = decimal.startsWith("-");
  const [whole = "0", fraction = ""] = (
    negative ? decimal.slice(1) : decimal
  ).split(".");
  const digits = whole.replace(/^0+(?=\d)/, "") || "0";
  const places = digits.length > 3 ? 2 : digits === "0" ? 6 : 4;
  const kept = fraction.slice(0, places);

  if (digits === "0" && !/[1-9]/.test(kept) && /[1-9]/.test(fraction)) {
    return `${negative ? "-" : ""}<0.${"0".repeat(places - 1)}1`;
  }
  const grouped = digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return `${negative ? "-" : ""}${grouped}.${kept.padEnd(places, "0")}`;
}

/** A token amount with the thing it counts. */
export function tokenWithSymbol(
  value: string | number | null | undefined,
  symbol: string,
): string {
  const amount = tokenAmount(value);
  return amount === DASH ? DASH : `${amount} ${symbol}`;
}

/**
 * A plain positional decimal string, whatever came in. `String(1e-7)` is `"1e-7"`, and every digit
 * routine here reads position, so exponents have to be gone before anything else looks at it.
 */
function decimalString(
  value: string | number | null | undefined,
): string | null {
  if (value == null || value === "") return null;
  if (typeof value === "number") {
    if (!Number.isFinite(value)) return null;
    return value.toLocaleString(LOCALE, {
      useGrouping: false,
      maximumFractionDigits: 20,
    });
  }
  const trimmed = value.trim();
  if (trimmed === "" || !Number.isFinite(Number(trimmed))) return null;
  return /e/i.test(trimmed) ? decimalString(Number(trimmed)) : trimmed;
}

/**
 * Amount displays shrink as digits are added so long values never overflow their
 * cell — capped px, then container-relative, then viewport-relative.
 */
export function fit(str: string | number): string {
  const n = String(str).length;
  const cap = n <= 7 ? 76 : n <= 9 ? 66 : n <= 12 ? 54 : 44;
  return `min(${cap}px, ${(132 / Math.max(n, 1)).toFixed(1)}cqi, 8vh)`;
}
