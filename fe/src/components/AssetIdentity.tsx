import styles from "./AssetIdentity.module.css";

type IdentifiedAsset = {
  symbol: string;
  net: string;
  logoUri?: string | null;
};

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
      <span className={styles.token} aria-hidden="true">
        {asset.logoUri ? (
          <img src={asset.logoUri} alt="" />
        ) : (
          asset.symbol.slice(0, 2)
        )}
      </span>
      <span className={styles.chain} aria-hidden="true">
        {asset.net.slice(0, 1)}
      </span>
    </span>
  );
}
