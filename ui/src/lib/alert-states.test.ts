import { describe, expect, it } from "bun:test";
import i18next from "i18next";

import {
  channelKindLabel,
  CHANNEL_KINDS,
  deliveryLabel,
  DELIVERY_STATUSES,
  HISTORY_STATES,
  RULE_STATES,
  stateLabel,
} from "@/lib/alert-states";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

// a private instance over the shipped catalogs, so the words under test are the
// real ones and no other suite's locale leaks in
const i18n = i18next.createInstance();
await i18n.init({
  resources: { en: { translation: en }, ru: { translation: ru } },
  lng: "en",
  interpolation: { escapeValue: false },
});
const tEn = i18n.getFixedT("en") as (key: string, options?: Record<string, unknown>) => string;
const tRu = i18n.getFixedT("ru") as (key: string, options?: Record<string, unknown>) => string;

describe("alert state labels", () => {
  // a state added to a list cannot reach the screen as a raw catalog key
  it("names every state, delivery and channel kind in both catalogs", () => {
    for (const t of [tEn, tRu]) {
      for (const state of [...RULE_STATES, ...HISTORY_STATES]) {
        expect(stateLabel(state, t)).not.toContain("pages.alerting");
      }
      for (const status of DELIVERY_STATUSES) {
        expect(deliveryLabel(status, t)).not.toContain("pages.alerting");
      }
      for (const kind of CHANNEL_KINDS) {
        expect(channelKindLabel(kind, t)).not.toContain("pages.alerting");
      }
    }
  });

  it("says the same state in the locale's own words", () => {
    expect(stateLabel("firing", tEn)).toBe("Firing");
    expect(stateLabel("firing", tRu)).toBe("Сработало");
    expect(deliveryLabel("skipped", tEn)).toBe("Skipped");
    expect(deliveryLabel("skipped", tRu)).toBe("Пропущено");
    expect(channelKindLabel("webhook", tRu)).toBe("Вебхук");
  });

  // none of the Russian words is the stored English identifier
  it("leaves no English identifier in the Russian words", () => {
    for (const state of [...RULE_STATES, ...HISTORY_STATES]) {
      expect(stateLabel(state, tRu).toLowerCase()).not.toBe(state);
    }
    for (const status of DELIVERY_STATUSES) {
      expect(deliveryLabel(status, tRu).toLowerCase()).not.toBe(status);
    }
  });

  // a value from a later build prints as stored, never blank and never a key
  it("prints a value it does not know as stored", () => {
    expect(stateLabel("degraded", tEn)).toBe("degraded");
    expect(deliveryLabel("queued", tRu)).toBe("queued");
    expect(channelKindLabel("pagerduty", tRu)).toBe("pagerduty");
    // an inherited property name is not a known state
    expect(stateLabel("constructor", tEn)).toBe("constructor");
  });
});
