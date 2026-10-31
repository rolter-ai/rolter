import { describe, expect, it } from "bun:test";
import { readFileSync } from "node:fs";
import { fileURLToPath, URL } from "node:url";

import { KNOWN_ERROR_CODES } from "../src/lib/error-copy";

// the dashboard has a sentence for each `code` the control plane sends, and
// `control-plane-openapi.mdx` is where an API client learns what each one means
// and which status it travels with. nothing tied the two, which is how
// `kek_unavailable` came to be the one code whose status changed and the one
// the table left out (#3218). a code added to `KNOWN_ERROR_CODES` now fails here
// until the page has a row for it
const PAGE = fileURLToPath(
  new URL("../../docs/user-docs/api/control-plane-openapi.mdx", import.meta.url),
);

/**
 * the backticked identifiers in the first two cells of every table row: the
 * code column leads in one table and follows the status in the other, and the
 * prose of the last cell is left alone
 */
function documentedCodes(page: string): string[] {
  const codes: string[] = [];
  for (const line of page.split("\n")) {
    if (!line.startsWith("|")) continue;
    for (const cell of line.split("|").slice(1, 3)) {
      const match = /^\s*`([a-z][a-z0-9_]*)`\s*$/.exec(cell);
      if (match) codes.push(match[1]);
    }
  }
  return codes;
}

describe("control-plane error codes in the user docs", () => {
  it("reads the codes out of table rows only", () => {
    const page = [
      "| Code | Status | When |",
      "| --- | --- | --- |",
      "| `name_taken` | `409` | prose naming `invalid_field` is not a row |",
      "| `502` | `analytics_query_failed` | the status-first table |",
    ].join("\n");
    expect(documentedCodes(page)).toEqual(["name_taken", "analytics_query_failed"]);
  });

  it("has a row for every code the dashboard knows", () => {
    const documented = new Set(documentedCodes(readFileSync(PAGE, "utf8")));
    const missing = KNOWN_ERROR_CODES.filter((code) => !documented.has(code));
    expect(missing).toEqual([]);
  });
});
