// the error carries an id so its control can point at it: a screen reader then
// hears the reason when it reaches the field, not only in the footer (#1527)
export interface FieldErrorProps {
  /** the id the control names in its `aria-describedby` */
  id: string;
  error?: string;
}

export function FieldError({ id, error }: FieldErrorProps) {
  if (!error) return null;
  return (
    <p id={id} className="text-xs leading-snug text-[color:var(--status-danger-text)]">
      {error}
    </p>
  );
}

/** a control's description: the hint under it plus its error when it has one */
export function describedBy(...ids: (string | false | undefined)[]): string | undefined {
  return ids.filter(Boolean).join(" ") || undefined;
}
