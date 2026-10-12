#!/usr/bin/env bun
// raw error display gate (#2728).
//
//   bun run check:raw-errors
//
// A failed control-plane call is shown through `describeError` / `errorText`
// (`ui/src/lib/error-copy.ts`): a translated sentence chosen by the stable
// `code`, then the status, with the server's English words only as detail
// where nothing translated explains them. Printing `error.message` bypasses
// that, so the operator reads an internal or untranslated string.
//
// **A screen or component that prints `(x as Error).message`, `x.error.message`
// or calls the removed `errorDetail(`** fails. A display that genuinely is not
// a server answer (a WebSocket constructor failure, an editor's own validation
// error) carries `// raw-error-allow: <reason>` on the line above it. Every
// honoured waiver is printed on every run, so the set stays visible.
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { Glob } from "bun";
import { blankComments } from "./check-story-focus";

const ROOT = join(import.meta.dir, "..");

const RAW = [/\bas Error\)\.message\b/, /\b\w+\.error\.message\b/, /\berrorDetail\(/];
const ALLOW = /raw-error-allow:\s*(\S.*)/;

export interface Violation {
  file: string;
  line: number;
  text: string;
}

export function checkSource(source: string, file: string) {
  const code = blankComments(source).split("\n");
  const raw = source.split("\n");
  const violations: Violation[] = [];
  const waived: { file: string; line: number; reason: string }[] = [];
  code.forEach((text, i) => {
    if (!RAW.some((re) => re.test(text))) return;
    const reason = ALLOW.exec(raw[i - 1] ?? "")?.[1];
    if (reason) waived.push({ file, line: i + 1, reason });
    else violations.push({ file, line: i + 1, text: text.trim() });
  });
  return { violations, waived };
}

export function checkAll() {
  const violations: Violation[] = [];
  const waived: { file: string; line: number; reason: string }[] = [];
  for (const dir of ["pages", "components"]) {
    for (const rel of new Glob("**/*.tsx").scanSync(join(ROOT, "src", dir))) {
      if (rel.endsWith(".stories.tsx") || rel.endsWith(".test.tsx")) continue;
      const file = `src/${dir}/${rel}`;
      const result = checkSource(readFileSync(join(ROOT, file), "utf8"), file);
      violations.push(...result.violations);
      waived.push(...result.waived);
    }
  }
  return { violations, waived };
}

if (import.meta.main) {
  const { violations, waived } = checkAll();
  for (const w of waived) console.log(`waived ${w.file}:${w.line}: ${w.reason}`);
  for (const v of violations) {
    console.error(`${v.file}:${v.line}: raw error display: ${v.text}`);
  }
  if (violations.length > 0) {
    console.error(
      `\n${violations.length} raw error display(s). Show a failed call with errorText(error, t) ` +
        "from @/lib/error-copy, or waive it with // raw-error-allow: <reason>.",
    );
    process.exit(1);
  }
  console.log("check:raw-errors: ok");
}
