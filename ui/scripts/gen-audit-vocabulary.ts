#!/usr/bin/env bun
// Regenerate the checked-in list of audited actions and target types:
//
//   bun run gen:audit
//
// The Audit Log filters are built from that file, and
// `audit-vocabulary-source.test.ts` fails when it disagrees with
// `crates/rolter-control/src` — run this after adding an audited action (#2127).
import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { readVocabulary } from "./audit-vocabulary-source";

const ROOT = join(import.meta.dir, "..", "..");
const SNAPSHOT = join(ROOT, "ui", "src", "lib", "audit-vocabulary.json");

const vocabulary = readVocabulary(ROOT);
writeFileSync(
  SNAPSHOT,
  `${JSON.stringify(
    {
      $comment:
        "generated from crates/rolter-control/src by `bun run gen:audit` — do not edit by hand",
      ...vocabulary,
    },
    null,
    2,
  )}\n`,
);
console.log(
  `wrote ${vocabulary.actions.length} actions and ${vocabulary.targets.length} targets to ${SNAPSHOT}`,
);
