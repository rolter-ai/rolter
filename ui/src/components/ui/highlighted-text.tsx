import * as React from "react";

/**
 * `text` with the characters at `indices` (code-point positions, as
 * `fuzzyMatch` returns them) set apart, so a filtered list shows why each row
 * matched. With no indices it is just the text, in one node.
 */
export function HighlightedText({ text, indices }: { text: string; indices: number[] }) {
  if (indices.length === 0) return <>{text}</>;
  const marked = new Set(indices);
  const runs: { text: string; hit: boolean }[] = [];
  Array.from(text).forEach((ch, i) => {
    const hit = marked.has(i);
    const last = runs[runs.length - 1];
    if (last && last.hit === hit) last.text += ch;
    else runs.push({ text: ch, hit });
  });
  return (
    <>
      {runs.map((run, i) =>
        run.hit ? (
          <span
            key={i}
            data-match=""
            className="font-semibold text-foreground underline decoration-[color:var(--red-folk-text)] decoration-2 underline-offset-2"
          >
            {run.text}
          </span>
        ) : (
          <React.Fragment key={i}>{run.text}</React.Fragment>
        ),
      )}
    </>
  );
}
