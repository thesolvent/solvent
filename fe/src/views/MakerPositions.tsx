import { useState } from "react";
import type { makerView } from "@/lib/makers";
import styles from "./MakersPage.module.css";

type PositionRow = ReturnType<typeof makerView>["positions"][number];
const POS_ACTIONS = ["Push", "Dock"];

function groupPositions(positions: PositionRow[]) {
  const groups = new Map<string, PositionRow[]>();
  for (const position of positions) {
    const group = groups.get(position.pair);
    if (group) group.push(position);
    else groups.set(position.pair, [position]);
  }
  return [...groups]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([pair, rows]) => ({
      pair,
      positions: rows.sort((a, b) => a.hash.localeCompare(b.hash)),
    }));
}

export function MakerPositions({ positions }: { positions: PositionRow[] }) {
  const groups = groupPositions(positions);
  const [selected, setSelected] = useState<{
    pair: string | null;
    position: string | null;
  }>();
  const first = groups[0];
  if (selected === undefined && first) {
    setSelected({ pair: first.pair, position: first.positions[0].hash });
  }
  return (
    <div data-scroll="1" className={styles.list}>
      {groups.map((group) => {
        const open = group.pair === selected?.pair;
        return (
          <div key={group.pair} className={styles.posGroup}>
            <button
              type="button"
              className={open ? styles.pairRowOpen : styles.pairRow}
              aria-expanded={open}
              onClick={() =>
                setSelected({
                  pair: open ? null : group.pair,
                  position: open ? null : group.positions[0].hash,
                })
              }
            >
              <span
                aria-hidden="true"
                className={open ? styles.caretOpen : styles.caret}
              >
                ▸
              </span>
              <span className={styles.posPair}>{group.pair}</span>
            </button>
            {open && (
              <PairPositions
                positions={group.positions}
                selected={selected?.position ?? null}
                onToggle={(position) =>
                  setSelected({ pair: group.pair, position })
                }
              />
            )}
          </div>
        );
      })}
    </div>
  );
}

function PairPositions({
  positions,
  selected,
  onToggle,
}: {
  positions: PositionRow[];
  selected: string | null;
  onToggle: (hash: string | null) => void;
}) {
  return (
    <div className={styles.pairPositions}>
      {positions.map((p) => {
        const open = selected === p.hash;
        const toggle = () => onToggle(open ? null : p.hash);
        return (
          <div key={p.hash} className={styles.posGroup}>
            <div className={open ? styles.posRowOpen : styles.posRow}>
              <button
                type="button"
                className={styles.posToggle}
                aria-label={`Position ${p.hash}`}
                aria-expanded={open}
                onClick={toggle}
              >
                <span className={open ? styles.caretOpen : styles.caret}>
                  ▸
                </span>
                <span className={styles.posPair}>{p.pair}</span>
                <span className={styles.posMeta}>{p.meta}</span>
                <span className={styles.posCov}>{p.cov}</span>
                <span
                  className={styles.posWidth}
                  style={{
                    background: p.widthBg,
                    color: p.widthFg,
                  }}
                >
                  {p.width}
                </span>
              </button>
              <span className={styles.posActions}>
                {POS_ACTIONS.map((label) => (
                  <button
                    key={label}
                    type="button"
                    className={styles.posAction}
                  >
                    {label}
                  </button>
                ))}
              </span>
            </div>

            {open && (
              <div
                className={styles.posDetail}
                role="region"
                aria-label={`Position ${p.hash} details`}
              >
                <div className={styles.posStats}>
                  {p.stats.map((st) => (
                    <div
                      key={st.label}
                      className={styles.posStat}
                      style={{
                        backgroundImage: `linear-gradient(${st.sep}, ${st.sep})`,
                      }}
                    >
                      <div className={styles.posStatLabel}>{st.label}</div>
                      <div className={styles.posStatValue}>{st.value}</div>
                    </div>
                  ))}
                </div>
                <div className={styles.coverage}>
                  <span className={styles.coverageLabel}>
                    Coverage {p.covNum}
                  </span>
                  <span className={styles.coverageBar}>
                    <span
                      className={styles.coverageA}
                      style={{
                        width: p.splitA,
                      }}
                    >
                      {p.labelA}
                    </span>
                    <span className={styles.coverageB}>{p.labelB}</span>
                  </span>
                  <button type="button" className={styles.clone}>
                    Clone
                  </button>
                </div>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}
