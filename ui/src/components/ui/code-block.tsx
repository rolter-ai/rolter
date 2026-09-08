import * as React from "react";
import { useTranslation } from "react-i18next";

import { CopyButton } from "@/components/CopyButton";
import {
  HIGHLIGHT_CHAR_LIMIT,
  splitLines,
  type CodeLanguage,
  type CodeNode,
} from "@/lib/code";
import { cn } from "@/lib/utils";

/**
 * The dashboard's one code surface (#949).
 *
 * Every structured payload an operator reads goes through this: request and
 * response bodies in the Logs and MCP drawers, effective-config sections, the
 * collector document, copy-as-code snippets, fenced blocks in a model reply.
 * One component means one keyboard story, one copy affordance and one palette,
 * instead of nine `<pre>` blocks that each got some of it right.
 *
 * The tokeniser is not imported here. It arrives through a dynamic `import()`
 * in an effect, so the first frame is the plain text — which is also what a
 * screen keeps if the chunk fails to load, and what a value past
 * `HIGHLIGHT_CHAR_LIMIT` keeps on purpose.
 */
export interface CodeBlockProps {
  /** the source text, verbatim; also what the copy button puts on the clipboard */
  value: string;
  language?: CodeLanguage;
  /**
   * names the scroll region for assistive technology. Pass one wherever more
   * than one block shares a screen — "Request body", "providers", a section
   * key — so the regions are told apart by name rather than by position.
   */
  label?: string;
  /** soft-wrap instead of scrolling sideways. Off by default: a snippet wrapped
   *  mid-token reads worse than one that scrolls (#948) */
  wrap?: boolean;
  /** caps the scroll region; a bare number is pixels */
  maxHeight?: number | string;
  /** a gutter drawn with a CSS counter, so the numbers never join a selection */
  lineNumbers?: boolean;
  /** the copy button, on unless the caller already provides one */
  copy?: boolean;
  /** the tighter type scale the log drawers use */
  density?: "default" | "compact";
  className?: string;
}

/** display names for the accessible label — codes and proper nouns, not copy */
const LANGUAGE_NAMES: Record<CodeLanguage, string> = {
  text: "Text",
  json: "JSON",
  toml: "TOML",
  yaml: "YAML",
  bash: "Bash",
  csv: "CSV",
  log: "Log",
  python: "Python",
  javascript: "JavaScript",
  typescript: "TypeScript",
  sql: "SQL",
  markdown: "Markdown",
};

export function CodeBlock({
  value,
  language = "text",
  label,
  wrap = false,
  maxHeight,
  lineNumbers = false,
  copy = true,
  density = "default",
  className,
}: CodeBlockProps) {
  const { t } = useTranslation();
  const [nodes, setNodes] = React.useState<CodeNode[] | null>(null);

  React.useEffect(() => {
    // plain text and oversized payloads never load the chunk at all
    if (language === "text" || value.length > HIGHLIGHT_CHAR_LIMIT) {
      setNodes(null);
      return;
    }
    let live = true;
    void import("@/lib/code-highlight")
      .then(({ highlight }) => {
        if (live) setNodes(highlight(value, language));
      })
      .catch(() => {
        // an air-gapped deployment serves the chunk from its own origin, so
        // this is a build or cache failure rather than a network one. the
        // payload still has to be readable
        if (live) setNodes(null);
      });
    return () => {
      live = false;
    };
  }, [value, language]);

  // the un-highlighted value is a valid tree of one text node, so streaming
  // output and the pre-load frame take exactly the same render path
  const tree = React.useMemo<CodeNode[]>(() => nodes ?? [value], [nodes, value]);
  const lines = React.useMemo(
    () => (lineNumbers ? splitLines(tree) : null),
    [lineNumbers, tree],
  );

  const name = label
    ? t("code.regionNamed", { label, language: LANGUAGE_NAMES[language] })
    : t("code.region", { language: LANGUAGE_NAMES[language] });

  return (
    <div className={cn("relative", className)}>
      {copy && (
        <div className="absolute right-1.5 top-1.5 z-10">
          <CopyButton
            value={value}
            label={label ? t("code.copyNamed", { label }) : t("code.copy")}
            className="h-7 w-7 bg-[color:var(--surface-elevated)] p-0 text-[color:var(--text-muted)] hover:text-foreground"
          />
        </div>
      )}
      {/* a scroll container has to be reachable from the keyboard, or the part
          of the payload below the fold needs a mouse (#1181); tabIndex plus a
          name make it a proper region rather than an unlabelled focus stop */}
      <pre
        tabIndex={0}
        role="region"
        aria-label={name}
        style={maxHeight === undefined ? undefined : { maxHeight }}
        className={cn(
          "rl-code overflow-auto rounded-md border border-[color:var(--border-subtle)] bg-[color:var(--surface-subtle)] leading-relaxed text-[color:var(--text-secondary)] focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring",
          density === "compact" ? "p-2.5 text-[0.6875rem]" : "p-3 text-xs",
          wrap ? "whitespace-pre-wrap break-words" : "whitespace-pre",
          lineNumbers && "rl-code--numbered",
          copy && "pr-10",
        )}
      >
        <code className={`language-${language}`}>
          {lines
            ? lines.map((line, i) => (
                <span key={i} className="rl-code-line">
                  <Nodes nodes={line} />
                  {/* the newline lives inside the line, not between the
                      elements: it keeps the block's text identical to the
                      source, so a selection copies what the payload says */}
                  {i < lines.length - 1 ? "\n" : null}
                </span>
              ))
            : <Nodes nodes={tree} />}
        </code>
      </pre>
    </div>
  );
}

function Nodes({ nodes }: { nodes: readonly CodeNode[] }) {
  return (
    <>
      {nodes.map((node, i) =>
        typeof node === "string" ? (
          <React.Fragment key={i}>{node}</React.Fragment>
        ) : (
          <span key={i} className={node.className}>
            <Nodes nodes={node.children} />
          </span>
        ),
      )}
    </>
  );
}
