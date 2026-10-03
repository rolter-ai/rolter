#!/usr/bin/env bun
// Refresh the dashboard's copy of the control plane's capability matrix:
//
//   bun run gen:rbac          (or `just gen-rbac` from the repository root,
//                              which rewrites the Rust artifact first)
//
// `crates/rolter-control/rbac-matrix.json` is written and verified by the
// rolter-control test suite from `CAPABILITIES` itself, so this script only
// copies it — byte for byte, because `rbac-matrix-artifact.test.ts` compares
// the two that way (#1369). Run it after changing a resource or an authority
// in `crates/rolter-control/src/rbac_matrix.rs` and regenerating the artifact.
import { copyFileSync } from "node:fs";

import { ARTIFACT, SNAPSHOT, readMatrix } from "./rbac-matrix-artifact";

// parse before copying so a truncated or hand-mangled artifact fails here
// rather than inside every gating story
const { resources } = readMatrix(ARTIFACT);
copyFileSync(ARTIFACT, SNAPSHOT);
console.log(`copied ${resources.length} capabilities to ${SNAPSHOT}`);
