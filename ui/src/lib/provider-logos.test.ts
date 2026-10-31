import { describe, expect, test } from "bun:test";

import { PROVIDER_KINDS } from "@/lib/api";
import { PROVIDER_LOGOS, providerLogoPath } from "@/lib/provider-logos";

// kinds the logo set has no mark for, which the pickers draw as a monogram.
// a kind belongs here on purpose: when `simple-icons` gains the mark, move it
// to `PROVIDER_LOGOS`; when a new kind arrives, it lands in one list or the
// other, never in neither
const MONOGRAM_KINDS = [
  "openai",
  "openai_compatible",
  "llama_cpp",
  "azure_openai",
  "bedrock",
  "groq",
  "xai",
  "cohere",
  "together",
  "fireworks",
  "aleph_alpha",
  "nebius",
  "zhipu",
  "yi",
  "baichuan",
  "gigachat",
  "cloud_ru",
  "mts_ai",
  "upstage",
  "rinna",
  "sarvam",
  "krutrim",
  "falcon",
  "system_one",
  "infinity",
];

describe("provider logos", () => {
  test("every provider kind has a logo or is named as a monogram", () => {
    const missing = PROVIDER_KINDS.filter(
      (kind) => !(kind in PROVIDER_LOGOS) && !MONOGRAM_KINDS.includes(kind),
    );
    expect(missing).toEqual([]);
  });

  test("no kind is listed in both places, and none is stale", () => {
    expect(MONOGRAM_KINDS.filter((kind) => kind in PROVIDER_LOGOS)).toEqual([]);
    const known: readonly string[] = PROVIDER_KINDS;
    expect(Object.keys(PROVIDER_LOGOS).filter((kind) => !known.includes(kind))).toEqual([]);
    expect(MONOGRAM_KINDS.filter((kind) => !known.includes(kind))).toEqual([]);
  });

  test("a kind without a mark has no path, so the picker draws a monogram", () => {
    expect(providerLogoPath("anthropic")).toStartWith("M");
    expect(providerLogoPath("groq")).toBeUndefined();
    expect(providerLogoPath("a_kind_from_a_newer_control_plane")).toBeUndefined();
  });
});
