import { CopyableText } from "@/components/ui/copyable-value";

/**
 * A labelled line inside a card: a small caps label and a mono value, optionally
 * copyable. A copyable address is `CopyableText` and wraps, because the end of
 * it is what tells one address from another; every other value is one truncated
 * line, so a long issuer or filter does not stretch the card. `note` says what
 * the value is for.
 */
export function LabelledValue({
  label,
  value,
  copyLabel,
  note,
}: {
  label: string;
  value: string;
  copyLabel?: string;
  note?: string;
}) {
  return (
    <div className="flex min-w-0 items-start gap-2">
      <span className="w-[104px] flex-none text-2xs uppercase leading-4 tracking-[0.07em] text-[color:var(--text-subtle)]">
        {label}
      </span>
      <div className="min-w-0 flex-1">
        {copyLabel ? (
          <CopyableText
            variant="inline"
            value={value}
            copyLabel={copyLabel}
            className="text-xs leading-4"
          />
        ) : (
          <span
            title={value}
            className="block truncate font-mono text-xs leading-4 text-[color:var(--text-secondary)]"
          >
            {value}
          </span>
        )}
        {note && <span className="mt-0.5 block text-xs text-muted-foreground">{note}</span>}
      </div>
    </div>
  );
}
