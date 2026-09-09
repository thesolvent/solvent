import { useState, type FormEvent } from "react";

import type { makerView } from "@/lib/makers";
import type { DockPositionInput, PushPositionInput } from "@/ports/positions";
import type { PositionActionStatus } from "@/services/positions";

import styles from "./MakersPage.module.css";

type MakerPositionRow = ReturnType<typeof makerView>["positions"][number];
type PositionRow = Omit<MakerPositionRow, "maker" | "tokens"> &
  Partial<Pick<MakerPositionRow, "maker" | "tokens">>;
type PositionEditor =
  | { kind: "push"; strategyHash: string; tokenIndex: number; amount: string }
  | { kind: "dock"; strategyHash: string };

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

export function MakerPositions({
  positions,
  canManage = true,
  actionStatus,
  onClearAction,
  onOpenPosition,
  onClone,
  onPush,
  onDock,
}: {
  positions: PositionRow[];
  canManage?: boolean;
  actionStatus?: PositionActionStatus;
  onClearAction?: () => void;
  onOpenPosition: (hash: string) => void;
  onClone?: (position: PositionRow) => void;
  onPush?: (input: PushPositionInput) => Promise<unknown>;
  onDock?: (input: DockPositionInput) => Promise<unknown>;
}) {
  const groups = groupPositions(positions);
  const [selected, setSelected] = useState<{
    pair: string | null;
    position: string | null;
  }>();
  const [editor, setEditor] = useState<PositionEditor>();
  const first = groups[0];
  const currentSelection =
    selected ??
    (first
      ? { pair: first.pair, position: first.positions[0].hash }
      : { pair: null, position: null });

  const closeEditor = () => {
    setEditor(undefined);
    onClearAction?.();
  };
  const openEditor = (next: PositionEditor, pair: string) => {
    setSelected({ pair, position: next.strategyHash });
    setEditor(next);
    onClearAction?.();
  };

  return (
    <div data-scroll="1" className={styles.list}>
      {groups.map((group) => {
        const open = group.pair === currentSelection.pair;
        return (
          <div key={group.pair} className={styles.posGroup}>
            <button
              type="button"
              className={open ? styles.pairRowOpen : styles.pairRow}
              aria-expanded={open}
              onClick={() => {
                closeEditor();
                setSelected({
                  pair: open ? null : group.pair,
                  position: open ? null : group.positions[0].hash,
                });
              }}
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
                selected={currentSelection.position}
                editor={editor}
                canManage={canManage}
                actionStatus={actionStatus}
                onToggle={(position) => {
                  closeEditor();
                  setSelected({ pair: group.pair, position });
                }}
                onOpenEditor={(next) => openEditor(next, group.pair)}
                onCloseEditor={closeEditor}
                onOpenPosition={onOpenPosition}
                onClone={onClone}
                onPush={onPush}
                onDock={onDock}
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
  editor,
  canManage,
  actionStatus,
  onToggle,
  onOpenEditor,
  onCloseEditor,
  onOpenPosition,
  onClone,
  onPush,
  onDock,
}: {
  positions: PositionRow[];
  selected: string | null;
  editor: PositionEditor | undefined;
  canManage: boolean;
  actionStatus: PositionActionStatus | undefined;
  onToggle: (hash: string | null) => void;
  onOpenEditor: (editor: PositionEditor) => void;
  onCloseEditor: () => void;
  onOpenPosition: (hash: string) => void;
  onClone: ((position: PositionRow) => void) | undefined;
  onPush: ((input: PushPositionInput) => Promise<unknown>) | undefined;
  onDock: ((input: DockPositionInput) => Promise<unknown>) | undefined;
}) {
  return (
    <div className={styles.pairPositions}>
      {positions.map((position) => {
        const open = selected === position.hash;
        const activeEditor =
          editor?.strategyHash === position.hash ? editor : undefined;
        const busy = actionStatus?.submitting ?? false;
        return (
          <div key={position.hash} className={styles.posGroup}>
            <div className={open ? styles.posRowOpen : styles.posRow}>
              <button
                type="button"
                className={styles.posDisclosure}
                aria-label={`${open ? "Collapse" : "Expand"} ${position.pair} position ${position.hash}`}
                aria-expanded={open}
                onClick={() => onToggle(open ? null : position.hash)}
              >
                <span className={open ? styles.caretOpen : styles.caret}>
                  ▸
                </span>
              </button>
              <button
                type="button"
                className={styles.posToggle}
                aria-label={`Open ${position.pair} position ${position.hash}`}
                onClick={() => onOpenPosition(position.hash)}
              >
                <span className={styles.posPair}>{position.pair}</span>
                <span className={styles.posMeta}>{position.meta}</span>
                <span className={styles.posCov}>{position.cov}</span>
                <span
                  className={styles.posWidth}
                  style={{
                    background: position.widthBg,
                    color: position.widthFg,
                  }}
                >
                  {position.width}
                </span>
              </button>
              <span className={styles.posActions}>
                {canManage && (
                  <>
                    <button
                      type="button"
                      className={styles.posAction}
                      disabled={busy}
                      aria-pressed={activeEditor?.kind === "push"}
                      onClick={() =>
                        onOpenEditor({
                          kind: "push",
                          strategyHash: position.hash,
                          tokenIndex: 0,
                          amount: "",
                        })
                      }
                    >
                      Push
                    </button>
                    <button
                      type="button"
                      className={styles.posAction}
                      disabled={busy}
                      aria-pressed={activeEditor?.kind === "dock"}
                      onClick={() =>
                        onOpenEditor({
                          kind: "dock",
                          strategyHash: position.hash,
                        })
                      }
                    >
                      Dock
                    </button>
                  </>
                )}
                {onClone && (
                  <button
                    type="button"
                    className={styles.posAction}
                    disabled={busy}
                    onClick={() => onClone(position)}
                  >
                    Clone
                  </button>
                )}
              </span>
            </div>

            {activeEditor && (
              <PositionActionEditor
                position={position}
                editor={activeEditor}
                status={actionStatus}
                onChange={onOpenEditor}
                onClose={onCloseEditor}
                onPush={onPush}
                onDock={onDock}
              />
            )}

            {open && (
              <div
                className={styles.posDetail}
                role="region"
                aria-label={`Position ${position.hash} details`}
              >
                <div className={styles.posStats}>
                  {position.stats.map((stat) => (
                    <div
                      key={stat.label}
                      className={styles.posStat}
                      style={{
                        backgroundImage: `linear-gradient(${stat.sep}, ${stat.sep})`,
                      }}
                    >
                      <div className={styles.posStatLabel}>{stat.label}</div>
                      <div className={styles.posStatValue}>{stat.value}</div>
                    </div>
                  ))}
                </div>
                <div className={styles.coverage}>
                  <span className={styles.coverageLabel}>
                    Coverage {position.covNum}
                  </span>
                  <span className={styles.coverageBar}>
                    <span
                      className={styles.coverageA}
                      style={{ width: position.splitA }}
                    >
                      {position.labelA}
                    </span>
                    <span className={styles.coverageB}>{position.labelB}</span>
                  </span>
                </div>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function PositionActionEditor({
  position,
  editor,
  status,
  onChange,
  onClose,
  onPush,
  onDock,
}: {
  position: PositionRow;
  editor: PositionEditor;
  status: PositionActionStatus | undefined;
  onChange: (editor: PositionEditor) => void;
  onClose: () => void;
  onPush: ((input: PushPositionInput) => Promise<unknown>) | undefined;
  onDock: ((input: DockPositionInput) => Promise<unknown>) | undefined;
}) {
  const currentStatus =
    status?.strategyHash === position.hash && status.kind === editor.kind
      ? status
      : undefined;
  const submitting = currentStatus?.submitting ?? false;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    const pending =
      editor.kind === "push"
        ? (() => {
            const token = position.tokens?.[editor.tokenIndex];
            return position.maker && token && onPush
              ? onPush({
                  maker: position.maker,
                  strategyHash: position.hash,
                  token,
                  amount: editor.amount,
                })
              : undefined;
          })()
        : position.maker && position.tokens?.length === 2
          ? onDock?.({
              maker: position.maker,
              strategyHash: position.hash,
              tokens: position.tokens.map(({ address }) => address),
            })
          : undefined;
    void pending?.then(onClose).catch(() => undefined);
  };

  return (
    <form
      className={styles.positionActionEditor}
      aria-label={`${editor.kind === "push" ? "Push to" : "Dock"} ${position.pair} position`}
      onSubmit={submit}
    >
      <div className={styles.positionActionCopy}>
        <span className={styles.positionActionTitle}>
          {editor.kind === "push"
            ? "Add available liquidity"
            : "Close position"}
        </span>
        <span className={styles.positionActionHint}>
          {editor.kind === "push"
            ? "Choose one token and the amount this strategy may quote."
            : "Remove both tokens from shared liquidity and stop quoting this strategy."}
        </span>
      </div>
      {editor.kind === "push" && (
        <>
          <div className={styles.positionTokenPicker}>
            {(position.tokens ?? []).map((token, index) => (
              <button
                key={token.address}
                type="button"
                className={
                  index === editor.tokenIndex
                    ? styles.positionTokenOn
                    : styles.positionToken
                }
                disabled={submitting}
                onClick={() => onChange({ ...editor, tokenIndex: index })}
              >
                {token.symbol}
              </button>
            ))}
          </div>
          <label className={styles.positionAmountField}>
            <span>Amount</span>
            <input
              value={editor.amount}
              inputMode="decimal"
              autoComplete="off"
              disabled={submitting}
              placeholder="0.00"
              aria-label="Push amount"
              onChange={(event) =>
                onChange({ ...editor, amount: event.target.value })
              }
            />
          </label>
        </>
      )}
      <div className={styles.positionActionButtons}>
        <button
          type="button"
          className={styles.positionActionCancel}
          disabled={submitting}
          onClick={onClose}
        >
          Cancel
        </button>
        <button
          type="submit"
          className={styles.positionActionConfirm}
          disabled={
            submitting ||
            (editor.kind === "push" &&
              (!editor.amount.trim() || onPush === undefined)) ||
            (editor.kind === "dock" && onDock === undefined)
          }
        >
          {submitting
            ? "Confirm in wallet…"
            : editor.kind === "push"
              ? "Push liquidity"
              : "Confirm dock"}
        </button>
      </div>
      {currentStatus?.problem && (
        <p className={styles.positionActionProblem} role="alert">
          {currentStatus.problem}
        </p>
      )}
    </form>
  );
}
