import { describe, expect, it } from "bun:test";
import i18next from "i18next";

import { PROVIDER_KINDS } from "@/lib/api";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";
import { providerKindDescription, providerKindName } from "@/lib/provider-kinds";

// a private instance over the shipped catalogs, so the words under test are the
// real ones and no other suite's locale leaks in
const i18n = i18next.createInstance();
await i18n.init({
  resources: { en: { translation: en }, ru: { translation: ru } },
  lng: "en",
  interpolation: { escapeValue: false },
});
const tEn = i18n.getFixedT("en");
const tRu = i18n.getFixedT("ru");

describe("providerKindName", () => {
  it("names a kind in the dashboard's language", () => {
    expect(providerKindName("openai_compatible", tEn)).toBe("OpenAI-compatible");
    expect(providerKindName("openai_compatible", tRu)).toBe("OpenAI-совместимый");
  });

  it("falls back to the stored id for a kind the catalog does not name", () => {
    expect(providerKindName("brand_new_kind", tEn)).toBe("brand_new_kind");
    expect(providerKindName("brand_new_kind", tRu)).toBe("brand_new_kind");
  });

  it("names every kind the dashboard offers, in every catalog", () => {
    for (const t of [tEn, tRu]) {
      for (const kind of PROVIDER_KINDS) {
        const name = providerKindName(kind, t);
        expect(name).not.toBe("");
        expect(name).not.toContain("providerSheet.kinds");
      }
    }
  });
});

describe("providerKindDescription", () => {
  it("describes a named kind", () => {
    expect(providerKindDescription("anthropic", tEn)).toBe(
      "Anthropic's Messages API for Claude models",
    );
  });

  it("is empty for a kind the catalog does not name", () => {
    expect(providerKindDescription("brand_new_kind", tEn)).toBe("");
  });
});
