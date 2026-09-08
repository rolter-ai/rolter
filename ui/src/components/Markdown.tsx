import { ImageOff } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { CodeBlock } from "@/components/ui/code-block";
import type { MdBlock, MdInline } from "@/lib/markdown";
import { cn } from "@/lib/utils";

/**
 * Markdown from a model, rendered as React elements (#955).
 *
 * Everything that makes this safe lives in `@/lib/markdown`: the source is
 * lexed, never parsed to HTML, and the tree it produces has no node that can
 * carry markup. This file only decides what each node looks like — so there is
 * no `dangerouslySetInnerHTML` here, and there is no sanitiser to keep in step
 * with a parser.
 *
 * The parser arrives through a dynamic `import()`, which does double duty: it
 * keeps `marked` out of the main bundle, and it makes the plain source the
 * first thing on screen. A reply still streaming in is therefore readable
 * before the chunk lands and re-flows into the formatted version, rather than
 * flickering between two empty states.
 */
export function Markdown({
  source,
  className,
}: {
  source: string;
  className?: string;
}) {
  const [blocks, setBlocks] = React.useState<MdBlock[] | null>(null);

  React.useEffect(() => {
    let live = true;
    void import("@/lib/markdown")
      .then(({ parseMarkdown }) => {
        if (live) setBlocks(parseMarkdown(source));
      })
      .catch(() => {
        if (live) setBlocks(null);
      });
    return () => {
      live = false;
    };
  }, [source]);

  if (!blocks) {
    return (
      <div className={cn("whitespace-pre-wrap break-words", className)}>{source}</div>
    );
  }

  return (
    <div className={cn("flex flex-col gap-2 break-words", className)}>
      <Blocks blocks={blocks} />
    </div>
  );
}

function Blocks({ blocks }: { blocks: readonly MdBlock[] }) {
  return (
    <>
      {blocks.map((block, i) => (
        <Block key={i} block={block} />
      ))}
    </>
  );
}

function Block({ block }: { block: MdBlock }) {
  const { t } = useTranslation();
  switch (block.type) {
    case "paragraph":
      return (
        <p className="leading-relaxed">
          <Inlines nodes={block.children} />
        </p>
      );
    case "heading":
      // a reply's headings are content, not the screen's outline: they sit
      // under the page's own h1/h2 rather than competing with them, which is
      // why the level is offset instead of used as written
      return (
        <div
          role="heading"
          aria-level={Math.min(block.depth + 2, 6)}
          className={cn(
            "font-semibold text-foreground",
            block.depth <= 2 ? "text-[1.05em]" : "text-[1em]",
          )}
        >
          <Inlines nodes={block.children} />
        </div>
      );
    case "code":
      return (
        <CodeBlock
          value={block.value}
          language={block.language}
          label={t("markdown.codeBlock")}
          maxHeight={320}
          density="compact"
        />
      );
    case "list":
      return block.ordered ? (
        <ol start={block.start} className="ml-4 list-decimal space-y-1 leading-relaxed">
          {block.items.map((item, i) => (
            <li key={i}>
              <Blocks blocks={item} />
            </li>
          ))}
        </ol>
      ) : (
        <ul className="ml-4 list-disc space-y-1 leading-relaxed">
          {block.items.map((item, i) => (
            <li key={i}>
              <Blocks blocks={item} />
            </li>
          ))}
        </ul>
      );
    case "table":
      return (
        <div className="overflow-x-auto" tabIndex={0} role="region" aria-label={t("markdown.table")}>
          <table className="w-full border-collapse text-left">
            <thead>
              <tr>
                {block.header.map((cell, i) => (
                  <th
                    key={i}
                    className="border-b border-[color:var(--border-default)] px-2 py-1 font-medium text-foreground"
                  >
                    <Inlines nodes={cell} />
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {block.rows.map((row, i) => (
                <tr key={i}>
                  {row.map((cell, j) => (
                    <td
                      key={j}
                      className="border-b border-[color:var(--border-subtle)] px-2 py-1 align-top"
                    >
                      <Inlines nodes={cell} />
                    </td>
                  ))}
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      );
    case "blockquote":
      return (
        <blockquote className="border-l-2 border-[color:var(--border-default)] pl-3 text-[color:var(--text-muted)]">
          <Blocks blocks={block.children} />
        </blockquote>
      );
    case "hr":
      return <hr className="border-[color:var(--border-subtle)]" />;
    case "raw":
      // markup a model emitted, shown as the characters it is. nothing here
      // reaches the browser as markup — that is the whole contract (#955)
      return (
        <p className="whitespace-pre-wrap font-mono text-[0.9em] text-[color:var(--text-muted)]">
          {block.value}
        </p>
      );
  }
}

function Inlines({ nodes }: { nodes: readonly MdInline[] }) {
  const { t } = useTranslation();
  return (
    <>
      {nodes.map((node, i) => {
        switch (node.type) {
          case "text":
            return <React.Fragment key={i}>{node.value}</React.Fragment>;
          case "code":
            return (
              <code
                key={i}
                className="rounded bg-[color:var(--surface-subtle)] px-1 py-0.5 text-[0.9em] text-[color:var(--text-primary)]"
              >
                {node.value}
              </code>
            );
          case "strong":
            return (
              <strong key={i} className="font-semibold text-foreground">
                <Inlines nodes={node.children} />
              </strong>
            );
          case "em":
            return (
              <em key={i}>
                <Inlines nodes={node.children} />
              </em>
            );
          case "del":
            return (
              <del key={i} className="text-[color:var(--text-muted)]">
                <Inlines nodes={node.children} />
              </del>
            );
          case "break":
            return <br key={i} />;
          case "link":
            // `noreferrer` covers `noopener` in every browser the dashboard
            // supports, and it also stops the target learning which deployment
            // the operator was reading the reply in
            return node.href ? (
              <a
                key={i}
                href={node.href}
                target="_blank"
                rel="noreferrer"
                className="underline decoration-[color:var(--border-strong)] underline-offset-2 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                <Inlines nodes={node.children} />
              </a>
            ) : (
              // the url failed the scheme allowlist — `javascript:`, `data:` —
              // so the label stays, the link does not
              <React.Fragment key={i}>
                <Inlines nodes={node.children} />
              </React.Fragment>
            );
          case "image":
            // never an <img>: a remote fetch would break the air-gapped
            // guarantee and tell a third party the reply had been read
            return (
              <span
                key={i}
                title={t("markdown.imageBlocked")}
                className="inline-flex items-center gap-1 rounded bg-[color:var(--surface-subtle)] px-1.5 py-0.5 text-[0.9em] text-[color:var(--text-muted)]"
              >
                <ImageOff className="h-3 w-3 shrink-0" aria-hidden />
                {node.alt || node.href}
              </span>
            );
        }
      })}
    </>
  );
}
