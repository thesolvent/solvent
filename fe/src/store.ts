import { create } from "zustand";
import { useShallow } from "zustand/react/shallow";

import { INITIAL_STATE, type AppState } from "./state";

interface AppStore extends AppState {
  set: (patch: Partial<AppState>) => void;
}

/** The shell's UI state. Server data lives in the query cache, never here. */
export const useAppStore = create<AppStore>((set) => ({
  ...INITIAL_STATE,
  set: (patch) => set(patch),
}));

/**
 * Subscribe to a slice of shell state.
 *
 * Pointer-driven keys (hover, drag) change on every mouse move, so a component that reads them
 * through the whole-state facade re-renders the rest of the app with it. Selecting narrows the
 * subscription to the keys a component actually uses.
 */
export function useAppSlice<T>(select: (state: AppState) => T): T {
  return useAppStore(useShallow(select));
}
