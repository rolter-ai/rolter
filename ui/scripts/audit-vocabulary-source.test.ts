import { describe, it, expect } from "bun:test";
import { fileURLToPath, URL } from "node:url";

import {
  parseActions,
  parseTargets,
  productionSource,
  readVocabulary,
} from "./audit-vocabulary-source";
import snapshot from "../src/lib/audit-vocabulary.json";
import { AUDIT_ACTIONS, AUDIT_GROUPS, auditGroup } from "../src/lib/audit-vocabulary";

const ROOT = fileURLToPath(new URL("../../", import.meta.url));

const SOURCE = `
async fn handler() {
    log_audit(
        &state,
        &principal,
        Some(org.id),
        "sso_provider.update",
        "sso_provider",
        id,
        json!({}),
    )
    .await;
    let span = stage_span!("snapshot.build");
}

#[cfg(test)]
mod tests {
    fn t() {
        assert_eq!("only_in_tests.update", x);
    }
}
`;

describe("audit vocabulary source", () => {
  it("reads actions and targets out of production code only", () => {
    expect(parseActions([SOURCE])).toEqual(["sso_provider.update"]);
    expect(parseTargets([SOURCE])).toEqual(["sso_provider"]);
    expect(productionSource(SOURCE)).not.toContain("only_in_tests");
  });

  it("matches the checked-in list; run `bun run gen:audit` when this fails", () => {
    const live = readVocabulary(ROOT);
    expect(snapshot.actions).toEqual(live.actions);
    expect(snapshot.targets).toEqual(live.targets);
  });

  it("offers every action the control plane audits, each under a named group", () => {
    const live = readVocabulary(ROOT);
    expect([...AUDIT_ACTIONS]).toEqual(live.actions);
    expect(live.actions).toContain("sso_provider.update");
    expect(live.actions.filter((a) => auditGroup(a) === "other")).toEqual([]);
    expect(AUDIT_GROUPS).toContain("other");
  });
});
