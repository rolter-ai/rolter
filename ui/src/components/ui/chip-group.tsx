import * as React from "react";
import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

// a multi-select over a short, fully visible list: every option is on screen as
// a toggleable chip, so picking three teams costs three clicks and no menu.
// a list long enough to need filtering belongs in the Combobox instead
export interface ChipGroupProps {
  label: string;
  /**
   * the rows to pick from. `id` is what the draft stores and what
   * `visibility.allowed_*_ids` carries — the control plane parses each one as
   * a uuid — while `name` is what the operator reads (#1189)
   */
  options: { id: string; name: string }[];
  selected: string[];
  onToggle: (v: string) => void;
  disabled?: boolean;
}

export function ChipGroup({ label, options, selected, onToggle, disabled }: ChipGroupProps) {
  const { t } = useTranslation();
  const id = React.useId();
  return (
    <div className="space-y-1.5" role="group" aria-labelledby={id}>
      <span id={id} className="text-xs font-medium text-[color:var(--text-secondary)]">
        {label}
      </span>
      <div className="flex flex-wrap gap-1.5">
        {options.length === 0 && (
          <p className="text-xs text-muted-foreground">{t("common.noneAvailable")}</p>
        )}
        {options.map((o) => {
          const on = selected.includes(o.id);
          return (
            <button
              key={o.id}
              type="button"
              disabled={disabled}
              onClick={() => onToggle(o.id)}
              aria-pressed={on}
              className={cn(
                "focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
                "inline-flex h-7 items-center rounded-full border px-2.5 font-mono text-xs transition-colors duration-[120ms] disabled:cursor-not-allowed disabled:opacity-50",
                on
                  ? "border-[color:var(--red-500)] bg-[color:var(--red-tint)] text-foreground"
                  : "border-[color:var(--border-subtle)] bg-transparent text-muted-foreground",
              )}
            >
              {o.name}
            </button>
          );
        })}
      </div>
    </div>
  );
}
