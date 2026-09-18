import * as React from "react";
import { useTranslation } from "react-i18next";

import { InfoHint } from "@/components/ui/info-hint";
import { cn } from "@/lib/utils";

// label + control + helper/error wrapper, mirrors the Rolter Design System
// Field. Composes around whatever control is passed as children (Input,
// Select, Textarea, Switch, ...).
export interface FieldProps extends React.HTMLAttributes<HTMLDivElement> {
  label?: string;
  /** explicit control id; when omitted the field generates one and applies it
   * to its child, so the label is never left dangling */
  htmlFor?: string;
  error?: string;
  /** helper text below the control; a node so a hint can carry a `DocsLink`
   * (#1164) — it is only rendered and tested for emptiness, never read */
  hint?: React.ReactNode;
  // optional explanatory note surfaced via an (i) button beside the label:
  // what the field is and what values to use
  info?: React.ReactNode;
}

interface ControlProps {
  id?: string;
  "aria-describedby"?: string;
  "aria-invalid"?: boolean | "true" | "false";
}

/**
 * What a `<label for>` may point at.
 *
 * Read in document order, so the *first* control inside a multi-child field is
 * the one the label names — a hint, a unit suffix or a "test" button beside it
 * is not what the label is about. `button` is in the list because the composed
 * controls render as one (a switch, a combobox trigger).
 */
const LABELABLE = 'input, select, textarea, button, [contenteditable="true"]';

export function Field({
  label,
  htmlFor,
  error,
  hint,
  info,
  className,
  children,
  ...props
}: FieldProps) {
  // almost no caller passes `htmlFor`, which left every label in the dashboard
  // pointing at nothing: a screen reader announced the control as unlabelled,
  // and `getByLabelText` could not find it either. Generate the id here and put
  // it on the child, so the association is the default rather than something
  // each of ~200 call sites has to remember.
  const { t } = useTranslation();
  const generated = React.useId();
  const messageId = React.useId();
  const single = React.Children.count(children) === 1 ? children : null;
  const control = React.isValidElement<ControlProps>(single) ? single : null;
  // hand the field two children — a control plus a hint, a row, a second
  // control — and there is no single element to clone the id onto, so the label
  // named nothing and `getByLabelText` could not find the control either. That
  // failure is silent: the field looks right on screen (#1264). The children are
  // wrapped and the first labelable control in them is bound after mount.
  const [wrapped, setWrapped] = React.useState<string>();
  const wrapper = React.useRef<HTMLDivElement>(null);
  const root = React.useRef<HTMLDivElement>(null);
  const byHand = !control && !htmlFor;
  // an explicit htmlFor over several children named the control but still left
  // its hint and error undescribed, so an invalid input sounded valid (#1527)
  const multi = !control && !!htmlFor;
  const controlId =
    htmlFor ?? control?.props.id ?? (control ? generated : wrapped);
  // the error or hint below the control is tied to it as its description, and
  // an error also flips aria-invalid, so a screen reader hears both the state
  // and the reason rather than a control that silently refuses to submit
  const message = error ?? hint;
  // a node-valued hint is a fresh element on every render, so the effect below
  // depends on whether there *is* a description rather than on the node itself
  const hasMessage = !!message;
  const described: ControlProps = {};
  if (control && !control.props.id && !htmlFor) described.id = controlId;
  if (message && control && !control.props["aria-describedby"]) {
    described["aria-describedby"] = messageId;
  }
  if (error && control && control.props["aria-invalid"] === undefined) {
    described["aria-invalid"] = true;
  }
  const labelled =
    control && Object.keys(described).length > 0
      ? React.cloneElement(control, described)
      : children;

  // the cloning above cannot reach into a wrapper, so the multi-child case is
  // resolved from the rendered DOM instead: whatever the children turned out to
  // be, the first labelable node among them is the control this field is about.
  // Layout effect rather than effect, so the association exists before paint
  // and before a story's first query.
  React.useLayoutEffect(() => {
    if (!byHand && !multi) return;
    const node = multi
      ? (root.current?.querySelector<HTMLElement>(`[id="${CSS.escape(htmlFor ?? "")}"]`) ?? null)
      : (wrapper.current?.querySelector<HTMLElement>(LABELABLE) ?? null);
    if (!node) {
      // a field with a label and nothing to attach it to is the bug this
      // component exists to prevent, and it is invisible on screen; an
      // explicit htmlFor already names its control, wherever that lives
      if (byHand && import.meta.env.DEV && label) {
        console.warn(
          `Field: "${label}" wraps no labelable control, so its label names ` +
            "nothing. Pass htmlFor and put the matching id on the control.",
        );
      }
      return;
    }
    // only what this field added is removed again, so a control that carries
    // its own id or description keeps it
    const added: string[] = [];
    if (byHand) {
      if (!node.id) {
        node.id = generated;
        added.push("id");
      }
      setWrapped(node.id);
    }
    if (hasMessage && !node.hasAttribute("aria-describedby")) {
      node.setAttribute("aria-describedby", messageId);
      added.push("aria-describedby");
    }
    if (error && !node.hasAttribute("aria-invalid")) {
      node.setAttribute("aria-invalid", "true");
      added.push("aria-invalid");
    }
    return () => {
      for (const attribute of added) node.removeAttribute(attribute);
    };
  }, [byHand, children, error, generated, hasMessage, htmlFor, label, messageId, multi]);

  return (
    <div ref={root} className={cn("space-y-1.5", className)} {...props}>
      {label && (
        <div className="flex items-center gap-1.5">
          <label htmlFor={controlId} className="text-sm font-medium leading-none">
            {label}
          </label>
          {info && <InfoHint text={info} label={t("common.aboutField", { label })} />}
        </div>
      )}
      {byHand ? (
        // the wrapper carries the same spacing the children had as siblings of
        // the label, so wrapping them changes nothing on screen
        <div ref={wrapper} className="space-y-1.5">
          {labelled}
        </div>
      ) : (
        labelled
      )}
      {error ? (
        <p id={messageId} role="alert" className="text-xs text-[color:var(--status-danger-text)]">
          {error}
        </p>
      ) : hint ? (
        <p id={messageId} className="text-xs text-muted-foreground">
          {hint}
        </p>
      ) : null}
    </div>
  );
}
