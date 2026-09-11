#!/usr/bin/env bun
// Regenerate the checked-in copy of the control plane's capability table:
//
//   bun run gen:rbac
//
// The stories' RBAC fixture is derived from that copy, and
// `rbac-matrix-source.test.ts` fails when the two disagree — so this script is
// what you run after adding a resource or changing an authority in
// `crates/rolter-control/src/rbac_matrix.rs` (#1298).
import { writeFileSync } from "node:fs";
import { join } from "node:path";

import { readCapabilities } from "./rbac-matrix-source";

const ROOT = join(import.meta.dir, "..");
const MATRIX = join(ROOT, "..", "crates", "rolter-control", "src", "rbac_matrix.rs");
const SNAPSHOT = join(ROOT, "src", "lib", "rbac-capabilities.json");

const capabilities = readCapabilities(MATRIX);
writeFileSync(
  SNAPSHOT,
  `${JSON.stringify(
    {
      $comment:
        "generated from crates/rolter-control/src/rbac_matrix.rs by `bun run gen:rbac` — do not edit by hand",
      capabilities,
    },
    null,
    2,
  )}\n`,
);
console.log(`wrote ${capabilities.length} capabilities to ${SNAPSHOT}`);
