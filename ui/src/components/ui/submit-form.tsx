import * as React from "react";
import { useTranslation } from "react-i18next";

// the <form> every editor sheet and dialog editor sits in (#1205): Enter in a
// field submits, a submit button means "submit", and a submit that cannot go
// through says why instead of doing nothing.
//
// the validation is the browser's own constraints (`required`, `minLength`,
// `pattern`, ...) read through `checkValidity()`, with the native bubble turned
// off (`noValidate`) so the message is the dashboard's own, in its language,
// under the field (`Field required`). a caller that knows more than the
// attributes do passes `canSubmit`, and is held the same way
//
// the form never disables its submit button for a validation error: pressing it
// is what asks for the reasons. a caller renders the button `type="submit"` and
// `aria-disabled` while `canSubmit` is false

interface Attempts {
  /** how many submits were refused since the form was last submitted for real */
  attempts: number;
  /** the controls at fault, counted after the latest refused submit */
  invalid: number;
}

const AttemptsContext = React.createContext<Attempts>({ attempts: 0, invalid: 0 });

/** Refused submits so far; `Field required` shows its message once this is above zero. */
export function useSubmitAttempts(): number {
  return React.useContext(AttemptsContext).attempts;
}

export type FormControl = HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement;

const CONTROLS = "input:not([type=hidden]), select, textarea";

/**
 * Whether a required control is still empty. A value of spaces counts as empty
 * for a required text field: the browser accepts it, and every save in the
 * dashboard trims it away.
 *
 * Only `required` is read from the attributes. `min`, `max`, `pattern` and the
 * rest stay with the screen's own checks, which say more than a generic
 * message can (a range, a charset, a conflict with another field).
 */
export function controlProblem(control: FormControl): "required" | null {
  if (control.disabled) return null;
  if (control.required) {
    if (control.validity.valueMissing) return "required";
    const checkable =
      control instanceof HTMLInputElement &&
      (control.type === "checkbox" || control.type === "radio");
    if (!checkable && control.value.trim() === "") return "required";
  }
  return null;
}

function missing(form: HTMLFormElement): boolean {
  return Array.from(form.querySelectorAll<FormControl>(CONTROLS)).some(
    (control) => controlProblem(control) !== null,
  );
}

function atFault(form: HTMLFormElement): FormControl[] {
  return Array.from(form.querySelectorAll<FormControl>(CONTROLS)).filter(
    (control) =>
      controlProblem(control) !== null || control.getAttribute("aria-invalid") === "true",
  );
}

export interface SubmitFormProps extends Omit<
  React.FormHTMLAttributes<HTMLFormElement>,
  "onSubmit" | "noValidate"
> {
  /** runs when the form is submitted and nothing is at fault */
  onSubmit: () => void;
  /** false while the caller knows the form cannot be submitted yet; a press then asks for the reasons */
  canSubmit?: boolean;
  /** a submit is in flight; a second one is ignored */
  busy?: boolean;
}

export function SubmitForm({
  onSubmit,
  canSubmit = true,
  busy = false,
  className,
  children,
  onInput,
  ...props
}: SubmitFormProps) {
  const form = React.useRef<HTMLFormElement>(null);
  const [state, setState] = React.useState<Attempts>({ attempts: 0, invalid: 0 });

  // after a refused submit the fields have rendered their messages: move focus
  // to the first one at fault and count them for the status line. a layout
  // effect on the attempt, so the count never lags a frame behind the press
  React.useLayoutEffect(() => {
    if (state.attempts === 0 || !form.current) return;
    const bad = atFault(form.current);
    bad[0]?.focus();
    setState((now) => (now.invalid === bad.length ? now : { ...now, invalid: bad.length }));
  }, [state.attempts]);

  return (
    <AttemptsContext.Provider value={state}>
      <form
        ref={form}
        noValidate
        className={className ?? "contents"}
        onSubmit={(event) => {
          event.preventDefault();
          if (busy) return;
          if (!canSubmit || missing(event.currentTarget)) {
            setState((now) => ({ ...now, attempts: now.attempts + 1 }));
            return;
          }
          setState({ attempts: 0, invalid: 0 });
          onSubmit();
        }}
        onInput={(event) => {
          onInput?.(event);
          // fixing a field updates the count; the messages follow on their own
          if (state.attempts === 0 || !form.current) return;
          const count = atFault(form.current).length;
          setState((now) => (now.invalid === count ? now : { ...now, invalid: count }));
        }}
        {...props}
      >
        {children}
      </form>
    </AttemptsContext.Provider>
  );
}

/**
 * The line in a footer after a refused submit: how many fields need attention,
 * or `fallback` when the caller's own rule (not a field) is what held it back.
 * A polite status, since the fields announce their own reasons.
 */
export function SubmitFormStatus({ fallback }: { fallback?: string }) {
  const { t } = useTranslation();
  const { attempts, invalid } = React.useContext(AttemptsContext);
  if (attempts === 0) return null;
  const message = invalid > 0 ? t("common.fieldsNeedAttention", { count: invalid }) : fallback;
  if (!message) return null;
  return (
    <p role="status" className="px-[22px] pt-2.5 text-xs text-[color:var(--status-danger-text)]">
      {message}
    </p>
  );
}
