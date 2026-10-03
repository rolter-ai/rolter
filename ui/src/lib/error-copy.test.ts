import { describe, expect, it } from "bun:test";

import { ApiError } from "@/lib/api";
import { describeError, KNOWN_ERROR_CODES } from "@/lib/error-copy";
import { GatewayError } from "@/lib/gateway";
import { flatten } from "@/lib/i18n/parity";
import en from "@/lib/i18n/locales/en.json";
import ru from "@/lib/i18n/locales/ru.json";

// echoes the key, so the assertion names the line chosen rather than its prose
const t = ((key: string) => key) as never;

describe("describeError", () => {
  it("translates a known code and never shows the server's message", () => {
    const copy = describeError(
      new ApiError("last superadmin (internal)", 409, "last_superadmin"),
      t,
    );
    expect(copy).toEqual({ message: "errors.api.codes.last_superadmin" });
  });

  it("keeps an invalid_field message as detail, since only it names the field", () => {
    const copy = describeError(new ApiError("slug must match ^[a-z0-9]", 400, "invalid_field"), t);
    expect(copy).toEqual({
      message: "errors.api.codes.invalid_field",
      detail: "slug must match ^[a-z0-9]",
    });
  });

  it("translates a taken name without the server's words", () => {
    const copy = describeError(new ApiError("provider name 'x' is in use", 409, "name_taken"), t);
    expect(copy).toEqual({ message: "errors.api.codes.name_taken" });
  });

  it("falls back to the status for an uncoded 401, 403, 429 and 5xx", () => {
    expect(describeError(new ApiError("x", 401), t).message).toBe("errors.api.unauthorized");
    expect(describeError(new ApiError("x", 403), t).message).toBe("errors.api.forbidden");
    expect(describeError(new ApiError("x", 429), t).message).toBe("errors.api.rateLimited");
    expect(describeError(new ApiError("pg: boom", 500), t)).toEqual({
      message: "errors.api.server",
    });
  });

  it("keeps an unknown message as detail under a generic line", () => {
    const copy = describeError(new ApiError("name must not be empty", 400, "brand_new"), t);
    expect(copy).toEqual({ message: "errors.api.generic", detail: "name must not be empty" });
  });

  it("reads a thrown non-ApiError as unreachable", () => {
    expect(describeError(new TypeError("Failed to fetch"), t).message).toBe(
      "errors.api.unreachable",
    );
  });

  it("keeps a gateway message as detail", () => {
    expect(describeError(new GatewayError("model overloaded", 503), t).detail).toBe(
      "model overloaded",
    );
  });
});

describe("error catalog", () => {
  it("has a line for every known code in every locale", () => {
    for (const catalog of [en, ru]) {
      const keys = flatten(catalog as never);
      for (const code of KNOWN_ERROR_CODES) {
        expect([...keys.keys()]).toContain(`errors.api.codes.${code}`);
      }
    }
  });
});
