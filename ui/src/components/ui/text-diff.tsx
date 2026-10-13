import * as React from "react";
import { useTranslation } from "react-i18next";

import { Segmented } from "@/components/ui/segmented";
import {
  foldUnchanged,
  pairLines,
  type DiffLine,
  type DiffSkip,
  type FieldChange,
} from "@/lib/text-diff";
import { cn } from "@/lib/utils";

// the dashboard's one text comparison (#2112): two versions of a content
// laid out as lines, inline or side by side, and two records laid out as fields.
//
// a change is never only a colour. each line carries a "+" or "-" in the gutter
// and a visually hidden "Added" / "Removed", so a screen reader and a reader who
// cannot tell red from green get the same diff. the scroll region is focusable
// and named, as `CodeBlock` does, because a long line scrolls sideways

const ADDED = "bg-[color:var(--status-success)]/10";
const REMOVED = "bg-[color:var(--status-danger)]/10";

type Layout = "inline" | "split";

function Gutter({ no }: { no?: number }) {
  return (
    <span
      aria-hidden
      className="w-9 shrink-0 select-none pr-2 text-right tabular-nums text-[color:var(--text-subtle)]"
    >
      {no ?? ""}
    </span>
  );
}

function Mark({ kind }: { kind: DiffLine["kind"] }) {
  const { t } = useTranslation();
  return (
    <>
      <span
        aria-hidden
        className={cn(
          "w-4 shrink-0 select-none text-center font-semibold",
          kind === "add" && "text-[color:var(--status-success-text)]",
          kind === "del" && "text-[color:var(--status-danger-text)]",
        )}
      >
        {kind === "add" ? "+" : kind === "del" ? "−" : ""}
      </span>
      {kind !== "same" && (
        <span className="sr-only">
          {kind === "add" ? t("repository.diff.added") : t("repository.diff.removed")}:
        </span>
      )}
    </>
  );
}

function Line({ line, side }: { line: DiffLine; side?: "old" | "new" }) {
  return (
    <div
      className={cn(
        "flex min-w-0 py-px",
        line.kind === "add" && ADDED,
        line.kind === "del" && REMOVED,
      )}
    >
      {side !== "new" && <Gutter no={line.oldNo} />}
      {side !== "old" && <Gutter no={line.newNo} />}
      <Mark kind={line.kind} />
      <span className="min-w-0 flex-1 whitespace-pre-wrap break-words pr-2">
        {line.text || " "}
      </span>
    </div>
  );
}

function SkipRow({ count }: { count: number }) {
  const { t } = useTranslation();
  return (
    <div className="border-y border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] px-3 py-0.5 text-center text-[0.6875rem] text-[color:var(--text-subtle)]">
      {t("repository.diff.unchanged", { count })}
    </div>
  );
}

export interface TextDiffProps {
  lines: DiffLine[];
  /** names the scroll region; say what is compared, "Decorators" */
  label: string;
  /** unchanged lines kept beside a change before a run folds away */
  context?: number;
  className?: string;
}

/** a line diff, inline by default with a switch to two columns */
export function TextDiff({ lines, label, context = 3, className }: TextDiffProps) {
  const { t } = useTranslation();
  const [layout, setLayout] = React.useState<Layout>("inline");
  const changed = lines.some((line) => line.kind !== "same");
  const folded = React.useMemo(() => foldUnchanged(lines, context), [lines, context]);
  return (
    <div className={cn("min-w-0", className)}>
      <div className="mb-2 flex items-center justify-between gap-2">
        <span className="text-xs font-medium text-muted-foreground">{label}</span>
        {changed && (
          <Segmented
            ariaLabel={t("repository.diff.layout", { name: label })}
            value={layout}
            options={[
              { value: "inline", label: t("repository.diff.inline") },
              { value: "split", label: t("repository.diff.split") },
            ]}
            onChange={setLayout}
          />
        )}
      </div>
      {!changed ? (
        <p className="text-xs text-muted-foreground">{t("repository.diff.identical")}</p>
      ) : (
        <div
          role="region"
          aria-label={label}
          tabIndex={0}
          className="max-h-80 overflow-auto rounded-lg border border-[color:var(--border-subtle)] bg-[color:var(--surface-app)] py-1 font-mono text-xs leading-5 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        >
          {layout === "inline" ? (
            <div className="min-w-max sm:min-w-0">
              {folded.map((row, index) =>
                row.kind === "skip" ? (
                  <SkipRow key={`skip-${index}`} count={(row as DiffSkip).count} />
                ) : (
                  <Line key={index} line={row as DiffLine} />
                ),
              )}
            </div>
          ) : (
            <SplitRows lines={lines} context={context} />
          )}
        </div>
      )}
    </div>
  );
}

function SplitRows({ lines, context }: { lines: DiffLine[]; context: number }) {
  // fold on the rows rather than on the lines, so a pair is never torn in two
  const rows = React.useMemo(() => pairLines(lines), [lines]);
  const keep = React.useMemo(() => {
    const near = rows.map(() => false);
    rows.forEach((row, index) => {
      if (row.left?.kind === "same") return;
      for (
        let k = Math.max(0, index - context);
        k <= Math.min(rows.length - 1, index + context);
        k++
      )
        near[k] = true;
    });
    return near;
  }, [rows, context]);
  const out: React.ReactNode[] = [];
  let skipped = 0;
  rows.forEach((row, index) => {
    if (!keep[index]) {
      skipped++;
      return;
    }
    if (skipped) out.push(<SkipRow key={`skip-${index}`} count={skipped} />);
    skipped = 0;
    out.push(
      <div
        key={index}
        className="grid min-w-[32rem] grid-cols-2 divide-x divide-[color:var(--border-subtle)]"
      >
        {row.left ? (
          <Line line={row.left} side="old" />
        ) : (
          <div className="bg-[color:var(--surface-app)]" />
        )}
        {row.right ? (
          <Line line={row.right} side="new" />
        ) : (
          <div className="bg-[color:var(--surface-app)]" />
        )}
      </div>,
    );
  });
  if (skipped) out.push(<SkipRow key="skip-end" count={skipped} />);
  return <div className="min-w-[32rem]">{out}</div>;
}

export interface FieldDiffProps {
  changes: FieldChange[];
  /** names the list; say what the rows are, "Variables" */
  label: string;
  /** show the rows that did not change too */
  showSame?: boolean;
  className?: string;
}

/** a record's fields before and after, one row per field that differs */
export function FieldDiff({ changes, label, showSame = false, className }: FieldDiffProps) {
  const { t } = useTranslation();
  const rows = showSame ? changes : changes.filter((change) => change.kind !== "same");
  return (
    <div className={cn("min-w-0", className)}>
      <p className="mb-2 text-xs font-medium text-muted-foreground">{label}</p>
      {rows.length === 0 ? (
        <p className="text-xs text-muted-foreground">{t("repository.diff.identical")}</p>
      ) : (
        <ul
          aria-label={label}
          className="divide-y divide-[color:var(--border-subtle)] border-y border-[color:var(--border-subtle)]"
        >
          {rows.map((change) => (
            <li key={change.field} className="grid gap-1 py-2 text-xs sm:grid-cols-[10rem_1fr]">
              <span className="min-w-0 break-words font-mono font-medium">
                {change.field}
                <span className="ml-2 font-sans font-normal text-muted-foreground">
                  {t(`repository.diff.field.${change.kind}`)}
                </span>
              </span>
              <span className="min-w-0 space-y-0.5 font-mono">
                {change.before !== undefined && change.kind !== "same" && (
                  <span className={cn("flex gap-1.5 rounded px-1.5 py-0.5", REMOVED)}>
                    <span aria-hidden className="text-[color:var(--status-danger-text)]">
                      −
                    </span>
                    <span className="sr-only">{t("repository.diff.before")}:</span>
                    <span className="min-w-0 break-all">{change.before}</span>
                  </span>
                )}
                {change.after !== undefined && change.kind !== "same" && (
                  <span className={cn("flex gap-1.5 rounded px-1.5 py-0.5", ADDED)}>
                    <span aria-hidden className="text-[color:var(--status-success-text)]">
                      +
                    </span>
                    <span className="sr-only">{t("repository.diff.after")}:</span>
                    <span className="min-w-0 break-all">{change.after}</span>
                  </span>
                )}
                {change.kind === "same" && <span className="break-all">{change.after}</span>}
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
