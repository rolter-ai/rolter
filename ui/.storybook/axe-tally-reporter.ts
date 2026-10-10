// The writer for `ROLTER_AXE_TALLY=<path>` (#1244): one JSON line per story,
// listing every axe violation at every impact, appended to the file the variable
// names. It never fails a story; vitest.config.ts only loads it for a tally run,
// where the preview has stopped failing on a violation and stopped excluding the
// page-level rules, so the lines count the whole band.
//
// A reporter rather than a write from the story: the browser cannot append to a
// file, and addon-a11y already hands every story's axe result to the node side as
// `meta.reports`.
import { appendFileSync } from "node:fs";
import type { Reporter, TestCase } from "vitest/node";

interface A11yReport {
  type?: string;
  result?: { violations?: { id: string; impact?: string | null; nodes: unknown[] }[] };
}

export default class AxeTallyReporter implements Reporter {
  private readonly path = process.env.ROLTER_AXE_TALLY ?? "";

  onTestCaseResult(testCase: TestCase): void {
    if (!this.path) return;
    const { storyId, reports } = testCase.meta() as { storyId?: string; reports?: A11yReport[] };
    const report = reports?.find((r) => r.type === "a11y");
    if (!storyId || !report?.result?.violations) return;
    const violations = report.result.violations.map((v) => ({
      id: v.id,
      impact: v.impact,
      nodes: v.nodes.length,
    }));
    appendFileSync(this.path, `${JSON.stringify({ story: storyId, violations })}\n`);
  }
}
