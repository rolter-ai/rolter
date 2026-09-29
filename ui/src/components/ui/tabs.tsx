import * as React from "react";

import { cn } from "@/lib/utils";

// underline tab bar. mirrors the Rolter Design System navigation/Tabs.
export interface TabItem {
  value: string;
  label: React.ReactNode;
  count?: number;
  // id of the tab button itself, so a panel can point back with
  // aria-labelledby; optional, callers that render no panel omit it
  id?: string;
  // id of the role="tabpanel" this tab controls, wired as aria-controls;
  // optional for the same reason
  panelId?: string;
}

export interface TabsProps extends Omit<React.HTMLAttributes<HTMLDivElement>, "onChange"> {
  tabs: TabItem[];
  value: string;
  onChange?: (value: string) => void;
}

export function Tabs({ tabs = [], value, onChange, className, ...props }: TabsProps) {
  const refs = React.useRef<(HTMLButtonElement | null)[]>([]);

  const selected = tabs.findIndex((t) => t.value === value);
  // a strip whose value matches nothing still needs one tab stop, so the
  // roving tabindex falls back to the first tab
  const roving = selected >= 0 ? selected : 0;

  // wai-aria tablist pattern: left/right wrap around the ends, home/end jump
  // to the edges, and selection follows focus (automatic activation)
  const onKeyDown = (e: React.KeyboardEvent<HTMLDivElement>) => {
    if (tabs.length === 0) return;
    let next: number;
    switch (e.key) {
      case "ArrowRight":
        next = (roving + 1) % tabs.length;
        break;
      case "ArrowLeft":
        next = (roving - 1 + tabs.length) % tabs.length;
        break;
      case "Home":
        next = 0;
        break;
      case "End":
        next = tabs.length - 1;
        break;
      default:
        return;
    }
    e.preventDefault();
    refs.current[next]?.focus();
    onChange?.(tabs[next].value);
  };

  return (
    <div
      className={cn(
        // a strip wider than a phone scrolls inside itself rather than pushing
        // the page sideways (#1242); the scrollbar is hidden, the tabs stay
        // reachable by swipe and by Tab
        "flex items-center gap-1 overflow-x-auto border-b border-[color:var(--border-subtle)] [scrollbar-width:none] [&::-webkit-scrollbar]:hidden",
        className,
      )}
      role="tablist"
      onKeyDown={onKeyDown}
      {...props}
    >
      {tabs.map((t, i) => {
        const active = t.value === value;
        return (
          <button
            key={t.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            id={t.id}
            role="tab"
            type="button"
            aria-selected={active}
            aria-controls={t.panelId}
            // roving tabindex: the strip is a single tab stop, arrows walk it
            tabIndex={i === roving ? 0 : -1}
            onClick={() => onChange?.(t.value)}
            className={cn(
              "relative shrink-0 cursor-pointer border-none bg-transparent px-2 py-2 text-sm font-medium transition-colors rounded-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
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
