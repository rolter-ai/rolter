import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * The actions and target types the control plane writes to the audit log (#2127).
 *
 * The Audit Log screen's filters used to be hand-written lists that drifted
 * from what `log_audit` and its siblings record. The control plane has no
 * endpoint listing the distinct values, so this reads them out of its Rust
 * source, the way `rbac-matrix-source.ts` reads the capability table:
 * `gen-audit-vocabulary.ts` writes `src/lib/audit-vocabulary.json` and
 * `audit-vocabulary-source.test.ts` fails the build while the two disagree.
 *
 * An audited action is a dotted lower-case literal (`sso_provider.update`) in
 * non-test code. Dotted literals that are not audit actions (a span name, an
 * OTel attribute, a filename) are listed in `NOT_ACTIONS`.
 */

const SOURCE_DIR = "crates/rolter-control/src";

/** dotted literals the control plane carries that are not audit actions */
const NOT_ACTIONS = new Set([
  "control.request",
  "http.response.status_code",
  "index.html",
  "scalar.js",
  "snapshot.build",
  "snapshot.encode",
  "snapshot.sanitize",
]);

/** non-test source: inline `#[cfg(test)] mod x { ... }` blocks end at a column-zero brace */
export function productionSource(source: string): string {
  return source.replace(/#\[cfg\(test\)\]\s*mod \w+ \{[\s\S]*?\n\}\n/g, "");
}

/** every audited action in the given sources, sorted */
export function parseActions(sources: string[]): string[] {
  const found = new Set<string>();
  for (const source of sources) {
    for (const [literal] of productionSource(source).matchAll(
      /"[a-z][a-z0-9_]*(?:\.[a-z0-9_]+)+"/g,
    )) {
      const action = literal.slice(1, -1);
      if (!NOT_ACTIONS.has(action)) found.add(action);
    }
  }
  return [...found].sort();
}

/** every target type written beside an action, sorted */
export function parseTargets(sources: string[]): string[] {
  const found = new Set<string>();
  for (const source of sources) {
    for (const match of productionSource(source).matchAll(
      /"[a-z][a-z0-9_]*(?:\.[a-z0-9_]+)+",\s*(?:Some\()?"([a-z][a-z0-9_]*)"/g,
    )) {
      found.add(match[1]!);
    }
  }
  return [...found].sort();
}

/** Read every `.rs` file under the control plane's `src/` (resolved from `root`, the repo root). */
export function readSources(root: string): string[] {
  const dir = join(root, SOURCE_DIR);
  return readdirSync(dir)
    .filter((name) => name.endsWith(".rs"))
    .map((name) => readFileSync(join(dir, name), "utf8"));
}

export interface AuditVocabulary {
  actions: string[];
  targets: string[];
}

export function readVocabulary(root: string): AuditVocabulary {
  const sources = readSources(root);
  return { actions: parseActions(sources), targets: parseTargets(sources) };
}
