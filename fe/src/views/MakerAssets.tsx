import type { makerView } from "@/lib/makers";

import styles from "./MakersPage.module.css";

type AssetRow = ReturnType<typeof makerView>["assets"][number];

export function MakerAssets({
  assets,
  onToggle,
  onOpenPosition,
}: {
  assets: AssetRow[];
  onToggle: (index: number) => void;
  onOpenPosition: (hash: string) => void;
}) {
  return (
    <>
      <div className={styles.assetHead}>
        <span />
        <span>Token</span>
        <span className={styles.right}>Wallet</span>
        <span className={styles.right}>Shared liq.</span>
        <span className={styles.right}>Fees · APY</span>
        <span className={styles.right}>Ratio</span>
      </div>
      <div data-scroll="1" className={styles.list}>
        {assets.map((asset, index) => (
          <div key={asset.address} className={styles.assetGroup}>
            <button
              type="button"
              className={asset.open ? styles.assetRowOpen : styles.assetRow}
              aria-label={`${asset.open ? "Collapse" : "Expand"} ${asset.sym} positions`}
              aria-expanded={asset.open}
              onClick={() => onToggle(index)}
            >
              <span className={styles.assetGlyph}>
                <span
                  aria-hidden="true"
                  className={
                    asset.open ? styles.assetCaretOpen : styles.assetCaret
                  }
                >
                  ▸
                </span>
                <span
                  className={styles.assetChip}
                  style={{ background: asset.tint }}
                >
                  {asset.sym}
                </span>
              </span>
              <span className={styles.stack}>
                <span className={styles.cellStrong}>{asset.sym}</span>
                <span className={styles.cellSub}>{asset.across}</span>
              </span>
              <span className={styles.stackRight}>
                <span className={styles.cellStrong}>{asset.wallet}</span>
                <span className={styles.cellSub}>{asset.walletAmt}</span>
              </span>
              <span className={styles.stackRight}>
                <span className={styles.cellStrong}>{asset.shared}</span>
                <span className={styles.cellSub}>{asset.sharedAmt}</span>
              </span>
              <span className={styles.stackRight}>
                <span className={styles.cellNum}>{asset.fees}</span>
                <span className={styles.cellSub}>{asset.apy}</span>
              </span>
              <span className={styles.cellRatio}>{asset.ratio}</span>
            </button>

            {asset.open && (
              <div className={styles.legPanel}>
                <div className={styles.legHead}>
                  <span>Position</span>
                  <span className={styles.right}>Current</span>
                  <span className={styles.right}>Opening</span>
                  <span className={styles.right}>Fees · APY</span>
                  <span className={styles.right}>Cov.</span>
                </div>
                {asset.legs.map((position) => (
                  <button
                    type="button"
                    key={position.hash}
                    className={styles.legRow}
                    aria-label={`Open ${position.pair} position ${position.hash}`}
                    onClick={() => onOpenPosition(position.hash)}
                  >
                    <span className={styles.stack}>
                      <span className={styles.legPair}>{position.pair}</span>
                      <span className={styles.cellSub}>{position.meta}</span>
                    </span>
                    <span className={styles.stackRight}>
                      <span className={styles.legCell}>{position.cur}</span>
                      <span className={styles.cellSub}>{position.curUsd}</span>
                    </span>
                    <span className={styles.legCell}>{position.op}</span>
                    <span className={styles.stackRight}>
                      <span className={styles.legCell}>{position.fees}</span>
                      <span className={styles.cellSub}>{position.apy}</span>
                    </span>
                    <span className={styles.legCov}>{position.cov}</span>
                  </button>
                ))}
              </div>
            )}
          </div>
        ))}
      </div>
    </>
  );
}
