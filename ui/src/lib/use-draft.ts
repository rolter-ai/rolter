import * as React from "react";

/**
 * The draft half of a settings form: what was loaded, what is being edited,
 * which fields differ, and the way back.
 *
 * A settings screen saves its fields as one blob, so it cannot tell a pristine
 * form from an edited one by looking at the button. This holds both copies and
 * answers "which fields changed" so Save can wait for an edit, a field can say
 * it was edited, and Discard has a value to restore. The Security screen is
 * the first to use it; the leave guard, the field errors and the saved line
 * that #2214 asks the other settings screens to share belong beside it.
 */
export interface DraftState<T> {
  /** what the server held at the last load or save; null before the first */
  saved: T | null;
  draft: T | null;
}

export type DraftAction<T> =
  | { type: "seed"; value: T }
  | { type: "patch"; patch: Partial<T> }
  | { type: "reset" }
  | { type: "commit"; value: T };

/**
 * How two values of one field are compared, by field. A field left out is
 * compared with `Object.is`. A text field that is edited into the same entries
 * with different spacing wants a looser one, so typing a blank line is no edit.
 */
export type FieldEquality<T> = { [K in keyof T]?: (a: T[K], b: T[K]) => boolean };

export function draftReducer<T extends object>(
  state: DraftState<T>,
  action: DraftAction<T>,
): DraftState<T> {
  switch (action.type) {
    // the first load only: a refetch behind an edit must not take the edit away
    case "seed":
      return state.saved === null ? { saved: action.value, draft: action.value } : state;
    case "patch":
      return state.draft === null
        ? state
        : { ...state, draft: { ...state.draft, ...action.patch } };
    case "reset":
      return { ...state, draft: state.saved };
    // a save landed: what the server answered is both the new baseline and the form
    case "commit":
      return { saved: action.value, draft: action.value };
  }
}

/** The fields of `draft` that differ from `saved`, in the draft's key order. */
export function changedKeys<T extends object>(
  saved: T,
  draft: T,
  equals: FieldEquality<T> = {},
): (keyof T)[] {
  return (Object.keys(draft) as (keyof T)[]).filter((key) => {
    const same = equals[key] as ((a: T[typeof key], b: T[typeof key]) => boolean) | undefined;
    return !(same ?? Object.is)(saved[key], draft[key]);
  });
}

export interface Draft<T> {
  /** null until `source` has arrived */
  draft: T | null;
  saved: T | null;
  changed: (keyof T)[];
  dirty: boolean;
  set: (patch: Partial<T>) => void;
  /** put every field back to what was loaded */
  reset: () => void;
  /** adopt `value` as the new saved copy and the form, after a save */
  commit: (value: T) => void;
}

/**
 * A draft seeded once from `source`, the query a screen is waiting on.
 *
 * `equals` should be a module constant: it is read on every render.
 */
export function useDraft<T extends object>(
  source: T | undefined,
  equals?: FieldEquality<T>,
): Draft<T> {
  const [state, dispatch] = React.useReducer(
    draftReducer as (s: DraftState<T>, a: DraftAction<T>) => DraftState<T>,
    { saved: null, draft: null } as DraftState<T>,
  );
  // seeded while rendering rather than from an effect: an effect leaves one
  // committed frame with no form, and a screen flashes empty between its
  // skeleton and its fields
  if (source !== undefined && state.saved === null) dispatch({ type: "seed", value: source });

  const changed = React.useMemo(
    () => (state.saved && state.draft ? changedKeys(state.saved, state.draft, equals) : []),
    [state.saved, state.draft, equals],
  );
  const set = React.useCallback((patch: Partial<T>) => dispatch({ type: "patch", patch }), []);
  const reset = React.useCallback(() => dispatch({ type: "reset" }), []);
  const commit = React.useCallback((value: T) => dispatch({ type: "commit", value }), []);

  return {
    draft: state.draft,
    saved: state.saved,
    changed,
    dirty: changed.length > 0,
    set,
    reset,
    commit,
  };
}
