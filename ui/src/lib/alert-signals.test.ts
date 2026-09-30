import { describe, expect, it } from "bun:test";
import i18next from "i18next";

import {
  ALERT_SIGNAL_SPECS,
  defaultThresholdInput,
  formatSignalValue,
  fromFormValue,
  signalDescription,
  signalLabel,
  signalSpec,
  thresholdInputMax,
  thresholdLabel,
  thresholdRangeKey,
  thresholdValid,
  toFormValue,
} from "@/lib/alert-signals";
import { ALERT_SIGNALS } from "@/lib/api";
import { formattersFor } from "@/lib/i18n/format";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

// a private instance over the shipped catalogs, so the plural forms and the
// placeholders under test are the real ones and no other suite's locale leaks in
const i18n = i18next.createInstance();
await i18n.init({
  resources: { en: { translation: en }, ru: { translation: ru } },
  lng: "en",
  interpolation: { escapeValue: false },
});
const tEn = i18n.getFixedT("en");
const tRu = i18n.getFixedT("ru");
const t = tEn as (key: string, options?: Record<string, unknown>) => string;
const fmt = formattersFor("en");

const read = (signal: string, value: number, windowSecs = 300, currency = "USD") =>
  formatSignalValue(signal, value, { fmt, t, currency, windowSecs });

describe("the signal table", () => {
  it("covers every signal the API accepts", () => {
    expect(Object.keys(ALERT_SIGNAL_SPECS).sort()).toEqual([...ALERT_SIGNALS].sort());
  });

  // every signal named in the catalogs, so a signal added to ALERT_SIGNALS
  // cannot reach the form as a raw key
  it("has a label and a description for every signal", () => {
    for (const signal of ALERT_SIGNALS) {
      expect(signalLabel(signal, t)).not.toContain("pages.alerting");
      expect(signalDescription(signal, t, "USD")).not.toContain("pages.alerting");
      expect(thresholdLabel(signal, t, "USD")).not.toContain("pages.alerting");
    }
  });

  it("knows nothing about a signal from a later build", () => {
    expect(signalSpec("queue_depth")).toBeNull();
    expect(signalLabel("queue_depth", t)).toBe("queue_depth");
    expect(signalDescription("queue_depth", t, "USD")).toBeNull();
    expect(thresholdLabel("queue_depth", t, "USD")).toBe("Threshold");
  });
});

describe("the percent round-trip", () => {
  it("takes error_rate as a percentage and sends the fraction", () => {
    expect(fromFormValue("error_rate", 5)).toBe(0.05);
    expect(toFormValue("error_rate", 0.05)).toBe(5);
  });

  // the binary noise a bare multiply or divide leaves in the request and the input
  it("does not send or show float noise", () => {
    expect(fromFormValue("error_rate", 1.1)).toBe(0.011);
    expect(toFormValue("error_rate", 0.07)).toBe(7);
    expect(toFormValue("error_rate", 0.125)).toBe(12.5);
  });

  it("leaves every other signal in its API unit", () => {
    for (const signal of ALERT_SIGNALS.filter((s) => s !== "error_rate")) {
      expect(fromFormValue(signal, 840)).toBe(840);
      expect(toFormValue(signal, 840)).toBe(840);
    }
    expect(fromFormValue("queue_depth", 5)).toBe(5);
  });
});

describe("the form's threshold", () => {
  it("starts each signal from its own default, typed in the form's unit", () => {
    expect(defaultThresholdInput("error_rate")).toBe("5");
    expect(defaultThresholdInput("p95_latency_ms")).toBe("8000");
    expect(defaultThresholdInput("spend_velocity")).toBe("50");
    expect(defaultThresholdInput("request_volume")).toBe("1000");
    expect(defaultThresholdInput("provider_health_flaps")).toBe("10");
  });

  // the fraction never passes 1, so 101 % is a rule that can never fire
  it("holds error_rate to 0-100 %", () => {
    expect(thresholdInputMax("error_rate")).toBe(100);
    expect(thresholdValid("error_rate", "100")).toBe(true);
    expect(thresholdValid("error_rate", "0")).toBe(true);
    expect(thresholdValid("error_rate", "101")).toBe(false);
    expect(t(thresholdRangeKey("error_rate"))).toBe("From 0 to 100%.");
  });

  it("holds every other signal to 0 or more, as the API does", () => {
    expect(thresholdInputMax("p95_latency_ms")).toBeUndefined();
    expect(thresholdValid("p95_latency_ms", "250000")).toBe(true);
    expect(thresholdValid("p95_latency_ms", "-1")).toBe(false);
    expect(t(thresholdRangeKey("request_volume"))).toBe("0 or more.");
  });

  it("refuses a blank or a non-number", () => {
    expect(thresholdValid("spend_velocity", "")).toBe(false);
    expect(thresholdValid("spend_velocity", "  ")).toBe(false);
    expect(thresholdValid("spend_velocity", "abc")).toBe(false);
  });

  it("names the unit in the field label", () => {
    expect(thresholdLabel("error_rate", t, "USD")).toBe("Threshold (%)");
    expect(thresholdLabel("p95_latency_ms", t, "USD")).toBe("Threshold (ms)");
    expect(thresholdLabel("spend_velocity", t, "EUR")).toBe("Threshold (EUR per hour)");
    expect(thresholdLabel("request_volume", t, "USD")).toBe("Threshold (requests per window)");
  });
});

describe("formatSignalValue", () => {
  it("reads each signal in its unit", () => {
    expect(read("error_rate", 0.05)).toBe("5%");
    expect(read("error_rate", 0.1234)).toBe("12.34%");
    expect(read("p95_latency_ms", 840.4)).toBe("840\u00a0ms");
    expect(read("p95_latency_ms", 8000)).toBe("8,000\u00a0ms");
    expect(read("spend_velocity", 12.5)).toBe("$12.50/h");
    expect(read("request_volume", 1000)).toBe("1,000 requests in 5m");
    expect(read("provider_health_flaps", 10)).toBe("10 failed health events in 5m");
  });

  // `cost_usd` is a historic name: spend is in whatever the deployment settles in
  it("prices spend in the settlement currency", () => {
    expect(read("spend_velocity", 12.5, 300, "EUR")).toBe("€12.50/h");
  });

  it("reads a count over the rule's own window", () => {
    expect(read("request_volume", 1, 3600)).toBe("1 request in 1h");
    expect(read("provider_health_flaps", 0, 600)).toBe("0 failed health events in 10m");
  });

  it("keeps an unknown signal's number bare", () => {
    expect(read("queue_depth", 0.05)).toBe("0.05");
  });

  it("declines the count in Russian", () => {
    const fmtRu = formattersFor("ru");
    const tr = tRu as (key: string, options?: Record<string, unknown>) => string;
    const readRu = (signal: string, value: number) =>
      formatSignalValue(signal, value, { fmt: fmtRu, t: tr, currency: "RUB", windowSecs: 300 });
    // ru groups with a no-break space, which \s matches
    expect(readRu("request_volume", 1)).toMatch(/^1 запрос за 5\sмин$/);
    expect(readRu("request_volume", 3)).toMatch(/^3 запроса за 5\sмин$/);
    expect(readRu("request_volume", 1000)).toMatch(/^1\s000 запросов за 5\sмин$/);
    expect(readRu("provider_health_flaps", 21)).toMatch(/^21 отказ за 5\sмин$/);
    expect(readRu("provider_health_flaps", 1.5)).toMatch(/^1,5 отказа за 5\sмин$/);
    expect(readRu("error_rate", 0.05)).toMatch(/^5\s%$/);
    // the number never breaks from its unit, on a card only a third as wide as the sheet
    expect(readRu("p95_latency_ms", 840)).toBe("840\u00a0мс");
    expect(readRu("request_volume", 340)).toBe("340 запросов за 5\u00a0мин");
    expect(readRu("spend_velocity", 12.5)).toMatch(/^12,50\s₽\/ч$/);
  });
});
