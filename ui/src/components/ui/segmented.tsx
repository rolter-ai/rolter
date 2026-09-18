import { cn } from "@/lib/utils";

// a two-to-four way choice rendered inline, for a mode that is always visible
// rather than hidden behind a Select. it is a real `radiogroup`, so a screen
// reader announces "1 of 3" and the arrow keys move between the options
export interface SegmentedProps<T extends string> {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  disabled?: boolean;
  /** id of the FieldLabel naming this group */
  labelledBy?: string;
  /** a name for a group with no visible label */
  ariaLabel?: string;
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  disabled,
  labelledBy,
  ariaLabel,
}: SegmentedProps<T>) {
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-label={ariaLabel}
      className="inline-flex w-fit rounded-md bg-[color:var(--surface-subtle)] p-0.5"
    >
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          aria-checked={value === o.value}
          disabled={disabled}
          onClick={() => onChange(o.value)}
          className={cn(
            "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
            "rounded px-2.5 py-1 text-xs font-medium transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50",
            value === o.value
              ? "bg-[color:var(--surface-base)] text-foreground shadow-sm"
              : "text-muted-foreground hover:text-foreground",
          )}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
