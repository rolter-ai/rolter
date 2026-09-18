import * as React from "react";

import { chordKeys, chordText, type Chord } from "@/lib/shortcuts";
import { cn } from "@/lib/utils";

// a printed keystroke (#1676) — the shape a hint beside a field and a row in
// the shortcut sheet both need, so neither re-invents the border, the mono
// face or the subdued tone.
//
// `KbdChord` is what callers reach for: it takes the chord straight off the
// shortcut table, resolves the platform modifier and prints one key per `<kbd>`
// the way an OS menu does. The whole group carries one accessible name — a
// reader hearing "kay" after "command" learns nothing, and `⌘K` read as a word
// is what the hint actually means.

export interface KbdProps extends React.HTMLAttributes<HTMLElement> {
  children: React.ReactNode;
}

export function Kbd({ className, children, ...props }: KbdProps) {
  return (
    <kbd
      className={cn(
        "inline-flex min-w-[1.5rem] items-center justify-center rounded border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] px-1.5 py-0.5 font-mono text-[0.6875rem] leading-none text-[color:var(--text-subtle)]",
        className,
      )}
      {...props}
    >
      {children}
    </kbd>
  );
}

export interface KbdChordProps {
  /** a `chord` from `SHORTCUTS` — `[MOD, "K"]`, `["/"]` */
  chord: Chord;
  /** override the platform guess; stories pin it so the glyph is not the runner's */
  apple?: boolean;
  className?: string;
}

export function KbdChord({ chord, apple, className }: KbdChordProps) {
  const keys = chordKeys(chord, apple);
  return (
    <span
      role="img"
      aria-label={chordText(chord, apple)}
      className={cn("inline-flex flex-none items-center gap-1", className)}
    >
      {keys.map((key) => (
        <Kbd key={key} aria-hidden="true">
          {key}
        </Kbd>
      ))}
    </span>
  );
}
