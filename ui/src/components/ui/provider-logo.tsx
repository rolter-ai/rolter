import { providerLogoPath } from "@/lib/provider-logos";
import { cn } from "@/lib/utils";

/**
 * A provider kind's mark, for the leading slot of a picker option (#3059).
 *
 * The mark is a single-colour path painted in `currentColor`, so it follows the
 * theme and keeps the secondary-text contrast in light and dark alike. A kind
 * the logo set has no mark for gets a monogram — the first letter of its name
 * in a quiet frame — never a broken image or a blank gap, so a list of mixed
 * kinds still lines up. Decorative either way: the option's label names it.
 */
export function ProviderLogo({
  kind,
  name,
  className,
}: {
  kind: string;
  /** what the kind is called, for the monogram's letter; the stored id when absent */
  name?: string;
  className?: string;
}) {
  const path = providerLogoPath(kind);
  const box = "h-4 w-4 flex-none text-[color:var(--text-secondary)]";
  if (path) {
    return (
      <svg
        aria-hidden
        data-provider-logo={kind}
        viewBox="0 0 24 24"
        fill="currentColor"
        className={cn(box, className)}
      >
        <path d={path} />
      </svg>
    );
  }
  // the letter is drawn from an attribute by a pseudo-element, so it stays out
  // of the option's text: a row reading "OOpenAI" breaks every lookup by text
  const letter = Array.from((name || kind).trim())[0]?.toUpperCase() ?? "?";
  return (
    <span
      aria-hidden
      data-provider-logo={kind}
      data-monogram={letter}
      className={cn(
        box,
        "inline-flex items-center justify-center rounded-[var(--radius-sm)] border border-[color:var(--border-default)] text-[0.5625rem] font-semibold leading-none before:content-[attr(data-monogram)]",
        className,
      )}
    />
  );
}
