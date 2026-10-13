import { describe, expect, it } from "bun:test";
import i18next from "i18next";

import {
  connectorHealthLabel,
  decoratorPositionLabel,
  decoratorRoleLabel,
  enumLabel,
  grantStateLabel,
  oauthStateLabel,
  skillRoleLabel,
} from "@/lib/enum-labels";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

const i18n = i18next.createInstance();
await i18n.init({
  resources: { en: { translation: en }, ru: { translation: ru } },
  lng: "en",
  interpolation: { escapeValue: false },
});
const tEn = i18n.getFixedT("en") as (key: string, options?: Record<string, unknown>) => string;
const tRu = i18n.getFixedT("ru") as (key: string, options?: Record<string, unknown>) => string;

const FAMILIES: [string, (value: string, t: typeof tEn) => string, string[]][] = [
  ["oauth state", oauthStateLabel, ["active", "expired", "revoked"]],
  ["connector health", connectorHealthLabel, ["healthy", "unhealthy", "unknown"]],
  ["decorator role", decoratorRoleLabel, ["system", "assistant", "user"]],
  ["decorator position", decoratorPositionLabel, ["prepend", "append"]],
  ["skill role", skillRoleLabel, ["viewer", "member", "admin"]],
];

describe("enum labels", () => {
  it.each(FAMILIES)("%s: every known value has a translation in en and ru", (_, label, values) => {
    for (const value of values) {
      for (const t of [tEn, tRu]) {
        const text = label(value, t);
        expect(text).not.toBe(value);
        expect(text).not.toMatch(/^pages\./);
      }
      // a label that equals its identifier in ru would be the bug this fixes
      expect(label(value, tRu).toLowerCase()).not.toBe(value);
    }
  });

  it.each(FAMILIES)("%s: a value from a later build prints as stored", (_, label) => {
    expect(label("quarantined", tRu)).toBe("quarantined");
    expect(label("constructor", tRu)).toBe("constructor");
  });

  it("reads a grant's boolean as active or revoked", () => {
    expect(grantStateLabel(true, tRu)).toBe(ru.pages.mcpOAuth.states.active);
    expect(grantStateLabel(false, tRu)).toBe(ru.pages.mcpOAuth.states.revoked);
  });

  it("falls back to the stored value for any table", () => {
    expect(enumLabel(tEn, { a: "pages.mcpOAuth.states.active" }, "b")).toBe("b");
  });
});
