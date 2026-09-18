import { ChevronDown } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { InfoHint } from "@/components/ui/info-hint";
import { cn } from "@/lib/utils";

// one collapsible group of fields inside a sheet. `open` is controlled by the
// caller, so a sheet can open the section a validation error landed in
export interface FormSectionProps {
  title: string;
  info?: string;
  open: boolean;
  onToggle: () => void;
  children: React.ReactNode;
  /** extra classes on the body, for a section that lays its fields out itself */
  className?: string;
}

export function FormSection({
  title,
  info,
  open,
  onToggle,
  children,
  className,
}: FormSectionProps) {
  const { t } = useTranslation();
  return (
    <div className="rounded-[10px] border border-[color:var(--border-subtle)]">
      {/* the toggle is a real button and the InfoHint sits beside it rather
          than inside it: this used to be a `div role="button"` wrapping the
          hint's own button, which is a nested interactive control — invalid
          HTML that a screen reader announces as one confused thing, and an
          axe `nested-interactive` failure (#1201) */}
      <div className="flex w-full items-center gap-2.5 px-[15px]">
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={open}
          className="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 py-[13px] text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          <span className="text-sm font-semibold">{title}</span>
          <ChevronDown
            className={cn(
              "ml-auto h-4 w-4 text-[color:var(--text-subtle)] transition-transform duration-[120ms]",
              open && "rotate-180",
            )}
          />
        </button>
        {info && <InfoHint text={info} label={t("common.aboutField", { label: title })} />}
      </div>
      {open && (
        <div
          className={cn(
            "border-t border-[color:var(--border-subtle)] px-[15px] pb-4 pt-3.5",
            className,
          )}
        >
          {children}
        </div>
      )}
    </div>
  );
}
