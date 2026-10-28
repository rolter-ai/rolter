import type { LucideIcon } from "lucide-react";
import { Link, type To } from "react-router";

/**
 * The amber notice that qualifies a spend figure: a glyph, a sentence in two
 * parts and the way to the rows behind it. Unpriced traffic (#969), withheld
 * responses and unknown usage (#3302) all say "this total is not the whole
 * story" the same way, so they share one frame.
 *
 * One tone: the amber warning pair, fill for the wash and border and the
 * `-text` half for the glyph. The wash used to be the red tint, which set an
 * amber icon on red and said two things at once (#1994).
 */
export function SpendNotice({
  icon: Icon,
  title,
  body,
  to,
  link,
}: {
  icon: LucideIcon;
  /** the count, said first and in the foreground colour */
  title: string;
  /** what it means for the figure above */
  body: string;
  to: To;
  /** the link's text */
  link: string;
}) {
  return (
    <div
      role="status"
      className="flex items-start gap-2.5 rounded-lg border border-[color:var(--status-warning)]/40 bg-[color:var(--status-warning)]/10 px-4 py-3"
    >
      <Icon
        aria-hidden
        className="mt-0.5 h-4 w-4 flex-none text-[color:var(--status-warning-text)]"
      />
      <p className="min-w-0 text-sm text-muted-foreground">
        <span className="font-medium text-foreground">{title}</span> {body}{" "}
        <Link
          to={to}
          className="rounded-sm font-medium text-foreground underline underline-offset-2 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          {link}
        </Link>
      </p>
    </div>
  );
}
