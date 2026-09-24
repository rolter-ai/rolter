// Run every user-journey script against the dogfood stack and summarise.
//
//   bun integration/dogfood/journeys/run.ts            # all of them
//   bun integration/dogfood/journeys/run.ts lead app   # just these
//
// Each script runs in its own process, so one crashing cannot take the rest
// with it. Results, screenshots and the summary land in integration/dogfood/.journeys/.
import { OUT } from "./harness";
import { existsSync, readFileSync, writeFileSync } from "node:fs";

const ALL = ["viewer", "engineer", "lead", "app", "finops", "admin", "secops", "devops"];
const wanted = process.argv.slice(2);
const scripts = wanted.length ? ALL.filter((s) => wanted.includes(s)) : ALL;

const sh = (file: string) => {
  const proc = Bun.spawnSync(["bun", `${import.meta.dir}/${file}`], { stdout: "inherit", stderr: "inherit" });
  return proc.exitCode;
};

console.log("== setup: cross-project traffic");
if (sh("setup.ts") !== 0) console.log("!! setup failed; isolation steps may be wrong");
for (const s of scripts) {
  console.log(`\n== ${s}`);
  const code = sh(`${s}.ts`);
  if (code !== 0) console.log(`!! ${s}.ts exited ${code}`);
}

const rows: { persona: string; step: string; name: string; status: string; note: string }[] = [];
for (const s of scripts) {
  const path = `${OUT}/results-${s}.json`;
  if (existsSync(path)) rows.push(...JSON.parse(readFileSync(path, "utf8")));
}
const count = (xs: typeof rows) => xs.reduce((a: Record<string, number>, r) => ((a[r.status] = (a[r.status] ?? 0) + 1), a), {});
const table = [
  "| persona | steps | pass | bug | partial | gap | fail | skip |",
  "| --- | --- | --- | --- | --- | --- | --- | --- |",
  ...scripts.map((s) => {
    const c = count(rows.filter((r) => r.persona === s));
    const n = Object.values(c).reduce((a, b) => a + b, 0);
    return `| ${s} | ${n} | ${c.pass ?? 0} | ${c.bug ?? 0} | ${c.partial ?? 0} | ${c.gap ?? 0} | ${c.fail ?? 0} | ${c.skip ?? 0} |`;
  }),
  "",
  "| step | status | note |",
  "| --- | --- | --- |",
  ...rows.map((r) => `| ${r.persona}/${r.step} | ${r.status} | ${r.note.replace(/\|/g, "\\|")} |`),
];
writeFileSync(`${OUT}/summary.md`, table.join("\n") + "\n");
writeFileSync(`${OUT}/results.json`, JSON.stringify(rows, null, 2));
const total = count(rows);
console.log(`\n${rows.length} steps: ${JSON.stringify(total)}\nsummary: ${OUT}/summary.md`);
process.exit(total.fail ? 1 : 0);
