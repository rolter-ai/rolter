import { Code2 } from "lucide-react";
import * as React from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import { CodeBlock } from "@/components/ui/code-block";
import {
  Dialog,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Tabs } from "@/components/ui/tabs";
import type { CodeLanguage } from "@/lib/code";
import {
  renderSnippet,
  SNIPPET_LANGS,
  type SnippetLang,
  type SnippetRequest,
} from "@/lib/snippets";

const LABELS: Record<SnippetLang, string> = {
  curl: "curl",
  python: "Python",
  javascript: "JavaScript",
};

/** what to highlight each snippet as — curl is a shell command line */
const HIGHLIGHT: Record<SnippetLang, CodeLanguage> = {
  curl: "bash",
  python: "python",
  javascript: "javascript",
};

/**
 * "Copy as code" for a Playground request.
 *
 * The Playground proves a route works; the next thing an operator does is wire
 * that route into an application. The useful output of a successful call is
 * therefore the code that reproduces it, not the reply — and rolter's whole
 * pitch is that the code is just the OpenAI SDK with a different `base_url`,
 * which is far more convincing shown than described.
 *
 * Which is why this dialog is the wide one and the snippet is highlighted
 * (#948). It is the moment an operator stops clicking and starts integrating,
 * and it used to render a wrapped, colourless block that looked worse than the
 * terminal it was about to be pasted into. The tabs are `Tabs` rather than a
 * `<select>` so all three languages are visible at once: the comparison is the
 * argument.
 */
export function CodeSnippetDialog({
  open,
  onOpenChange,
  request,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  request: SnippetRequest;
}) {
  const { t } = useTranslation();
  const [lang, setLang] = React.useState<SnippetLang>("curl");

  // window is read at render rather than module load so the snippet follows
  // whatever host the dashboard is actually being served from
  const snippet = React.useMemo(
    () =>
      renderSnippet(
        lang,
        request,
        typeof window === "undefined" ? "" : window.location.origin,
      ),
    [lang, request],
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange} size="lg">
      <DialogHeader>
        <DialogTitle>{t("playground.copyAsCode")}</DialogTitle>
        <DialogDescription>{t("playground.copyAsCodeHint")}</DialogDescription>
      </DialogHeader>

      <div className="flex flex-col gap-3">
        <Tabs
          aria-label={t("playground.language")}
          tabs={SNIPPET_LANGS.map((l) => ({ value: l, label: LABELS[l] }))}
          value={lang}
          onChange={(v) => setLang(v as SnippetLang)}
        />

        {/* the block scrolls sideways rather than wrapping: a snippet broken
            mid-URL reads as two broken lines, not as one long one (#948).
            CodeBlock owns the copy button, the focusable scroll region and
            the name, so each language is copyable on its own terms */}
        <CodeBlock
          value={snippet}
          language={HIGHLIGHT[lang]}
          label={t("playground.snippet")}
          maxHeight={420}
        />
      </div>

      <DialogFooter>
        <Button variant="ghost" onClick={() => onOpenChange(false)}>
          {t("common.close")}
        </Button>
      </DialogFooter>
    </Dialog>
  );
}

/** The trigger, so a caller only has to own the request it describes. */
export function CopyAsCodeButton({ request }: { request: SnippetRequest }) {
  const { t } = useTranslation();
  const [open, setOpen] = React.useState(false);
  return (
    <>
      {/* a labelled control, not a bare glyph: the dialog behind this is the
          payoff for integrating with rolter at all, and an anonymous `</>`
          icon made it something an operator had to click to discover (#963).
          the label collapses below `sm`, where the header has no room for it —
          the aria-label and the tooltip carry the name there */}
      <Button
        size="sm"
        variant="ghost"
        className="h-8 gap-1.5"
        onClick={() => setOpen(true)}
        aria-label={t("playground.copyAsCode")}
        title={t("playground.copyAsCode")}
      >
        <Code2 className="h-3.5 w-3.5" />
        <span className="hidden sm:inline">{t("playground.copyAsCode")}</span>
      </Button>
      {open && (
        <CodeSnippetDialog open={open} onOpenChange={setOpen} request={request} />
      )}
    </>
  );
}
