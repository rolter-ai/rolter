import * as React from "react";

import { useDraft, type FieldEquality } from "@/lib/use-draft";
import { useFieldErrors, type FieldErrorsOptions } from "@/lib/use-field-errors";
import { useLeaveGuard } from "@/lib/use-leave-guard";

export interface SettingsFormOptions<T extends object, K extends string> {
  /** the query's data mapped into the form's shape; undefined until it has arrived */
  source: T | undefined;
  /** how a field's two values are compared; a module constant, read on every render */
  equals?: FieldEquality<T>;
  /**
   * The screen's field errors, given the draft (null before the form has
   * loaded): the field order, the wire names, the control ids and the screen's
   * own checks. Left out for a screen with no field that can be wrong.
   */
  fieldErrors?: (draft: T | null) => FieldErrorsOptions<K>;
}

const NO_FIELDS = { order: [] as never[], wire: {}, idOf: () => "" };

/**
 * The form state every deployment-settings screen shares (#2214): what was
 * loaded and what is being edited (`useDraft`), the field errors a refused
 * save pins to its field (`useFieldErrors`), the prompt that stops a draft being
 * lost to navigation (`useLeaveGuard`), and when the last save of this sitting
 * happened. `SettingsSaveBar` reads it for the footer and renders the prompt.
 *
 * ```tsx
 * const form = useSettingsForm({ source, equals: EQUALS, fieldErrors: (draft) => ({ … }) });
 * // an input:   value={draft.x} onChange={(e) => form.set({ x: e.target.value })}
 * // a save:     onSuccess: (dto) => form.commit(fromDto(dto))
 * // the footer: <SettingsSaveBar form={form} pending={save.isPending} … />
 * ```
 */
export function useSettingsForm<T extends object, K extends string = never>({
  source,
  equals,
  fieldErrors,
}: SettingsFormOptions<T, K>) {
  const draft = useDraft(source, equals);
  const fields = useFieldErrors<K>(
    fieldErrors ? fieldErrors(draft.draft) : (NO_FIELDS as FieldErrorsOptions<K>),
  );
  const leavePrompt = useLeaveGuard(draft.dirty);
  // by this tab's clock, set when a save lands; null until one does
  const [savedAt, setSavedAt] = React.useState<number | null>(null);
  const { set: patch, reset: restore, commit: adopt } = draft;
  const { edit, clear } = fields;

  return {
    /** the form being edited; null until `source` has arrived */
    draft: draft.draft,
    /** what the server held at the last load or save */
    saved: draft.saved,
    /** the fields whose value differs from what the server holds */
    changed: draft.changed,
    dirty: draft.dirty,
    fields,
    savedAt,
    /** render once, anywhere in the screen: asks before a dirty draft is left behind */
    leavePrompt,
    /** an edit also answers the control plane's complaint about the fields it touches */
    set: (change: Partial<T>) => {
      patch(change);
      edit(...(Object.keys(change) as K[]));
    },
    /** Discard: every field back to what was loaded, and the complaints with it */
    discard: () => {
      restore();
      clear();
    },
    /** a save landed: what the server answered is both the form and the baseline */
    commit: (value: T) => {
      adopt(value);
      clear();
      setSavedAt(Date.now());
    },
  };
}

export type SettingsForm<T extends object = object, K extends string = string> = ReturnType<
  typeof useSettingsForm<T, K>
>;
