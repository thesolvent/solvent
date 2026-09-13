import { useState } from "react";

import styles from "./AssetIdentity.module.css";

type IdentifiedAsset = {
  symbol: string;
  net?: string;
  logoUri?: string | null;
  chainLogoUri?: string | null;
};

function IdentityIcon({
  className,
  fallback,
  logoUri,
}: {
  className: string;
  fallback: string;
  logoUri?: string | null;
}) {
  const [loaded, setLoaded] = useState(Boolean(logoUri));
  return (
    <span className={className} aria-hidden="true">
      {loaded && logoUri ? (
        <img src={logoUri} alt="" onError={() => setLoaded(false)} />
      ) : (
        fallback
      )}
    </span>
  );
}

export function AssetIdentity({
  asset,
  showChain = true,
}: {
  asset: IdentifiedAsset | undefined;
  showChain?: boolean;
}) {
  if (!asset) return null;
  return (
    <span
      className={styles.root}
      data-asset-identity=""
      aria-label={`${asset.symbol} token on ${asset.net ?? "network"}`}
    >
      <IdentityIcon
        className={styles.token}
        fallback={asset.symbol.slice(0, 2)}
        logoUri={asset.logoUri}
      />
      {showChain && (
        <IdentityIcon
          className={styles.chain}
          fallback={(asset.net ?? "N").slice(0, 1)}
          logoUri={asset.chainLogoUri}
        />
      )}
    </span>
  );
}
