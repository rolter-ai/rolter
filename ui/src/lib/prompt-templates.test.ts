import { describe, expect, test } from "bun:test";

import {
  addedScopes,
  publishImpact,
  referencedVariables,
  requiredVariables,
  templateVarsExample,
  type TemplateContent,
} from "./prompt-templates";

const system = (content: string) => ({
  role: "system" as const,
  position: "prepend" as const,
  content,
});

const live: TemplateContent = {
  variables: [
    { name: "customer_name", required: true },
    { name: "tone", required: false, default: "calm" },
  ],
  decorators: [system("You support {{ customer_name }}. Keep the tone {{tone}}.")],
};

describe("requiredVariables", () => {
  test("a variable declared required is required", () => {
    expect(requiredVariables(live)).toEqual(["customer_name"]);
  });

  // the gateway reads the flag, not the decorators: a required variable is
  // refused when missing even if nothing renders it (#2280)
  test("a required variable no decorator references is still required", () => {
    expect(
      requiredVariables({
        variables: [{ name: "unused", required: true }],
        decorators: [system("no placeholders")],
      }),
    ).toEqual(["unused"]);
  });

  // and the other way round: optional with nothing to fall back on renders
  // empty rather than refusing the request
  test("an optional variable with no default is not required", () => {
    expect(
      requiredVariables({
        variables: [{ name: "ticket", required: false }],
        decorators: [system("Ticket {{ticket}}")],
      }),
    ).toEqual([]);
  });

  test("an optional variable with an empty default is not required", () => {
    expect(
      requiredVariables({
        variables: [{ name: "note", required: false, default: "" }],
        decorators: [system("{{ note }}")],
      }),
    ).toEqual([]);
  });

  test("placeholders are read across decorators, once each", () => {
    expect(
      referencedVariables([system("{{a}} {{ b }}"), system("{{a}}"), system("{{ not valid }}")]),
    ).toEqual(["a", "b"]);
  });
});

describe("publishImpact", () => {
  test("with nothing live, every required variable is required and nothing is new", () => {
    expect(publishImpact(live)).toEqual({
      required: ["customer_name"],
      newlyRequired: [],
      dropped: [],
    });
  });

  test("a version that adds a required variable and drops another", () => {
    const next: TemplateContent = {
      variables: [
        { name: "customer_name", required: true },
        { name: "ticket_id", required: true },
      ],
      decorators: [system("{{customer_name}} on {{ticket_id}}")],
    };
    expect(publishImpact(next, live)).toEqual({
      required: ["customer_name", "ticket_id"],
      newlyRequired: ["ticket_id"],
      dropped: ["tone"],
    });
  });

  test("losing a default makes a variable newly required", () => {
    const next: TemplateContent = {
      variables: [
        { name: "customer_name", required: true },
        { name: "tone", required: true },
      ],
      decorators: live.decorators,
    };
    expect(publishImpact(next, live).newlyRequired).toEqual(["tone"]);
  });
});

describe("addedScopes", () => {
  const project = { scope_type: "project" as const, scope_id: "p" };
  const route = { scope_type: "route" as const, scope_id: "r" };

  test("scopes the live version does not reach", () => {
    expect([...addedScopes([project, route], [project])]).toEqual(["route:r"]);
  });

  test("every scope is new when nothing is live", () => {
    expect([...addedScopes([project], undefined)]).toEqual(["project:p"]);
  });
});

describe("templateVarsExample", () => {
  test("samples win over defaults, and a blank stands in for the rest", () => {
    expect(
      JSON.parse(
        templateVarsExample(
          [
            { name: "customer_name", required: true },
            { name: "tone", required: false, default: "calm" },
            { name: "ticket", required: true },
            { name: "", required: true },
          ],
          { customer_name: "Aster Labs" },
        ),
      ),
    ).toEqual({
      rolter_template_vars: { customer_name: "Aster Labs", tone: "calm", ticket: "…" },
    });
  });
});
