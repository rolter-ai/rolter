// every persona account × every leaf screen of the dashboard: which screens
// render, which refuse ("You do not have access"), which fail to load, and
// which wait on a project the scope switcher could not select
import { LEAVES, OUT, close, railLabels, scopeOf, screenState, signIn } from "./harness";
import { writeFileSync } from "node:fs";

const PERSONAS = ["dev", "orgadmin", "lead", "engineer", "engineer2", "viewer", "finops"];
const MARK: Record<string, string> = { ok: "·", denied: "D", error: "E", "no-scope": "S" };

const results = await Promise.all(
  PERSONAS.map(async (who) => {
    const email = `${who}@rolter.local`;
    const { page, context } = await signIn(email);
    await page.waitForTimeout(1500);
    const scope = await scopeOf(page, email);
    const rail = await railLabels(page);
    const screens: Record<string, { state: string; text: string }> = {};
    for (const key of LEAVES) screens[key] = await screenState(page, key);
    await context.close();
    const counts = Object.values(screens).reduce((a: Record<string, number>, s) => ((a[s.state] = (a[s.state] ?? 0) + 1), a), {});
    console.log(`${who.padEnd(10)} scope=${scope.org || "—"}/${scope.team || "—"}/${scope.project || "—"} ${scope.message} ${JSON.stringify(counts)}`);
    return { who, scope, rail, counts, screens };
  }),
);
await close();

const lines = [
  `| screen | ${PERSONAS.join(" | ")} |`,
  `| --- | ${PERSONAS.map(() => "---").join(" | ")} |`,
  ...LEAVES.map((key) => `| ${key} | ${results.map((r) => MARK[r.screens[key].state] ?? "?").join(" | ")} |`),
  "",
  "`·` renders · `D` refuses (\"You do not have access\") · `E` fails to load · `S` waits on a project",
];
writeFileSync(`${OUT}/survey.json`, JSON.stringify(results, null, 2));
writeFileSync(`${OUT}/survey.md`, lines.join("\n") + "\n");
console.log(`\nwrote ${OUT}/survey.md`);
