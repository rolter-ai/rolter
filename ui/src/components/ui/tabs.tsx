import * as React from "react";

import { cn } from "@/lib/utils";

// underline tab bar. mirrors the Rolter Design System navigation/Tabs.
export interface TabItem {
  value: string;
  label: React.ReactNode;
  count?: number;
}

export interface TabsProps extends Omit<React.HTMLAttributes<HTMLDivElement>, "onChange"> {
  tabs: TabItem[];
  value: string;
  onChange?: (value: string) => void;
}

export function Tabs({ tabs = [], value, onChange, className, ...props }: TabsProps) {
  return (
    <div
      className={cn(
        "flex items-center gap-1 border-b border-[color:var(--border-subtle)]",
        className,
      )}
      role="tablist"
      {...props}
    >
      {tabs.map((t) => {
        const active = t.value === value;
        return (
          <button
            key={t.value}
            role="tab"
            aria-selected={active}
            onClick={() => onChange?.(t.value)}
            className={cn(
              "relative cursor-pointer border-none bg-transparent px-2 py-2 text-sm font-medium transition-colors",
              active
                ? "text-foreground after:absolute after:inset-x-2 after:-bottom-px after:h-0.5 after:rounded-full after:bg-foreground"
                : "text-muted-foreground hover:text-foreground",
            )}
          >
            {t.label}
            {t.count != null && (
              <span className="ml-1.5 font-mono text-[0.6875rem] text-[color:var(--text-subtle)]">
                {t.count}
              </span>
            )}
          </button>
        );
      })}
    </div>
  );
}
