import { describe, expect, test } from "bun:test";

import {
  audioInputPatch,
  audioInputTokenPatch,
  audioOutputTokenPatch,
  cachedAudioPatch,
  cacheWrite1hPatch,
  cacheWritePatch,
  imagePatch,
  speechPatch,
} from "@/lib/cache-write-rate";

describe("cacheWritePatch", () => {
  test("sends nothing for an input nobody touched, so the stored rate is kept", () => {
    expect(cacheWritePatch("", "")).toEqual({});
    expect(cacheWritePatch("3.750000", "3.750000")).toEqual({});
    expect(cacheWritePatch(" 3.75 ", "3.75")).toEqual({});
  });

  test("sends the rate that was typed", () => {
    expect(cacheWritePatch("3.75", "")).toEqual({ cache_write_per_mtok: "3.75" });
    expect(cacheWritePatch("4.5", "3.750000")).toEqual({ cache_write_per_mtok: "4.5" });
    expect(cacheWritePatch(" 4.5 ", "3.750000")).toEqual({ cache_write_per_mtok: "4.5" });
  });

  test("sends null for a rate that was emptied, which clears it", () => {
    expect(cacheWritePatch("", "3.750000")).toEqual({ cache_write_per_mtok: null });
    expect(cacheWritePatch("  ", "3.750000")).toEqual({ cache_write_per_mtok: null });
  });

  test("keeps null on the wire, where an undefined key would be dropped", () => {
    expect(JSON.stringify(cacheWritePatch("", "3.75"))).toBe('{"cache_write_per_mtok":null}');
    expect(JSON.stringify(cacheWritePatch("", ""))).toBe("{}");
  });
});

describe("cacheWrite1hPatch", () => {
  test("sends nothing for an input nobody touched, so the stored rate is kept", () => {
    expect(cacheWrite1hPatch("", "")).toEqual({});
    expect(cacheWrite1hPatch("6.000000", "6.000000")).toEqual({});
    expect(cacheWrite1hPatch(" 6 ", "6")).toEqual({});
  });

  test("sends the rate that was typed, under its own key", () => {
    expect(cacheWrite1hPatch("6", "")).toEqual({ cache_write_1h_per_mtok: "6" });
    expect(cacheWrite1hPatch(" 7.5 ", "6.000000")).toEqual({ cache_write_1h_per_mtok: "7.5" });
    expect(cacheWrite1hPatch("0", "6.000000")).toEqual({ cache_write_1h_per_mtok: "0" });
  });

  test("sends null for a rate that was emptied, which clears it", () => {
    expect(cacheWrite1hPatch("", "6.000000")).toEqual({ cache_write_1h_per_mtok: null });
    expect(cacheWrite1hPatch("  ", "6.000000")).toEqual({ cache_write_1h_per_mtok: null });
  });

  test("keeps null on the wire, and leaves the 5 minute key out", () => {
    expect(JSON.stringify(cacheWrite1hPatch("", "6"))).toBe('{"cache_write_1h_per_mtok":null}');
    expect(JSON.stringify(cacheWrite1hPatch("", ""))).toBe("{}");
    expect(JSON.stringify({ ...cacheWritePatch("3", ""), ...cacheWrite1hPatch("", "6") })).toBe(
      '{"cache_write_per_mtok":"3","cache_write_1h_per_mtok":null}',
    );
  });
});

describe("unit rate patches (#2937)", () => {
  test("each names its own key, sends nothing untouched and null when emptied", () => {
    expect(imagePatch("", "")).toEqual({});
    expect(imagePatch("0.04", "")).toEqual({ image_per_unit: "0.04" });
    expect(imagePatch("", "0.040000")).toEqual({ image_per_unit: null });
    expect(audioInputPatch("0.006000", "0.006000")).toEqual({});
    expect(audioInputPatch("0.01", "0.006000")).toEqual({ audio_input_per_minute: "0.01" });
    expect(speechPatch("", "15.000000")).toEqual({ speech_per_mchar: null });
    expect(JSON.stringify(speechPatch("", "15"))).toBe('{"speech_per_mchar":null}');
  });
});

describe("audio token rate patches (#1882)", () => {
  test("each names its own key, sends nothing untouched and null when emptied", () => {
    expect(audioInputTokenPatch("", "")).toEqual({});
    expect(audioInputTokenPatch("32", "")).toEqual({ audio_input_per_mtok: "32" });
    expect(audioOutputTokenPatch("", "64.000000")).toEqual({ audio_output_per_mtok: null });
    expect(cachedAudioPatch("0.400000", "0.400000")).toEqual({});
    expect(cachedAudioPatch("0.5", "0.400000")).toEqual({ cached_audio_input_per_mtok: "0.5" });
    expect(JSON.stringify(audioOutputTokenPatch("", "64"))).toBe('{"audio_output_per_mtok":null}');
  });
});
