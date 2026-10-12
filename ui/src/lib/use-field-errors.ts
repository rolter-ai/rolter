import type { TFunction } from "i18next";
import * as React from "react";

import { describedBy } from "@/components/ui/field-error";
import { serverFieldError } from "@/lib/field-errors";

export interface FieldErrorsOptions<K extends string> {
  /** the fields in the order they sit on the screen, so focus lands on the first one that is wrong */
  order: readonly K[];
  /** wire name of a request field (`retention_days`) → the form field it belongs to */
  wire: Record<string, K>;
  /** the DOM id of a field's control, which focus is moved to */
  idOf: (key: K) => string;
  /** the screen's own checks, already translated; the server stays the authority */
  local?: Partial<Record<K, string | undefined>>;
  /**
   * a field whose complaint waits for a first press on Save (`useErrorVisibility`).
   * Its local error stays out of the display and the footer count, but still
   * stops a save and takes the focus
   */
  shows?: (key: K) => boolean;
  /** a field that cannot be edited right now, so no complaint about it holds Save */
  skip?: (key: K) => boolean;
  /**
   * a refusal the wire table cannot place, such as one row of a list
   * (`injected_headers.x-team`); tried before the table
   */
  place?: (error: unknown) => { field: K; message: string } | null;
}

/**
 * The inline-error pattern of a settings screen (#2096, #2651, #2734): the
 * screen's own checks and the control plane's refusals shown under the field
 * they belong to, Save that stays pressable and says why, a count for the
 * footer, and a 400 that names a field pinned to it instead of a toast.
 * See `docs/dev-docs/development/form-primitives.md`.
 */
export function useFieldErrors<K extends string>({
  order,
  wire,
  idOf,
  local = {},
  shows,
  skip,
  place,
}: FieldErrorsOptions<K>) {
  const [server, setServer] = React.useState<Partial<Record<K, string>>>({});

  const failing = (key: K): string | undefined =>
    skip?.(key) ? undefined : (local[key] ?? server[key]);
  const errorOf = (key: K): string | undefined =>
    skip?.(key) ? undefined : (((shows?.(key) ?? true) ? local[key] : undefined) ?? server[key]);

  /** the fields with an error on show, in screen order */
  const invalid = order.filter((key) => errorOf(key));

  const focusField = (key: K) => {
    const node = document.getElementById(idOf(key)) as HTMLInputElement | null;
    if (!node || node.disabled) return false;
    node.focus();
    return true;
  };

  return {
    errorOf,
    invalid,
    /** the id an error is rendered under, for the control's `aria-describedby` */
    errorId: (key: K) => `${idOf(key)}-error`,
    /** the id and the invalid/described-by pair a control wears while it has an error */
    inputProps: (key: K, ...also: (string | false | undefined)[]) => ({
      id: idOf(key),
      "aria-invalid": errorOf(key) ? (true as const) : undefined,
      "aria-describedby": describedBy(...also, !!errorOf(key) && `${idOf(key)}-error`),
    }),
    /** an edit answers the control plane's complaint about the fields it touched */
    edit: (...keys: K[]) =>
      setServer((all) => {
        if (!keys.some((key) => all[key] !== undefined)) return all;
        const next = { ...all };
        for (const key of keys) delete next[key];
        return next;
      }),
    /** forgets every server complaint: after a save that went through, or a discard */
    clear: () => setServer({}),
    /** whether a refusal names one of this screen's fields, so `refuse` would place it */
    names: (error: unknown): boolean => !!(place?.(error) ?? serverFieldError(error, wire)),
    /**
     * Pins a rejected save to the field it names and moves focus there.
     * False means the refusal names none of this screen's fields and belongs
     * in a toast or a banner.
     */
    refuse: (error: unknown): boolean => {
      const hit = place?.(error) ?? serverFieldError(error, wire);
      if (!hit) return false;
      setServer({ [hit.field]: hit.message } as Partial<Record<K, string>>);
      focusField(hit.field);
      return true;
    },
    /**
     * Save's press: with a field at fault it moves focus to the first one
     * rather than doing nothing, otherwise it runs the save. False when it held.
     */
    submit: (run: () => void): boolean => {
      const bad = order.filter((key) => failing(key));
      if (bad.length > 0) {
        bad.some(focusField);
        return false;
      }
      run();
      return true;
    },
  };
}

/** a screen's `validate()` answers with catalog keys; this renders them in the dashboard's language */
export function translateErrors<K extends string>(
  keys: Partial<Record<K, string>>,
  t: TFunction,
  values?: Record<string, unknown>,
): Partial<Record<K, string>> {
  return Object.fromEntries(
    Object.entries<string | undefined>(keys).flatMap(([key, value]) =>
      value ? [[key, t(value, values)]] : [],
    ),
  ) as Partial<Record<K, string>>;
}
