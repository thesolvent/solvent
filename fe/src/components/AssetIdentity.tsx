import styles from "./AssetIdentity.module.css";

type IdentifiedAsset = {
  symbol: string;
  net: string;
  logoUri?: string | null;
  /** The chain's own mark, when the deployment names one; falls back to its initial. */
  chainLogoUri?: string | null;
};

/**
 * A token's icon over its initials, with the chain it lives on beside it.
 *
 * The initials and the chain initial are always rendered and the images layered on top, so a
 * request that is slow, blocked, or never answered degrades to them on its own. Keying the
 * fallback on `onError` leaves an empty circle whenever a request hangs rather than fails, which
 * is what a blocked host does.
 */
export function AssetIdentity({
  asset,
}: {
  asset: IdentifiedAsset | undefined;
}) {
  if (!asset) return null;
  return (
    <span
      className={styles.root}
      data-asset-identity=""
      aria-label={`${asset.symbol} token on ${asset.net}`}
    >
      <TokenMark logoUri={asset.logoUri} symbol={asset.symbol} />
      <span className={`${styles.chain} ${styles.trailing}`} aria-hidden="true">
        {asset.net.slice(0, 1)}
        {asset.chainLogoUri && <img alt="" src={asset.chainLogoUri} />}
      </span>
    </span>
  );
}

/** One token's mark, without the chain badge. */
function TokenMark({
  symbol,
  logoUri,
  className,
}: {
  symbol: string;
  logoUri?: string | null;
  className?: string;
}) {
  return (
    <span
      aria-hidden="true"
      className={className ? `${styles.token} ${className}` : styles.token}
    >
      {symbol.slice(0, 2).toUpperCase()}
      {logoUri && <img alt="" loading="lazy" src={logoUri} />}
    </span>
  );
}
