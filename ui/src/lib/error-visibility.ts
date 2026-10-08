import * as React from "react";

/**
 * When a form shows its validation errors (#2810).
 *
 * A required field is not wrong until somebody has had the chance to fill it,
 * so a pristine form opens with no error on it. An error appears when its field
 * was touched (focus came in and left), and every error appears once a submit
 * was refused. The form still computes every error all the time, since the
 * count behind the summary and the submit refusal read the same list; this
 * only decides which of them are on screen.
 *
 * `reset` returns the form to pristine and belongs where the form opens.
 */
export interface ErrorVisibility<K extends string> {
  /** mark `field` as visited, so its error may show */
  touch: (field: K) => void;
  /** a submit was refused: every error shows from here on */
  attempt: () => void;
  reset: () => void;
  /** whether the error of `field` is allowed on screen */
  shows: (field: K) => boolean;
  /** how many refused submits there have been, for an effect that follows each */
  attempts: number;
}

export function useErrorVisibility<K extends string>(): ErrorVisibility<K> {
  const [touched, setTouched] = React.useState<ReadonlySet<K>>(() => new Set());
  const [attempts, setAttempts] = React.useState(0);

  const touch = React.useCallback((field: K) => {
    setTouched((prev) => (prev.has(field) ? prev : new Set(prev).add(field)));
  }, []);
  const attempt = React.useCallback(() => setAttempts((n) => n + 1), []);
  const reset = React.useCallback(() => {
    setTouched((prev) => (prev.size === 0 ? prev : new Set()));
    setAttempts(0);
  }, []);
  const shows = React.useCallback(
    (field: K) => attempts > 0 || touched.has(field),
    [attempts, touched],
  );

  return React.useMemo(
    () => ({ touch, attempt, reset, shows, attempts }),
    [touch, attempt, reset, shows, attempts],
  );
}
