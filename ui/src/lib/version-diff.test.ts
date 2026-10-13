import { describe, expect, test } from "bun:test";

import type { SkillVersionRow } from "@/lib/api";
import { diffPromptVersions, diffSkillVersions } from "@/lib/version-diff";

const describeVariable = (variable: { required: boolean; default?: string }) =>
  variable.required ? "required" : `default: ${variable.default ?? ""}`;

describe("diffPromptVersions", () => {
  const v2 = {
    variables: [
      { name: "customer_name", required: true },
      { name: "tone", required: false, default: "calm" },
    ],
    decorators: [{ role: "system" as const, position: "prepend" as const, content: "Be kind." }],
  };

  test("two identical versions differ in nothing", () => {
    expect(diffPromptVersions(v2, v2, describeVariable).changed).toBe(0);
  });

  test("names a changed decorator and the variables that came and went", () => {
    const v3 = {
      variables: [
        { name: "customer_name", required: true },
        { name: "ticket_id", required: true },
        { name: "tone", required: false, default: "warm" },
      ],
      decorators: [{ role: "system" as const, position: "prepend" as const, content: "Be brief." }],
    };
    const diff = diffPromptVersions(v2, v3, describeVariable);
    expect(diff.variables.map((row) => [row.field, row.kind])).toEqual([
      ["customer_name", "same"],
      ["tone", "changed"],
      ["ticket_id", "added"],
    ]);
    expect(diff.decorators.filter((line) => line.kind !== "same").map((line) => line.text)).toEqual(
      ["Be kind.", "Be brief."],
    );
    expect(diff.changed).toBe(4);
  });

  test("a reordered decorator is a change", () => {
    const a = { role: "system" as const, position: "prepend" as const, content: "A" };
    const b = { role: "user" as const, position: "append" as const, content: "B" };
    const diff = diffPromptVersions(
      { variables: [], decorators: [a, b] },
      { variables: [], decorators: [b, a] },
      describeVariable,
    );
    expect(diff.changed).toBeGreaterThan(0);
  });
});

describe("diffSkillVersions", () => {
  const labels = {
    source: "Source",
    reference: "Reference",
    inline: "inline",
    artifact: "artifact",
  };
  const base: SkillVersionRow = {
    skill_id: "s",
    version: 1,
    content: "# Skill\n\nDo it.\n",
    content_ref: null,
    metadata: { runtime: "node", tags: ["a"] },
    created_at: "2026-01-01T00:00:00Z",
  };

  test("compares content lines and metadata keys", () => {
    const next = {
      ...base,
      version: 2,
      content: "# Skill\n\nDo it well.\n",
      metadata: { runtime: "bun", tags: ["a"], owner: "ops" },
    };
    const diff = diffSkillVersions(base, next, labels);
    expect(diff.content.filter((line) => line.kind !== "same").map((line) => line.text)).toEqual([
      "Do it.",
      "Do it well.",
    ]);
    expect(diff.metadata.map((row) => [row.field, row.kind])).toEqual([
      ["runtime", "changed"],
      ["tags", "same"],
      ["owner", "added"],
    ]);
    expect(diff.metadata[0]).toMatchObject({ before: '"node"', after: '"bun"' });
    expect(diff.changed).toBe(4);
  });

  test("moving from inline content to a reference changes the source", () => {
    const ref = { ...base, version: 2, content: null, content_ref: "oci://reg/skill@sha256:1" };
    const diff = diffSkillVersions(base, ref, labels);
    expect(diff.source.map((row) => [row.field, row.kind])).toEqual([
      ["Source", "changed"],
      ["Reference", "added"],
    ]);
    expect(diff.content.every((line) => line.kind === "del")).toBe(true);
  });
});
