import { readFileSync } from "node:fs";

/**
 * The control plane's capability table, read out of its Rust source (#1298).
 *
 * `ui/src/pages/story-harness.tsx` answers `GET /api/v1/rbac/matrix` and
 * `GET /api/v1/rbac/effective` for the gating stories, and it used to do it
 * from a hand-written copy of `CAPABILITIES`
 * (`crates/rolter-control/src/rbac_matrix.rs`). Nothing compared the two, so
 * the copy drifted: #1258 found it calling `model` and `model_price` org-scoped
 * admin resources when both are deployment-wide catalogs only a superadmin
 * writes, which let two screens gate on capabilities the control plane does not
 * define while their stories passed.
 *
 * The control plane publishes that table over HTTP but emits no build artifact,
 * so there is nothing checked in to diff against — hence this parser, and
 * #1369 for the snapshot export that would replace it. It reads the `const
 * CAPABILITIES` table directly; `scripts/gen-rbac-capabilities.ts` writes the
 * result to `src/lib/rbac-capabilities.json`, and `rbac-matrix-source.test.ts`
 * fails the build when the two disagree.
 */

/** The authority an action takes, spelled as `Authority` spells it. */
export type Authority = "viewer" | "member" | "admin" | "superadmin" | "authenticated";

/** One row of the table: an authority per action, `null` where there is no such action. */
export interface CapabilityRow {
  resource: string;
  scope: string;
  read: Authority | null;
  create: Authority | null;
  update: Authority | null;
  delete: Authority | null;
}

/** the `const` aliases the table is written in terms of */
const AUTHORITIES: Record<string, Authority | null> = {
  VIEWER: "viewer",
  MEMBER: "member",
  ADMIN: "admin",
  SUPER: "superadmin",
  ANYONE: "authenticated",
  NA: null,
};

const ACTION_FIELDS = ["read", "create", "update", "delete"] as const;

const SOURCE = "crates/rolter-control/src/rbac_matrix.rs";

/**
 * The body of `const CAPABILITIES: &[Capability] = &[ ... ];`.
 *
 * Bounded by the closing `];` at column zero rather than by brace counting:
 * every entry is indented, so the first unindented terminator is the table's
 * own, and the module carries several later `&[...]` literals in its tests.
 */
function capabilityTable(source: string): string {
  const start = source.indexOf("const CAPABILITIES");
  if (start < 0) throw new Error(`no \`const CAPABILITIES\` table in ${SOURCE}`);
  const open = source.indexOf("&[", source.indexOf("=", start));
  const end = source.indexOf("\n];", open);
  if (open < 0 || end < 0) throw new Error(`could not find the end of \`CAPABILITIES\` in ${SOURCE}`);
  // comments go first: the table annotates individual fields as well as
  // whole entries ("an org is created out of band…"), and a comment sitting
  // between `create:` and `update:` would break a match over the entry
  return source.slice(open + 2, end).replace(/\/\/[^\n]*/g, "");
}

function authority(alias: string, resource: string, field: string): Authority | null {
  if (!(alias in AUTHORITIES)) {
    throw new Error(
      `unknown authority \`${alias}\` on \`${resource}.${field}\` in ${SOURCE}; ` +
        "teach ui/scripts/rbac-matrix-source.ts about it",
    );
  }
  return AUTHORITIES[alias] ?? null;
}

/**
 * Parse `CAPABILITIES` out of the module source, in table order.
 *
 * Order is preserved because the published matrix is the table's order, and a
 * fixture that reordered it would be a fixture the dashboard could not have
 * received.
 */
export function parseCapabilities(source: string): CapabilityRow[] {
  const table = capabilityTable(source);
  const entry =
    /Capability\s*\{\s*resource:\s*"([^"]+)",\s*scope:\s*"([^"]+)",\s*read:\s*(\w+),\s*create:\s*(\w+),\s*update:\s*(\w+),\s*delete:\s*(\w+),?\s*\}/g;
  const rows: CapabilityRow[] = [];
  for (const [, resource, scope, ...aliases] of table.matchAll(entry)) {
    const row: CapabilityRow = { resource, scope, read: null, create: null, update: null, delete: null };
    ACTION_FIELDS.forEach((field, i) => {
      row[field] = authority(aliases[i]!, resource, field);
    });
    rows.push(row);
  }
  // count the `resource:` lines independently and insist the two agree: a
  // single entry whose shape the pattern misses would otherwise drop out of
  // the fixture silently, which is exactly the drift this parser exists to
  // catch (it happened while writing it — an entry with a comment between two
  // of its fields parsed to nothing)
  const declared = table.match(/\bresource:\s*"/g)?.length ?? 0;
  if (rows.length !== declared) {
    throw new Error(
      `parsed ${rows.length} of ${declared} capabilities out of ${SOURCE}; ` +
        "an entry does not match the expected `Capability { resource, scope, read, create, update, delete }` shape",
    );
  }
  const duplicate = rows.find((row, i) => rows.findIndex((r) => r.resource === row.resource) !== i);
  if (duplicate) throw new Error(`\`${duplicate.resource}\` appears twice in ${SOURCE}`);
  return rows;
}

/** Read and parse the capability table from `path`. */
export function readCapabilities(path: string): CapabilityRow[] {
  return parseCapabilities(readFileSync(path, "utf8"));
}

/**
 * Every `(resource, action, authority)` triple two tables disagree on.
 *
 * Reported per pair rather than as one deep-equality failure so the message
 * names what drifted — "model:create is admin here and undefined in the
 * control plane" is actionable where "objects are not equal" is not.
 */
export function drift(expected: CapabilityRow[], actual: CapabilityRow[]): string[] {
  const differences: string[] = [];
  const byResource = (rows: CapabilityRow[]) => new Map(rows.map((row) => [row.resource, row]));
  const [want, have] = [byResource(expected), byResource(actual)];
  for (const resource of new Set([...want.keys(), ...have.keys()])) {
    const [a, b] = [want.get(resource), have.get(resource)];
    if (!b) {
      differences.push(`${resource}: in ${SOURCE}, missing from the fixture`);
      continue;
    }
    if (!a) {
      differences.push(`${resource}: in the fixture, missing from ${SOURCE}`);
      continue;
    }
    if (a.scope !== b.scope) differences.push(`${resource}: scope is ${a.scope} in ${SOURCE}, ${b.scope} in the fixture`);
    for (const action of ACTION_FIELDS) {
      if (a[action] !== b[action]) {
        differences.push(
          `${resource}:${action} takes ${a[action] ?? "no such action"} in ${SOURCE}, ` +
            `${b[action] ?? "no such action"} in the fixture`,
        );
      }
    }
  }
  return differences;
}
