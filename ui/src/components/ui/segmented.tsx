import * as React from "react";

import { cn } from "@/lib/utils";

// a two-to-four way choice rendered inline, for a mode that is always visible
// rather than hidden behind a Select. it is a real `radiogroup`, so a screen
// reader announces "1 of 3". roving tabindex: the checked option is the one
// tab stop, and the arrow keys (and Home/End) move focus and select
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
  const refs = React.useRef<(HTMLButtonElement | null)[]>([]);
  // the tab stop: the checked option, or the first when nothing matches
  const stop = Math.max(
    0,
    options.findIndex((o) => o.value === value),
  );
  const onKeyDown = (event: React.KeyboardEvent, index: number) => {
    const last = options.length - 1;
    let next: number;
    switch (event.key) {
      case "ArrowRight":
      case "ArrowDown":
        next = index === last ? 0 : index + 1;
        break;
      case "ArrowLeft":
      case "ArrowUp":
        next = index === 0 ? last : index - 1;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = last;
        break;
      default:
        return;
    }
    event.preventDefault();
    refs.current[next]?.focus();
    onChange(options[next].value);
  };
  return (
    <div
      role="radiogroup"
      aria-labelledby={labelledBy}
      aria-label={ariaLabel}
      className="inline-flex w-fit rounded-md bg-[color:var(--surface-subtle)] p-0.5"
    >
      {options.map((o, i) => (
        <button
          key={o.value}
          type="button"
          role="radio"
          ref={(node) => {
            refs.current[i] = node;
          }}
          tabIndex={i === stop ? 0 : -1}
          onKeyDown={(event) => onKeyDown(event, i)}
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
